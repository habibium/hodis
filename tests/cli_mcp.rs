//! `hodis mcp` run the way an MCP client runs it: a child process spoken to
//! over standard input and output, with a scratch config naming mock providers.
//! `SERPER_API_KEY` is cleared unless a test sets it.
//!
//! Tests are multi-threaded because they block on the child process while the
//! mock serves it.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SEARXNG_BODY: &str = include_str!("fixtures/healthy.json");
const SERPER_BODY: &str = include_str!("fixtures/serper_healthy.json");

/// `hodis mcp` with a config file holding `settings`.
fn hodis_mcp(dir: &TempDir, settings: &str) -> anyhow::Result<Command> {
    let config = dir.path().join("config.toml");
    fs::write(&config, settings)?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_hodis"));
    command
        .arg("mcp")
        .env("HODIS_CONFIG", config)
        .env(hodis::engine::DATA_DIR_ENV, dir.path())
        .env(hodis::cache::CACHE_DIR_ENV, dir.path())
        .env_remove("SERPER_API_KEY");
    Ok(command)
}

fn send(stdin: &mut impl Write, message: &Value) -> anyhow::Result<()> {
    writeln!(stdin, "{message}")?;
    Ok(stdin.flush()?)
}

/// The next line on stdout, which must be a JSON-RPC message.
fn receive(stdout: &mut impl BufRead) -> anyhow::Result<Value> {
    let mut line = String::new();
    stdout.read_line(&mut line)?;
    let message: Value = serde_json::from_str(&line)
        .map_err(|e| anyhow::anyhow!("stdout carried {line:?}, not a message: {e}"))?;
    assert_eq!(message["jsonrpc"], "2.0", "{message}");
    Ok(message)
}

#[tokio::test(flavor = "multi_thread")]
async fn stdout_carries_mcp_messages_and_nothing_else() -> anyhow::Result<()> {
    let searxng = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEARXNG_BODY))
        .mount(&searxng)
        .await;
    let dir = tempfile::tempdir()?;
    // A misspelt key draws a warning, which belongs on stderr.
    let settings = format!(
        "colour = \"never\"\n[searxng]\nurl = \"{}\"\n",
        searxng.uri()
    );
    let mut child = hodis_mcp(&dir, &settings)?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));

    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}
        }}),
    )?;
    assert_eq!(receive(&mut stdout)?["id"], 1);
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )?;
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "web_search",
            "arguments": {"query": "dns lookup"}
        }}),
    )?;
    let called = receive(&mut stdout)?;
    assert_eq!(called["id"], 2);
    assert_eq!(
        called["result"]["structuredContent"]["results"][0]["title"],
        "DNS Lookup Tool - DNS Tools - MxToolbox"
    );

    // Closing stdin is how a client says goodbye.
    drop(stdin);
    let mut rest = String::new();
    stdout.read_to_string(&mut rest)?;
    assert_eq!(rest, "");
    assert!(child.wait()?.success());
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr is piped")
        .read_to_string(&mut stderr)?;
    assert!(stderr.contains("unknown key `colour`"), "{stderr}");
    Ok(())
}

#[test]
fn a_client_hanging_up_before_the_handshake_ends_it_quietly() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;

    let output = hodis_mcp(&dir, "")?.stdin(Stdio::null()).output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        (&output.stdout[..], &output.stderr[..]),
        (&b""[..], &b""[..])
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_provider_flag_overrides_the_config() -> anyhow::Result<()> {
    let serper = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("X-API-KEY", "key-from-env"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SERPER_BODY))
        .expect(1)
        .mount(&serper)
        .await;
    let dir = tempfile::tempdir()?;
    let settings = format!(
        "provider = \"searxng\"\n[serper]\nurl = \"{}\"\n",
        serper.uri()
    );
    let mut command = hodis_mcp(&dir, &settings)?;
    command
        .args(["--provider", "serper"])
        .env("SERPER_API_KEY", "key-from-env");

    let client = ()
        .serve(TokioChildProcess::new(tokio::process::Command::from(
            command,
        ))?)
        .await?;
    let Value::Object(arguments) = json!({"query": "capital of norway"}) else {
        unreachable!("an object literal");
    };
    let result = client
        .call_tool(CallToolRequestParams::new("web_search").with_arguments(arguments))
        .await?;
    client.cancel().await?;

    let content = result.structured_content.expect("a structured result");
    assert_eq!(content["providers"], json!(["serper"]));
    assert_eq!(content["answer"]["content"], "Oslo");
    Ok(())
}
