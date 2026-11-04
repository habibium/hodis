//! The wizard hodis opens on its first run in a terminal, and on `hodis setup`:
//! it asks which provider to use, then only what that provider needs, and does
//! the work while showing what is changing. Nothing is saved until the provider
//! is ready, so giving up part way leaves the config as it was.

mod prompt;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode};
use tokio::time::timeout;
use url::Url;

use crate::config::{self, Config, SearXNGChoice};
use crate::engine::{self, Engine, is_reachable};
use crate::keys::{self, ApiKey, Keychain, KeyedProvider};
use crate::progress::Progress;
use crate::providers::duckduckgo::DuckDuckGoProvider;
use crate::providers::searxng::SearXNGProvider;
use crate::providers::serper::SerperProvider;
use crate::{Provider, SearchParams, SearchProvider};
pub use prompt::{Choice, Prompts, TerminalPrompts};

/// What a skipped setup tells the user.
pub const SKIPPED: &str = "Nothing was changed. Run `hodis setup` when you want to choose.";

const SERPER_SIGN_UP: &str = "https://serper.dev/";

/// A search that a working provider of any kind can answer.
const TEST_QUERY: &str = "hello world";

/// Longer than a search's own timeout: a new instance answers its first search
/// slowly, and nobody is waiting on a result here.
const TEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Measured on a fresh install; a little different from one system to another.
const DOWNLOAD_SIZE: &str = "about 90 MiB";
const INSTALLED_SIZE: &str = "about 280 MiB";

/// How a setup ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A provider is ready and the config file says so.
    Saved,
    /// The user left without choosing, and nothing changed.
    Skipped,
}

/// What the user did on a provider's own screens.
enum Next {
    Saved,
    /// Back to the choice of provider.
    Back,
}

/// How SearXNG is to be run.
enum Route {
    Installed,
    Instance(Url),
    AskAddress,
}

/// Which keychain to use, opened only if a key needs it.
#[derive(Debug)]
enum KeychainSource {
    System,
    #[cfg(test)]
    Given(Keychain),
}

impl KeychainSource {
    fn open(&self) -> Result<Keychain> {
        match self {
            Self::System => Keychain::open(),
            #[cfg(test)]
            Self::Given(keychain) => Ok(keychain.clone()),
        }
    }
}

/// The wizard, asking through `P` and reporting to `W`.
#[derive(Debug)]
pub struct Wizard<'a, P, W: Write> {
    config: &'a Config,
    path: PathBuf,
    client: Client,
    engine: Engine,
    keychain: KeychainSource,
    prompts: P,
    progress: Progress<W>,
}

impl<'a, P: Prompts, W: Write> Wizard<'a, P, W> {
    /// A wizard saving to the config file at `path`, starting from `config`.
    pub fn new(config: &'a Config, path: &Path, prompts: P, progress: Progress<W>) -> Result<Self> {
        Ok(Self::with(
            config,
            path,
            Engine::resolve()?,
            KeychainSource::System,
            prompts,
            progress,
        ))
    }

    fn with(
        config: &'a Config,
        path: &Path,
        engine: Engine,
        keychain: KeychainSource,
        prompts: P,
        progress: Progress<W>,
    ) -> Self {
        Self {
            config,
            path: path.to_owned(),
            client: Client::new(),
            engine,
            keychain,
            prompts,
            progress,
        }
    }

    /// Runs until a provider is set up or the user leaves.
    pub async fn run(mut self) -> Result<Outcome> {
        self.progress
            .line("Let's choose where hodis sends your searches.\n");
        loop {
            let question = "Which search provider do you want to use?";
            let Some(choice) = self.prompts.choose(question, &provider_choices())? else {
                return Ok(Outcome::Skipped);
            };
            let provider = PROVIDERS[choice];
            match self.set_up(provider).await? {
                Some(Next::Saved) => return Ok(Outcome::Saved),
                Some(Next::Back) => {}
                None => return Ok(Outcome::Skipped),
            }
        }
    }

    /// Sets up `provider`, offering a way out if that fails. `None` means the
    /// user is leaving.
    async fn set_up(&mut self, provider: Provider) -> Result<Option<Next>> {
        loop {
            let attempt = match provider {
                Provider::DuckDuckGo => self.duckduckgo().await,
                Provider::SearXNG => self.searxng().await,
                Provider::Serper => self.serper().await,
            };
            let error = match attempt {
                Ok(next) => return Ok(Some(next)),
                Err(error) => error,
            };

            self.progress.finish();
            self.progress.line("");
            self.progress
                .error(&format!("{} is not set up:\n{error:#}\n", provider.name()));
            let choices = [
                Choice::new("Try again", ""),
                Choice::new("Choose another provider", ""),
                Choice::new("Leave setup", "Nothing is saved"),
            ];
            match self.prompts.choose("What now?", &choices)? {
                Some(0) => {}
                Some(1) => return Ok(Some(Next::Back)),
                _ => return Ok(None),
            }
        }
    }

    async fn duckduckgo(&mut self) -> Result<Next> {
        self.progress.line(
            "\nDuckDuckGo needs nothing installed. It has no search API, so hodis reads its\n\
             results page, and heavy use can get you a CAPTCHA for a while.",
        );
        self.progress.heading("Setting up DuckDuckGo");
        self.progress.plan(&[3, 1]);

        self.progress.step("Testing a search");
        let provider = DuckDuckGoProvider {
            base_url: self.config.duckduckgo.url.clone(),
        };
        // A CAPTCHA or a dropped connection now says little about tomorrow.
        match test_search(&provider, &self.client).await {
            Ok(results) => self.progress.note(&format!("{results} results")),
            Err(e) => self.progress.warn(&format!(
                "DuckDuckGo did not answer the test search, so searches may fail for now: {e:#}"
            )),
        }

        self.save(Provider::DuckDuckGo, &SearXNGChoice::Unchanged)
    }

    async fn searxng(&mut self) -> Result<Next> {
        let configured = &self.config.searxng.url;
        let found = is_reachable(&self.client, configured).await;
        let installed = self.engine.is_provisioned();

        let mut routes = Vec::new();
        let mut choices = Vec::new();
        if found {
            routes.push(Route::Instance(configured.clone()));
            choices.push(Choice::new(
                format!("Use the one at {configured}"),
                "It answered just now",
            ));
        }
        routes.push(Route::Installed);
        choices.push(if installed {
            Choice::new("Use the one hodis installed", "Already on this computer")
        } else {
            Choice::new(
                "Install one for me",
                format!("Downloads {DOWNLOAD_SIZE}, runs only on this computer"),
            )
        });
        routes.push(Route::AskAddress);
        choices.push(Choice::new(
            "Use another address",
            "A SearXNG you run elsewhere",
        ));

        self.progress.line("");
        let question = "Which SearXNG should hodis search?";
        let Some(choice) = self.prompts.choose(question, &choices)? else {
            return Ok(Next::Back);
        };
        match routes.swap_remove(choice) {
            Route::Installed => self.install_searxng(installed).await,
            Route::Instance(url) => self.use_instance(url).await,
            Route::AskAddress => {
                let Some(url) = self.ask_address()? else {
                    return Ok(Next::Back);
                };
                self.use_instance(url).await
            }
        }
    }

    async fn install_searxng(&mut self, installed: bool) -> Result<Next> {
        if !installed {
            self.show_install_plan();
            if !self.prompts.confirm("Continue?")? {
                return Ok(Next::Back);
            }
        }

        let url = self
            .engine
            .ensure_running(&self.client, &mut self.progress)
            .await?;

        self.progress.heading("Finishing");
        self.progress.plan(&[3, 1]);
        self.progress.step("Testing a search");
        // SearXNG is up by now, so a failure here is an upstream engine's.
        match test_search(&SearXNGProvider { base_url: url }, &self.client).await {
            Ok(results) => self.progress.note(&format!("{results} results")),
            Err(e) => self.progress.warn(&format!(
                "SearXNG is running but did not answer the test search: {e:#}"
            )),
        }

        self.save(Provider::SearXNG, &SearXNGChoice::Embedded)
    }

    fn show_install_plan(&mut self) {
        let root = self.engine.paths().root().display().to_string();
        let parts = [
            (format!("uv {}", engine::UV_VERSION), "installs the rest"),
            (format!("Python {}", engine::PYTHON_VERSION), "runs SearXNG"),
            (
                format!("SearXNG {}", engine::short_version()),
                "with about 40 Python packages",
            ),
        ];
        let width = parts.iter().map(|(name, _)| name.len()).max().unwrap_or(0);

        self.progress.line("\nThis will be installed:\n");
        for (name, purpose) in parts {
            self.progress.line(&format!("  {name:width$}  {purpose}"));
        }
        self.progress.line(&format!(
            "\nNeed to get {DOWNLOAD_SIZE}. After this, {INSTALLED_SIZE} of disk space will be used.\n\
             It all goes in {root}\n\
             Nothing goes anywhere else, so deleting that folder removes it all.\n"
        ));
    }

    /// Asks until the answer is an address or there is no answer.
    fn ask_address(&mut self) -> Result<Option<Url>> {
        loop {
            let Some(text) = self
                .prompts
                .text("Address of your SearXNG (empty to go back):")?
            else {
                return Ok(None);
            };
            match parse_address(&text) {
                Ok(url) => return Ok(Some(url)),
                Err(e) => self.progress.warn(&format!("{e:#}")),
            }
        }
    }

    async fn use_instance(&mut self, url: Url) -> Result<Next> {
        self.progress.heading("Setting up SearXNG");
        self.progress.plan(&[3, 1]);
        self.progress.step(&format!("Testing a search at {url}"));

        let provider = SearXNGProvider {
            base_url: url.clone(),
        };
        let results = test_search(&provider, &self.client)
            .await
            .map_err(explain_searxng_error)?;
        self.progress.note(&format!("{results} results"));

        self.save(Provider::SearXNG, &SearXNGChoice::Instance(url))
    }

    async fn serper(&mut self) -> Result<Next> {
        let provider = KeyedProvider::Serper;
        self.progress.line(&format!(
            "\nSerper serves Google results through an API key, kept in your system keychain.\n\
             Get a key at {SERPER_SIGN_UP}\n"
        ));

        let Some(key) = self.serper_key(provider)? else {
            return Ok(Next::Back);
        };
        let storing = key.typed;

        self.progress.heading("Setting up Serper");
        self.progress
            .plan(if storing { &[3, 1, 1] } else { &[3, 1] });
        self.progress.step("Testing the key, which uses one search");
        let search = SerperProvider {
            base_url: self.config.serper.url.clone(),
            key: key.key.clone(),
        };
        let results = test_search(&search, &self.client).await?;
        self.progress.note(&format!("{results} results"));

        if storing {
            self.progress.step("Storing the key");
            let stored = self
                .keychain
                .open()
                .and_then(|keychain| keychain.set(provider, &key.key));
            match stored {
                Ok(()) => self.progress.note("stored in the system keychain"),
                Err(_) => self.progress.warn(&format!(
                    "there is no system keychain to keep the key in, so set {} in your environment to use it",
                    provider.env_var()
                )),
            }
        }

        self.save(Provider::Serper, &SearXNGChoice::Unchanged)
    }

    /// The key to use, which only one typed in here needs storing.
    fn serper_key(&mut self, provider: KeyedProvider) -> Result<Option<SerperKey>> {
        if provider.env_var_is_set() {
            let key = keys::find(provider)?;
            self.progress
                .line(&format!("Using the key in {}.", provider.env_var()));
            return Ok(Some(SerperKey { key, typed: false }));
        }

        let stored = self
            .keychain
            .open()
            .ok()
            .and_then(|keychain| keychain.get(provider).ok().flatten());
        if let Some(key) = stored
            && self
                .prompts
                .confirm("Keep the key already in your keychain?")?
        {
            return Ok(Some(SerperKey { key, typed: false }));
        }

        loop {
            let Some(text) = self
                .prompts
                .secret("Serper API key (hidden, empty to go back):")?
            else {
                return Ok(None);
            };
            match ApiKey::new(&text) {
                Ok(key) => return Ok(Some(SerperKey { key, typed: true })),
                Err(e) => self.progress.warn(&format!("{e:#}")),
            }
        }
    }

    fn save(&mut self, provider: Provider, searxng: &SearXNGChoice) -> Result<Next> {
        self.progress.step("Saving your choice");
        config::save_choice(&self.path, provider, searxng)?;
        self.progress
            .note(&format!("wrote {}", self.path.display()));
        self.progress.finish();
        self.progress.line(&format!(
            "\nDone. hodis now searches with {}. Run `hodis setup` to change that.\n",
            provider.name()
        ));
        Ok(Next::Saved)
    }
}

/// An API key, and whether the user typed it in just now.
struct SerperKey {
    key: ApiKey,
    typed: bool,
}

/// In the order offered: the one that works straight away comes first.
const PROVIDERS: [Provider; 3] = [Provider::DuckDuckGo, Provider::SearXNG, Provider::Serper];

fn provider_choices() -> [Choice; 3] {
    [
        Choice::new("DuckDuckGo", "Works right away, nothing to install"),
        Choice::new("SearXNG", "Many engines at once, run from your computer"),
        Choice::new("Serper", "Google results, needs an API key"),
    ]
}

/// An address as typed: `localhost:8888` means `http://localhost:8888/`.
fn parse_address(text: &str) -> Result<Url> {
    let text = text.trim();
    let url = if text.contains("://") {
        Url::parse(text)
    } else {
        Url::parse(&format!("http://{text}"))
    }
    .with_context(|| format!("{text} is not an address"))?;
    config::base_url("the address", url)
}

/// Runs one search through `provider`, giving how many results came back.
async fn test_search(provider: &dyn SearchProvider, client: &Client) -> Result<usize> {
    let params = SearchParams {
        query: TEST_QUERY.to_owned(),
        page: None,
    };
    match timeout(TEST_TIMEOUT, provider.search(client, &params)).await {
        Ok(response) => Ok(response?.results.len()),
        Err(_) => bail!("no answer within {}s", TEST_TIMEOUT.as_secs()),
    }
}

/// SearXNG refuses `format=json` unless its settings allow it, as they do not
/// by default, and its 403 alone does not tell anyone that.
fn explain_searxng_error(error: anyhow::Error) -> anyhow::Error {
    let refused = error
        .downcast_ref::<reqwest::Error>()
        .and_then(reqwest::Error::status)
        == Some(StatusCode::FORBIDDEN);
    if refused {
        error.context(
            "this SearXNG refused a JSON search; add `json` to `search.formats` in its settings.yml",
        )
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::path::PathBuf;

    use anstream::adapter::strip_str;
    use keyring_core::sample::Store;
    use tempfile::TempDir;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{Choice, KeychainSource, Outcome, Prompts, Wizard, parse_address};
    use crate::config::Config;
    use crate::engine::{self, Engine, Paths};
    use crate::keys::{ApiKey, Keychain, KeyedProvider};
    use crate::progress::Progress;

    const DUCKDUCKGO_BODY: &str = include_str!("../../tests/fixtures/duckduckgo_healthy.html");
    const SEARXNG_BODY: &str = include_str!("../../tests/fixtures/healthy.json");
    const SERPER_BODY: &str = include_str!("../../tests/fixtures/serper_healthy.json");

    const DUCKDUCKGO: usize = 0;
    const SEARXNG: usize = 1;
    const SERPER: usize = 2;

    #[derive(Debug, Clone, Copy)]
    enum Answer {
        Pick(usize),
        Back,
        Yes,
        No,
        Text(&'static str),
    }

    /// A user who gives these answers in order, and no others.
    #[derive(Debug)]
    struct Script {
        answers: VecDeque<Answer>,
        asked: Vec<String>,
    }

    impl Script {
        fn new(answers: &[Answer]) -> Self {
            Self {
                answers: answers.iter().copied().collect(),
                asked: Vec::new(),
            }
        }

        fn next(&mut self, question: &str) -> Answer {
            self.asked.push(question.to_owned());
            self.answers
                .pop_front()
                .unwrap_or_else(|| panic!("nothing left to answer {question:?}"))
        }
    }

    impl Prompts for &mut Script {
        fn choose(&mut self, question: &str, _: &[Choice]) -> io::Result<Option<usize>> {
            Ok(match self.next(question) {
                Answer::Pick(index) => Some(index),
                Answer::Back => None,
                answer => panic!("{answer:?} does not answer {question:?}"),
            })
        }

        fn confirm(&mut self, question: &str) -> io::Result<bool> {
            Ok(match self.next(question) {
                Answer::Yes => true,
                Answer::No => false,
                answer => panic!("{answer:?} does not answer {question:?}"),
            })
        }

        fn text(&mut self, question: &str) -> io::Result<Option<String>> {
            Ok(match self.next(question) {
                Answer::Text(text) => Some(text.to_owned()),
                Answer::Back => None,
                answer => panic!("{answer:?} does not answer {question:?}"),
            })
        }

        fn secret(&mut self, question: &str) -> io::Result<Option<String>> {
            self.text(question)
        }
    }

    /// A wizard's surroundings: a config directory, an empty data directory and
    /// a keychain of its own.
    struct Setup {
        dir: TempDir,
        config: Config,
        keychain: Keychain,
    }

    struct Finished {
        outcome: Outcome,
        output: String,
        script: Script,
    }

    impl Setup {
        fn new(settings: &str) -> Self {
            Self {
                dir: tempfile::tempdir().expect("a scratch directory"),
                config: Config::parse(settings).expect("the settings parse").config,
                keychain: Keychain::new(Store::new().expect("a sample store")),
            }
        }

        fn config_path(&self) -> PathBuf {
            self.dir.path().join("config").join("config.toml")
        }

        fn saved(&self) -> Option<String> {
            fs::read_to_string(self.config_path()).ok()
        }

        fn paths(&self) -> Paths {
            Paths::new(self.dir.path().join("data"))
        }

        async fn run(&self, answers: &[Answer]) -> Finished {
            let mut script = Script::new(answers);
            let mut output = Vec::new();
            let outcome = Wizard::with(
                &self.config,
                &self.config_path(),
                Engine::new(self.paths()),
                KeychainSource::Given(self.keychain.clone()),
                &mut script,
                Progress::plain(&mut output),
            )
            .run()
            .await
            .expect("the wizard finishes");

            assert_eq!(script.answers.len(), 0, "unasked: {:?}", script.answers);
            Finished {
                outcome,
                output: strip_str(&String::from_utf8_lossy(&output)).to_string(),
                script,
            }
        }
    }

    async fn duckduckgo(response: ResponseTemplate) -> (MockServer, String) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/html/"))
            .respond_with(response)
            .mount(&server)
            .await;
        let settings = format!("[duckduckgo]\nurl = \"{}\"\n", server.uri());
        (server, settings)
    }

    async fn searxng(search: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(path("/healthz"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(path("/search"))
            .respond_with(search)
            .mount(&server)
            .await;
        server
    }

    /// A SearXNG address nothing answers at.
    const NOBODY: &str = "[searxng]\nurl = \"http://127.0.0.1:9/\"\n";

    fn html(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body.to_owned(), "text/html")
    }

    fn json(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body.to_owned(), "application/json")
    }

    #[tokio::test]
    async fn duckduckgo_is_tested_then_saved() {
        let (_server, settings) = duckduckgo(html(DUCKDUCKGO_BODY)).await;
        let setup = Setup::new(&settings);

        let done = setup.run(&[Answer::Pick(DUCKDUCKGO)]).await;

        assert_eq!(done.outcome, Outcome::Saved);
        assert_eq!(
            setup.saved().as_deref(),
            Some("provider = \"duckduckgo\"\n")
        );
        assert!(
            done.output.contains("(1/2) Testing a search"),
            "{}",
            done.output
        );
        assert!(
            done.output.contains("(2/2) Saving your choice"),
            "{}",
            done.output
        );
        let wrote = format!("wrote {}", setup.config_path().display());
        assert!(done.output.contains(&wrote), "{}", done.output);
    }

    #[tokio::test]
    async fn a_captcha_does_not_stop_duckduckgo_being_chosen() {
        let (_server, settings) = duckduckgo(ResponseTemplate::new(500)).await;
        let setup = Setup::new(&settings);

        let done = setup.run(&[Answer::Pick(DUCKDUCKGO)]).await;

        assert_eq!(done.outcome, Outcome::Saved);
        assert!(done.output.contains("warning:"), "{}", done.output);
    }

    #[tokio::test]
    async fn leaving_at_the_first_question_changes_nothing() {
        let setup = Setup::new("");

        let done = setup.run(&[Answer::Back]).await;

        assert_eq!(done.outcome, Outcome::Skipped);
        assert_eq!(setup.saved(), None);
    }

    #[tokio::test]
    async fn only_searxng_asks_which_searxng() {
        let (_server, settings) = duckduckgo(html(DUCKDUCKGO_BODY)).await;
        let setup = Setup::new(&settings);

        let done = setup.run(&[Answer::Pick(DUCKDUCKGO)]).await;

        assert_eq!(done.script.asked.len(), 1, "{:?}", done.script.asked);
    }

    #[tokio::test]
    async fn an_instance_that_answers_is_offered_first_and_recorded() {
        let server = searxng(json(SEARXNG_BODY)).await;
        let setup = Setup::new(&format!("[searxng]\nurl = \"{}\"\n", server.uri()));

        let done = setup.run(&[Answer::Pick(SEARXNG), Answer::Pick(0)]).await;

        assert_eq!(done.outcome, Outcome::Saved);
        let saved = setup.saved().expect("the config is saved");
        assert!(saved.contains("provider = \"searxng\""), "{saved}");
        assert!(
            saved.contains(&format!("url = \"{}/\"", server.uri())),
            "{saved}"
        );
    }

    #[tokio::test]
    async fn an_address_is_asked_again_until_it_is_one() {
        let server = searxng(json(SEARXNG_BODY)).await;
        let address: &'static str = server.uri().leak();
        let setup = Setup::new(NOBODY);

        let done = setup
            .run(&[
                Answer::Pick(SEARXNG),
                Answer::Pick(1),
                Answer::Text("not a valid address at all"),
                Answer::Text(address),
            ])
            .await;

        assert_eq!(done.outcome, Outcome::Saved);
        assert!(done.output.contains("is not an address"), "{}", done.output);
    }

    #[tokio::test]
    async fn an_instance_refusing_json_says_how_to_allow_it() {
        let server = searxng(ResponseTemplate::new(403)).await;
        let setup = Setup::new(&format!("[searxng]\nurl = \"{}\"\n", server.uri()));

        let done = setup
            .run(&[Answer::Pick(SEARXNG), Answer::Pick(0), Answer::Pick(2)])
            .await;

        assert_eq!(done.outcome, Outcome::Skipped);
        assert_eq!(setup.saved(), None, "a failed setup must not be saved");
        assert!(done.output.contains("search.formats"), "{}", done.output);
    }

    #[tokio::test]
    async fn declining_the_install_downloads_nothing() {
        let setup = Setup::new(NOBODY);

        let done = setup
            .run(&[
                Answer::Pick(SEARXNG),
                Answer::Pick(0),
                Answer::No,
                Answer::Back,
            ])
            .await;

        assert_eq!(done.outcome, Outcome::Skipped);
        assert!(!setup.paths().root().exists(), "nothing is written");
        assert!(done.output.contains("Need to get about"), "{}", done.output);
    }

    #[tokio::test]
    async fn an_install_is_described_before_it_starts() {
        let setup = Setup::new(NOBODY);

        let done = setup
            .run(&[
                Answer::Pick(SEARXNG),
                Answer::Pick(0),
                Answer::No,
                Answer::Back,
            ])
            .await;

        let root = setup.paths().root().display().to_string();
        for expected in [
            format!("uv {}", engine::UV_VERSION),
            format!("Python {}", engine::PYTHON_VERSION),
            format!("SearXNG {}", engine::short_version()),
            format!("It all goes in {root}"),
        ] {
            assert!(
                done.output.contains(&expected),
                "{expected}: {}",
                done.output
            );
        }
    }

    #[tokio::test]
    async fn an_installed_searxng_needs_no_confirming_and_is_saved_by_name() {
        let server = searxng(json(SEARXNG_BODY)).await;
        let setup = Setup::new(NOBODY);
        engine::fake_install(&setup.paths(), server.address().port())
            .expect("the install is faked");

        let done = setup.run(&[Answer::Pick(SEARXNG), Answer::Pick(0)]).await;

        assert_eq!(done.outcome, Outcome::Saved);
        let saved = setup.saved().expect("the config is saved");
        assert!(saved.contains("provider = \"searxng\""), "{saved}");
        assert!(saved.contains("embedded = \"only\""), "{saved}");
        assert!(!saved.contains("url"), "{saved}");
    }

    fn serper_settings(server: &MockServer) -> String {
        format!("[serper]\nurl = \"{}\"\n", server.uri())
    }

    /// The tests below read this variable's absence, and a process cannot
    /// safely change its own environment while other tests run.
    fn serper_key_in_environment() -> bool {
        let set = KeyedProvider::Serper.env_var_is_set();
        if set {
            eprintln!("skipped: SERPER_API_KEY is set");
        }
        set
    }

    #[tokio::test]
    async fn a_serper_key_is_tested_before_it_is_stored() {
        if serper_key_in_environment() {
            return;
        }
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("X-API-KEY", "good-key"))
            .respond_with(json(SERPER_BODY))
            .expect(1)
            .mount(&server)
            .await;
        let setup = Setup::new(&serper_settings(&server));

        let done = setup
            .run(&[Answer::Pick(SERPER), Answer::Text("good-key")])
            .await;

        assert_eq!(done.outcome, Outcome::Saved);
        assert_eq!(
            setup.keychain.get(KeyedProvider::Serper).expect("readable"),
            Some(ApiKey::new("good-key").expect("a valid key"))
        );
        assert_eq!(setup.saved().as_deref(), Some("provider = \"serper\"\n"));
        assert!(!done.output.contains("good-key"), "{}", done.output);
    }

    #[tokio::test]
    async fn a_refused_serper_key_is_not_kept() {
        if serper_key_in_environment() {
            return;
        }
        let server = MockServer::start().await;
        Mock::given(path("/search"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_raw(r#"{"message":"Unauthorized."}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let setup = Setup::new(&serper_settings(&server));

        let done = setup
            .run(&[
                Answer::Pick(SERPER),
                Answer::Text("bad-key"),
                Answer::Pick(2),
            ])
            .await;

        assert_eq!(done.outcome, Outcome::Skipped);
        assert_eq!(setup.saved(), None);
        assert_eq!(
            setup.keychain.get(KeyedProvider::Serper).expect("readable"),
            None
        );
        assert!(done.output.contains("Unauthorized"), "{}", done.output);
    }

    #[tokio::test]
    async fn a_stored_serper_key_can_be_kept_without_typing_it() {
        if serper_key_in_environment() {
            return;
        }
        let server = MockServer::start().await;
        Mock::given(path("/search"))
            .and(header("X-API-KEY", "stored-key"))
            .respond_with(json(SERPER_BODY))
            .mount(&server)
            .await;
        let setup = Setup::new(&serper_settings(&server));
        setup
            .keychain
            .set(
                KeyedProvider::Serper,
                &ApiKey::new("stored-key").expect("a valid key"),
            )
            .expect("the key is stored");

        let done = setup.run(&[Answer::Pick(SERPER), Answer::Yes]).await;

        assert_eq!(done.outcome, Outcome::Saved);
        assert!(!done.output.contains("Storing the key"), "{}", done.output);
    }

    #[tokio::test]
    async fn a_failure_can_lead_to_another_provider() {
        if serper_key_in_environment() {
            return;
        }
        let server = MockServer::start().await;
        Mock::given(path("/search"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let (_duckduckgo, duckduckgo_settings) = duckduckgo(html(DUCKDUCKGO_BODY)).await;
        let setup = Setup::new(&format!(
            "{duckduckgo_settings}{}",
            serper_settings(&server)
        ));

        let done = setup
            .run(&[
                Answer::Pick(SERPER),
                Answer::Text("bad-key"),
                Answer::Pick(1),
                Answer::Pick(DUCKDUCKGO),
            ])
            .await;

        assert_eq!(done.outcome, Outcome::Saved);
        assert_eq!(
            setup.saved().as_deref(),
            Some("provider = \"duckduckgo\"\n")
        );
    }

    #[test]
    fn an_address_can_leave_out_the_scheme_and_the_slash() {
        let parsed = |text| parse_address(text).expect("the address parses").to_string();
        assert_eq!(parsed("localhost:8888"), "http://localhost:8888/");
        assert_eq!(
            parsed("https://search.example/sx"),
            "https://search.example/sx/"
        );
    }

    #[test]
    fn an_address_must_be_web_one() {
        assert!(parse_address("ftp://example.com").is_err());
        assert!(parse_address("two words").is_err());
    }
}
