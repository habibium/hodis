//! Reading `config.toml`: defaults, validation, and what gets reported.

use std::fs;
use std::time::Duration;

use clap::ValueEnum;

use hodis::config::{Config, Embedded, SearXNGChoice, save_choice};
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
    assert_eq!(config.searxng.embedded, Embedded::Fallback);
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
    assert_eq!(config.searxng.embedded, Embedded::Fallback);
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

    assert_eq!(parse(url).searxng.embedded, Embedded::Never);
    assert_eq!(
        parse(&format!("{url}embedded = true")).searxng.embedded,
        Embedded::Fallback
    );
    assert_eq!(
        parse("[searxng]\nembedded = false").searxng.embedded,
        Embedded::Never
    );
}

#[test]
fn embedded_can_ask_for_hodis_s_own_alone() {
    assert_eq!(
        parse("[searxng]\nembedded = \"only\"").searxng.embedded,
        Embedded::Only
    );
}

#[test]
fn any_other_word_for_embedded_is_refused_with_the_choices() {
    let error = Config::parse("[searxng]\nembedded = \"sometimes\"").expect_err("not a choice");

    let message = format!("{error:#}");
    assert!(message.contains("true, false or \"only\""), "{message}");
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

fn saved(before: Option<&str>, provider: Provider, searxng: &SearXNGChoice) -> String {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("hodis").join("config.toml");
    if let Some(before) = before {
        fs::create_dir_all(path.parent().expect("the path has a directory"))
            .expect("the directory is created");
        fs::write(&path, before).expect("the config is written");
    }
    save_choice(&path, provider, searxng).expect("the choice is saved");
    fs::read_to_string(&path).expect("the config is readable")
}

fn own_instance() -> SearXNGChoice {
    SearXNGChoice::Instance("http://sx.example/".parse().expect("a valid url"))
}

#[test]
fn a_saved_choice_creates_the_file_and_its_directory() {
    assert_eq!(
        saved(None, Provider::DuckDuckGo, &SearXNGChoice::Unchanged),
        "provider = \"duckduckgo\"\n"
    );
}

#[test]
fn a_saved_choice_keeps_the_rest_of_the_file() {
    let before = "# my settings\nformat = \"json\" # for scripts\n\n[cache]\nttl = 60\n";
    let after = saved(Some(before), Provider::Serper, &SearXNGChoice::Unchanged);

    assert!(after.contains("# my settings"), "{after}");
    assert!(after.contains("format = \"json\" # for scripts"), "{after}");
    assert_eq!(
        parse(&after).providers,
        Providers::Listed(vec![Provider::Serper])
    );
    assert_eq!(parse(&after).cache_ttl, Duration::from_mins(1));
}

#[test]
fn a_saved_choice_replaces_a_list_of_providers() {
    let after = saved(
        Some("provider = [\"searxng\", \"serper\"]\n"),
        Provider::DuckDuckGo,
        &SearXNGChoice::Unchanged,
    );
    assert_eq!(after, "provider = \"duckduckgo\"\n");
}

#[test]
fn an_instance_is_saved_so_that_hodis_will_not_replace_it() {
    let after = saved(None, Provider::SearXNG, &own_instance());

    let config = parse(&after);
    assert_eq!(config.searxng.url.as_str(), "http://sx.example/");
    assert_eq!(config.searxng.embedded, Embedded::Never);
}

#[test]
fn choosing_hodis_s_own_searxng_forgets_an_old_address() {
    let after = saved(
        Some("[searxng]\nurl = \"http://old.example/\"\nembedded = false\n"),
        Provider::SearXNG,
        &SearXNGChoice::Embedded,
    );

    assert!(!after.contains("old.example"), "{after}");
    assert_eq!(parse(&after).searxng.embedded, Embedded::Only);
}

#[test]
fn choosing_hodis_s_own_searxng_is_saved_on_a_first_run() {
    let after = saved(None, Provider::SearXNG, &SearXNGChoice::Embedded);

    assert_eq!(
        parse(&after).providers,
        Providers::Listed(vec![Provider::SearXNG])
    );
    assert_eq!(parse(&after).searxng.embedded, Embedded::Only);
}

#[test]
fn a_new_address_replaces_the_old_one_and_its_setting() {
    let after = saved(
        Some("[searxng]\nurl = \"http://old.example/\"\nembedded = true\n"),
        Provider::SearXNG,
        &own_instance(),
    );

    let config = parse(&after);
    assert_eq!(config.searxng.url.as_str(), "http://sx.example/");
    assert_eq!(config.searxng.embedded, Embedded::Never);
}

#[test]
fn an_address_replaces_the_choice_of_hodis_s_own() {
    let after = saved(
        Some("[searxng]\nembedded = \"only\"\n"),
        Provider::SearXNG,
        &own_instance(),
    );

    assert_eq!(parse(&after).searxng.embedded, Embedded::Never);
}

#[test]
fn a_comment_above_searxng_outlives_either_choice_of_instance() {
    let before = "# my search setup\n[searxng]\nurl = \"http://old.example/\"\n";

    for choice in [SearXNGChoice::Embedded, own_instance()] {
        let after = saved(Some(before), Provider::SearXNG, &choice);

        assert!(after.contains("# my search setup"), "{after}");
        assert!(!after.contains("old.example"), "{after}");
    }
}

#[test]
fn a_choice_of_instance_replaces_one_written_on_a_single_line() {
    let after = saved(
        Some("searxng = { url = \"http://old.example/\" }\n"),
        Provider::SearXNG,
        &SearXNGChoice::Embedded,
    );

    assert!(!after.contains("old.example"), "{after}");
    assert_eq!(parse(&after).searxng.embedded, Embedded::Only);
}

#[test]
fn a_saved_choice_of_another_provider_keeps_the_searxng_chosen_before() {
    let after = saved(
        Some("[searxng]\nembedded = \"only\"\n"),
        Provider::DuckDuckGo,
        &SearXNGChoice::Unchanged,
    );

    assert_eq!(parse(&after).searxng.embedded, Embedded::Only);
}

#[cfg(unix)]
#[test]
fn a_saved_choice_goes_through_a_link_to_the_file_it_points_at() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let target = dir.path().join("dotfiles.toml");
    let link = dir.path().join("config.toml");
    fs::write(&target, "# kept in a dotfiles folder\n").expect("the config is written");
    std::os::unix::fs::symlink(&target, &link).expect("the link is made");

    save_choice(&link, Provider::Serper, &SearXNGChoice::Unchanged).expect("the choice is saved");

    assert!(link.is_symlink(), "the link was replaced");
    let after = fs::read_to_string(&target).expect("the target is readable");
    assert!(after.contains("# kept in a dotfiles folder"), "{after}");
    assert!(after.contains("provider = \"serper\""), "{after}");
    let leftovers = fs::read_dir(dir.path())
        .expect("the directory is readable")
        .count();
    assert_eq!(leftovers, 2, "a temporary file was left behind");
}

#[cfg(unix)]
#[test]
fn a_link_to_nothing_is_reported_rather_than_replaced() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let link = dir.path().join("config.toml");
    std::os::unix::fs::symlink(dir.path().join("missing.toml"), &link).expect("the link is made");

    let error = save_choice(&link, Provider::Serper, &SearXNGChoice::Unchanged)
        .expect_err("there is nothing to write to");

    assert!(
        format!("{error:#}").contains(&link.display().to_string()),
        "{error:#}"
    );
    assert!(link.is_symlink(), "the link was replaced");
}

#[test]
fn a_config_file_that_is_not_toml_is_left_alone() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("config.toml");
    fs::write(&path, "not = = toml").expect("the config is written");

    let error = save_choice(&path, Provider::Serper, &SearXNGChoice::Unchanged)
        .expect_err("it cannot be edited");

    assert!(
        format!("{error:#}").contains("invalid config file"),
        "{error:#}"
    );
    assert_eq!(
        fs::read_to_string(&path).expect("the file is still there"),
        "not = = toml"
    );
}
