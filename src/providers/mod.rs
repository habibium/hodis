pub mod searxng;
pub mod serper;

use std::io::Write;

use anyhow::{Result, bail};
use reqwest::Client;
use url::Url;

use crate::config::{self, Config};
use crate::engine::{Engine, is_reachable};
use crate::keys::{self, KeyedProvider};
use crate::{Provider, SearchProvider};
use searxng::SearXNGProvider;
use serper::SerperProvider;

/// `provider`, set up as `config` says and ready to search. Steps taken to set
/// up the engine hodis runs itself are reported to `progress`.
pub async fn connect(
    provider: Provider,
    config: &Config,
    client: &Client,
    progress: &mut impl Write,
) -> Result<Box<dyn SearchProvider>> {
    Ok(match provider {
        Provider::SearXNG => Box::new(SearXNGProvider {
            base_url: searxng_url(client, &config.searxng, progress).await?,
        }),
        Provider::Serper => Box::new(SerperProvider {
            base_url: config.serper.url.clone(),
            key: keys::find(KeyedProvider::Serper)?,
        }),
        Provider::DuckDuckGo => bail!("hodis cannot search DuckDuckGo yet"),
    })
}

/// The configured instance is one the user may or may not be running; when
/// nothing answers there, hodis falls back to the engine it runs itself, if
/// allowed.
async fn searxng_url(
    client: &Client,
    instance: &config::SearXNG,
    progress: &mut impl Write,
) -> Result<Url> {
    if !instance.embedded || is_reachable(client, &instance.url).await {
        Ok(instance.url.clone())
    } else {
        Engine::resolve()?.ensure_running(client, progress).await
    }
}
