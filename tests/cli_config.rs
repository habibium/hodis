//! How the binary finds and reports its config file. Every case exits before a
//! search, so nothing reaches the network; the data directory is pointed at a
//! scratch one all the same, so a regression cannot provision a real engine.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn hodis(config: &Path, data: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hodis"))
        .env("HODIS_CONFIG", config)
        .env(hodis::engine::DATA_DIR_ENV, data.path())
        .args(args)
        .output()
        .expect("the binary runs")
}

#[test]
fn a_config_file_named_by_hodis_config_must_exist() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let config = dir.path().join("missing.toml");

    let output = hodis(&config, &dir, &["rust"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("does not exist"), "{stderr}");
    assert!(stderr.contains(&config.display().to_string()), "{stderr}");
    Ok(())
}

#[test]
fn an_invalid_config_file_stops_the_search() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let config = dir.path().join("config.toml");
    fs::write(&config, "format = 'xml'\n")?;

    let output = hodis(&config, &dir, &["rust"]);

    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "",
        "nothing is rendered"
    );
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("unknown variant `xml`"), "{stderr}");
    Ok(())
}
