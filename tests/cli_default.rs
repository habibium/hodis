//! The binary with no provider chosen, against mocks of SearXNG and
//! DuckDuckGo that a scratch config file names: it searches SearXNG when the
//! instance answers, and otherwise DuckDuckGo.
//!
//! Tests are multi-threaded because they block on the child process while the
//! mocks serve it.

use std::fs;
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SEARXNG_BODY: &str = include_str!("fixtures/healthy.json");
const DUCKDUCKGO_PAGE: &str = include_str!("fixtures/duckduckgo_healthy.html");

/// hodis printing json, with `searxng` and `duckduckgo` standing in for the
/// real services and hodis allowed to run its own SearXNG.
fn hodis(searxng: &MockServer, duckduckgo: &MockServer, dir: &TempDir) -> anyhow::Result<Command> {
    let config = dir.path().join("config.toml");
    fs::write(
        &config,
        format!(
            "[searxng]\nurl = \"{}\"\nembedded = true\n[duckduckgo]\nurl = \"{}\"\n",
            searxng.uri(),
            duckduckgo.uri()
        ),
    )?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_hodis"));
    command
        .env("HODIS_CONFIG", config)
        .env(hodis::engine::DATA_DIR_ENV, dir.path())
        .env(hodis::cache::CACHE_DIR_ENV, dir.path())
        .args(["--format", "json"]);
    Ok(command)
}

fn document(output: &Output) -> anyhow::Result<Value> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

async fn duckduckgo(searches: u64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/html/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(DUCKDUCKGO_PAGE))
        .expect(searches)
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn a_searxng_instance_that_answers_is_searched() -> anyhow::Result<()> {
    let searxng = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/healthz"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&searxng)
        .await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEARXNG_BODY))
        .expect(1)
        .mount(&searxng)
        .await;
    let duckduckgo = duckduckgo(0).await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&searxng, &duckduckgo, &dir)?
        .args(["dns", "lookup"])
        .output()?;

    assert_eq!(document(&output)?["providers"], json!(["searxng"]));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn without_searxng_duckduckgo_is_searched_and_nothing_is_installed() -> anyhow::Result<()> {
    // Answers nothing, not even its health check.
    let searxng = MockServer::start().await;
    let duckduckgo = duckduckgo(1).await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&searxng, &duckduckgo, &dir)?
        .args(["rust", "borrow", "checker"])
        .output()?;

    let document = document(&output)?;
    assert_eq!(document["providers"], json!(["duckduckgo"]));
    assert_eq!(document["results"].as_array().map(Vec::len), Some(10));
    for installed in ["bin", "engine"] {
        assert!(
            !dir.path().join(installed).exists(),
            "hodis set up its own SearXNG"
        );
    }
    let stderr = String::from_utf8(output.stderr)?;
    assert!(!stderr.contains("setup:"), "{stderr}");
    Ok(())
}
