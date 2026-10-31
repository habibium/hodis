use std::{io, num::NonZeroU32};

use anstream::{AutoStream, ColorChoice};
use anyhow::Result;
use clap::Parser;
use reqwest::Client;
use url::Url;

use hodis::{
    SearchParams, SearchProvider,
    config::{Config, Loaded},
    engine::{Engine, is_reachable},
    output::{ColorWhen, OutputFormat, render, render_diagnostics, render_warning},
    providers::searxng::{SEARXNG_BASE_URL, SearXNGProvider},
};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let Loaded {
        config,
        path,
        unknown_keys,
    } = Config::load()?;
    let format = cli.format.unwrap_or(config.format);
    ColorChoice::from(cli.color.unwrap_or(config.color)).write_global();
    let client = Client::builder().timeout(config.timeout).build()?;
    let mut out = AutoStream::auto(io::stdout().lock());
    let mut err = AutoStream::auto(io::stderr().lock());

    if let Some(path) = &path {
        for key in &unknown_keys {
            render_warning(
                &format!("unknown key `{key}` in {}", path.display()),
                &mut err,
            )?;
        }
    }

    // The default instance is one the user may or may not be running; when
    // nothing answers there, fall back to the engine hodis manages itself.
    let default_url = Url::parse(SEARXNG_BASE_URL)?;
    let base_url = if is_reachable(&client, &default_url).await {
        default_url
    } else {
        Engine::resolve()?.ensure_running(&client, &mut err).await?
    };

    let searxng = SearXNGProvider { base_url };
    let params = SearchParams {
        query: cli.query.join(" "),
        page: cli.page,
    };

    let response = searxng.search(&client, &params).await?;

    render(&response, format, &mut out)?;
    render_diagnostics(&response.diagnostics, &mut err)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::Cli;

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn flags_left_out_defer_to_the_config() {
        let cli = Cli::try_parse_from(["hodis", "rust"]).expect("a bare query parses");
        assert_eq!((cli.format, cli.color), (None, None));
    }
}
