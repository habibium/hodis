use anstream::AutoStream;
use anyhow::Result;
use hodis::{
    SearchProvider,
    output::{render, render_diagnostics},
    providers::searxng::{SEARXNG_BASE_URL, SearXNGProvider},
};
use reqwest::Client;
use std::{io, time::Duration};
use url::Url;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let searxng = SearXNGProvider {
        base_url: Url::parse(SEARXNG_BASE_URL)?,
    };

    let response = searxng.search(&client, "12 GBP to USD").await?;

    let mut out = AutoStream::auto(io::stdout().lock());
    let mut err = AutoStream::auto(io::stderr().lock());
    render(&response, &mut out)?;
    render_diagnostics(&response.diagnostics, &mut err)?;

    Ok(())
}
