//! End-to-end certification of the embedded engine.
//!
//! Ignored by default: provisions a real interpreter and SearXNG, downloading
//! a few hundred megabytes. Asserts only a healthy `/healthz`, so no upstream
//! engine sees traffic.

use std::time::Duration;

use hodis::engine::{Engine, Paths, is_reachable};
use hodis::progress::Progress;

#[tokio::test]
#[ignore = "downloads a python interpreter and searxng; run: cargo test -- --ignored"]
async fn a_scratch_data_directory_bootstraps_into_a_healthy_instance() -> anyhow::Result<()> {
    let scratch = tempfile::tempdir()?;
    let engine = Engine::new(Paths::new(scratch.path().to_path_buf()));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    let mut progress = Progress::plain(Vec::new());
    let url = engine.ensure_running(&client, &mut progress).await?;
    let report = progress.written();
    for expected in ["Installing Python", "+ flask", "added engine", "running at"] {
        assert!(report.contains(expected), "{expected}: {report}");
    }

    assert!(
        is_reachable(&client, &url).await,
        "the url ensure_running returned must serve a healthy instance"
    );
    assert_eq!(
        url.host_str(),
        Some("127.0.0.1"),
        "the embedded instance must never leave the loopback interface"
    );
    assert!(
        engine.paths().pidfile().is_file(),
        "a started instance must leave a pid file behind"
    );

    // A second call must find the instance rather than start another one.
    let reused = engine.ensure_running(&client, &mut progress).await?;
    assert_eq!(url, reused, "a healthy instance must be reused as is");

    assert!(engine.stop(&client).await?, "stop must report a live kill");
    assert!(
        !engine.paths().pidfile().is_file(),
        "stopping must clear the pid file"
    );

    // An install cut short leaves its virtual environment, which uv refuses to
    // replace unless told to, and which must not block the next attempt.
    std::fs::remove_file(engine.paths().revision())?;
    let retried = engine.ensure_running(&client, &mut progress).await?;
    assert!(is_reachable(&client, &retried).await);
    engine.stop(&client).await?;
    Ok(())
}
