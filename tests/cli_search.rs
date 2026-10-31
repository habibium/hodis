//! The binary end to end, searching a mock SearXNG instance that a scratch
//! config file names, so nothing reaches the network.

use std::fs;
use std::process::{Command, Stdio};
use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HEALTHY_BODY: &str = include_str!("fixtures/healthy.json");

// Multi-threaded, because the test blocks on the child while the mock serves it.
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
    let config = dir.path().join("config.toml");
    fs::write(&config, format!("[searxng]\nurl = \"{}\"\n", server.uri()))?;

    let mut child = Command::new(env!("CARGO_BIN_EXE_hodis"))
        .env("HODIS_CONFIG", &config)
        .env(hodis::engine::DATA_DIR_ENV, dir.path())
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
