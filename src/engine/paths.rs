//! Filesystem layout of the embedded engine.
//!
//! Every path the engine reads or writes is derived here, so the on-disk layout
//! is described in one place and can be pointed at a scratch directory in
//! tests. Nothing outside the resolved root is ever touched, which keeps
//! uninstalling hodis a matter of deleting one directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Environment variable that overrides the data directory.
pub const DATA_DIR_ENV: &str = "HODIS_DATA_DIR";

/// Name of the uv executable on the host platform.
const UV_EXE: &str = if cfg!(windows) { "uv.exe" } else { "uv" };
/// Directory holding a virtualenv's executables.
const VENV_BIN: &str = if cfg!(windows) { "Scripts" } else { "bin" };
/// Name of the Python executable inside a virtualenv.
const PYTHON_EXE: &str = if cfg!(windows) {
    "python.exe"
} else {
    "python"
};

/// Resolved locations of the embedded engine's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Resolves the per-user data directory, honouring [`DATA_DIR_ENV`].
    pub fn resolve() -> Result<Self> {
        if let Some(dir) = std::env::var_os(DATA_DIR_ENV) {
            return Ok(Self::new(PathBuf::from(dir)));
        }
        let base = dirs::data_dir().context("no data directory for the current user")?;
        Ok(Self::new(base.join("hodis")))
    }

    /// Uses `root` as the data directory verbatim.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// uv binary hodis downloads and owns, rather than whichever one is on
    /// `PATH`, so a bootstrap behaves the same on every machine.
    #[must_use]
    pub fn uv(&self) -> PathBuf {
        self.root.join("bin").join(UV_EXE)
    }

    /// uv's download cache, kept inside the data directory so it disappears
    /// with the rest of the engine.
    #[must_use]
    pub fn uv_cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// Everything belonging to the SearXNG install: interpreter, packages, and
    /// runtime state.
    #[must_use]
    pub fn engine(&self) -> PathBuf {
        self.root.join("engine")
    }

    /// Where uv unpacks the managed Python builds.
    #[must_use]
    pub fn python_install_dir(&self) -> PathBuf {
        self.engine().join("python")
    }

    #[must_use]
    pub fn venv(&self) -> PathBuf {
        self.engine().join("venv")
    }

    #[must_use]
    pub fn venv_python(&self) -> PathBuf {
        self.venv().join(VENV_BIN).join(PYTHON_EXE)
    }

    /// Import directory of the virtualenv on Windows.
    ///
    /// Windows virtualenvs use a fixed `Lib\site-packages`, where unix
    /// qualifies the path with the interpreter version. Only the Windows
    /// layout is spelled out because it is the only one hodis writes into.
    #[must_use]
    pub fn venv_site_packages(&self) -> PathBuf {
        self.venv().join("Lib").join("site-packages")
    }

    /// Stand-in for the unix-only `pwd` module, written on Windows so that
    /// importing SearXNG succeeds.
    #[must_use]
    pub fn pwd_stub(&self) -> PathBuf {
        self.venv_site_packages().join("pwd.py")
    }

    #[must_use]
    pub fn settings(&self) -> PathBuf {
        self.engine().join("settings.yml")
    }

    /// Process id of the instance hodis last started.
    #[must_use]
    pub fn pidfile(&self) -> PathBuf {
        self.engine().join("searxng.pid")
    }

    /// Loopback port chosen when the instance was provisioned. Kept separate
    /// from `settings.yml` so reading it back needs no YAML parser.
    #[must_use]
    pub fn portfile(&self) -> PathBuf {
        self.engine().join("port")
    }

    /// SearXNG revision the venv was provisioned from. A mismatch against the
    /// revision compiled into hodis triggers a re-provision.
    #[must_use]
    pub fn revision(&self) -> PathBuf {
        self.engine().join("revision")
    }

    /// Scratch directory holding the unpacked SearXNG source during a
    /// provision. Removed once the install succeeds.
    #[must_use]
    pub fn source(&self) -> PathBuf {
        self.engine().join("source")
    }

    /// Combined stdout and stderr of the running instance.
    #[must_use]
    pub fn log(&self) -> PathBuf {
        self.engine().join("searxng.log")
    }
}
