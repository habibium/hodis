//! Full-corpus certification: parse every recorded response in `local/samples`.
//!
//! The corpus is gitignored and regenerable, so this test is ignored by
//! default. Run it with `cargo test-all` (or `cargo test -- --include-ignored`)
//! after generating a corpus with `cargo corpus`.

use anyhow::Context;
use hodis::providers::searxng::{SEARXNG_BASE_URL, SearXNGProvider};
use url::Url;

// Must match the default output directory in examples/generate-corpus.rs.
const CORPUS_DIR: &str = "local/samples";

#[test]
#[ignore = "needs the gitignored local/samples corpus; generate: cargo corpus, run: cargo test -- --ignored"]
fn every_recorded_response_parses() -> anyhow::Result<()> {
    let entries = std::fs::read_dir(CORPUS_DIR).with_context(|| {
        format!("no {CORPUS_DIR} directory; generate a corpus with: cargo corpus")
    })?;

    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "{CORPUS_DIR} is empty; generate a corpus with: cargo corpus"
    );

    let provider = SearXNGProvider {
        base_url: Url::parse(SEARXNG_BASE_URL)?,
    };

    let (mut kept, mut skipped, mut answers) = (0_usize, 0_usize, 0_usize);
    for path in &files {
        let body = std::fs::read_to_string(path)?;
        let r = provider
            .parse_body("sample", &body)
            .with_context(|| format!("{} failed to parse", path.display()))?;
        kept += r.results.len();
        skipped += r.diagnostics.skipped_results;
        answers += usize::from(r.answer.is_some());
    }

    println!(
        "corpus: {} responses, kept {kept}, skipped {skipped}, answers {answers}",
        files.len()
    );
    Ok(())
}
