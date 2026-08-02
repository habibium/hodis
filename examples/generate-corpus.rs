//! Generate a corpus of raw `SearXNG` responses for the corpus sweep test.
//!
//! Usage:
//!     cargo corpus                          -> default queries into local/samples
//!     cargo corpus <out-dir>                -> default queries, custom directory
//!     cargo corpus <out-dir> <queries.tsv>  -> custom queries (category<TAB>query per line)
//!
//! Point at a non-default instance with the `SEARXNG_URL` env var.
//!
//! Response bodies are saved unmodified. A body is only saved if it is JSON
//! containing a `results` array; failures are retried once, then skipped.
//! Numbering continues after the highest existing `<n>.json` so reruns
//! append instead of overwriting.

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

use anyhow::{Context, Result, bail};
use hodis::providers::searxng::SEARXNG_BASE_URL;
use reqwest::Client;
use url::Url;

/// Minimum delay between requests. Sustained rates near 1 request/second
/// get a personal instance's upstream engines suspended within minutes,
/// and some engines enforce small daily quotas. Do not lower this.
const PACING: Duration = Duration::from_secs(5);

const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

const DEFAULT_QUERIES: &[(&str, &str)] = &[
    ("general", "ottoman empire"),
    ("general", "theory of relativity"),
    ("general", "great barrier reef"),
    ("general", "printing press history"),
    ("general", "human genome project"),
    ("general", "silk road trade routes"),
    ("general", "aurora borealis cause"),
    ("general", "roman aqueducts"),
    ("general", "monarch butterfly migration"),
    ("general", "deep sea hydrothermal vents"),
    ("it", "rust lifetimes explained"),
    ("it", "postgres vacuum tuning"),
    ("it", "linux cgroups"),
    ("it", "http3 vs http2"),
    ("it", "webassembly use cases"),
    ("it", "git rebase vs merge"),
    ("it", "kubernetes ingress"),
    ("it", "sqlite write ahead log"),
    ("science", "crispr off target effects"),
    ("science", "exoplanet detection methods"),
    ("science", "mrna vaccine mechanism"),
    ("science", "plate tectonics evidence"),
    ("science", "quantum tunneling"),
    ("science", "neutrino oscillation"),
    ("science", "photosynthesis efficiency"),
    ("science", "gut microbiome research"),
    ("images", "brutalist architecture"),
    ("images", "bioluminescent ocean"),
    ("images", "sahara desert dunes"),
    ("images", "art nouveau posters"),
    ("images", "hubble deep field"),
    ("news", "renewable energy policy"),
    ("news", "global chip shortage"),
    ("news", "space tourism industry"),
    ("news", "electric vehicle market"),
    ("news", "ocean plastic cleanup"),
    ("videos", "pottery wheel tutorial"),
    ("videos", "jazz piano improvisation"),
    ("videos", "city timelapse night"),
    ("videos", "glassblowing demonstration"),
    ("files", "blender"),
    ("files", "gimp"),
    ("files", "vlc media player"),
    ("files", "inkscape"),
    ("music", "miles davis"),
    ("music", "bossa nova"),
    ("music", "gregorian chant"),
    ("map", "kyoto"),
    ("map", "patagonia"),
    ("map", "lake baikal"),
    ("q&a", "why do cats purr"),
    ("q&a", "how do planes fly"),
    ("q&a", "what causes inflation"),
];

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let out_dir = PathBuf::from(args.get(1).map_or("local/samples", String::as_str));
    fs::create_dir_all(&out_dir)?;

    let queries = match args.get(2) {
        Some(path) => read_queries(Path::new(path))?,
        None => DEFAULT_QUERIES
            .iter()
            .map(|&(c, q)| (c.to_owned(), q.to_owned()))
            .collect(),
    };

    let base = env::var("SEARXNG_URL").unwrap_or_else(|_| SEARXNG_BASE_URL.to_owned());
    let search = Url::parse(&base)?.join("search")?;
    let client = Client::builder().timeout(REQUEST_TIMEOUT).build()?;

    let mut number = highest_existing_number(&out_dir)?;
    let manifest = out_dir.join("manifest.tsv");
    let total = queries.len();
    let mut saved = 0_usize;

    for (i, (category, query)) in queries.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(PACING).await;
        }

        let mut body = fetch_validated(&client, &search, query, category).await;
        if body.is_err() {
            tokio::time::sleep(PACING).await;
            body = fetch_validated(&client, &search, query, category).await;
        }

        match body {
            Ok(body) => {
                number += 1;
                fs::write(out_dir.join(format!("{number}.json")), &body)?;
                append_line(&manifest, &format!("{number}\t{category}\t{query}\n"))?;
                saved += 1;
                println!("[{n}/{total}] {number}.json <- {category}: {query}", n = i + 1);
            }
            Err(e) => eprintln!("[{n}/{total}] SKIP {category}: {query} ({e:#})", n = i + 1),
        }
    }

    println!("saved {saved}/{total} responses to {}", out_dir.display());
    Ok(())
}

/// Fetch one response and require it to be JSON with a `results` array.
/// The raw body is returned untouched so what lands on disk is exactly
/// what the wire carried.
async fn fetch_validated(
    client: &Client,
    search: &Url,
    query: &str,
    category: &str,
) -> Result<String> {
    let body = client
        .get(search.clone())
        .query(&[("q", query), ("format", "json"), ("categories", category)])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let value: serde_json::Value =
        serde_json::from_str(&body).context("body is not JSON (anti-bot page? format disabled?)")?;
    if !value.get("results").is_some_and(serde_json::Value::is_array) {
        bail!("JSON body has no `results` array");
    }
    Ok(body)
}

/// Lines of `category<TAB>query`; blank lines ignored.
fn read_queries(path: &Path) -> Result<Vec<(String, String)>> {
    fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (category, query) = line
                .split_once('\t')
                .with_context(|| format!("expected `category<TAB>query`, got: {line}"))?;
            Ok((category.to_owned(), query.to_owned()))
        })
        .collect()
}

/// Highest `<n>.json` already present, so reruns append after it.
fn highest_existing_number(dir: &Path) -> Result<u32> {
    Ok(fs::read_dir(dir)?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| entry.path().file_stem()?.to_str()?.parse::<u32>().ok())
        .max()
        .unwrap_or(0))
}

fn append_line(path: &Path, line: &str) -> Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}
