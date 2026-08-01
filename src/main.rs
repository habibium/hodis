use anyhow::Result;
use hodis::{
    SearchProvider,
    providers::searxng::{SEARXNG_BASE_URL, SearXNGProvider},
};
use reqwest::Client;
use std::{fs::File, io::BufWriter, time::Duration};
use url::Url;

const RESULT_FILE: &str = "local/result.json";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let searxng = SearXNGProvider {
        base_url: Url::parse(SEARXNG_BASE_URL)?,
    };

    let results = searxng.search(&client, "12 GBP to USD").await?;
    let file = File::create(RESULT_FILE)?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, &results)?;
    println!("Results wrote to {RESULT_FILE}");

    Ok(())
}
