use std::{io, num::NonZeroU32, time::Duration};

use anstream::{AutoStream, ColorChoice};
use anyhow::Result;
use clap::{Parser, ValueEnum};
use reqwest::Client;
use url::Url;

use hodis::{
    SearchParams, SearchProvider,
    engine::{Engine, is_reachable},
    output::{render, render_diagnostics},
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
    /// When to use colors
    #[arg(long, value_name = "WHEN", value_enum, default_value_t = ColorWhen::Auto)]
    color: ColorWhen,
}

#[derive(Clone, Copy, ValueEnum)]
enum ColorWhen {
    Auto,
    Always,
    Never,
}

impl From<ColorWhen> for ColorChoice {
    fn from(when: ColorWhen) -> Self {
        match when {
            ColorWhen::Auto => Self::Auto,
            ColorWhen::Always => Self::Always,
            ColorWhen::Never => Self::Never,
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    ColorChoice::from(cli.color).write_global();
    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let mut out = AutoStream::auto(io::stdout().lock());
    let mut err = AutoStream::auto(io::stderr().lock());

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

    render(&response, &mut out)?;
    render_diagnostics(&response.diagnostics, &mut err)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, ColorWhen};

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn color_is_automatic_by_default() {
        let cli = Cli::try_parse_from(["hodis", "rust"]).expect("a bare query parses");
        assert!(matches!(cli.color, ColorWhen::Auto));
    }
}
