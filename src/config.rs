//! Settings from the user's `config.toml`, with built-in defaults for whatever
//! it leaves out. Command-line flags take precedence over both.

use std::env;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use etcetera::{BaseStrategy, choose_base_strategy};
use serde::Deserialize;
use serde::de::value::SeqAccessDeserializer;
use serde::de::{self, Deserializer, IntoDeserializer, SeqAccess, Visitor};
use toml_edit::{DocumentMut, Item, Table, value};
use url::Url;

use crate::output::{ColorWhen, OutputFormat};
use crate::providers::duckduckgo::DUCKDUCKGO_BASE_URL;
use crate::providers::searxng::SEARXNG_BASE_URL;
use crate::providers::serper::SERPER_BASE_URL;
use crate::{Provider, Providers};

/// Environment variable naming a config file to read instead of the default.
pub const CONFIG_ENV: &str = "HODIS_CONFIG";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

const DEFAULT_CACHE_TTL: Duration = Duration::from_hours(1);

/// Settings in effect.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub providers: Providers,
    pub format: OutputFormat,
    pub color: ColorWhen,
    /// Time allowed for a search. Providers that take longer are left out.
    pub timeout: Duration,
    pub searxng: SearXNG,
    pub serper: Serper,
    pub duckduckgo: DuckDuckGo,
    /// How long search results are reused. Zero turns the cache off.
    pub cache_ttl: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearXNG {
    /// Ends in `/`: endpoints are joined onto it, and a join replaces whatever
    /// follows the last slash.
    pub url: Url,
    /// When to search the instance hodis installs and runs itself.
    pub embedded: Embedded,
}

/// When hodis searches the SearXNG instance it installs and runs itself. With
/// no provider chosen, hodis only ever looks at `url`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Embedded {
    /// Never, so `url` is the only instance. The default once `url` is set.
    Never,
    /// When nothing answers at `url`. The default otherwise.
    Fallback,
    /// Always, even when something answers at `url`.
    Only,
}

impl<'de> Deserialize<'de> for Embedded {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Setting;

        impl Visitor<'_> for Setting {
            type Value = Embedded;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("true, false or \"only\"")
            }

            fn visit_bool<E: de::Error>(self, on: bool) -> Result<Self::Value, E> {
                Ok(if on {
                    Embedded::Fallback
                } else {
                    Embedded::Never
                })
            }

            fn visit_str<E: de::Error>(self, word: &str) -> Result<Self::Value, E> {
                if word == "only" {
                    Ok(Embedded::Only)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(word), &self))
                }
            }
        }

        deserializer.deserialize_any(Setting)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Serper {
    /// The API's address, which only a proxy in front of Serper changes. Ends
    /// in `/`, like [`SearXNG::url`].
    pub url: Url,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DuckDuckGo {
    /// The results page's address, which only a proxy in front of DuckDuckGo
    /// changes. Ends in `/`, like [`SearXNG::url`].
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
    #[serde(default, deserialize_with = "one_or_many")]
    provider: Option<Vec<Provider>>,
    format: Option<OutputFormat>,
    color: Option<ColorWhen>,
    timeout: Option<NonZeroU64>,
    #[serde(default)]
    searxng: SearXNGTable,
    #[serde(default)]
    serper: SerperTable,
    #[serde(default)]
    duckduckgo: DuckDuckGoTable,
    #[serde(default)]
    cache: CacheTable,
}

#[derive(Default, Deserialize)]
struct SearXNGTable {
    url: Option<Url>,
    embedded: Option<Embedded>,
}

#[derive(Default, Deserialize)]
struct SerperTable {
    url: Option<Url>,
}

#[derive(Default, Deserialize)]
struct DuckDuckGoTable {
    url: Option<Url>,
}

#[derive(Default, Deserialize)]
struct CacheTable {
    ttl: Option<u64>,
}

/// What a saved choice of provider says about where SearXNG is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearXNGChoice {
    /// Whatever the file says, as the provider chosen does not use it.
    Unchanged,
    /// The instance hodis runs itself, even if another answers at `url`.
    Embedded,
    /// An instance the user runs, which hodis must not replace with its own if
    /// it stops answering.
    Instance(Url),
}

/// Records a choice of provider in the config file at `path`, creating it if
/// needed. Everything else in the file stays as it is, comments included.
pub fn save_choice(path: &Path, provider: Provider, searxng: &SearXNGChoice) -> Result<()> {
    let path = &link_target(path)?;
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e).with_context(|| format!("cannot read config file {}", path.display()));
        }
    };
    let mut document: DocumentMut = text
        .parse()
        .with_context(|| format!("invalid config file {}", path.display()))?;

    document["provider"] = value(provider.id());
    match searxng {
        SearXNGChoice::Unchanged => {}
        SearXNGChoice::Embedded => choose_searxng(&mut document, "embedded", "only"),
        SearXNGChoice::Instance(url) => choose_searxng(&mut document, "url", url.as_str()),
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    // Renamed into place, so that a crash cannot leave half a config file.
    let temporary = path.with_extension("toml.new");
    fs::write(&temporary, document.to_string())
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("cannot write {}", path.display()))
}

/// `path`, or the file it links to. Renaming over a link would replace it,
/// cutting the config off from the dotfiles folder the link points into.
fn link_target(path: &Path) -> Result<PathBuf> {
    if path.is_symlink() {
        fs::canonicalize(path).with_context(|| format!("cannot follow the link {}", path.display()))
    } else {
        Ok(path.to_owned())
    }
}

/// Sets `key` in place of the `url` and `embedded` of the old choice, in the
/// table as it stands so that comments around it stay.
fn choose_searxng(document: &mut DocumentMut, key: &str, setting: &str) {
    if !document.get("searxng").is_some_and(Item::is_table_like) {
        document["searxng"] = Item::Table(Table::new());
    }
    if let Some(table) = document["searxng"].as_table_like_mut() {
        table.remove("url");
        table.remove("embedded");
        table.insert(key, value(setting));
    }
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
        let duckduckgo_url = match document.duckduckgo.url {
            Some(url) => base_url("duckduckgo.url", url)?,
            None => Url::parse(DUCKDUCKGO_BASE_URL)?,
        };

        let providers = match document.provider {
            None => Providers::Auto,
            Some(providers) => {
                ensure!(
                    !providers.is_empty(),
                    "provider must name at least one provider"
                );
                Providers::Listed(providers)
            }
        };

        let config = Self {
            providers,
            format: document.format.unwrap_or(OutputFormat::Pretty),
            color: document.color.unwrap_or(ColorWhen::Auto),
            timeout: document
                .timeout
                .map_or(DEFAULT_TIMEOUT, |secs| Duration::from_secs(secs.get())),
            searxng: SearXNG {
                url,
                embedded: document.searxng.embedded.unwrap_or(if url_set {
                    Embedded::Never
                } else {
                    Embedded::Fallback
                }),
            },
            serper: Serper { url: serper_url },
            duckduckgo: DuckDuckGo {
                url: duckduckgo_url,
            },
            cache_ttl: document
                .cache
                .ttl
                .map_or(DEFAULT_CACHE_TTL, Duration::from_secs),
        };
        Ok(Loaded {
            config,
            path: None,
            unknown_keys,
        })
    }
}

/// `provider` names one provider, or a list of them. Each is read as a
/// [`Provider`], so a misspelt one gets serde's own message listing the names.
fn one_or_many<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<Provider>>, D::Error> {
    struct OneOrMany;

    impl<'de> Visitor<'de> for OneOrMany {
        type Value = Vec<Provider>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a provider or a list of providers")
        }

        fn visit_str<E: de::Error>(self, name: &str) -> Result<Self::Value, E> {
            Provider::deserialize(name.into_deserializer()).map(|provider| vec![provider])
        }

        fn visit_seq<A: SeqAccess<'de>>(self, providers: A) -> Result<Self::Value, A::Error> {
            Vec::deserialize(SeqAccessDeserializer::new(providers))
        }
    }

    deserializer.deserialize_any(OneOrMany).map(Some)
}

/// `localhost:8888` parses as a URL with `localhost` as its scheme, so the
/// scheme is checked rather than trusted.
pub(crate) fn base_url(key: &str, mut url: Url) -> Result<Url> {
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
