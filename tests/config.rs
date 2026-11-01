//! Reading `config.toml`: defaults, validation, and what gets reported.

use std::fs;
use std::time::Duration;

use clap::ValueEnum;

use hodis::config::Config;
use hodis::output::{ColorWhen, OutputFormat};
use hodis::{Provider, Providers};

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
    assert_eq!(config.providers, Providers::Auto);
    assert_eq!(config.format, OutputFormat::Pretty);
    assert_eq!(config.color, ColorWhen::Auto);
    assert_eq!(config.timeout, Duration::from_secs(10));
    assert_eq!(config.cache_ttl, Duration::from_hours(1));
    assert_eq!(config.searxng.url.as_str(), "http://localhost:8888/");
    assert!(config.searxng.embedded);
    assert_eq!(config.serper.url.as_str(), "https://google.serper.dev/");
    assert_eq!(
        config.duckduckgo.url.as_str(),
        "https://html.duckduckgo.com/"
    );
    assert_eq!(loaded.unknown_keys, Vec::<String>::new());
}

#[test]
fn every_setting_is_read() {
    let config = parse(
        r#"
        provider = "serper"
        format = "json"
        color = "never"
        timeout = 3

        [searxng]
        url = "https://search.example.org/"
        embedded = true

        [serper]
        url = "https://serper-proxy.example.org/"

        [duckduckgo]
        url = "https://duckduckgo-proxy.example.org"

        [cache]
        ttl = 90
        "#,
    );

    assert_eq!(config.cache_ttl, Duration::from_secs(90));

    assert_eq!(config.providers, Providers::Listed(vec![Provider::Serper]));
    assert_eq!(config.format, OutputFormat::Json);
    assert_eq!(config.color, ColorWhen::Never);
    assert_eq!(config.timeout, Duration::from_secs(3));
    assert_eq!(config.searxng.url.as_str(), "https://search.example.org/");
    assert!(config.searxng.embedded);
    assert_eq!(
        config.serper.url.as_str(),
        "https://serper-proxy.example.org/"
    );
    assert_eq!(
        config.duckduckgo.url.as_str(),
        "https://duckduckgo-proxy.example.org/"
    );
}

#[test]
fn values_are_spelled_as_on_the_command_line() {
    for &format in OutputFormat::value_variants() {
        let flag = format.to_possible_value().expect("no format is hidden");
        let name = flag.get_name();
        assert_eq!(parse(&format!("format = '{name}'")).format, format);
    }
    for &provider in Provider::value_variants() {
        let flag = provider
            .to_possible_value()
            .expect("only offered providers are listed");
        let name = flag.get_name();
        assert_eq!(
            parse(&format!("provider = '{name}'")).providers,
            Providers::Listed(vec![provider])
        );
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
    let loaded = Config::parse("fromat = 'json'\n\n[searxng]\nurll = 'https://example.org/'\n")
        .expect("unknown keys are not fatal");

    assert_eq!(loaded.unknown_keys, ["fromat", "searxng.urll"]);
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
fn an_instance_url_without_a_scheme_is_an_error() {
    let message = error("[searxng]\nurl = 'localhost:8888'");

    assert!(message.contains("http or https"), "{message}");
}

#[test]
fn a_serper_url_must_be_http_and_gains_a_trailing_slash() {
    let message = error("[serper]\nurl = 'proxy.internal:8080'");
    assert!(
        message.contains("serper.url must be an http or https URL"),
        "{message}"
    );

    let url = parse("[serper]\nurl = 'https://proxy.example.org/serper'")
        .serper
        .url;
    assert_eq!(url.as_str(), "https://proxy.example.org/serper/");
}

#[test]
fn a_cache_ttl_of_zero_is_kept_to_turn_the_cache_off() {
    assert_eq!(parse("[cache]\nttl = 0").cache_ttl, Duration::ZERO);
}

#[test]
fn a_list_of_providers_keeps_its_order() {
    assert_eq!(
        parse("provider = ['serper', 'searxng']").providers,
        Providers::Listed(vec![Provider::Serper, Provider::SearXNG])
    );
}

#[test]
fn an_empty_list_of_providers_is_refused() {
    let message = error("provider = []");
    assert!(
        message.contains("provider must name at least one provider"),
        "{message}"
    );
}

#[test]
fn a_misspelt_provider_in_a_list_is_named() {
    let message = error("provider = ['searxng', 'serpr']");
    assert!(message.contains("unknown variant `serpr`"), "{message}");
}

#[test]
fn an_instance_url_gains_the_trailing_slash_endpoints_join_onto() {
    let url = parse("[searxng]\nurl = 'https://example.org/searxng'")
        .searxng
        .url;

    assert_eq!(url.as_str(), "https://example.org/searxng/");
    assert_eq!(
        url.join("search").expect("a relative path joins").as_str(),
        "https://example.org/searxng/search"
    );
}

#[test]
fn setting_an_instance_url_turns_the_embedded_fallback_off_unless_asked() {
    let url = "[searxng]\nurl = 'https://example.org/'\n";

    assert!(!parse(url).searxng.embedded);
    assert!(parse(&format!("{url}embedded = true")).searxng.embedded);
    assert!(!parse("[searxng]\nembedded = false").searxng.embedded);
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
