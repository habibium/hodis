//! Reading `config.toml`: defaults, validation, and what gets reported.

use std::fs;
use std::time::Duration;

use clap::ValueEnum;

use hodis::config::Config;
use hodis::output::{ColorWhen, OutputFormat};

fn parse(text: &str) -> Config {
    Config::parse(text).expect("the config is valid").config
}

fn error(text: &str) -> String {
    let error = Config::parse(text).expect_err("the config is invalid");
    format!("{error:#}")
}

#[test]
fn an_empty_file_keeps_every_default() {
    let loaded = Config::parse("").expect("an empty file is valid");

    let config = loaded.config;
    assert_eq!(config.format, OutputFormat::Pretty);
    assert_eq!(config.color, ColorWhen::Auto);
    assert_eq!(config.timeout, Duration::from_secs(10));
    assert_eq!(loaded.unknown_keys, Vec::<String>::new());
}

#[test]
fn every_setting_is_read() {
    let config = parse(
        r#"
        format = "json"
        color = "never"
        timeout = 3
        "#,
    );

    assert_eq!(config.format, OutputFormat::Json);
    assert_eq!(config.color, ColorWhen::Never);
    assert_eq!(config.timeout, Duration::from_secs(3));
}

#[test]
fn values_are_spelled_as_on_the_command_line() {
    for &format in OutputFormat::value_variants() {
        let flag = format.to_possible_value().expect("no format is hidden");
        let name = flag.get_name();
        assert_eq!(parse(&format!("format = '{name}'")).format, format);
    }
    for &color in ColorWhen::value_variants() {
        let flag = color
            .to_possible_value()
            .expect("no color choice is hidden");
        let name = flag.get_name();
        assert_eq!(parse(&format!("color = '{name}'")).color, color);
    }
}

#[test]
fn unknown_keys_are_reported_and_otherwise_ignored() {
    let loaded =
        Config::parse("fromat = 'json'\ntimout = 3\n").expect("unknown keys are not fatal");

    assert_eq!(loaded.unknown_keys, ["fromat", "timout"]);
    assert_eq!(loaded.config, parse(""));
}

#[test]
fn an_invalid_value_is_an_error_naming_its_line() {
    let message = error("color = 'auto'\nformat = 'xml'\n");

    assert!(message.contains("line 2"), "{message}");
    assert!(message.contains("unknown variant `xml`"), "{message}");
}

#[test]
fn a_zero_timeout_is_an_error() {
    let message = error("timeout = 0");

    assert!(message.contains("nonzero"), "{message}");
}

#[test]
fn a_missing_file_reads_as_none() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;

    assert_eq!(Config::read(&dir.path().join("config.toml"))?, None);
    Ok(())
}

#[test]
fn a_file_is_read_with_its_path() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    fs::write(&path, "format = 'urls'\n")?;

    let loaded = Config::read(&path)?.expect("the file exists");
    assert_eq!(loaded.config.format, OutputFormat::Urls);
    assert_eq!(loaded.path.as_deref(), Some(path.as_path()));
    Ok(())
}

#[test]
fn an_error_in_a_file_names_the_file() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    fs::write(&path, "format = 1\n")?;

    let error = Config::read(&path).expect_err("a number is not a format");
    assert!(
        format!("{error:#}").contains(&path.display().to_string()),
        "{error:#}"
    );
    Ok(())
}
