//! Settings from the user's `config.toml`, with built-in defaults for whatever
//! it leaves out. Command-line flags take precedence over both.

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use etcetera::{BaseStrategy, choose_base_strategy};
use serde::Deserialize;

use crate::output::{ColorWhen, OutputFormat};

/// Environment variable naming a config file to read instead of the default.
pub const CONFIG_ENV: &str = "HODIS_CONFIG";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Settings in effect.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub format: OutputFormat,
    pub color: ColorWhen,
    /// Time allowed for each search request.
    pub timeout: Duration,
}

/// Settings as read, along with what hodis ignored in them.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub config: Config,
    /// The file the settings came from, if any.
    pub path: Option<PathBuf>,
    /// Keys hodis does not know, such as misspelled ones, as dotted paths.
    pub unknown_keys: Vec<String>,
}

#[derive(Deserialize)]
struct Document {
    format: Option<OutputFormat>,
    color: Option<ColorWhen>,
    timeout: Option<NonZeroU64>,
}

/// The config file hodis reads: the one `HODIS_CONFIG` names, else
/// `hodis/config.toml` in the user's config directory.
#[must_use]
pub fn path() -> Option<PathBuf> {
    named_path().or_else(default_path)
}

fn named_path() -> Option<PathBuf> {
    env::var_os(CONFIG_ENV)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

fn default_path() -> Option<PathBuf> {
    let base = choose_base_strategy().ok()?;
    Some(base.config_dir().join("hodis").join("config.toml"))
}

impl Config {
    /// Reads the file at [`path`]. Defaults stand in for a missing file, unless
    /// `HODIS_CONFIG` named it.
    pub fn load() -> Result<Loaded> {
        if let Some(path) = named_path() {
            return Self::read(&path)?
                .with_context(|| format!("config file {} does not exist", path.display()));
        }
        if let Some(path) = default_path()
            && let Some(loaded) = Self::read(&path)?
        {
            return Ok(loaded);
        }
        Self::parse("")
    }

    /// Reads the config file at `path`, or gives `None` if there is none.
    pub fn read(path: &Path) -> Result<Option<Loaded>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("cannot read config file {}", path.display()));
            }
        };
        let mut loaded = Self::parse(&text)
            .with_context(|| format!("invalid config file {}", path.display()))?;
        loaded.path = Some(path.to_owned());
        Ok(Some(loaded))
    }

    /// Parses a config file's contents; an empty one gives the defaults.
    pub fn parse(text: &str) -> Result<Loaded> {
        let mut unknown_keys = Vec::new();
        let document: Document =
            serde_ignored::deserialize(toml::Deserializer::parse(text)?, |key| {
                unknown_keys.push(key.to_string());
            })?;

        let config = Self {
            format: document.format.unwrap_or(OutputFormat::Pretty),
            color: document.color.unwrap_or(ColorWhen::Auto),
            timeout: document
                .timeout
                .map_or(DEFAULT_TIMEOUT, |secs| Duration::from_secs(secs.get())),
        };
        Ok(Loaded {
            config,
            path: None,
            unknown_keys,
        })
    }
}
