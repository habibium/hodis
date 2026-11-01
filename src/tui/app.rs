//! What the interface holds and how keys change it. Drawing, the terminal and
//! the network are left to the rest of the module, so all of this runs in
//! tests as plain data.

use std::io;
use std::num::NonZeroU32;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use url::Url;

use super::input::Input;
use crate::{Providers, SearchParams, SearchResponse, SearchResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Typing a query.
    Editing,
    /// Moving through the results.
    Browsing,
}

/// What the app asks of the network and the desktop around it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Search for `params`, skipping cached results if `fresh`.
    Search {
        params: SearchParams,
        fresh: bool,
    },
    Open(Url),
    Copy(Url),
    Quit,
}

/// What the area below the search field shows.
#[derive(Debug)]
pub(crate) enum Shown {
    Nothing,
    Page {
        query: String,
        page: NonZeroU32,
        response: Box<SearchResponse>,
    },
    Failed {
        query: String,
        page: NonZeroU32,
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    /// A query submitted.
    New,
    /// The page after or before the one shown.
    Turn,
    /// The page shown, searched again past the cache.
    Refresh,
}

#[derive(Debug)]
pub(crate) struct Pending {
    pub(crate) query: String,
    pub(crate) page: NonZeroU32,
    pub(crate) reason: Reason,
}

/// A message that replaces the status line until the next key.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Notice {
    Info(String),
    Error(String),
}

#[derive(Debug)]
pub(crate) struct App {
    pub(crate) input: Input,
    pub(crate) mode: Mode,
    pub(crate) shown: Shown,
    /// The search under way. Its results replace what is shown when they
    /// arrive.
    pub(crate) pending: Option<Pending>,
    pub(crate) notice: Option<Notice>,
    pub(crate) selected: usize,
    /// The first result drawn, which drawing moves to keep the selection in
    /// view.
    pub(crate) scroll: usize,
    /// How many results the last frame had room for, which is how far a page
    /// key moves.
    pub(crate) visible: usize,
    /// Counts frames of the animation shown while searching.
    pub(crate) ticks: usize,
    /// The providers searched, named before the first search.
    pub(crate) providers: Providers,
}

impl App {
    pub(crate) fn new(providers: Providers) -> Self {
        Self {
            input: Input::default(),
            mode: Mode::Editing,
            shown: Shown::Nothing,
            pending: None,
            notice: None,
            selected: 0,
            scroll: 0,
            visible: 1,
            ticks: 0,
            providers,
        }
    }

    /// Searches for `params` as though they had been typed in.
    pub(crate) fn search_for(&mut self, params: SearchParams) -> Command {
        self.input.set(&params.query);
        self.mode = Mode::Browsing;
        self.start(
            params.query,
            params.page.unwrap_or(NonZeroU32::MIN),
            Reason::New,
        )
    }

    pub(crate) fn is_searching(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
    }

    pub(crate) fn results(&self) -> &[SearchResult] {
        match &self.shown {
            Shown::Page { response, .. } => &response.results,
            Shown::Nothing | Shown::Failed { .. } => &[],
        }
    }

    pub(crate) fn selected_result(&self) -> Option<&SearchResult> {
        self.results().get(self.selected)
    }

    /// Whether leaving the search field has results, or a search, to go back
    /// to.
    pub(crate) fn has_results_view(&self) -> bool {
        self.pending.is_some() || !matches!(self.shown, Shown::Nothing)
    }

    pub(crate) fn handle(&mut self, event: &Event) -> Option<Command> {
        match event {
            // Windows reports releases too, and a key acts once.
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.notice = None;
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    return Some(Command::Quit);
                }
                match self.mode {
                    Mode::Editing => self.edit(*key),
                    Mode::Browsing => self.browse(*key),
                }
            }
            Event::Paste(text) if self.mode == Mode::Editing => {
                self.input.paste(text);
                None
            }
            _ => None,
        }
    }

    /// Takes in how the search under way went.
    pub(crate) fn searched(&mut self, outcome: anyhow::Result<SearchResponse>) {
        let Some(Pending {
            query,
            page,
            reason,
        }) = self.pending.take()
        else {
            return;
        };
        match outcome {
            // Stepping past the last page keeps the last page in view.
            Ok(response) if response.results.is_empty() && reason == Reason::Turn => {
                self.notice = Some(Notice::Info(format!("Page {page} has no results")));
            }
            Ok(response) => {
                if reason != Reason::Refresh {
                    self.selected = 0;
                    self.scroll = 0;
                }
                self.shown = Shown::Page {
                    query,
                    page,
                    response: Box::new(response),
                };
                self.select(self.selected);
            }
            Err(e) if reason == Reason::New => {
                self.shown = Shown::Failed {
                    query,
                    page,
                    error: format!("{e:#}"),
                };
                self.selected = 0;
                self.scroll = 0;
            }
            // What is shown is still right; only the step away from it failed.
            Err(e) => self.notice = Some(Notice::Error(format!("{e:#}"))),
        }
    }

    pub(crate) fn opened(&mut self, url: &Url, outcome: io::Result<()>) {
        self.notice = Some(match outcome {
            Ok(()) => Notice::Info(format!("Opening {url}")),
            Err(e) => Notice::Error(format!("cannot open a browser: {e}")),
        });
    }

    pub(crate) fn copied(&mut self, url: &Url, outcome: io::Result<()>) {
        self.notice = Some(match outcome {
            Ok(()) => Notice::Info(format!("Copied {url}")),
            Err(e) => Notice::Error(format!("cannot copy the link: {e}")),
        });
    }

    fn edit(&mut self, key: KeyEvent) -> Option<Command> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Enter => return self.submit(),
            KeyCode::Esc => return self.stop_editing(),
            KeyCode::Char('a') if ctrl && !alt => self.input.home(),
            KeyCode::Char('e') if ctrl && !alt => self.input.end(),
            KeyCode::Char('u') if ctrl && !alt => self.input.delete_to_start(),
            KeyCode::Char('k') if ctrl && !alt => self.input.delete_to_end(),
            KeyCode::Char('w') if ctrl && !alt => self.input.delete_word(),
            KeyCode::Char('b') if alt && !ctrl => self.input.word_left(),
            KeyCode::Char('f') if alt && !ctrl => self.input.word_right(),
            // Windows reports AltGr, which types characters such as `@` on
            // many keyboards, as Ctrl and Alt together.
            KeyCode::Char(c) if ctrl == alt => self.input.insert(c),
            KeyCode::Backspace if ctrl || alt => self.input.delete_word(),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left if ctrl || alt => self.input.word_left(),
            KeyCode::Right if ctrl || alt => self.input.word_right(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            _ => {}
        }
        None
    }

    fn browse(&mut self, key: KeyEvent) -> Option<Command> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('n') => self.select(self.selected.saturating_add(1)),
                KeyCode::Char('p') => self.select(self.selected.saturating_sub(1)),
                _ => {}
            }
            return None;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Some(Command::Quit),
            KeyCode::Char('/' | 'i') => {
                self.mode = Mode::Editing;
                self.input.end();
            }
            KeyCode::Down | KeyCode::Char('j') => self.select(self.selected.saturating_add(1)),
            KeyCode::Up | KeyCode::Char('k') => self.select(self.selected.saturating_sub(1)),
            KeyCode::PageDown => self.select(self.selected.saturating_add(self.visible.max(1))),
            KeyCode::PageUp => self.select(self.selected.saturating_sub(self.visible.max(1))),
            KeyCode::Home | KeyCode::Char('g') => self.select(0),
            KeyCode::End | KeyCode::Char('G') => self.select(usize::MAX),
            KeyCode::Enter | KeyCode::Char('o') => return self.open(),
            KeyCode::Char('y') => {
                return self
                    .selected_result()
                    .map(|result| Command::Copy(result.url.clone()));
            }
            KeyCode::Right | KeyCode::Char('n') => return self.next_page(),
            KeyCode::Left | KeyCode::Char('p') => return self.previous_page(),
            KeyCode::Char('r') => return self.refresh(),
            _ => {}
        }
        None
    }

    fn submit(&mut self) -> Option<Command> {
        let query = self.input.text().trim();
        if query.is_empty() {
            return None;
        }
        let query = query.to_owned();
        self.mode = Mode::Browsing;
        Some(self.start(query, NonZeroU32::MIN, Reason::New))
    }

    /// Leaves the search field, putting back the query of the search it
    /// returns to. With nothing to return to, leaving quits.
    fn stop_editing(&mut self) -> Option<Command> {
        let query = match (&self.pending, &self.shown) {
            (Some(pending), _) => pending.query.clone(),
            (None, Shown::Page { query, .. } | Shown::Failed { query, .. }) => query.clone(),
            (None, Shown::Nothing) => return Some(Command::Quit),
        };
        self.input.set(&query);
        self.mode = Mode::Browsing;
        None
    }

    fn select(&mut self, index: usize) {
        self.selected = index.min(self.results().len().saturating_sub(1));
    }

    fn open(&mut self) -> Option<Command> {
        let url = &self.selected_result()?.url;
        // A link to a local file or to another program's scheme can do more
        // than show a page.
        if matches!(url.scheme(), "http" | "https") {
            Some(Command::Open(url.clone()))
        } else {
            self.notice = Some(Notice::Error(
                "hodis opens only web links (http and https)".to_owned(),
            ));
            None
        }
    }

    fn next_page(&mut self) -> Option<Command> {
        if self.pending.is_some() {
            return None;
        }
        let Shown::Page {
            query,
            page,
            response,
        } = &self.shown
        else {
            return None;
        };
        if response.results.is_empty() {
            return None;
        }
        let (query, page) = (query.clone(), page.checked_add(1)?);
        Some(self.start(query, page, Reason::Turn))
    }

    fn previous_page(&mut self) -> Option<Command> {
        if self.pending.is_some() {
            return None;
        }
        let Shown::Page { query, page, .. } = &self.shown else {
            return None;
        };
        let Some(previous) = NonZeroU32::new(page.get() - 1) else {
            self.notice = Some(Notice::Info("This is the first page".to_owned()));
            return None;
        };
        let query = query.clone();
        Some(self.start(query, previous, Reason::Turn))
    }

    fn refresh(&mut self) -> Option<Command> {
        if self.pending.is_some() {
            return None;
        }
        let (query, page) = match &self.shown {
            Shown::Page { query, page, .. } | Shown::Failed { query, page, .. } => {
                (query.clone(), *page)
            }
            Shown::Nothing => return None,
        };
        Some(self.start(query, page, Reason::Refresh))
    }

    fn start(&mut self, query: String, page: NonZeroU32, reason: Reason) -> Command {
        let params = SearchParams {
            query: query.clone(),
            // The first page is what a search without one gets, which keeps
            // the request the same as the command line's.
            page: (page > NonZeroU32::MIN).then_some(page),
        };
        self.pending = Some(Pending {
            query,
            page,
            reason,
        });
        self.ticks = 0;
        Command::Search {
            params,
            fresh: reason == Reason::Refresh,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::num::NonZeroU32;

    use anyhow::anyhow;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use url::Url;

    use super::{App, Command, Mode, Notice, Shown};
    use crate::{Diagnostics, Provider, Providers, SearchParams, SearchResponse, SearchResult};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            assert_eq!(app.handle(&key(KeyCode::Char(c))), None);
        }
    }

    fn search(query: &str, page: Option<u32>, fresh: bool) -> Command {
        let params = SearchParams {
            query: query.to_owned(),
            page: page.and_then(NonZeroU32::new),
        };
        Command::Search { params, fresh }
    }

    fn response(urls: &[&str]) -> SearchResponse {
        SearchResponse {
            query: "rust".to_owned(),
            answer: None,
            results: urls
                .iter()
                .map(|url| SearchResult {
                    title: format!("Title of {url}"),
                    url: Url::parse(url).expect("test urls are valid"),
                    snippet: String::new(),
                    image: None,
                    source: String::new(),
                    provider: Provider::SearXNG,
                })
                .collect(),
            providers: vec![Provider::SearXNG],
            diagnostics: Diagnostics::default(),
        }
    }

    /// An app showing `urls` as the first page of results for "rust".
    fn browsing(urls: &[&str]) -> App {
        let mut app = App::new(Providers::Auto);
        type_text(&mut app, "rust");
        app.handle(&key(KeyCode::Enter));
        app.searched(Ok(response(urls)));
        app
    }

    const THREE: [&str; 3] = [
        "https://www.rust-lang.org/",
        "https://doc.rust-lang.org/book/",
        "https://crates.io/",
    ];

    #[test]
    fn enter_searches_for_what_was_typed() {
        let mut app = App::new(Providers::Auto);
        type_text(&mut app, "  rust book ");
        assert_eq!(
            app.handle(&key(KeyCode::Enter)),
            Some(search("rust book", None, false))
        );
        assert_eq!(app.mode, Mode::Browsing);
        assert!(app.is_searching());
    }

    #[test]
    fn a_blank_query_is_not_searched() {
        let mut app = App::new(Providers::Auto);
        type_text(&mut app, "   ");
        assert_eq!(app.handle(&key(KeyCode::Enter)), None);
        assert_eq!(app.mode, Mode::Editing);
    }

    #[test]
    fn results_arriving_are_shown_from_the_top() {
        let app = browsing(&THREE);
        assert!(!app.is_searching());
        assert_eq!(app.results().len(), 3);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn the_selection_moves_within_the_results() {
        let mut app = browsing(&THREE);
        app.handle(&key(KeyCode::Up));
        assert_eq!(app.selected, 0);
        app.handle(&key(KeyCode::Char('j')));
        app.handle(&key(KeyCode::Down));
        app.handle(&key(KeyCode::Down));
        assert_eq!(app.selected, 2);
        app.handle(&key(KeyCode::Char('k')));
        assert_eq!(app.selected, 1);
        app.handle(&key(KeyCode::Char('G')));
        assert_eq!(app.selected, 2);
        app.handle(&key(KeyCode::Home));
        assert_eq!(app.selected, 0);
        app.handle(&ctrl('n'));
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn page_keys_move_by_what_fits_on_screen() {
        let mut app = browsing(&THREE);
        app.visible = 2;
        app.handle(&key(KeyCode::PageDown));
        assert_eq!(app.selected, 2);
        app.handle(&key(KeyCode::PageUp));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn enter_opens_and_y_copies_the_selected_link() {
        let mut app = browsing(&THREE);
        app.handle(&key(KeyCode::Down));
        let url = Url::parse(THREE[1]).expect("test urls are valid");
        assert_eq!(
            app.handle(&key(KeyCode::Enter)),
            Some(Command::Open(url.clone()))
        );
        assert_eq!(
            app.handle(&key(KeyCode::Char('y'))),
            Some(Command::Copy(url.clone()))
        );

        app.opened(&url, Ok(()));
        assert_eq!(app.notice, Some(Notice::Info(format!("Opening {url}"))));
        app.copied(&url, Err(io::Error::other("no terminal")));
        assert_eq!(
            app.notice,
            Some(Notice::Error(
                "cannot copy the link: no terminal".to_owned()
            ))
        );
    }

    #[test]
    fn only_web_links_are_opened() {
        let mut app = browsing(&["file:///etc/passwd", "mailto:someone@example.com"]);
        assert_eq!(app.handle(&key(KeyCode::Enter)), None);
        assert!(matches!(app.notice, Some(Notice::Error(_))));
        app.handle(&key(KeyCode::Down));
        assert_eq!(app.handle(&key(KeyCode::Char('o'))), None);
        // Copying one is harmless.
        assert!(matches!(
            app.handle(&key(KeyCode::Char('y'))),
            Some(Command::Copy(_))
        ));
    }

    #[test]
    fn the_next_key_clears_a_notice() {
        let mut app = browsing(&THREE);
        app.handle(&key(KeyCode::Char('p')));
        assert_eq!(
            app.notice,
            Some(Notice::Info("This is the first page".to_owned()))
        );
        app.handle(&key(KeyCode::Down));
        assert_eq!(app.notice, None);
    }

    #[test]
    fn pages_turn_both_ways() {
        let mut app = browsing(&THREE);
        assert_eq!(
            app.handle(&key(KeyCode::Char('n'))),
            Some(search("rust", Some(2), false))
        );
        app.searched(Ok(response(&["https://example.com/2"])));
        assert!(matches!(app.shown, Shown::Page { page, .. } if page.get() == 2));

        assert_eq!(
            app.handle(&key(KeyCode::Left)),
            Some(search("rust", None, false))
        );
        app.searched(Ok(response(&THREE)));
        assert!(matches!(app.shown, Shown::Page { page, .. } if page.get() == 1));
    }

    #[test]
    fn a_page_without_results_keeps_the_last_page_in_view() {
        let mut app = browsing(&THREE);
        app.handle(&key(KeyCode::Down));
        app.handle(&key(KeyCode::Char('n')));
        app.searched(Ok(response(&[])));

        assert!(matches!(app.shown, Shown::Page { page, .. } if page.get() == 1));
        assert_eq!(app.results().len(), 3);
        assert_eq!(app.selected, 1);
        assert_eq!(
            app.notice,
            Some(Notice::Info("Page 2 has no results".to_owned()))
        );
    }

    #[test]
    fn a_failed_turn_keeps_the_page_and_says_why() {
        let mut app = browsing(&THREE);
        app.handle(&key(KeyCode::Char('n')));
        app.searched(Err(anyhow!("no answer within 10s")));

        assert_eq!(app.results().len(), 3);
        assert_eq!(
            app.notice,
            Some(Notice::Error("no answer within 10s".to_owned()))
        );
    }

    #[test]
    fn a_failed_search_replaces_the_results() {
        let mut app = browsing(&THREE);
        app.handle(&key(KeyCode::Char('/')));
        app.handle(&ctrl('u'));
        type_text(&mut app, "other");
        app.handle(&key(KeyCode::Enter));
        app.searched(Err(anyhow!("cannot search SearXNG")));

        assert!(matches!(
            &app.shown,
            Shown::Failed { query, error, .. } if query == "other" && error == "cannot search SearXNG"
        ));
        assert_eq!(app.results().len(), 0);
    }

    #[test]
    fn keys_for_another_search_wait_for_the_one_under_way() {
        let mut app = browsing(&THREE);
        app.handle(&key(KeyCode::Char('n')));
        assert_eq!(app.handle(&key(KeyCode::Char('n'))), None);
        assert_eq!(app.handle(&key(KeyCode::Char('r'))), None);
        // The results on screen can still be used.
        assert!(matches!(
            app.handle(&key(KeyCode::Enter)),
            Some(Command::Open(_))
        ));
    }

    #[test]
    fn refreshing_searches_the_same_page_past_the_cache() {
        let mut app = browsing(&THREE);
        app.handle(&key(KeyCode::Char('n')));
        app.searched(Ok(response(&THREE)));
        app.handle(&key(KeyCode::End));

        assert_eq!(
            app.handle(&key(KeyCode::Char('r'))),
            Some(search("rust", Some(2), true))
        );
        app.searched(Ok(response(&THREE)));
        assert_eq!(app.selected, 2, "a refresh keeps the selection");
    }

    #[test]
    fn a_failed_search_can_be_tried_again() {
        let mut app = App::new(Providers::Auto);
        type_text(&mut app, "rust");
        app.handle(&key(KeyCode::Enter));
        app.searched(Err(anyhow!("timed out")));

        assert_eq!(
            app.handle(&key(KeyCode::Char('r'))),
            Some(search("rust", None, true))
        );
    }

    #[test]
    fn editing_starts_from_the_query_and_escape_puts_it_back() {
        let mut app = browsing(&THREE);
        app.handle(&key(KeyCode::Char('/')));
        assert_eq!(app.mode, Mode::Editing);
        type_text(&mut app, " book");
        assert_eq!(app.input.text(), "rust book");

        assert_eq!(app.handle(&key(KeyCode::Esc)), None);
        assert_eq!(app.mode, Mode::Browsing);
        assert_eq!(app.input.text(), "rust");
    }

    #[test]
    fn typing_in_the_field_does_not_trigger_browsing_keys() {
        let mut app = browsing(&THREE);
        app.handle(&key(KeyCode::Char('i')));
        type_text(&mut app, "qjnyr/");
        assert_eq!(app.input.text(), "rustqjnyr/");
        assert_eq!(app.mode, Mode::Editing);
    }

    #[test]
    fn quitting() {
        let mut app = App::new(Providers::Auto);
        assert_eq!(app.handle(&key(KeyCode::Esc)), Some(Command::Quit));

        let mut app = browsing(&THREE);
        assert_eq!(app.handle(&key(KeyCode::Char('q'))), Some(Command::Quit));
        assert_eq!(app.handle(&key(KeyCode::Esc)), Some(Command::Quit));
        app.handle(&key(KeyCode::Char('/')));
        assert_eq!(app.handle(&ctrl('c')), Some(Command::Quit));
    }

    #[test]
    fn a_key_acts_once_where_releases_are_reported() {
        let mut app = browsing(&THREE);
        let release = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Down,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        app.handle(&key(KeyCode::Down));
        app.handle(&release);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn altgr_characters_are_typed() {
        let mut app = App::new(Providers::Auto);
        app.handle(&Event::Key(KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )));
        assert_eq!(app.input.text(), "@");
    }

    #[test]
    fn pasting_goes_into_the_field_only_while_typing() {
        let mut app = App::new(Providers::Auto);
        app.handle(&Event::Paste("capital\nof norway".to_owned()));
        assert_eq!(app.input.text(), "capital of norway");

        let mut app = browsing(&THREE);
        app.handle(&Event::Paste("ignored".to_owned()));
        assert_eq!(app.input.text(), "rust");
    }

    #[test]
    fn a_search_given_at_the_start_runs_at_once() {
        let mut app = App::new(Providers::Auto);
        let command = app.search_for(SearchParams {
            query: "rust".to_owned(),
            page: NonZeroU32::new(3),
        });
        assert_eq!(command, search("rust", Some(3), false));
        assert_eq!(app.input.text(), "rust");
        assert_eq!(app.mode, Mode::Browsing);
    }
}
