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
//! - *provisioned* — an interpreter, settings and a matching provision marker
//!   exist, so the instance only needs starting;
//! - *absent* — anything else, which triggers a full provision first.
//!
//! Treating a mismatched marker as absent is what makes upgrading hodis
//! re-provision the engine rather than run a stale one.
//!
//! Provisioning assumes nothing of the host but a `tar`: it needs no git, no
//! system Python and no compiler. Everything is fetched from two hosts, the uv
//! release and the SearXNG source archive, and every Python dependency
//! installs as a prebuilt wheel.

mod daemon;
mod paths;
mod settings;

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail, ensure};
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

/// Version of [`SEARXNG_REV`], in the `YYYY.M.D+shorthash` form SearXNG
/// derives from a commit's date and abbreviated hash. Upstream reads this out
/// of git; hodis has to state it, so it lives beside the revision and the two
/// are updated together.
pub const SEARXNG_VERSION: &str = "2026.8.14+094c33d40";

/// Repository [`SEARXNG_REV`] is taken from, reported by the instance's own
/// version information.
const SEARXNG_GIT_URL: &str = "https://github.com/searxng/searxng";

/// Branch [`SEARXNG_REV`] is taken from.
const SEARXNG_GIT_BRANCH: &str = "master";

/// Generation of the provisioning recipe, bumped whenever hodis builds
/// [`SEARXNG_REV`] differently enough that an install made the old way is
/// wrong. Recorded alongside the revision so those installs are replaced
/// instead of being run indefinitely.
const PROVISION_RECIPE: &str = "2";

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
            && fs::read_to_string(self.paths.revision())
                .is_ok_and(|marker| marker.trim() == provision_marker())
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
        let source = self.fetch_searxng_source(client).await?;

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
                .arg(source.join("requirements.txt"))
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
                .arg(searxng_requirement(&source)?),
            "install SearXNG",
        )?;

        // The unpacked source is only needed for the build.
        let _ = fs::remove_dir_all(self.paths.source());

        self.write_settings()?;
        fs::write(self.paths.revision(), provision_marker())
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

    /// Unpacks the pinned SearXNG source and freezes its version, returning the
    /// directory to install from.
    ///
    /// SearXNG is installed from a local tree rather than from the archive URL
    /// because its `setup.py` reads the version out of git: `searx/version.py`
    /// imports `searx.version_frozen` and, when that import fails, shells out
    /// to `git show`. Release archives do not carry `version_frozen.py` — it is
    /// generated at release time — so on a machine without git the build dies
    /// with `FileNotFoundError: 'git'`. Writing the module before building is
    /// what upstream's own container image does, and it is only possible with
    /// the source unpacked first. Installing from a URL would reintroduce the
    /// git requirement.
    async fn fetch_searxng_source(&self, client: &Client) -> Result<PathBuf> {
        let scratch = self.paths.source();
        // An interrupted provision would otherwise leave a half-unpacked tree.
        let _ = fs::remove_dir_all(&scratch);
        fs::create_dir_all(&scratch)
            .with_context(|| format!("cannot create {}", scratch.display()))?;

        let url = format!("https://github.com/searxng/searxng/archive/{SEARXNG_REV}.tar.gz");
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

        let archive = scratch.join("searxng.tar.gz");
        fs::write(&archive, &archive_bytes)
            .with_context(|| format!("cannot write {}", archive.display()))?;
        extract(&archive, &scratch)?;
        let _ = fs::remove_file(&archive);

        // The archive wraps everything in a directory named after the
        // revision, so the tree is found by what it contains.
        let tree = find_shallow(&scratch, OsStr::new("setup.py"))
            .and_then(|setup| setup.parent().map(Path::to_path_buf))
            .context("the SearXNG archive contained no setup.py")?;

        let frozen = tree.join("searx").join("version_frozen.py");
        fs::write(&frozen, version_frozen())
            .with_context(|| format!("cannot write {}", frozen.display()))?;

        Ok(tree)
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

/// Marker recording what an installed engine was provisioned from.
///
/// It carries the recipe as well as the revision: an install predating a change
/// in how hodis builds that revision is just as stale as one built from a
/// different revision, and a marker holding only the revision could not tell.
fn provision_marker() -> String {
    format!("{SEARXNG_REV} {PROVISION_RECIPE}")
}

/// PEP 508 requirement pointing at the unpacked source tree.
///
/// Naming the package is what makes `--no-build-isolation-package searxng`
/// apply: handed a bare directory, uv cannot know which package it holds until
/// the build it is meant to configure has already run, and the build then fails
/// on SearXNG's undeclared imports.
fn searxng_requirement(source: &Path) -> Result<String> {
    let absolute = std::path::absolute(source)
        .with_context(|| format!("cannot resolve {}", source.display()))?;
    let url = Url::from_file_path(&absolute)
        .map_err(|()| anyhow!("{} is not a usable file url", absolute.display()))?;
    Ok(format!("searxng @ {url}"))
}

/// Contents of `searx/version_frozen.py` for the pinned revision.
///
/// This is what `searx/version.py` writes when asked to freeze, and importing
/// it is the one path through that module which never invokes git. The values
/// mirror upstream's derivation: the tag and version string are the commit's
/// date and short hash, and the Docker tag is the same with `+` swapped for a
/// character a tag may contain.
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

/// Unpacks a downloaded archive.
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
        .with_context(|| format!("cannot run tar to unpack {}", archive.display()))?;
    ensure!(
        status.success(),
        "tar could not unpack {}",
        archive.display()
    );
    Ok(())
}

/// Finds `name` in `dir` or in one of its immediate subdirectories.
///
/// uv's tar releases wrap their payload in a directory and its zip releases do
/// not, and the SearXNG archive wraps its tree in a directory named after the
/// revision, so files are looked for rather than assumed to be at a fixed path.
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

#[cfg(test)]
mod tests {
    use super::{SEARXNG_REV, SEARXNG_VERSION, provision_marker, version_frozen};

    #[test]
    fn the_frozen_version_defines_every_name_searxng_unpacks() {
        // `searx/version.py` reads exactly these five names off the module and
        // only falls back to running git when the import fails. A missing name
        // is an AttributeError at build time, not a graceful degradation.
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
        // Guards the maintenance hazard of moving SEARXNG_REV without
        // SEARXNG_VERSION: the instance would then report a version belonging
        // to a different commit.
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
        // An install made before the recipe changed must not pass for current,
        // so the marker cannot be the bare revision.
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
}
