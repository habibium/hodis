use std::{io, num::NonZeroU32, time::Duration};

use anstream::AutoStream;
use anyhow::Result;
use clap::Parser;
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
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let mut out = AutoStream::auto(io::stdout().lock());
    let mut err = AutoStream::auto(io::stderr().lock());

    // The default instance is a local one the user may or may not be running.
    // When nothing answers there, hodis falls back to the SearXNG it installs
    // and manages itself, so a fresh machine needs no setup.
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
