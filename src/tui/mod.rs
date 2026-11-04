//! The full-screen interface that `hodis` opens in a terminal when given no
//! query: a search field, the results to move through, and keys to open or
//! copy a result's link.

mod app;
mod input;
mod view;

use std::future::{self, Future};
use std::io::{self, Write};
use std::pin::Pin;
use std::time::Duration;

use anstream::{AutoStream, ColorChoice};
use anyhow::Result;
use crossterm::clipboard::CopyToClipboard;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream};
use crossterm::execute;
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::Backend;
use reqwest::Client;
use tokio::time::{MissedTickBehavior, interval};
use url::Url;

use crate::cache::Cache;
use crate::config::Config;
use crate::progress::Progress;
use crate::{Providers, SearchParams, SearchResponse, providers};
use app::{App, Command};
use view::Theme;

/// How often the animation shown while searching moves on.
const TICK: Duration = Duration::from_millis(80);

/// Runs the interface until the user quits, searching for `first` straight
/// away when given. When the providers include SearXNG, the engine hodis runs
/// itself is set up first if needed, with the steps reported to `progress`.
pub async fn run(
    config: Config,
    providers: Providers,
    cache: Option<Cache>,
    first: Option<SearchParams>,
    progress: &mut Progress<impl Write>,
) -> Result<()> {
    let client = Client::builder().build()?;
    providers::prepare(&providers, &config, &client, progress).await?;

    let session = Session {
        config,
        providers,
        client,
        cache,
    };
    let theme = Theme::new(AutoStream::choice(&io::stdout()) != ColorChoice::Never);
    let mut app = App::new(session.providers.clone());
    let first = first.map(|params| app.search_for(params));

    let outcome = match ratatui::try_init() {
        Ok(mut terminal) => match execute!(io::stdout(), EnableBracketedPaste) {
            Ok(()) => {
                let mut events = TerminalEvents(EventStream::new());
                let mut desktop = System;
                event_loop(
                    &mut terminal,
                    &mut events,
                    &session,
                    &mut desktop,
                    &mut app,
                    &theme,
                    first,
                )
                .await
            }
            Err(e) => Err(e.into()),
        },
        Err(e) => Err(anyhow::Error::from(e).context("cannot take over the terminal")),
    };
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    outcome
}

/// What every search made in the interface shares.
#[derive(Debug)]
struct Session {
    config: Config,
    providers: Providers,
    client: Client,
    cache: Option<Cache>,
}

impl Session {
    async fn search(&self, params: SearchParams, fresh: bool) -> Result<SearchResponse> {
        let cache = self
            .cache
            .clone()
            .map(|cache| if fresh { cache.without_reads() } else { cache });
        // The screen is taken, so a search starting the engine does so quietly.
        let mut progress = Progress::plain(io::sink());
        providers::search(
            &self.providers,
            &params,
            &self.config,
            &self.client,
            cache.as_ref(),
            &mut progress,
        )
        .await
    }
}

/// Where terminal events come from.
trait Events {
    /// The next event, or None when no more will come. `app` is what the
    /// screen shows while waiting.
    async fn next(&mut self, app: &App) -> Option<io::Result<Event>>;
}

struct TerminalEvents(EventStream);

impl Events for TerminalEvents {
    async fn next(&mut self, _app: &App) -> Option<io::Result<Event>> {
        self.0.next().await
    }
}

/// What the interface uses beyond the terminal.
trait Desktop {
    fn open(&mut self, url: &Url) -> io::Result<()>;
    fn copy(&mut self, text: &str) -> io::Result<()>;
}

/// The user's own browser and clipboard.
struct System;

impl Desktop for System {
    fn open(&mut self, url: &Url) -> io::Result<()> {
        open::that_detached(url.as_str())
    }

    /// Through the terminal, whose clipboard is the user's even over SSH.
    fn copy(&mut self, text: &str) -> io::Result<()> {
        execute!(io::stdout(), CopyToClipboard::to_clipboard_from(text))
    }
}

async fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    events: &mut impl Events,
    session: &Session,
    desktop: &mut impl Desktop,
    app: &mut App,
    theme: &Theme,
    mut command: Option<Command>,
) -> Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let mut search: Option<Pin<Box<dyn Future<Output = Result<SearchResponse>> + '_>>> = None;
    let mut ticks = interval(TICK);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        match command.take() {
            Some(Command::Quit) => return Ok(()),
            // Replacing a search under way drops it, which cancels it.
            Some(Command::Search { params, fresh }) => {
                search = Some(Box::pin(session.search(params, fresh)));
            }
            Some(Command::Open(url)) => {
                let outcome = desktop.open(&url);
                app.opened(&url, outcome);
            }
            Some(Command::Copy(url)) => {
                let outcome = desktop.copy(url.as_str());
                app.copied(&url, outcome);
            }
            None => {}
        }

        terminal.draw(|frame| view::draw(frame, app, theme))?;

        tokio::select! {
            event = events.next(app) => match event {
                Some(event) => command = app.handle(&event?),
                None => return Ok(()),
            },
            searched = finish(&mut search), if search.is_some() => {
                search = None;
                app.searched(searched);
            }
            _ = ticks.tick(), if app.is_searching() => app.tick(),
        }
    }
}

/// The output of `future`, which `select!` polls only while there is one.
async fn finish<T>(future: &mut Option<Pin<Box<dyn Future<Output = T> + '_>>>) -> T {
    match future {
        Some(future) => future.await,
        None => future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::future;
    use std::io;
    use std::num::NonZeroU32;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use reqwest::Client;
    use url::Url;
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{App, Desktop, Events, Session, Theme, event_loop};
    use crate::config::Config;
    use crate::{Provider, Providers, SearchParams};

    const HEALTHY_BODY: &str = include_str!("../../tests/fixtures/healthy.json");

    /// Events in order, each sent once no search is under way, the way a
    /// person waits for results before acting on them.
    struct Script(VecDeque<Event>);

    impl Events for Script {
        async fn next(&mut self, app: &App) -> Option<io::Result<Event>> {
            if app.is_searching() {
                future::pending::<()>().await;
            }
            self.0.pop_front().map(Ok)
        }
    }

    #[derive(Default)]
    struct Recorded {
        opened: Vec<Url>,
        copied: Vec<String>,
    }

    impl Desktop for Recorded {
        fn open(&mut self, url: &Url) -> io::Result<()> {
            self.opened.push(url.clone());
            Ok(())
        }

        fn copy(&mut self, text: &str) -> io::Result<()> {
            self.copied.push(text.to_owned());
            Ok(())
        }
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(text: &str) -> impl Iterator<Item = Event> + '_ {
        text.chars().map(|c| key(KeyCode::Char(c)))
    }

    fn session(server: &MockServer) -> Session {
        let config = Config::parse(&format!("[searxng]\nurl = \"{}\"\n", server.uri()))
            .expect("the config parses")
            .config;
        Session {
            config,
            providers: Providers::Listed(vec![Provider::SearXNG]),
            client: Client::new(),
            cache: None,
        }
    }

    async fn searxng(page: Option<u32>, response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        let mock = Mock::given(method("GET")).and(path("/search"));
        let mock = match page {
            Some(page) => mock.and(query_param("pageno", page.to_string())),
            None => mock.and(query_param_is_missing("pageno")),
        };
        mock.respond_with(response).expect(1).mount(&server).await;
        server
    }

    fn healthy() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_string(HEALTHY_BODY)
    }

    /// Runs the interface on a `width` by `height` test terminal through
    /// `events`, searching for `first` before any of them.
    async fn run(
        session: &Session,
        (width, height): (u16, u16),
        first: Option<SearchParams>,
        events: impl IntoIterator<Item = Event>,
    ) -> (Terminal<TestBackend>, Recorded) {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("a test terminal opens");
        let mut app = App::new(session.providers.clone());
        let first = first.map(|params| app.search_for(params));
        let mut desktop = Recorded::default();
        event_loop(
            &mut terminal,
            &mut Script(events.into_iter().collect()),
            session,
            &mut desktop,
            &mut app,
            &Theme::new(true),
            first,
        )
        .await
        .expect("the interface runs");
        (terminal, desktop)
    }

    fn dns_lookup(page: Option<u32>) -> SearchParams {
        SearchParams {
            query: "dns lookup".to_owned(),
            page: page.and_then(NonZeroU32::new),
        }
    }

    #[tokio::test]
    async fn a_typed_search_shows_results_to_open_and_copy() {
        let server = searxng(None, healthy()).await;
        let session = session(&server);
        let second = "https://toolbox.googleapps.com/apps/dig/";

        let events = typed("dns lookup").chain([
            key(KeyCode::Enter),
            key(KeyCode::Down),
            key(KeyCode::Char('y')),
            key(KeyCode::Enter),
        ]);
        let (terminal, desktop) = run(&session, (80, 24), None, events).await;

        let screen = terminal.backend().to_string();
        assert!(
            screen.contains("1. DNS Lookup Tool - DNS Tools - MxToolbox"),
            "{screen}"
        );
        assert!(screen.contains(&format!("Opening {second}")), "{screen}");
        assert_eq!(desktop.copied, [second]);
        assert_eq!(
            desktop.opened.iter().map(Url::as_str).collect::<Vec<_>>(),
            [second]
        );
    }

    #[tokio::test]
    async fn a_search_given_at_the_start_runs_before_any_key() {
        let server = searxng(None, healthy()).await;
        let (terminal, _) = run(&session(&server), (80, 24), Some(dns_lookup(None)), []).await;

        let screen = terminal.backend().to_string();
        assert!(screen.contains("Page 1 · 3 results · SearXNG"), "{screen}");
    }

    #[tokio::test]
    async fn the_next_page_is_searched_for() {
        let server = searxng(None, healthy()).await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("pageno", "2"))
            .respond_with(healthy())
            .expect(1)
            .mount(&server)
            .await;

        let events = [key(KeyCode::Char('n'))];
        let (terminal, _) = run(&session(&server), (80, 24), Some(dns_lookup(None)), events).await;

        let screen = terminal.backend().to_string();
        assert!(screen.contains("Page 2 · 3 results"), "{screen}");
    }

    #[tokio::test]
    async fn a_failed_search_says_why() {
        let server = searxng(Some(4), ResponseTemplate::new(500)).await;
        let (terminal, _) = run(&session(&server), (80, 24), Some(dns_lookup(Some(4))), []).await;

        let screen = terminal.backend().to_string();
        assert!(
            screen.contains("error: cannot search SearXNG: "),
            "{screen}"
        );
        assert!(screen.contains("(500 Internal Server"), "{screen}");
    }

    #[tokio::test]
    async fn quitting_drops_the_search_under_way() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(healthy().set_delay(std::time::Duration::from_secs(30)))
            .mount(&server)
            .await;
        let session = session(&server);

        let events = typed("dns lookup").chain([key(KeyCode::Enter)]);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("a test terminal opens");
        let mut app = App::new(session.providers.clone());
        let mut events: VecDeque<Event> = events.collect();
        // Ctrl-C arrives while the search waits on its slow answer.
        events.push_back(Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        let quit = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            event_loop(
                &mut terminal,
                &mut Unwaiting(events),
                &session,
                &mut Recorded::default(),
                &mut app,
                &Theme::new(true),
                None,
            ),
        )
        .await;
        assert!(
            matches!(quit, Ok(Ok(()))),
            "the interface waited for the search"
        );
    }

    /// Events sent without waiting for searches.
    struct Unwaiting(VecDeque<Event>);

    impl Events for Unwaiting {
        async fn next(&mut self, _app: &App) -> Option<io::Result<Event>> {
            match self.0.pop_front() {
                Some(event) => Some(Ok(event)),
                None => future::pending().await,
            }
        }
    }
}
