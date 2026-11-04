//! The embedded SearXNG instance hodis installs, starts and reuses.
//!
//! Lifecycle is re-read from disk each call: running (recorded port answers a
//! probe), provisioned (interpreter, settings and a matching marker), or
//! absent. A mismatched marker counts as absent, so upgrading hodis
//! re-provisions rather than running a stale engine.
//!
//! Provisioning needs nothing on the host: no tar, no git, no Python, no
//! compiler.

mod archive;
mod daemon;
mod paths;
mod settings;
mod uv_output;

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::{MissedTickBehavior, interval};
use url::Url;

use crate::progress::{Progress, TICK};
pub use paths::{DATA_DIR_ENV, Paths};
use uv_output::UvOutput;

/// Downloaded into hodis's data directory rather than taken from `PATH`, so a
/// bootstrap behaves identically on every machine.
pub const UV_VERSION: &str = "0.11.13";

pub const PYTHON_VERSION: &str = "3.13";

pub const SEARXNG_REV: &str = "094c33d406da578ac851f2d8ad10fdfa344a9493";

/// Upstream derives this from git; hodis must state it. Update with
/// [`SEARXNG_REV`], whose short hash it embeds.
pub const SEARXNG_VERSION: &str = "2026.8.14+094c33d40";

const SEARXNG_GIT_URL: &str = "https://github.com/searxng/searxng";

const SEARXNG_GIT_BRANCH: &str = "master";

/// Bumped when hodis builds [`SEARXNG_REV`] differently enough that installs
/// made the old way are wrong; recorded so they get replaced.
const PROVISION_RECIPE: &str = "3";

/// SearXNG parses its engine definitions before it serves, which a virus
/// scanner reading every file for the first time can make slow.
const STARTUP_TIMEOUT: Duration = Duration::from_mins(3);

const STARTUP_POLL: Duration = Duration::from_millis(250);

/// Tens of megabytes, so the short timeout suiting a search would not do.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(10);

/// Each step's share of the bar, near enough the time it takes.
const UV_WEIGHT: u64 = 15;
const PROVISION_WEIGHTS: [u64; 7] = [30, 2, 5, 25, 15, 3, 5];

/// What a log's last lines say, in an error message.
const TAIL_LINES: usize = 12;

/// About what Python takes up unpacked. uv shows nothing as it downloads with
/// its output piped, so the bar follows the files appearing instead.
const PYTHON_SIZE: u64 = 100 * 1024 * 1024;

/// How often to measure them.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// Held while making sure the engine runs, so that searches made at once by one
/// process set up and start a single instance between them.
static SETUP: Mutex<()> = Mutex::const_new(());

/// A local health endpoint, so probing costs an upstream engine nothing.
pub async fn is_reachable(client: &Client, base_url: &Url) -> bool {
    daemon::is_healthy(client, base_url).await
}

/// The SearXNG instance hodis manages for itself.
#[derive(Debug)]
pub struct Engine {
    paths: Paths,
}

impl Engine {
    /// Places the engine in the current user's data directory.
    pub fn resolve() -> Result<Self> {
        Ok(Self::new(Paths::resolve()?))
    }

    #[must_use]
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    #[must_use]
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Returns the base URL of a healthy instance, installing and starting one
    /// if needed.
    pub async fn ensure_running<W: Write>(
        &self,
        client: &Client,
        progress: &mut Progress<W>,
    ) -> Result<Url> {
        let _setup = SETUP.lock().await;
        if let Some(url) = self.running_url(client).await {
            return Ok(url);
        }

        if self.is_provisioned() {
            progress.plan(&[1]);
        } else {
            self.provision(client, progress).await?;
        }

        let url = self.start(client, progress).await?;
        progress.finish();
        Ok(url)
    }

    /// Reports whether one was actually running.
    pub async fn stop(&self, client: &Client) -> Result<bool> {
        daemon::stop(client, &self.paths).await
    }

    async fn running_url(&self, client: &Client) -> Option<Url> {
        let port = daemon::read_port(&self.paths)?;
        let url = daemon::base_url(port).ok()?;
        daemon::is_healthy(client, &url).await.then_some(url)
    }

    /// Whether SearXNG is installed and only needs starting.
    #[must_use]
    pub fn is_provisioned(&self) -> bool {
        self.paths.venv_python().is_file()
            && self.paths.settings().is_file()
            && fs::read_to_string(self.paths.revision())
                .is_ok_and(|marker| marker.trim() == provision_marker())
    }

    async fn provision<W: Write>(&self, client: &Client, progress: &mut Progress<W>) -> Result<()> {
        let engine = self.paths.engine();
        fs::create_dir_all(&engine)
            .with_context(|| format!("cannot create {}", engine.display()))?;
        let log_path = self.paths.setup_log();
        let mut log = File::create(&log_path)
            .with_context(|| format!("cannot create {}", log_path.display()))?;

        let needs_uv = !self.paths.uv().is_file();
        progress.heading(&format!(
            "Setting up SearXNG in {}",
            self.paths.root().display()
        ));
        let mut weights = Vec::from(PROVISION_WEIGHTS);
        if needs_uv {
            weights.insert(0, UV_WEIGHT);
        }
        progress.plan(&weights);

        if needs_uv {
            self.install_uv(client, progress).await?;
        }
        self.install_python(&mut log, progress).await?;

        progress.step(&format!("Downloading SearXNG {}", short_version()));
        let source = self.fetch_searxng_source(client, progress).await?;

        self.install_searxng(&source, &mut log, progress).await?;
        self.finish_install(progress).await
    }

    async fn install_python<W: Write>(
        &self,
        log: &mut File,
        progress: &mut Progress<W>,
    ) -> Result<()> {
        progress.step(&format!("Installing Python {PYTHON_VERSION}"));
        // Without these, uv also writes outside the data directory: a python
        // launcher in ~/.local/bin and, on Windows, an entry in the registry.
        self.run_uv_watching(
            self.uv()
                .args(["python", "install", "--no-bin", "--no-registry"])
                .arg(PYTHON_VERSION),
            "install Python",
            log,
            progress,
            Some(&self.paths.python_install_dir()),
        )
        .await?;
        self.added(&self.paths.python_install_dir(), progress);

        progress.step("Creating a virtual environment");
        // `--clear` because an earlier attempt that was cut short leaves one,
        // and uv otherwise refuses to go on.
        self.run_uv(
            self.uv()
                .args(["venv", "--clear", "--python", PYTHON_VERSION])
                .arg(self.paths.venv()),
            "create the virtual environment",
            log,
            progress,
        )
        .await?;
        self.added(&self.paths.venv(), progress);
        Ok(())
    }

    async fn install_searxng<W: Write>(
        &self,
        source: &Path,
        log: &mut File,
        progress: &mut Progress<W>,
    ) -> Result<()> {
        progress.step("Installing Python packages");
        // Wheels only: an end-user machine has no compiler, so a dependency
        // needing a build must fail loudly rather than emit compiler errors.
        self.run_uv(
            self.uv()
                .arg("pip")
                .arg("install")
                .arg("--python")
                .arg(self.paths.venv_python())
                .arg("--only-binary")
                .arg(":all:")
                .arg("-r")
                .arg(source.join("requirements.txt"))
                // The build below runs from this environment, not an isolated
                // one, so its backend must be installed here.
                .arg("setuptools")
                .arg("wheel"),
            "install SearXNG's dependencies",
            log,
            progress,
        )
        .await?;

        progress.step("Installing SearXNG");
        // SearXNG's setup.py imports the searx package, pulling in msgspec and
        // pyyaml without declaring either as a build requirement; only an
        // unisolated build against the environment above satisfies them.
        // Compiling now spares the first start, which has a deadline, the work.
        self.run_uv(
            self.uv()
                .arg("pip")
                .arg("install")
                .arg("--python")
                .arg(self.paths.venv_python())
                .arg("--compile-bytecode")
                .arg("--no-build-isolation-package")
                .arg("searxng")
                .arg(searxng_requirement(source)?),
            "install SearXNG",
            log,
            progress,
        )
        .await
    }

    async fn finish_install<W: Write>(&self, progress: &mut Progress<W>) -> Result<()> {
        progress.step("Finishing up");
        // SearXNG imports the unix-only `pwd` unconditionally. Written after
        // the install because uv owns site-packages until then.
        if cfg!(windows) {
            let stub = self.paths.pwd_stub();
            fs::write(&stub, pwd_stub())
                .with_context(|| format!("cannot write {}", stub.display()))?;
            self.added(&stub, progress);
        }

        let _ = fs::remove_dir_all(self.paths.source());
        progress.note(&format!("removed {}", self.shown(&self.paths.source())));

        self.write_settings()?;
        self.added(&self.paths.settings(), progress);
        fs::write(self.paths.revision(), provision_marker())
            .with_context(|| format!("cannot write {}", self.paths.revision().display()))?;

        // The cache rivals the installation in size and only pays off on a
        // re-provision. Best-effort: keeping it is not a failure.
        let cleaned = self
            .uv()
            .args(["cache", "clean"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        if cleaned.is_ok_and(|status| status.success()) {
            progress.note(&format!("removed {}", self.shown(&self.paths.uv_cache())));
        }
        Ok(())
    }

    fn added<W: Write>(&self, path: &Path, progress: &mut Progress<W>) {
        progress.note(&format!("added {}", self.shown(path)));
    }

    /// `path` from the data directory, which the heading has already named.
    fn shown(&self, path: &Path) -> String {
        path.strip_prefix(self.paths.root())
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// Unpacks the pinned source and freezes its version, returning the
    /// directory to install from.
    ///
    /// A local tree, not the archive URL: `searx/version.py` falls back to
    /// `git show` when `searx.version_frozen` is missing, and archives omit
    /// that module, so a git-less machine dies with `FileNotFoundError: 'git'`.
    /// Writing it first, as upstream's container image does, needs the source
    /// unpacked; a URL requirement would bring the git dependency back.
    async fn fetch_searxng_source<W: Write>(
        &self,
        client: &Client,
        progress: &mut Progress<W>,
    ) -> Result<PathBuf> {
        let scratch = self.paths.source();
        // An interrupted provision would otherwise leave a half-unpacked tree.
        let _ = fs::remove_dir_all(&scratch);

        let url = format!("https://github.com/searxng/searxng/archive/{SEARXNG_REV}.tar.gz");
        let bytes = download(client, &url, progress).await?;
        let unpacked = unpack("searxng.tar.gz", bytes, scratch.clone()).await?;
        progress.note(&format!(
            "unpacked {} files into {}",
            unpacked.files,
            self.shown(&scratch)
        ));

        // The archive wraps the tree in a directory named after the revision.
        let tree = find_shallow(&scratch, OsStr::new("setup.py"))
            .and_then(|setup| setup.parent().map(Path::to_path_buf))
            .context("the SearXNG archive contained no setup.py")?;

        let frozen = tree.join("searx").join("version_frozen.py");
        fs::write(&frozen, version_frozen())
            .with_context(|| format!("cannot write {}", frozen.display()))?;

        Ok(tree)
    }

    async fn install_uv<W: Write>(
        &self,
        client: &Client,
        progress: &mut Progress<W>,
    ) -> Result<()> {
        let asset = uv_asset()?;
        progress.step(&format!("Downloading uv {UV_VERSION}"));

        let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{asset}");
        let bytes = download(client, &url, progress).await?;

        let unpacked = self.paths.root().join("bin").join("unpacked");
        // An interrupted attempt would otherwise leave its debris here.
        let _ = fs::remove_dir_all(&unpacked);
        unpack(asset, bytes, unpacked.clone()).await?;

        let executable = self.paths.uv();
        let name = executable
            .file_name()
            .expect("the uv path always ends in a file name");
        let found = find_shallow(&unpacked, name)
            .with_context(|| format!("the uv release {asset} contained no {}", name.display()))?;
        fs::rename(&found, &executable)
            .with_context(|| format!("cannot move uv into {}", executable.display()))?;
        let _ = fs::remove_dir_all(&unpacked);

        self.added(&executable, progress);
        Ok(())
    }

    fn write_settings(&self) -> Result<()> {
        let port = settings::choose_port(daemon::read_port(&self.paths))?;
        let secret = settings::secret_key()?;

        let file = self.paths.settings();
        fs::write(&file, settings::render(&secret, port))
            .with_context(|| format!("cannot write {}", file.display()))?;
        fs::write(self.paths.portfile(), port.to_string())
            .with_context(|| format!("cannot write {}", self.paths.portfile().display()))?;
        Ok(())
    }

    async fn start<W: Write>(&self, client: &Client, progress: &mut Progress<W>) -> Result<Url> {
        let port = daemon::read_port(&self.paths)
            .context("the local search engine has no recorded port")?;
        let url = daemon::base_url(port)?;

        progress.step("Starting SearXNG");
        let mut instance = daemon::spawn(&self.paths)?;

        let began = Instant::now();
        while began.elapsed() < STARTUP_TIMEOUT {
            if daemon::is_healthy(client, &url).await {
                progress.note(&format!("running at {url}"));
                return Ok(url);
            }
            // Only a failure counts: a launcher may hand over and exit cleanly.
            if let Ok(Some(status)) = instance.try_wait()
                && !status.success()
            {
                bail!(
                    "SearXNG stopped while starting ({status})\n{}\nIts log is at {}",
                    tail(&self.paths.log()),
                    self.paths.log().display()
                );
            }
            sleep_ticking(STARTUP_POLL, progress).await;
        }

        // Left running, it would hold the port, and on Windows its own files
        // too, against the next attempt.
        let _ = daemon::terminate(instance.id());
        bail!(
            "SearXNG did not answer within {}s\n{}\nIts log is at {}",
            STARTUP_TIMEOUT.as_secs(),
            tail(&self.paths.log()),
            self.paths.log().display()
        )
    }

    async fn run_uv<W: Write>(
        &self,
        command: &mut Command,
        what: &str,
        log: &mut File,
        progress: &mut Progress<W>,
    ) -> Result<()> {
        self.run_uv_watching(command, what, log, progress, None)
            .await
    }

    /// Runs uv to do `what`, turning what it prints into progress and keeping
    /// all of it in the setup log. Run with its output piped, it prints plain
    /// lines and no animation. If it says nothing as it goes, `watched` is the
    /// directory to follow instead.
    async fn run_uv_watching<W: Write>(
        &self,
        command: &mut Command,
        what: &str,
        log: &mut File,
        progress: &mut Progress<W>,
        watched: Option<&Path>,
    ) -> Result<()> {
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("cannot run uv to {what}"))?;
        let stderr = child.stderr.take().expect("stderr is piped");
        let mut lines = BufReader::new(stderr).split(b'\n');

        let mut output = UvOutput::default();
        let mut last = VecDeque::with_capacity(TAIL_LINES);
        let mut ticks = interval(TICK);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut measured = Instant::now();

        loop {
            tokio::select! {
                line = lines.next_segment() => {
                    let Some(line) = line.with_context(|| format!("cannot read uv's output to {what}"))? else {
                        break;
                    };
                    let line = String::from_utf8_lossy(&line);
                    let line = line.trim_end();
                    let _ = writeln!(log, "{line}");
                    if last.len() == TAIL_LINES {
                        last.pop_front();
                    }
                    last.push_back(line.to_owned());
                    output.read(line, progress);
                }
                _ = ticks.tick() => {
                    if let Some(watched) = watched
                        && measured.elapsed() >= WATCH_INTERVAL
                        && !output.is_finishing()
                    {
                        measured = Instant::now();
                        let size = directory_size(watched).min(PYTHON_SIZE);
                        progress.share(size, PYTHON_SIZE + PYTHON_SIZE / 4);
                    }
                    progress.tick();
                }
            }
        }

        let status = child
            .wait()
            .await
            .with_context(|| format!("cannot run uv to {what}"))?;
        if !status.success() {
            let mut message = format!("uv could not {what}:");
            for line in last {
                let _ = write!(message, "\n  {line}");
            }
            let _ = write!(
                message,
                "\nThe full log is at {}",
                self.paths.setup_log().display()
            );
            bail!(message);
        }
        Ok(())
    }

    /// Confined to hodis's own cache and interpreter directory, so it neither
    /// reads nor disturbs a uv the user may have configured.
    fn uv(&self) -> Command {
        let mut command = Command::new(self.paths.uv());
        command
            .env("UV_CACHE_DIR", self.paths.uv_cache())
            .env("UV_PYTHON_INSTALL_DIR", self.paths.python_install_dir())
            // A system interpreter is untested against this SearXNG revision.
            .env("UV_MANAGED_PYTHON", "1")
            // Ignore any uv.toml or pyproject.toml near the working directory.
            .env("UV_NO_CONFIG", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        command
    }
}

/// Empty by design: a fake `getpwuid` would invent account data, whereas an
/// `AttributeError` names the problem if upstream adds a second call site.
fn pwd_stub() -> String {
    "\"\"\"Stand-in for the unix-only `pwd` module, written by hodis.

SearXNG's `searx/valkeydb.py` imports `pwd` unconditionally, so on Windows
`import searx.webapp` fails before the application starts. Its only use,
`pwd.getpwuid(os.getuid())`, also needs `os.getuid`, absent on Windows too, and
sits in error logging for a Valkey connection hodis never configures, so
satisfying the import is enough.

This module intentionally defines nothing: a missing `getpwuid` raises an
AttributeError naming the problem, which beats invented account details.
\"\"\"
"
    .to_owned()
}

/// [`SEARXNG_VERSION`] without the commit, for reading.
#[must_use]
pub fn short_version() -> &'static str {
    SEARXNG_VERSION
        .split_once('+')
        .map_or(SEARXNG_VERSION, |(version, _)| version)
}

/// Carries the recipe as well as the revision: an install predating a change in
/// how hodis builds that revision is stale, and a bare revision cannot say so.
fn provision_marker() -> String {
    format!("{SEARXNG_REV} {PROVISION_RECIPE}")
}

/// Naming the package is what makes `--no-build-isolation-package searxng`
/// apply: given a bare directory, uv cannot identify the package until the
/// build it is meant to configure has already run.
fn searxng_requirement(source: &Path) -> Result<String> {
    let absolute = std::path::absolute(source)
        .with_context(|| format!("cannot resolve {}", source.display()))?;
    let url = Url::from_file_path(&absolute)
        .map_err(|()| anyhow!("{} is not a usable file url", absolute.display()))?;
    Ok(format!("searxng @ {url}"))
}

/// What `searx/version.py` writes when asked to freeze; importing it is the one
/// path through that module which never invokes git. The Docker tag swaps `+`
/// for `-` because a container tag cannot hold a plus.
fn version_frozen() -> String {
    let docker_tag = SEARXNG_VERSION.replace('+', "-");
    format!(
        "# SPDX-License-Identifier: AGPL-3.0-or-later
# pylint: disable=missing-module-docstring
# this file is generated automatically by searx/version.py

VERSION_STRING = \"{SEARXNG_VERSION}\"
VERSION_TAG = \"{SEARXNG_VERSION}\"
DOCKER_TAG = \"{docker_tag}\"
GIT_URL = \"{SEARXNG_GIT_URL}\"
GIT_BRANCH = \"{SEARXNG_GIT_BRANCH}\"
"
    )
}

fn uv_asset() -> Result<&'static str> {
    let linux_libc = if cfg!(target_env = "musl") {
        "musl"
    } else {
        "gnu"
    };

    let asset = match (std::env::consts::OS, std::env::consts::ARCH, linux_libc) {
        ("linux", "x86_64", "gnu") => "uv-x86_64-unknown-linux-gnu.tar.gz",
        ("linux", "x86_64", _) => "uv-x86_64-unknown-linux-musl.tar.gz",
        ("linux", "aarch64", "gnu") => "uv-aarch64-unknown-linux-gnu.tar.gz",
        ("linux", "aarch64", _) => "uv-aarch64-unknown-linux-musl.tar.gz",
        ("macos", "x86_64", _) => "uv-x86_64-apple-darwin.tar.gz",
        ("macos", "aarch64", _) => "uv-aarch64-apple-darwin.tar.gz",
        ("windows", "x86_64", _) => "uv-x86_64-pc-windows-msvc.zip",
        ("windows", "aarch64", _) => "uv-aarch64-pc-windows-msvc.zip",
        (os, arch, _) => bail!("hodis cannot set up a local search engine on {os} {arch}"),
    };
    Ok(asset)
}

/// Unpacks away from the runtime, which keeps the bar moving meanwhile.
async fn unpack(name: &str, bytes: Vec<u8>, into: PathBuf) -> Result<archive::Unpacked> {
    let name = name.to_owned();
    tokio::task::spawn_blocking(move || archive::unpack(&name, &bytes, &into))
        .await
        .context("unpacking stopped unexpectedly")?
}

/// Tens of megabytes, reported as they arrive.
async fn download<W: Write>(
    client: &Client,
    url: &str,
    progress: &mut Progress<W>,
) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .with_context(|| format!("cannot download {url}"))?
        .error_for_status()
        .with_context(|| format!("cannot download {url}"))?;

    let total = response.content_length();
    let mut bytes = Vec::new();
    let mut ticks = interval(TICK);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            chunk = response.chunk() => {
                let Some(chunk) = chunk.with_context(|| format!("cannot read the response from {url}"))? else {
                    return Ok(bytes);
                };
                bytes.extend_from_slice(&chunk);
                progress.bytes(bytes.len() as u64, total);
            }
            _ = ticks.tick() => progress.tick(),
        }
    }
}

async fn sleep_ticking<W: Write>(duration: Duration, progress: &mut Progress<W>) {
    progress.tick();
    tokio::time::sleep(duration).await;
}

/// What the files under `path` take up, counting what can be read.
fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => directory_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |data| data.len()),
            _ => 0,
        })
        .sum()
}

/// The end of a log, for an error message.
fn tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    lines[start..].join("\n")
}

/// Finds `name` in `dir` or one level below. uv's tar releases nest their
/// payload in a directory and its zip releases do not, so the depth is searched
/// rather than assumed.
fn find_shallow(dir: &Path, name: &OsStr) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }

    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|e| {
            let nested = e.path().join(name);
            nested.is_file().then_some(nested)
        })
}

/// Leaves `paths` looking like an install whose instance listens on `port`.
#[cfg(test)]
pub(crate) fn fake_install(paths: &Paths, port: u16) -> Result<()> {
    let python = paths.venv_python();
    fs::create_dir_all(python.parent().context("the interpreter has a directory")?)?;
    fs::write(python, "")?;
    fs::write(paths.settings(), "")?;
    fs::write(paths.revision(), provision_marker())?;
    fs::write(paths.portfile(), port.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        Engine, Paths, Progress, SEARXNG_REV, SEARXNG_VERSION, provision_marker, pwd_stub,
        settings, version_frozen,
    };

    #[test]
    fn the_frozen_version_defines_every_name_searxng_unpacks() {
        // A name `searx/version.py` expects but does not find is an
        // AttributeError at build time, not a graceful degradation.
        let frozen = version_frozen();
        for name in [
            "VERSION_STRING",
            "VERSION_TAG",
            "DOCKER_TAG",
            "GIT_URL",
            "GIT_BRANCH",
        ] {
            assert!(
                frozen.contains(&format!("{name} = \"")),
                "version_frozen.py must define {name}:\n{frozen}"
            );
        }
    }

    #[test]
    fn the_docker_tag_replaces_the_version_separator() {
        // Upstream swaps `+` for `-` because a container tag cannot hold `+`.
        let frozen = version_frozen();
        let expected = SEARXNG_VERSION.replace('+', "-");
        assert!(
            frozen.contains(&format!("DOCKER_TAG = \"{expected}\"")),
            "expected DOCKER_TAG {expected}:\n{frozen}"
        );
        assert!(
            !expected.contains('+'),
            "a docker tag must not contain a plus"
        );
    }

    #[test]
    fn the_pinned_version_names_the_pinned_revision() {
        // Moving SEARXNG_REV without SEARXNG_VERSION would make the instance
        // report a version belonging to a different commit.
        let (date, short_hash) = SEARXNG_VERSION
            .split_once('+')
            .expect("the version is date+shorthash");
        assert!(
            SEARXNG_REV.starts_with(short_hash),
            "{short_hash} is not a prefix of the pinned revision {SEARXNG_REV}"
        );
        assert!(
            date.split('.').count() == 3 && date.split('.').all(|p| p.parse::<u32>().is_ok()),
            "expected a YYYY.M.D date, got {date}"
        );
    }

    #[test]
    fn the_provision_marker_distinguishes_recipes_from_revisions() {
        // An install made before the recipe changed must not pass for current.
        let marker = provision_marker();
        assert!(
            marker.starts_with(SEARXNG_REV),
            "the marker still names the revision: {marker}"
        );
        assert_ne!(
            marker, SEARXNG_REV,
            "a bare revision cannot express a recipe change"
        );
    }

    #[test]
    fn the_pwd_stub_defines_nothing_executable() {
        let stub = pwd_stub();
        assert!(
            stub.starts_with("\"\"\"") && stub.trim_end().ends_with("\"\"\""),
            "the stub is one docstring and nothing else:\n{stub}"
        );
        // Delimiters at the first and last byte, and exactly one pair, leave
        // nowhere for a statement to hide. Keywords are not scanned for: the
        // prose discusses `import` and `getpwuid` while defining neither.
        assert_eq!(
            stub.matches("\"\"\"").count(),
            2,
            "exactly one docstring opens and closes:\n{stub}"
        );
    }

    #[test]
    fn the_pwd_stub_says_why_it_exists() {
        // A reader finding this in site-packages must be able to tell it from a
        // real module without leaving the file.
        let stub = pwd_stub();
        for expected in [
            "hodis",
            "valkeydb",
            "getpwuid",
            "os.getuid",
            "AttributeError",
        ] {
            assert!(
                stub.contains(expected),
                "the stub must explain {expected:?}:\n{stub}"
            );
        }
    }

    #[test]
    fn the_pwd_stub_lands_on_the_windows_import_path() {
        // Asserted everywhere though only a Windows provision writes the file.
        let paths = Paths::new(PathBuf::from("/data/hodis"));
        let stub = paths.pwd_stub();

        assert!(
            stub.ends_with("pwd.py"),
            "python imports the module by file name: {}",
            stub.display()
        );
        assert!(
            stub.starts_with(paths.venv()),
            "the stub only shadows hodis's own interpreter: {}",
            stub.display()
        );
        let tail: Vec<_> = stub
            .components()
            .rev()
            .take(3)
            .map(|c| c.as_os_str().to_owned())
            .collect();
        assert_eq!(
            tail,
            ["pwd.py", "site-packages", "Lib"],
            "expected the windows virtualenv import path: {}",
            stub.display()
        );
    }

    #[test]
    fn the_frozen_version_is_python_hodis_did_not_mangle() {
        let frozen = version_frozen();
        assert!(
            frozen.starts_with("# SPDX-License-Identifier: AGPL-3.0-or-later"),
            "the generated module keeps upstream's license header:\n{frozen}"
        );
        assert_eq!(
            frozen.matches('"').count(),
            10,
            "five quoted values and nothing needing escapes:\n{frozen}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn searches_made_at_once_start_one_instance() -> anyhow::Result<()> {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;
        use std::time::Duration;

        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

        let scratch = tempfile::tempdir()?;
        let engine = Engine::new(Paths::new(scratch.path().to_path_buf()));
        let paths = engine.paths();

        // An installed engine whose interpreter records being launched, then
        // leaves the serving to the mock below.
        let launches = scratch.path().join("launches");
        let python = paths.venv_python();
        fs::create_dir_all(
            python
                .parent()
                .expect("the interpreter sits in a directory"),
        )?;
        fs::write(
            &python,
            format!("#!/bin/sh\necho launched >> '{}'\n", launches.display()),
        )?;
        fs::set_permissions(&python, fs::Permissions::from_mode(0o755))?;
        fs::write(paths.settings(), "")?;
        fs::write(paths.revision(), provision_marker())?;
        let port = settings::free_port()?;
        fs::write(paths.portfile(), port.to_string())?;

        let record = launches.clone();
        let instance = tokio::spawn(async move {
            while !record.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
            let server = MockServer::builder().listener(listener).start().await;
            Mock::given(path("/healthz"))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;
            anyhow::Ok(server)
        });

        let client = reqwest::Client::new();
        let (mut first_progress, mut second_progress) =
            (Progress::plain(Vec::new()), Progress::plain(Vec::new()));
        let (first, second) = tokio::join!(
            engine.ensure_running(&client, &mut first_progress),
            engine.ensure_running(&client, &mut second_progress),
        );
        let _instance = instance.await??;

        assert_eq!(first?, second?);
        assert_eq!(fs::read_to_string(&launches)?.lines().count(), 1);
        Ok(())
    }
}
