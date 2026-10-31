//! Settings from the user's `config.toml`, with built-in defaults for whatever
//! it leaves out. Command-line flags take precedence over both.

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use etcetera::{BaseStrategy, choose_base_strategy};
use serde::Deserialize;
use url::Url;

use crate::Provider;
use crate::output::{ColorWhen, OutputFormat};
use crate::providers::searxng::SEARXNG_BASE_URL;
use crate::providers::serper::SERPER_BASE_URL;

/// Environment variable naming a config file to read instead of the default.
pub const CONFIG_ENV: &str = "HODIS_CONFIG";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Settings in effect.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The provider searches go to.
    pub provider: Provider,
    pub format: OutputFormat,
    pub color: ColorWhen,
    /// Time allowed for each search request.
    pub timeout: Duration,
    pub searxng: SearXNG,
    pub serper: Serper,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearXNG {
    /// Ends in `/`: endpoints are joined onto it, and a join replaces whatever
    /// follows the last slash.
    pub url: Url,
    /// Whether hodis may install and run its own instance when nothing answers
    /// at `url`. On unless the config file sets `url`.
    pub embedded: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Serper {
    /// The API's address, which only a proxy in front of Serper changes. Ends
    /// in `/`, like [`SearXNG::url`].
    pub url: Url,
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
    provider: Option<Provider>,
    format: Option<OutputFormat>,
    color: Option<ColorWhen>,
    timeout: Option<NonZeroU64>,
    #[serde(default)]
    searxng: SearXNGTable,
    #[serde(default)]
    serper: SerperTable,
}

#[derive(Default, Deserialize)]
struct SearXNGTable {
    url: Option<Url>,
    embedded: Option<bool>,
}

#[derive(Default, Deserialize)]
struct SerperTable {
    url: Option<Url>,
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

        let url_set = document.searxng.url.is_some();
        let url = match document.searxng.url {
            Some(url) => base_url("searxng.url", url)?,
            None => Url::parse(SEARXNG_BASE_URL)?,
        };
        let serper_url = match document.serper.url {
            Some(url) => base_url("serper.url", url)?,
            None => Url::parse(SERPER_BASE_URL)?,
        };

        let config = Self {
            provider: document.provider.unwrap_or(Provider::SearXNG),
            format: document.format.unwrap_or(OutputFormat::Pretty),
            color: document.color.unwrap_or(ColorWhen::Auto),
            timeout: document
                .timeout
                .map_or(DEFAULT_TIMEOUT, |secs| Duration::from_secs(secs.get())),
            searxng: SearXNG {
                url,
                embedded: document.searxng.embedded.unwrap_or(!url_set),
            },
            serper: Serper { url: serper_url },
        };
        Ok(Loaded {
            config,
            path: None,
            unknown_keys,
        })
    }
}

/// `localhost:8888` parses as a URL with `localhost` as its scheme, so the
/// scheme is checked rather than trusted.
fn base_url(key: &str, mut url: Url) -> Result<Url> {
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "{key} must be an http or https URL, not {url}"
    );
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}
