//! The embedded SearXNG instance hodis installs, starts and reuses.
//!
//! Lifecycle is re-read from disk each call: running (recorded port answers a
//! probe), provisioned (interpreter, settings and a matching marker), or
//! absent. A mismatched marker counts as absent, so upgrading hodis
//! re-provisions rather than running a stale engine.
//!
//! Provisioning needs only `tar` on the host: no git, no Python, no compiler.

mod daemon;
mod paths;
mod settings;

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail, ensure};
use reqwest::Client;
use url::Url;

use crate::output::render_setup_step;
pub use paths::{DATA_DIR_ENV, Paths};

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

/// SearXNG parses its engine definitions before it serves.
const STARTUP_TIMEOUT: Duration = Duration::from_mins(1);

const STARTUP_POLL: Duration = Duration::from_millis(250);

/// Tens of megabytes, so the short timeout suiting a search would not do.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(10);

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
    /// if needed. `progress` belongs on stderr so piped search output stays
    /// clean.
    pub async fn ensure_running(&self, client: &Client, progress: &mut impl Write) -> Result<Url> {
        if let Some(url) = self.running_url(client).await {
            return Ok(url);
        }

        if !self.is_provisioned() {
            self.provision(client, progress).await?;
        }

        self.start(client, progress).await
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

    fn is_provisioned(&self) -> bool {
        self.paths.venv_python().is_file()
            && self.paths.settings().is_file()
            && fs::read_to_string(self.paths.revision())
                .is_ok_and(|marker| marker.trim() == provision_marker())
    }

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

        // Wheels only: an end-user machine has no compiler, so a dependency
        // needing a build must fail loudly rather than emit compiler errors.
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
                // The build below runs from this environment, not an isolated
                // one, so its backend must be installed here.
                .arg("setuptools")
                .arg("wheel"),
            "install SearXNG's dependencies",
        )?;

        // SearXNG's setup.py imports the searx package, pulling in msgspec and
        // pyyaml without declaring either as a build requirement; only an
        // unisolated build against the environment above satisfies them.
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

        // SearXNG imports the unix-only `pwd` unconditionally. Written after
        // the install because uv owns site-packages until then.
        if cfg!(windows) {
            let stub = self.paths.pwd_stub();
            fs::write(&stub, pwd_stub())
                .with_context(|| format!("cannot write {}", stub.display()))?;
        }

        let _ = fs::remove_dir_all(self.paths.source());

        self.write_settings()?;
        fs::write(self.paths.revision(), provision_marker())
            .with_context(|| format!("cannot write {}", self.paths.revision().display()))?;

        // The cache rivals the installation in size and only pays off on a
        // re-provision. Best-effort: keeping it is not a failure.
        let _ = self
            .uv()
            .args(["cache", "clean"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Ok(())
    }

    /// Unpacks the pinned source and freezes its version, returning the
    /// directory to install from.
    ///
    /// A local tree, not the archive URL: `searx/version.py` falls back to
    /// `git show` when `searx.version_frozen` is missing, and archives omit
    /// that module, so a git-less machine dies with `FileNotFoundError: 'git'`.
    /// Writing it first, as upstream's container image does, needs the source
    /// unpacked; a URL requirement would bring the git dependency back.
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

        // The archive wraps the tree in a directory named after the revision.
        let tree = find_shallow(&scratch, OsStr::new("setup.py"))
            .and_then(|setup| setup.parent().map(Path::to_path_buf))
            .context("the SearXNG archive contained no setup.py")?;

        let frozen = tree.join("searx").join("version_frozen.py");
        fs::write(&frozen, version_frozen())
            .with_context(|| format!("cannot write {}", frozen.display()))?;

        Ok(tree)
    }

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
        // An interrupted attempt would otherwise leave its debris here.
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

/// Every target ships a `tar` — bsdtar on macOS and Windows 10 or later, where
/// it also reads zip — so this keeps archive crates out of the tree.
fn extract(archive: &Path, into: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .stdin(Stdio::null())
        .stdout(io::stderr())
        .status()
        .with_context(|| format!("cannot run tar to unpack {}", archive.display()))?;
    ensure!(
        status.success(),
        "tar could not unpack {}",
        archive.display()
    );
    Ok(())
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

/// uv's progress reaches the terminal through stderr, its stdout included:
/// hodis's own stdout carries search output only.
fn run(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .stdout(io::stderr())
        .status()
        .with_context(|| format!("cannot run uv to {what}"))?;
    ensure!(status.success(), "uv could not {what}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Paths, SEARXNG_REV, SEARXNG_VERSION, provision_marker, pwd_stub, version_frozen};

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
}
