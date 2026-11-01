//! The binary searching Serper end to end, against a mock that a scratch config
//! file names in place of the real API. `SERPER_API_KEY` is cleared unless a
//! test sets it, so a key in the developer's environment cannot leak in.
//!
//! Tests are multi-threaded because they block on the child process while the
//! mock serves it.

use std::fs;
use std::process::Command;

use tempfile::TempDir;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HEALTHY_BODY: &str = include_str!("fixtures/serper_healthy.json");

/// hodis with a config file that sends Serper searches to `server`, plus
/// whatever `settings` adds at the top level.
fn hodis(server: &MockServer, dir: &TempDir, settings: &str) -> anyhow::Result<Command> {
    let config = dir.path().join("config.toml");
    fs::write(
        &config,
        format!("{settings}\n[serper]\nurl = \"{}\"\n", server.uri()),
    )?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_hodis"));
    command
        .env("HODIS_CONFIG", config)
        .env(hodis::engine::DATA_DIR_ENV, dir.path())
        .env(hodis::cache::CACHE_DIR_ENV, dir.path())
        .env_remove("SERPER_API_KEY");
    Ok(command)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_key_in_serper_api_key_reaches_serper() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("X-API-KEY", "key-from-env"))
        .and(body_json(serde_json::json!({"q": "capital of norway"})))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&server, &dir, "")?
        .env("SERPER_API_KEY", "key-from-env")
        .args(["--provider", "serper", "--format", "urls"])
        .args(["capital", "of", "norway"])
        .output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)?,
        "https://en.wikipedia.org/wiki/Oslo\n\
         https://www.visitoslo.com/en/\n\
         https://www.britannica.com/place/Oslo\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_config_file_can_make_serper_the_default() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&server, &dir, "provider = \"serper\"")?
        .env("SERPER_API_KEY", "key-from-env")
        .arg("oslo")
        .output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_key_is_reported_with_serpers_reason() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_string(r#"{"message": "Unauthorized.", "statusCode": 403}"#),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&server, &dir, "")?
        .env("SERPER_API_KEY", "wrong-key")
        .args(["--provider", "serper", "oslo"])
        .output()?;

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("Serper refused the search (403 Forbidden): Unauthorized."),
        "{stderr}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_key_in_serper_api_key_stops_before_any_request() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&server, &dir, "")?
        .env("SERPER_API_KEY", "two words")
        .args(["--provider", "serper", "oslo"])
        .output()?;

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("SERPER_API_KEY does not hold a valid key"),
        "{stderr}"
    );
    assert_eq!(server.received_requests().await.map(|r| r.len()), Some(0));
    Ok(())
}

// Pointing the session bus at nothing takes the keychain away, on Linux only.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn without_a_key_or_a_keychain_the_error_says_to_set_serper_api_key() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir()?;

    let output = hodis(&server, &dir, "")?
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", dir.path().join("no-bus").display()),
        )
        .args(["--provider", "serper", "oslo"])
        .output()?;

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("no Serper API key: set SERPER_API_KEY"),
        "{stderr}"
    );
    assert_eq!(server.received_requests().await.map(|r| r.len()), Some(0));
    Ok(())
}
