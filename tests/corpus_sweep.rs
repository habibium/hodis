//! Full-corpus certification: parse every recorded response in `local/samples`.
//!
//! The corpus is gitignored and regenerable (`cargo corpus`), so this test
//! adapts at runtime: with a corpus present it certifies the parser against
//! every recorded body; without one it passes as a no-op after printing a
//! hint (visible with `cargo test -- --nocapture`).

use anyhow::Context;
use hodis::providers::searxng::{SEARXNG_BASE_URL, SearXNGProvider};
use url::Url;

// Must match the default output directory in examples/generate-corpus.rs.
const CORPUS_DIR: &str = "local/samples";

#[test]
fn every_recorded_response_parses() -> anyhow::Result<()> {
    let Ok(entries) = std::fs::read_dir(CORPUS_DIR) else {
        eprintln!("corpus sweep skipped: no {CORPUS_DIR} directory — generate one with: cargo corpus");
        return Ok(());
    };

    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    if files.is_empty() {
        eprintln!("corpus sweep skipped: {CORPUS_DIR} is empty — generate a corpus with: cargo corpus");
        return Ok(());
    }

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
