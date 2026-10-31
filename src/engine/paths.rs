//! Filesystem layout of the embedded engine.
//!
//! Every engine path is derived here, and nothing outside the resolved root is
//! ever touched, so uninstalling is deleting one directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use etcetera::{BaseStrategy, choose_base_strategy};

/// Environment variable that overrides the data directory.
pub const DATA_DIR_ENV: &str = "HODIS_DATA_DIR";

const UV_EXE: &str = if cfg!(windows) { "uv.exe" } else { "uv" };
const VENV_BIN: &str = if cfg!(windows) { "Scripts" } else { "bin" };
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
    pub fn resolve() -> Result<Self> {
        if let Some(dir) = std::env::var_os(DATA_DIR_ENV) {
            return Ok(Self::new(PathBuf::from(dir)));
        }
        let base = choose_base_strategy().context("no data directory for the current user")?;
        Ok(Self::new(base.data_dir().join("hodis")))
    }

    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// hodis's own copy, never whichever uv is on `PATH`.
    #[must_use]
    pub fn uv(&self) -> PathBuf {
        self.root.join("bin").join(UV_EXE)
    }

    /// Inside the data directory so it disappears with the rest of the engine.
    #[must_use]
    pub fn uv_cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    #[must_use]
    pub fn engine(&self) -> PathBuf {
        self.root.join("engine")
    }

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

    /// Windows only: its virtualenvs use a fixed `Lib\site-packages`, where
    /// unix qualifies the path with the interpreter version.
    #[must_use]
    pub fn venv_site_packages(&self) -> PathBuf {
        self.venv().join("Lib").join("site-packages")
    }

    /// Stand-in for the unix-only `pwd` module, written on Windows.
    #[must_use]
    pub fn pwd_stub(&self) -> PathBuf {
        self.venv_site_packages().join("pwd.py")
    }

    #[must_use]
    pub fn settings(&self) -> PathBuf {
        self.engine().join("settings.yml")
    }

    #[must_use]
    pub fn pidfile(&self) -> PathBuf {
        self.engine().join("searxng.pid")
    }

    /// Separate from `settings.yml` so reading it back needs no YAML parser.
    #[must_use]
    pub fn portfile(&self) -> PathBuf {
        self.engine().join("port")
    }

    /// What the venv was provisioned from; a mismatch forces a re-provision.
    #[must_use]
    pub fn revision(&self) -> PathBuf {
        self.engine().join("revision")
    }

    /// Unpacked source, removed once the install succeeds.
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
