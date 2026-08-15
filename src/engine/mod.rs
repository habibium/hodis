//! An embedded SearXNG instance that hodis installs, starts and reuses.
//!
//! A fresh machine has no search backend, and asking people to run Docker or
//! set up Python is setup hodis can do for them. On the first search that finds
//! no instance, hodis downloads uv, has it provision a private interpreter and
//! a SearXNG pinned to [`SEARXNG_REV`], writes a loopback-only configuration
//! and starts the instance in the background. Every later search finds it
//! already running and pays nothing.
//!
//! The lifecycle is resolved from what is on disk, never from cached state:
//!
//! - *running* — the recorded port answers a health probe, so it is used as is;
//! - *provisioned* — an interpreter, settings and a matching revision marker
//!   exist, so the instance only needs starting;
//! - *absent* — anything else, which triggers a full provision first.
//!
//! Treating a mismatched revision marker as absent is what makes upgrading
//! hodis re-provision the engine rather than run a stale one.

mod daemon;
mod paths;
mod settings;

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use reqwest::Client;
use url::Url;

use crate::output::render_setup_step;
pub use paths::{DATA_DIR_ENV, Paths};

/// uv release hodis provisions with. Pinned, and downloaded into hodis's own
/// data directory rather than taken from `PATH`, so a bootstrap behaves
/// identically on every machine.
pub const UV_VERSION: &str = "0.11.13";

/// Python version SearXNG is installed into.
pub const PYTHON_VERSION: &str = "3.13";

/// SearXNG revision the embedded instance is built from.
pub const SEARXNG_REV: &str = "094c33d406da578ac851f2d8ad10fdfa344a9493";

/// Longest a freshly started instance may take to answer a health probe.
/// SearXNG parses its engine definitions before it serves, which is seconds on
/// a warm machine and rather more on a cold one.
const STARTUP_TIMEOUT: Duration = Duration::from_mins(1);

/// Gap between health probes while waiting for startup.
const STARTUP_POLL: Duration = Duration::from_millis(250);

/// Ceiling for the uv download, which is tens of megabytes and would otherwise
/// inherit the short timeout that suits a search request.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(10);

/// Reports whether a SearXNG instance is already answering at `base_url`.
///
/// Used to decide whether hodis needs its own instance at all. The probe is a
/// local health endpoint, so it costs an upstream engine nothing.
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

    /// Places the engine at an explicit location.
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
    ///
    /// `progress` receives the first-run narration; it belongs on stderr so
    /// that piped search output stays clean.
    pub async fn ensure_running(&self, client: &Client, progress: &mut impl Write) -> Result<Url> {
        if let Some(url) = self.running_url(client).await {
            return Ok(url);
        }

        if !self.is_provisioned() {
            self.provision(client, progress).await?;
        }

        self.start(client, progress).await
    }

    /// Stops the instance, reporting whether one was actually running.
    pub async fn stop(&self, client: &Client) -> Result<bool> {
        daemon::stop(client, &self.paths).await
    }

    /// URL of an already-running instance, if one answers on the recorded port.
    async fn running_url(&self, client: &Client) -> Option<Url> {
        let port = daemon::read_port(&self.paths)?;
        let url = daemon::base_url(port).ok()?;
        daemon::is_healthy(client, &url).await.then_some(url)
    }

    /// Whether the installed engine matches what this build of hodis expects.
    fn is_provisioned(&self) -> bool {
        self.paths.venv_python().is_file()
            && self.paths.settings().is_file()
            && fs::read_to_string(self.paths.revision()).is_ok_and(|rev| rev.trim() == SEARXNG_REV)
    }

    /// Installs uv, a private Python and SearXNG, then writes the instance
    /// configuration.
    async fn provision(&self, client: &Client, progress: &mut impl Write) -> Result<()> {
        render_setup_step(
            "setting up the local search engine; this happens once and downloads a few hundred megabytes",
            progress,
        )?;

        let engine = self.paths.engine();
        fs::create_dir_all(&engine)
            .with_context(|| format!("cannot create {}", engine.display()))?;

        self.ensure_uv(client, progress).await?;

        render_setup_step(&format!("installing Python {PYTHON_VERSION}"), progress)?;
        run(
            self.uv().args(["python", "install", PYTHON_VERSION]),
            "install Python",
        )?;
        run(
            self.uv()
                .arg("venv")
                .arg("--python")
                .arg(PYTHON_VERSION)
                .arg(self.paths.venv()),
            "create the virtual environment",
        )?;

        render_setup_step("installing SearXNG", progress)?;
        // Pass one: every runtime requirement, as a prebuilt wheel. Refusing
        // source distributions here is deliberate — an end-user machine has no
        // compiler, so a dependency that would need building must fail loudly
        // rather than emit pages of compiler errors.
        run(
            self.uv()
                .arg("pip")
                .arg("install")
                .arg("--python")
                .arg(self.paths.venv_python())
                .arg("--only-binary")
                .arg(":all:")
                .arg("-r")
                .arg(searxng_requirements_url())
                // SearXNG builds with setuptools, which pass two runs from
                // this environment rather than an isolated one.
                .arg("setuptools")
                .arg("wheel"),
            "install SearXNG's dependencies",
        )?;

        // Pass two: SearXNG itself. Its `setup.py` imports the `searx` package,
        // which pulls in msgspec and pyyaml, yet declares neither as a build
        // requirement. Building against the environment just populated
        // satisfies those imports; an isolated build cannot.
        run(
            self.uv()
                .arg("pip")
                .arg("install")
                .arg("--python")
                .arg(self.paths.venv_python())
                .arg("--no-build-isolation-package")
                .arg("searxng")
                .arg(searxng_requirement()),
            "install SearXNG",
        )?;

        self.write_settings()?;
        fs::write(self.paths.revision(), SEARXNG_REV)
            .with_context(|| format!("cannot write {}", self.paths.revision().display()))?;

        // The download cache is only worth what a re-provision would save, and
        // it is comparable in size to the installation itself. Reclaiming it is
        // best-effort: a full engine that kept its cache is not a failure.
        let _ = self
            .uv()
            .args(["cache", "clean"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Ok(())
    }

    /// Downloads the pinned uv release unless hodis already owns a copy.
    async fn ensure_uv(&self, client: &Client, progress: &mut impl Write) -> Result<()> {
        if self.paths.uv().is_file() {
            return Ok(());
        }

        let asset = uv_asset()?;
        render_setup_step(&format!("downloading uv {UV_VERSION}"), progress)?;

        let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{asset}");
        let archive_bytes = client
            .get(&url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .with_context(|| format!("cannot download {url}"))?
            .error_for_status()
            .with_context(|| format!("cannot download {url}"))?
            .bytes()
            .await
            .with_context(|| format!("cannot read the response from {url}"))?;

        let bin = self.paths.root().join("bin");
        let unpacked = bin.join("unpacked");
        // A previous interrupted attempt would otherwise leave its debris here.
        let _ = fs::remove_dir_all(&unpacked);
        fs::create_dir_all(&unpacked)
            .with_context(|| format!("cannot create {}", unpacked.display()))?;

        let archive = bin.join(asset);
        fs::write(&archive, &archive_bytes)
            .with_context(|| format!("cannot write {}", archive.display()))?;
        extract(&archive, &unpacked)?;
        let _ = fs::remove_file(&archive);

        let executable = self.paths.uv();
        let name = executable
            .file_name()
            .expect("the uv path always ends in a file name");
        let found = find_shallow(&unpacked, name)
            .with_context(|| format!("the uv release {asset} contained no {}", name.display()))?;
        fs::rename(&found, &executable)
            .with_context(|| format!("cannot move uv into {}", executable.display()))?;
        let _ = fs::remove_dir_all(&unpacked);

        Ok(())
    }

    /// Picks a port, generates a secret and writes both the settings file and
    /// the port record that later runs read back.
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

    /// Starts the instance and waits for it to answer.
    async fn start(&self, client: &Client, progress: &mut impl Write) -> Result<Url> {
        let port = daemon::read_port(&self.paths)
            .context("the local search engine has no recorded port")?;
        let url = daemon::base_url(port)?;

        render_setup_step("starting the local search engine", progress)?;
        daemon::spawn(&self.paths)?;

        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            if daemon::is_healthy(client, &url).await {
                return Ok(url);
            }
            tokio::time::sleep(STARTUP_POLL).await;
        }

        bail!(
            "the local search engine did not answer within {}s; its log is at {}",
            STARTUP_TIMEOUT.as_secs(),
            self.paths.log().display()
        )
    }

    /// A uv invocation confined to hodis's own cache and interpreter directory,
    /// so it neither reads nor disturbs a uv the user may have configured.
    fn uv(&self) -> Command {
        let mut command = Command::new(self.paths.uv());
        command
            .env("UV_CACHE_DIR", self.paths.uv_cache())
            .env("UV_PYTHON_INSTALL_DIR", self.paths.python_install_dir())
            // Never fall back to a system interpreter: the pinned one is the
            // only version this revision of SearXNG is known to run on.
            .env("UV_MANAGED_PYTHON", "1")
            // Ignore any uv.toml or pyproject.toml near the working directory.
            .env("UV_NO_CONFIG", "1")
            .stdin(Stdio::null());
        command
    }
}

/// PEP 508 requirement for the pinned SearXNG.
///
/// A source archive rather than a `git+https` reference: pip-style git
/// requirements shell out to git, which a fresh machine need not have, and
/// cloning SearXNG's full history costs far more than fetching one snapshot.
fn searxng_requirement() -> String {
    format!("searxng @ https://github.com/searxng/searxng/archive/{SEARXNG_REV}.tar.gz")
}

/// SearXNG's own pinned requirements, read straight from the revision being
/// installed so the two never drift apart.
fn searxng_requirements_url() -> String {
    format!("https://raw.githubusercontent.com/searxng/searxng/{SEARXNG_REV}/requirements.txt")
}

/// uv release asset for the host platform.
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

/// Unpacks a uv release archive.
///
/// Every platform hodis targets ships a `tar`: GNU tar on Linux, bsdtar on
/// macOS and on Windows 10 and later, where it also reads the zip release.
/// Using it keeps archive and compression crates out of the dependency tree.
fn extract(archive: &Path, into: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .stdin(Stdio::null())
        .status()
        .context("cannot run tar to unpack the uv release")?;
    ensure!(status.success(), "tar could not unpack the uv release");
    Ok(())
}

/// Finds `name` in `dir` or in one of its immediate subdirectories.
///
/// uv's tar releases wrap their payload in a directory and its zip releases do
/// not, so the executable is looked for rather than assumed to be at a fixed
/// depth.
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

/// Runs a provisioning command, letting its own progress reach the terminal.
fn run(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("cannot run uv to {what}"))?;
    ensure!(status.success(), "uv could not {what}");
    Ok(())
}
