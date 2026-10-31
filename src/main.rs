use std::{
    io::{self, IsTerminal, Write},
    num::NonZeroU32,
};

use anstream::{AutoStream, ColorChoice};
use anyhow::{Context, Result};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use reqwest::Client;

use hodis::{
    SearchParams, SearchProvider,
    config::{self, Config, Loaded},
    engine::{Engine, is_reachable},
    keys::{ApiKey, Keychain, KeyedProvider},
    output::{ColorWhen, OutputFormat, render, render_diagnostics, render_warning},
    providers::searxng::SearXNGProvider,
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
    /// Your search query
    #[arg(required = true)]
    query: Vec<String>,
    /// Result page number (starts at 1)
    #[arg(short, long, value_name = "NUMBER")]
    page: Option<NonZeroU32>,
    /// Output format (default: pretty)
    #[arg(short, long, value_enum)]
    format: Option<OutputFormat>,
    /// When to use colors (default: auto)
    #[arg(long, value_name = "WHEN", value_enum)]
    color: Option<ColorWhen>,
}

#[derive(Subcommand)]
enum Command {
    /// Store or delete the API keys of providers that need one
    #[command(subcommand)]
    Key(KeyCommand),
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

    match cli.command {
        Some(Command::Key(command)) => manage_key(command, &mut err),
        None => search(cli.search, config, &mut err).await,
    }
}

async fn search(args: Search, config: Config, err: &mut impl Write) -> Result<()> {
    let format = args.format.unwrap_or(config.format);
    let client = Client::builder().timeout(config.timeout).build()?;
    let mut out = AutoStream::auto(io::stdout().lock());

    // The instance is one the user may or may not be running; when nothing
    // answers there, fall back to the engine hodis manages itself, if allowed.
    let instance = config.searxng;
    let base_url = if !instance.embedded || is_reachable(&client, &instance.url).await {
        instance.url
    } else {
        Engine::resolve()?.ensure_running(&client, err).await?
    };

    let searxng = SearXNGProvider { base_url };
    let params = SearchParams {
        query: args.query.join(" "),
        page: args.page,
    };

    let response = searxng.search(&client, &params).await?;

    match render(&response, format, &mut out) {
        // A reader that stops early, such as `head`, closes the pipe: that ends
        // the output, the way it ends any other Unix tool's, without an error.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
        result => result?,
    }
    render_diagnostics(&response.diagnostics, err)?;

    Ok(())
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

/// `Cli::parse`, with `--help` naming the config file.
fn parse_args() -> Cli {
    let mut command = Cli::command();
    if let Some(path) = config::path() {
        command = command.after_help(format!("Config file: {}", path.display()));
    }
    Cli::from_arg_matches(&command.get_matches()).unwrap_or_else(|e| e.exit())
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, Command, KeyCommand, KeyedProvider};

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from([&["hodis"], args].concat()).expect("the arguments parse")
    }

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
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
    fn a_double_dash_searches_for_a_command_name() {
        let cli = parse(&["--", "key"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.search.query, ["key"]);
    }

    #[test]
    fn a_query_is_still_required_without_a_command() {
        assert!(Cli::try_parse_from(["hodis"]).is_err());
        assert!(Cli::try_parse_from(["hodis", "--format", "json"]).is_err());
    }
}
