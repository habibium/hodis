//! Which SearXNG the binary searches when hodis's own is running: one mock
//! listens on the port hodis recorded for it, another at the configured `url`.
//!
//! Tests are multi-threaded because they block on the child process while the
//! mocks serve it.

use std::fs;
use std::process::Command;

use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BODY: &str = include_str!("fixtures/healthy.json");

/// A SearXNG that must get exactly `searches` searches.
async fn searxng(searches: u64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/healthz"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(BODY))
        .expect(searches)
        .mount(&server)
        .await;
    server
}

fn record_as_hodis_s_own(dir: &TempDir, own: &MockServer) -> anyhow::Result<()> {
    let engine = dir.path().join("engine");
    fs::create_dir_all(&engine)?;
    fs::write(engine.join("port"), own.address().port().to_string())?;
    Ok(())
}

fn search(dir: &TempDir, config: &str) -> anyhow::Result<()> {
    let path = dir.path().join("config.toml");
    fs::write(&path, config)?;
    let output = Command::new(env!("CARGO_BIN_EXE_hodis"))
        .env("HODIS_CONFIG", path)
        .env(hodis::engine::DATA_DIR_ENV, dir.path())
        .env(hodis::cache::CACHE_DIR_ENV, dir.path())
        .args(["--format", "json", "rust"])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn embedded_only_searches_hodis_s_own_past_one_that_answers_at_url() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let own = searxng(1).await;
    let other = searxng(0).await;
    record_as_hodis_s_own(&dir, &own)?;

    let url = other.uri();
    search(
        &dir,
        &format!("provider = \"searxng\"\n[searxng]\nurl = \"{url}\"\nembedded = \"only\"\n"),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn embedded_true_leaves_hodis_s_own_alone_while_url_answers() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let own = searxng(0).await;
    let other = searxng(1).await;
    record_as_hodis_s_own(&dir, &own)?;

    let url = other.uri();
    search(
        &dir,
        &format!("provider = \"searxng\"\n[searxng]\nurl = \"{url}\"\nembedded = true\n"),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_to_hodis_s_own_does_not_reuse_results_from_url() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let own = searxng(1).await;
    let other = searxng(1).await;
    record_as_hodis_s_own(&dir, &own)?;

    let url = other.uri();
    let before = format!("provider = \"searxng\"\n[searxng]\nurl = \"{url}\"\n");
    search(&dir, &before)?;
    search(&dir, &format!("{before}embedded = \"only\"\n"))
}
