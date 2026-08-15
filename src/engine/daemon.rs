//! Supervision of the embedded SearXNG process.
//!
//! The instance outlives the hodis invocation that started it, so later
//! searches pay no startup cost. What survives between invocations is the port
//! file and the pid file; liveness is decided by asking the instance itself
//! over HTTP rather than by inspecting the process table, which keeps the
//! check portable and, more importantly, makes "healthy" mean "actually able to
//! serve a search".

use std::fs;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Client;
use url::Url;

use super::paths::Paths;

/// How long a health probe may take. The endpoint is served from memory, so a
/// slow answer means a busy or wedged instance either way.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Loopback URL of an instance listening on `port`.
pub fn base_url(port: u16) -> Result<Url> {
    Url::parse(&format!("http://127.0.0.1:{port}/")).context("loopback url is malformed")
}

/// Reports whether a SearXNG instance is answering at `base_url`.
///
/// `/healthz` is served locally and never reaches an upstream engine, so it is
/// safe to poll as often as needed.
pub async fn is_healthy(client: &Client, base_url: &Url) -> bool {
    let Ok(url) = base_url.join("healthz") else {
        return false;
    };
    let response = client.get(url).timeout(PROBE_TIMEOUT).send().await;
    matches!(response, Ok(r) if r.status().is_success())
}

/// Reads the port recorded when the instance was provisioned.
pub fn read_port(paths: &Paths) -> Option<u16> {
    fs::read_to_string(paths.portfile())
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Reads the process id of the instance hodis last started.
pub fn read_pid(paths: &Paths) -> Option<u32> {
    fs::read_to_string(paths.pidfile())
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Starts `python -m searx.webapp` in the background and records its pid.
pub fn spawn(paths: &Paths) -> Result<u32> {
    let log_path = paths.log();
    let log = fs::File::create(&log_path)
        .with_context(|| format!("cannot open {} for the instance log", log_path.display()))?;

    let mut command = Command::new(paths.venv_python());
    command
        .args(["-m", "searx.webapp"])
        .env("SEARXNG_SETTINGS_PATH", paths.settings())
        .current_dir(paths.engine())
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log.try_clone().context("cannot share the instance log")?,
        ))
        .stderr(Stdio::from(log));
    detach(&mut command);

    // The child is deliberately not awaited: hodis exits while the instance
    // keeps running, and the operating system reparents it.
    let child = command
        .spawn()
        .with_context(|| format!("cannot start {}", paths.venv_python().display()))?;
    let pid = child.id();

    fs::write(paths.pidfile(), pid.to_string())
        .with_context(|| format!("cannot write {}", paths.pidfile().display()))?;
    Ok(pid)
}

/// Terminates a recorded instance and clears its pid file.
///
/// Signalling only happens once the recorded port is confirmed to be serving
/// SearXNG, so a stale pid file whose number has since been recycled by an
/// unrelated process cannot make hodis kill a stranger.
pub async fn stop(client: &Client, paths: &Paths) -> Result<bool> {
    let Some(pid) = read_pid(paths) else {
        return Ok(false);
    };

    let running = match read_port(paths) {
        Some(port) => is_healthy(client, &base_url(port)?).await,
        None => false,
    };
    if !running {
        let _ = fs::remove_file(paths.pidfile());
        return Ok(false);
    }

    let status = terminate_command(pid)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("cannot run the platform's process termination tool")?;
    anyhow::ensure!(status.success(), "could not stop the instance (pid {pid})");

    let _ = fs::remove_file(paths.pidfile());
    Ok(true)
}

/// Detaches the instance from hodis's terminal so it survives the CLI exiting
/// and ignores terminal signals such as Ctrl-C.
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

/// Signalling is delegated to the platform's standard tool rather than to a
/// libc or Win32 binding, so process control costs no extra dependency and no
/// unsafe code.
#[cfg(unix)]
fn terminate_command(pid: u32) -> Command {
    let mut command = Command::new("kill");
    command.args(["-TERM".to_owned(), pid.to_string()]);
    command
}

#[cfg(windows)]
fn terminate_command(pid: u32) -> Command {
    let mut command = Command::new("taskkill");
    command.args([
        "/PID".to_owned(),
        pid.to_string(),
        "/T".to_owned(),
        "/F".to_owned(),
    ]);
    command
}
