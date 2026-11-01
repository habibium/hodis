//! The binary end to end, searching a mock SearXNG instance that a scratch
//! config file names, so nothing reaches the network.
//!
//! Tests are multi-threaded because they block on the child process while the
//! mock serves it.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HEALTHY_BODY: &str = include_str!("fixtures/healthy.json");

/// A config file in `dir` naming `server` as the SearXNG instance to search.
fn config_for(server: &MockServer, dir: &TempDir) -> anyhow::Result<PathBuf> {
    let config = dir.path().join("config.toml");
    fs::write(
        &config,
        format!(
            "provider = \"searxng\"\n[searxng]\nurl = \"{}\"\n",
            server.uri()
        ),
    )?;
    Ok(config)
}

fn hodis(server: &MockServer, dir: &TempDir) -> anyhow::Result<Command> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hodis"));
    command
        .env("HODIS_CONFIG", config_for(server, dir)?)
        .env(hodis::engine::DATA_DIR_ENV, dir.path())
        .env(hodis::cache::CACHE_DIR_ENV, dir.path());
    Ok(command)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_that_stops_early_ends_the_output_quietly() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(HEALTHY_BODY)
                // Holds the results back until the reader below is long gone.
                .set_delay(Duration::from_millis(200)),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir()?;

    let mut child = hodis(&server, &dir)?
        .args(["--format", "urls", "rust"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    drop(child.stdout.take());
    let output = child.wait_with_output()?;

    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert!(output.status.success(), "{}", output.status);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_double_dash_searches_for_a_command_name() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&server, &dir)?.args(["--", "key"]).output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repeated_search_is_answered_from_the_cache_unless_told_not_to() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(2)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir()?;
    let urls = |extra: &[&str]| -> anyhow::Result<String> {
        let output = hodis(&server, &dir)?
            .args(["--format", "urls"])
            .args(extra)
            .args(["dns", "lookup"])
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8(output.stdout)?)
    };

    let searched = urls(&[])?;
    let cached = urls(&[])?;
    let fresh = urls(&["--no-cache"])?;

    assert_eq!(cached, searched);
    assert_eq!(fresh, searched);
    Ok(())
}
