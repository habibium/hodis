//! The binary searching SearXNG and Serper at once, against mocks of both that
//! a scratch config file names, with Serper's key in `SERPER_API_KEY`.
//!
//! Tests are multi-threaded because they block on the child process while the
//! mocks serve it.

use std::fs;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SEARXNG_BODY: &str = include_str!("fixtures/healthy.json");
const SERPER_BODY: &str = include_str!("fixtures/serper_healthy.json");

struct Mocks {
    searxng: MockServer,
    serper: MockServer,
}

async fn mocks(searxng: ResponseTemplate, serper: ResponseTemplate) -> Mocks {
    let mocks = Mocks {
        searxng: MockServer::start().await,
        serper: MockServer::start().await,
    };
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(searxng)
        .mount(&mocks.searxng)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(serper)
        .mount(&mocks.serper)
        .await;
    mocks
}

fn answering(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string(body)
}

/// hodis printing json from `mocks`, with `settings` at the top of its config.
fn hodis(mocks: &Mocks, dir: &TempDir, settings: &str) -> anyhow::Result<Command> {
    let config = dir.path().join("config.toml");
    fs::write(
        &config,
        format!(
            "{settings}\n[searxng]\nurl = \"{}\"\n[serper]\nurl = \"{}\"\n",
            mocks.searxng.uri(),
            mocks.serper.uri()
        ),
    )?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_hodis"));
    command
        .env("HODIS_CONFIG", config)
        .env(hodis::engine::DATA_DIR_ENV, dir.path())
        .env("SERPER_API_KEY", "test-key")
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

fn result_providers(document: &Value) -> Vec<&str> {
    let mut providers: Vec<&str> = document["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|result| result["provider"].as_str())
        .collect();
    providers.sort_unstable();
    providers.dedup();
    providers
}

#[tokio::test(flavor = "multi_thread")]
async fn both_providers_results_merge_into_one_list() -> anyhow::Result<()> {
    let mocks = mocks(answering(SEARXNG_BODY), answering(SERPER_BODY)).await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&mocks, &dir, "")?
        .args(["--provider", "searxng,serper", "capital", "of", "norway"])
        .output()?;

    let document = document(&output)?;
    assert_eq!(document["providers"], json!(["searxng", "serper"]));
    assert_eq!(document["results"].as_array().map(Vec::len), Some(6));
    assert_eq!(result_providers(&document), ["searxng", "serper"]);
    assert_eq!(document["answer"]["provider"], "searxng");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_provider_named_first_gives_the_answer() -> anyhow::Result<()> {
    let mocks = mocks(answering(SEARXNG_BODY), answering(SERPER_BODY)).await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&mocks, &dir, "")?
        .args(["--provider", "serper,searxng", "capital", "of", "norway"])
        .output()?;

    let document = document(&output)?;
    assert_eq!(document["providers"], json!(["serper", "searxng"]));
    assert_eq!(document["answer"]["content"], "Oslo");
    assert_eq!(document["answer"]["provider"], "serper");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_config_file_can_name_several_providers() -> anyhow::Result<()> {
    let mocks = mocks(answering(SEARXNG_BODY), answering(SERPER_BODY)).await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&mocks, &dir, "provider = ['searxng', 'serper']")?
        .args(["capital", "of", "norway"])
        .output()?;

    assert_eq!(
        document(&output)?["providers"],
        json!(["searxng", "serper"])
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_out_of_time_is_left_out_and_named() -> anyhow::Result<()> {
    let late = answering(SERPER_BODY).set_delay(Duration::from_secs(5));
    let mocks = mocks(answering(SEARXNG_BODY), late).await;
    let dir = tempfile::tempdir()?;

    let started = Instant::now();
    let output = hodis(&mocks, &dir, "timeout = 1")?
        .args(["--provider", "searxng,serper", "capital", "of", "norway"])
        .output()?;

    assert!(
        started.elapsed() < Duration::from_secs(4),
        "hodis waited for Serper"
    );
    let document = document(&output)?;
    assert_eq!(document["providers"], json!(["searxng"]));
    assert_eq!(
        document["diagnostics"]["failed_providers"],
        json!([{"provider": "serper", "reason": "no answer within 1s"}])
    );
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("warning: cannot search Serper: no answer within 1s"),
        "{stderr}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_provider_is_named_while_the_other_answers() -> anyhow::Result<()> {
    let refusal = ResponseTemplate::new(403).set_body_string(r#"{"message": "Unauthorized."}"#);
    let mocks = mocks(answering(SEARXNG_BODY), refusal).await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&mocks, &dir, "")?
        .args(["--provider", "searxng,serper", "capital", "of", "norway"])
        .output()?;

    let document = document(&output)?;
    assert_eq!(document["providers"], json!(["searxng"]));
    let reason = document["diagnostics"]["failed_providers"][0]["reason"]
        .as_str()
        .unwrap_or_default();
    assert!(
        reason.contains("403") && reason.contains("Unauthorized."),
        "{reason}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn every_provider_failing_is_an_error_naming_each() -> anyhow::Result<()> {
    let mocks = mocks(ResponseTemplate::new(500), ResponseTemplate::new(403)).await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&mocks, &dir, "")?
        .args(["--provider", "searxng,serper", "capital", "of", "norway"])
        .output()?;

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("cannot search any provider: SearXNG: "),
        "{stderr}"
    );
    assert!(
        stderr.contains("; Serper: Serper refused the search (403"),
        "{stderr}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lone_provider_out_of_time_is_an_error() -> anyhow::Result<()> {
    let late = answering(SEARXNG_BODY).set_delay(Duration::from_secs(5));
    let mocks = mocks(late, answering(SERPER_BODY)).await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&mocks, &dir, "timeout = 1")?
        .args(["--provider", "searxng", "capital", "of", "norway"])
        .output()?;

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("cannot search SearXNG"), "{stderr}");
    assert!(stderr.contains("no answer within 1s"), "{stderr}");
    Ok(())
}
