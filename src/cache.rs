//! Search responses kept on disk for a while, so that repeating a search costs
//! neither a request nor a Serper credit.

use std::env;
use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use etcetera::{BaseStrategy, choose_base_strategy};
use serde::Deserialize;
use serde_json::json;

use crate::config::Config;
use crate::{Provider, SearchParams, SearchResponse};

/// Environment variable naming the directory hodis keeps its cache in.
pub const CACHE_DIR_ENV: &str = "HODIS_CACHE_DIR";

/// Each provider's response to a search, one file apiece, reused until it is
/// older than the TTL.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
    ttl: Duration,
    reads: bool,
}

#[derive(Deserialize)]
struct Entry {
    key: String,
    response: SearchResponse,
}

/// Keeps temporary file names apart between writers in one process.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

impl Cache {
    /// The cache in `searches` under the directory `HODIS_CACHE_DIR` names, or
    /// else under `hodis` in the user's cache directory. None when `ttl` is
    /// zero or the user has no cache directory.
    #[must_use]
    pub fn open(ttl: Duration) -> Option<Self> {
        if ttl.is_zero() {
            return None;
        }
        let root = match env::var_os(CACHE_DIR_ENV).filter(|dir| !dir.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => choose_base_strategy().ok()?.cache_dir().join("hodis"),
        };
        Some(Self::new(root.join("searches"), ttl))
    }

    /// A cache keeping its entries in `dir`.
    #[must_use]
    pub fn new(dir: PathBuf, ttl: Duration) -> Self {
        Self {
            dir,
            ttl,
            reads: true,
        }
    }

    /// This cache, storing fresh responses but never handing any back.
    #[must_use]
    pub fn without_reads(self) -> Self {
        Self {
            reads: false,
            ..self
        }
    }

    pub(crate) fn get(
        &self,
        provider: Provider,
        config: &Config,
        params: &SearchParams,
    ) -> Option<SearchResponse> {
        if !self.reads {
            return None;
        }
        let key = key(provider, config, params);
        let path = self.dir.join(file_name(&key));
        let age = fs::metadata(&path).ok()?.modified().ok()?.elapsed().ok()?;
        if age >= self.ttl {
            return None;
        }
        let entry: Entry = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
        (entry.key == key).then_some(entry.response)
    }

    /// Keeps `response` for later searches. A cache that cannot be written only
    /// costs those searches their speed, so failing to is not an error.
    pub(crate) fn put(
        &self,
        provider: Provider,
        config: &Config,
        params: &SearchParams,
        response: &SearchResponse,
    ) {
        // Every engine failing at once looks like this too, and is not worth
        // repeating for an hour.
        if response.results.is_empty() {
            return;
        }
        let _ = self.write(&key(provider, config, params), response);
    }

    fn write(&self, key: &str, response: &SearchResponse) -> io::Result<()> {
        create_private_dir(&self.dir)?;
        self.prune();

        let name = file_name(key);
        // Renamed into place, so that a reader never finds half an entry.
        let temp = self.dir.join(format!(
            "{name}.{}-{}.tmp",
            process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &temp,
            serde_json::to_vec(&json!({"key": key, "response": response}))?,
        )?;
        fs::rename(&temp, self.dir.join(name))
    }

    /// Deletes the entries too old to use, so the cache holds about one TTL's
    /// worth of searches.
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= self.ttl);
            if stale {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Everything that shapes a provider's response, so that changing any of it,
/// such as the instance searched, misses the cache.
fn key(provider: Provider, config: &Config, params: &SearchParams) -> String {
    let instance = match provider {
        Provider::SearXNG => config.searxng.url.as_str(),
        Provider::Serper => config.serper.url.as_str(),
        Provider::DuckDuckGo => config.duckduckgo.url.as_str(),
    };
    let query = params
        .query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let page = params.page.map_or(1, NonZeroU32::get);
    json!([provider, instance, page, query]).to_string()
}

/// FNV-1a rather than std's hasher, whose output may change between Rust
/// releases and orphan every entry.
fn file_name(key: &str) -> String {
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}.json")
}

/// Searches can be as private as a browser's history, so only their owner may
/// read the directory holding them.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir)
}
