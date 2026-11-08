use std::{
    io::{self, IsTerminal, Write},
    num::NonZeroU32,
    path::Path,
};

use anstream::{AutoStream, ColorChoice};
use anyhow::{Context, Result, ensure};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, error::ErrorKind};
use reqwest::Client;

use hodis::{
    Provider, Providers, SearchParams,
    cache::Cache,
    config::{self, Config, Loaded},
    keys::{ApiKey, Keychain, KeyedProvider},
    mcp,
    output::{ColorWhen, OutputFormat, render, render_diagnostics, render_warning},
    progress::Progress,
    providers,
    setup::{self, Outcome, TerminalPrompts, Wizard},
    tui,
};

#[derive(Parser)]
#[command(
    version,
    about,
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    search: Search,
}

#[derive(Args)]
struct Search {
    /// Your search query (leave it out in a terminal to search interactively)
    query: Vec<String>,
    /// Search in a full-screen interface, starting with the query if given
    #[arg(short, long, conflicts_with = "format")]
    interactive: bool,
    /// Result page number (starts at 1)
    #[arg(short, long, value_name = "NUMBER")]
    page: Option<NonZeroU32>,
    /// Search providers, comma-separated; several are searched at once
    /// (default: the one `hodis setup` saved)
    #[arg(long, value_enum, value_delimiter = ',', value_name = "PROVIDER")]
    provider: Vec<Provider>,
    /// Search afresh instead of reusing a cached result
    #[arg(long)]
    no_cache: bool,
    /// Output format (default: pretty)
    #[arg(short, long, value_enum, requires = "query")]
    format: Option<OutputFormat>,
    /// When to use colors (default: auto)
    #[arg(long, value_name = "WHEN", value_enum)]
    color: Option<ColorWhen>,
}

#[derive(Subcommand)]
enum Command {
    /// Choose a search provider and set it up
    ///
    /// hodis asks this on its first run in a terminal. Run it again to change
    /// provider, or to retry a setup that failed. Before you choose, it shows
    /// what each provider gives and costs, as listed below.
    Setup,
    /// Store or delete the API keys of providers that need one
    #[command(subcommand)]
    Key(KeyCommand),
    /// Serve web searches to AI agents over MCP
    ///
    /// An MCP client, such as an AI coding agent, starts this command and talks
    /// to it over standard input and output. Searches go to the providers the
    /// config file chooses unless `--provider` names others.
    Mcp {
        /// Search providers, comma-separated; several are searched at once
        /// (default: the one `hodis setup` saved)
        #[arg(long, value_enum, value_delimiter = ',', value_name = "PROVIDER")]
        provider: Vec<Provider>,
    },
}

#[derive(Clone, Copy, Subcommand)]
enum KeyCommand {
    /// Store a provider's API key in the system keychain
    ///
    /// The key is read from standard input, or prompted for without echo in a
    /// terminal. The provider's environment variable, such as `SERPER_API_KEY`,
    /// takes precedence over a stored key.
    Set {
        /// The provider the key is for
        provider: KeyedProvider,
    },
    /// Delete a provider's API key from the system keychain
    Delete {
        /// The provider whose key to delete
        provider: KeyedProvider,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = parse_args();
    let Loaded {
        config,
        path,
        unknown_keys,
    } = Config::load()?;
    ColorChoice::from(cli.search.color.unwrap_or(config.color)).write_global();
    let mut err = AutoStream::auto(io::stderr().lock());

    if let Some(path) = &path {
        for key in &unknown_keys {
            render_warning(
                &format!("unknown key `{key}` in {}", path.display()),
                &mut err,
            )?;
        }
    }

    let config = if is_first_run(&cli, path.as_deref()) {
        set_up(config, &mut err).await?
    } else {
        config
    };

    match cli.command {
        Some(Command::Setup) => {
            ensure!(can_prompt(), "`hodis setup` needs a terminal");
            set_up(config, &mut err).await.map(|_| ())
        }
        Some(Command::Key(command)) => manage_key(command, &mut err),
        Some(Command::Mcp { provider }) => {
            if io::stdin().is_terminal() {
                render_warning(
                    "`hodis mcp` is for an MCP client to start; it waits for the client's messages on standard input",
                    &mut err,
                )?;
            }
            // Its lock on stderr would hold up any other thread writing there.
            drop(err);
            let providers = chosen(provider, &config);
            let cache = Cache::open(config.cache_ttl);
            mcp::Server::new(config, providers, cache)?
                .serve_stdio()
                .await
        }
        None if cli.search.interactive || cli.search.query.is_empty() => {
            interactive(cli.search, config, &mut err).await
        }
        None => search(cli.search, config, &mut err).await,
    }
}

/// Whether to ask which provider to use before anything else: a person is
/// there to answer, has not said, and has no config file saying so.
fn is_first_run(cli: &Cli, config_file: Option<&Path>) -> bool {
    cli.command.is_none() && cli.search.provider.is_empty() && config_file.is_none() && can_prompt()
}

/// Asks which provider to use and sets it up, giving the settings to go on
/// with: the new ones, or `config` if the user skipped this.
async fn set_up(config: Config, err: &mut impl Write) -> Result<Config> {
    let path = config::path().context("there is no config directory for the current user")?;
    let progress = Progress::new(&mut *err, true);
    let outcome = Wizard::new(&config, &path, TerminalPrompts::new(), progress)?
        .run()
        .await?;
    match outcome {
        Outcome::Saved => Ok(Config::read(&path)?.map_or(config, |loaded| loaded.config)),
        Outcome::Skipped => {
            writeln!(err, "{}", setup::SKIPPED)?;
            Ok(config)
        }
    }
}

async fn interactive(args: Search, config: Config, err: &mut impl Write) -> Result<()> {
    ensure!(
        can_draw(),
        "the interactive search needs a terminal on standard input and output"
    );
    let providers = chosen(args.provider, &config);
    let cache = open_cache(config.cache_ttl, args.no_cache);
    let first = (!args.query.is_empty()).then(|| SearchParams {
        query: args.query.join(" "),
        page: args.page,
    });
    let mut progress = Progress::new(err, io::stderr().is_terminal());
    tui::run(config, providers, cache, first, &mut progress).await
}

async fn search(args: Search, config: Config, err: &mut impl Write) -> Result<()> {
    let format = args.format.unwrap_or(config.format);
    let client = Client::builder().build()?;
    let mut out = AutoStream::auto(io::stdout().lock());

    let providers = chosen(args.provider, &config);
    let cache = open_cache(config.cache_ttl, args.no_cache);
    let params = SearchParams {
        query: args.query.join(" "),
        page: args.page,
    };

    let mut progress = Progress::new(&mut *err, io::stderr().is_terminal());
    let response = providers::search(
        &providers,
        &params,
        &config,
        &client,
        cache.as_ref(),
        &mut progress,
    )
    .await?;
    drop(progress);

    match render(&response, format, &mut out) {
        // A reader that stops early, such as `head`, closes the pipe: that ends
        // the output, the way it ends any other Unix tool's, without an error.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
        result => result?,
    }
    render_diagnostics(&response.diagnostics, err)?;

    Ok(())
}

/// The providers `--provider` names, or else the config file's choice.
fn chosen(flag: Vec<Provider>, config: &Config) -> Providers {
    if flag.is_empty() {
        config.providers.clone()
    } else {
        Providers::Listed(flag)
    }
}

/// The cache, still kept up to date but not read from if `no_cache`.
fn open_cache(ttl: std::time::Duration, no_cache: bool) -> Option<Cache> {
    Cache::open(ttl).map(|cache| {
        if no_cache {
            cache.without_reads()
        } else {
            cache
        }
    })
}

fn manage_key(command: KeyCommand, err: &mut impl Write) -> Result<()> {
    let provider = match command {
        KeyCommand::Set { provider } => {
            // Opened before asking for the key, so a machine without a keychain
            // says so before anyone pastes a key in.
            let keychain = open_keychain(provider)?;
            keychain.set(provider, &read_key(provider)?)?;
            writeln!(
                err,
                "Stored the {} key in the system keychain.",
                provider.name()
            )?;
            provider
        }
        KeyCommand::Delete { provider } => {
            if open_keychain(provider)?.delete(provider)? {
                writeln!(
                    err,
                    "Deleted the {} key from the system keychain.",
                    provider.name()
                )?;
            } else {
                writeln!(err, "The system keychain holds no {} key.", provider.name())?;
            }
            provider
        }
    };

    if provider.env_var_is_set() {
        render_warning(
            &format!(
                "{} is set, and searches use it instead of the keychain",
                provider.env_var()
            ),
            err,
        )?;
    }
    Ok(())
}

fn open_keychain(provider: KeyedProvider) -> Result<Keychain> {
    Keychain::open().with_context(|| {
        format!(
            "no system keychain is available; set {} in the environment instead",
            provider.env_var()
        )
    })
}

/// The key typed at a prompt in a terminal, or else piped in.
fn read_key(provider: KeyedProvider) -> Result<ApiKey> {
    let stdin = io::stdin();
    let text = if stdin.is_terminal() {
        rpassword::prompt_password(format!("{} API key: ", provider.name()))?
    } else {
        io::read_to_string(stdin)?
    };
    ApiKey::new(&text)
}

/// `Cli::parse`, with the help comparing the providers and naming the config
/// file.
fn parse_args() -> Cli {
    let mut command = command();
    let cli = Cli::from_arg_matches(&command.get_matches_mut()).unwrap_or_else(|e| e.exit());
    if let Err(e) = require_query(&cli, &mut command, can_draw()) {
        e.exit();
    }
    cli
}

/// The command line, with `-h` pointing at the providers that `--help` and
/// `hodis setup --help` compare.
fn command() -> clap::Command {
    let command = Cli::command();
    let header = *command.get_styles().get_header();
    let providers = format!("{header}Providers:{header:#}\n{}", setup::trade_offs());
    let config_file = config::path().map(|path| format!("\n\nConfig file: {}", path.display()));
    let config_file = config_file.as_deref().unwrap_or_default();

    command
        .after_help(format!(
            "Run `hodis --help` to compare the providers.{config_file}"
        ))
        // Clap shows this instead of `after_help` on `--help`, not after it.
        .after_long_help(format!(
            "{providers}\n\n  \
             Name several to search them all at once, as in --provider searxng,serper.\n  \
             Run `hodis setup` to choose one and set it up.{config_file}"
        ))
        .mut_subcommand("setup", |setup| setup.after_long_help(providers))
}

/// A search without a query is interactive, which only a terminal can show.
fn require_query(
    cli: &Cli,
    command: &mut clap::Command,
    terminal: bool,
) -> Result<(), clap::Error> {
    if cli.command.is_none() && cli.search.query.is_empty() && !cli.search.interactive && !terminal
    {
        return Err(command.error(
            ErrorKind::MissingRequiredArgument,
            "a search query is required when not run in a terminal",
        ));
    }
    Ok(())
}

/// Whether questions can be asked: they appear on standard error, and a search
/// sent elsewhere is likely a script's, which nobody is there to answer for.
fn can_prompt() -> bool {
    can_draw() && io::stderr().is_terminal()
}

/// Whether the interactive search can run: it reads keys from standard input,
/// draws on standard output, and moves the cursor, which a dumb terminal
/// cannot.
fn can_draw() -> bool {
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use hodis::{Provider, setup};

    use super::{Cli, Command, KeyCommand, KeyedProvider, command, require_query};

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from([&["hodis"], args].concat()).expect("the arguments parse")
    }

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
        command().debug_assert();
    }

    #[test]
    fn the_long_help_compares_the_providers() {
        let help = command().render_long_help().to_string();
        assert!(help.contains(&setup::trade_offs()), "{help}");
        assert!(help.contains("--provider searxng,serper"), "{help}");
        for provider in [Provider::DuckDuckGo, Provider::SearXNG, Provider::Serper] {
            let value = format!("{}:", provider.id());
            let line = help
                .lines()
                .find(|line| line.trim_start().starts_with(&format!("- {value}")))
                .unwrap_or_else(|| panic!("no possible value {value}: {help}"));
            assert!(line.ends_with(&provider.summary()), "{line}");
        }
    }

    #[test]
    fn the_short_help_points_at_the_comparison() {
        let help = command().render_help().to_string();
        assert!(!help.contains(&setup::trade_offs()), "{help}");
        assert!(help.contains("`hodis --help`"), "{help}");
    }

    #[test]
    fn setup_help_compares_the_providers() {
        let mut command = command();
        let setup_command = command
            .find_subcommand_mut("setup")
            .expect("a setup command");
        let help = setup_command.render_long_help().to_string();
        assert!(help.contains(&setup::trade_offs()), "{help}");
    }

    #[test]
    fn flags_left_out_defer_to_the_config() {
        let cli = parse(&["rust"]);
        assert_eq!((cli.search.format, cli.search.color), (None, None));
    }

    #[test]
    fn a_command_name_first_runs_the_command() {
        let cli = parse(&["key", "set", "serper"]);
        assert!(matches!(
            cli.command,
            Some(Command::Key(KeyCommand::Set {
                provider: KeyedProvider::Serper
            }))
        ));
    }

    #[test]
    fn a_command_name_later_in_the_query_is_searched_for() {
        let cli = parse(&["rust", "key", "set"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.search.query, ["rust", "key", "set"]);
    }

    #[test]
    fn mcp_takes_providers_of_its_own() {
        assert!(matches!(
            parse(&["mcp"]).command,
            Some(Command::Mcp { provider }) if provider.is_empty()
        ));
        assert!(matches!(
            parse(&["mcp", "--provider", "serper,searxng"]).command,
            Some(Command::Mcp { provider })
                if provider == [Provider::Serper, Provider::SearXNG]
        ));
    }

    #[test]
    fn a_double_dash_searches_for_a_command_name() {
        let cli = parse(&["--", "key"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.search.query, ["key"]);
    }

    #[test]
    fn every_provider_is_offered_by_its_lowercase_name() {
        assert_eq!(
            parse(&["--provider", "duckduckgo,searxng,serper", "rust"])
                .search
                .provider,
            [Provider::DuckDuckGo, Provider::SearXNG, Provider::Serper]
        );
        assert!(Cli::try_parse_from(["hodis", "--provider", "duck-duck-go", "rust"]).is_err());
    }

    #[test]
    fn several_providers_are_named_with_commas_or_repeated_flags() {
        let both = [Provider::SearXNG, Provider::Serper];
        assert_eq!(
            parse(&["--provider", "searxng,serper", "rust"])
                .search
                .provider,
            both
        );
        assert_eq!(
            parse(&["--provider", "searxng", "--provider", "serper", "rust"])
                .search
                .provider,
            both
        );
    }

    #[test]
    fn a_query_is_required_outside_a_terminal() {
        let bare = parse(&[]);
        assert!(require_query(&bare, &mut Cli::command(), false).is_err());
        assert!(require_query(&bare, &mut Cli::command(), true).is_ok());
        assert!(require_query(&parse(&["rust"]), &mut Cli::command(), false).is_ok());
        assert!(
            require_query(
                &parse(&["key", "delete", "serper"]),
                &mut Cli::command(),
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn setup_is_a_command_unless_it_is_part_of_a_query() {
        assert!(matches!(parse(&["setup"]).command, Some(Command::Setup)));
        assert!(parse(&["--", "setup"]).command.is_none());
        assert!(parse(&["how", "to", "setup"]).command.is_none());
    }

    #[test]
    fn a_format_needs_a_query_to_format() {
        assert!(Cli::try_parse_from(["hodis", "--format", "json"]).is_err());
    }

    #[test]
    fn the_interactive_search_takes_an_optional_query() {
        let cli = parse(&["-i"]);
        assert!(cli.search.interactive && cli.search.query.is_empty());
        let cli = parse(&["--interactive", "rust", "book"]);
        assert!(cli.search.interactive);
        assert_eq!(cli.search.query, ["rust", "book"]);
        assert!(Cli::try_parse_from(["hodis", "-i", "--format", "json", "rust"]).is_err());
    }
}
