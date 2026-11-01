//! Drawing the app: the search field at the top, the answer and results below
//! it, and a status line and the keys at the bottom.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::app::{App, Mode, Notice, Reason, Shown};
use crate::output::{one_line, warnings};
use crate::{Provider, SearchResponse, SearchResult};

const SNIPPET_LINES: usize = 2;
const ANSWER_LINES: usize = 3;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Marks the selected result's lines, in the gutter left of the results.
const BAR: &str = "▌";
const GUTTER: u16 = 2;

/// Colors come from the terminal's own palette of 16, which its theme sets,
/// so they suit it.
#[derive(Debug, Clone)]
pub(crate) struct Theme {
    title: Style,
    url: Style,
    source: Style,
    answer: Style,
    accent: Style,
    warning: Style,
    error: Style,
    key: Style,
    faint: Style,
}

impl Theme {
    pub(crate) fn new(colored: bool) -> Self {
        let paint = |style: Style, color: Color| if colored { style.fg(color) } else { style };
        Self {
            title: paint(Style::new().bold(), Color::LightMagenta),
            // Without color, an underline tells the address from the text.
            url: if colored {
                Style::new().fg(Color::Cyan)
            } else {
                Style::new().underlined()
            },
            source: Style::new().dim(),
            answer: Style::new().bold(),
            accent: paint(Style::new().bold(), Color::LightMagenta),
            warning: paint(Style::new(), Color::Yellow),
            error: paint(Style::new().bold(), Color::Red),
            key: Style::new().bold(),
            faint: Style::new().dim(),
        }
    }
}

pub(crate) fn draw(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let [field, body, status, keys] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_field(frame, field, app, theme);
    // A blank line under the field.
    let body = Rect {
        y: body.y.saturating_add(1),
        height: body.height.saturating_sub(1),
        ..body
    };
    draw_body(frame, body, app, theme);
    frame.render_widget(status_line(app, theme), status);
    frame.render_widget(key_line(app, usize::from(keys.width), theme), keys);
}

fn draw_field(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let editing = app.mode == Mode::Editing;
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(if editing { theme.accent } else { theme.faint })
        .title(" hodis ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // One column of padding inside the border.
    let text = Rect {
        x: inner.x.saturating_add(1),
        width: inner.width.saturating_sub(1),
        ..inner
    };
    if text.is_empty() {
        return;
    }
    let (visible, cursor) = app.input.window(usize::from(text.width));
    let line = if app.input.text().is_empty() {
        Line::styled("Search the web", theme.faint)
    } else {
        Line::raw(visible)
    };
    frame.render_widget(line, text);
    if editing {
        let column = u16::try_from(cursor).unwrap_or(u16::MAX);
        frame.set_cursor_position((text.x.saturating_add(column), text.y));
    }
}

fn draw_body(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let width = usize::from(area.width.saturating_sub(GUTTER));
    if width == 0 || area.height == 0 {
        return;
    }
    let text = Rect {
        x: area.x + GUTTER,
        width: area.width - GUTTER,
        ..area
    };
    match &app.shown {
        Shown::Nothing => {}
        Shown::Failed { error, .. } => {
            let message = Line::from(vec![
                Span::styled("error: ", theme.error),
                Span::raw(one_line(error)),
            ]);
            frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: true }), text);
        }
        Shown::Page {
            query, response, ..
        } if response.results.is_empty() => {
            let message = format!("No results for “{}”.", one_line(query));
            frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: true }), text);
        }
        Shown::Page { response, .. } => {
            let answer = answer_lines(response, width, usize::from(area.height) / 3, theme);
            let height = u16::try_from(answer.len())
                .unwrap_or(u16::MAX)
                .min(area.height);
            let [top, rest] =
                Layout::vertical([Constraint::Length(height), Constraint::Fill(1)]).areas(area);
            frame.render_widget(
                Paragraph::new(answer),
                Rect {
                    x: text.x,
                    width: text.width,
                    ..top
                },
            );
            let results = result_blocks(&response.results, width, theme);
            draw_results(frame, rest, app, results, theme);
        }
    }
}

/// The answer, its source and a blank line in at most `height` lines, or
/// nothing if they do not fit, which leaves the room to the results.
fn answer_lines(
    response: &SearchResponse,
    width: usize,
    height: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let Some(answer) = &response.answer else {
        return Vec::new();
    };
    let room = height.saturating_sub(2).min(ANSWER_LINES);
    if room == 0 {
        return Vec::new();
    }
    let mut lines: Vec<Line> = wrap(&one_line(&answer.content), width, room)
        .into_iter()
        .map(|line| Line::styled(line, theme.answer))
        .collect();
    if let Some(url) = &answer.url {
        lines.push(Line::styled(fit(url.as_str(), width), theme.source));
    }
    lines.push(Line::default());
    lines
}

/// Each result's lines: its number and title, its address, and the start of
/// its snippet.
fn result_blocks(results: &[SearchResult], width: usize, theme: &Theme) -> Vec<Vec<Line<'static>>> {
    results
        .iter()
        .zip(1..)
        .map(|(result, number)| {
            let title = format!("{number}. {}", one_line(&result.title));
            let source = one_line(&result.source);
            let source = if source.is_empty() {
                String::new()
            } else {
                format!(" [{source}]")
            };
            // The source goes first when room runs out.
            let title_line = if title.width() + source.width() <= width {
                Line::from(vec![
                    Span::styled(title, theme.title),
                    Span::styled(source, theme.source),
                ])
            } else {
                Line::styled(fit(&title, width), theme.title)
            };

            let mut lines = vec![
                title_line,
                Line::styled(fit(result.url.as_str(), width), theme.url),
            ];
            lines.extend(
                wrap(&one_line(&result.snippet), width, SNIPPET_LINES)
                    .into_iter()
                    .map(Line::raw),
            );
            lines
        })
        .collect()
}

/// Draws as many results as fit, a blank line apart, scrolled so that the
/// selected one shows whole when there is room for it.
fn draw_results(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    results: Vec<Vec<Line<'static>>>,
    theme: &Theme,
) {
    if results.is_empty() || area.is_empty() {
        return;
    }
    let height = usize::from(area.height);
    let selected = app.selected.min(results.len() - 1);
    let span = |from: usize, to: usize| -> usize {
        results[from..=to].iter().map(Vec::len).sum::<usize>() + (to - from)
    };
    let mut scroll = app.scroll.min(selected);
    while scroll < selected && span(scroll, selected) > height {
        scroll += 1;
    }
    app.scroll = scroll;

    let mut y = area.y;
    let mut whole = 0;
    for (index, lines) in results.into_iter().enumerate().skip(scroll) {
        if index > scroll {
            y += 1;
        }
        if y >= area.bottom() {
            break;
        }
        if usize::from(y) + lines.len() <= usize::from(area.bottom()) {
            whole += 1;
        }
        for line in lines {
            if y >= area.bottom() {
                break;
            }
            if index == selected {
                frame.render_widget(Span::styled(BAR, theme.accent), Rect::new(area.x, y, 1, 1));
            }
            frame.render_widget(line, Rect::new(area.x + GUTTER, y, area.width - GUTTER, 1));
            y += 1;
        }
    }
    app.visible = whole.max(1);
}

fn status_line(app: &App, theme: &Theme) -> Line<'static> {
    if let Some(notice) = &app.notice {
        return match notice {
            Notice::Info(text) => Line::raw(format!(" {}", one_line(text))),
            Notice::Error(text) => Line::from(vec![
                Span::raw(" "),
                Span::styled("error: ", theme.error),
                Span::raw(one_line(text)),
            ]),
        };
    }
    if let Some(pending) = &app.pending {
        let frame = SPINNER[app.ticks % SPINNER.len()];
        let doing = match pending.reason {
            Reason::New => format!("Searching for “{}”", one_line(&pending.query)),
            Reason::Turn => format!("Loading page {}", pending.page),
            Reason::Refresh => format!("Searching again for “{}”", one_line(&pending.query)),
        };
        return Line::from(vec![
            Span::styled(format!(" {frame} "), theme.accent),
            Span::raw(doing),
        ]);
    }
    match &app.shown {
        Shown::Nothing => Line::styled(
            format!(" Searches go to {}", names(&app.providers)),
            theme.faint,
        ),
        Shown::Failed { .. } => Line::default(),
        Shown::Page { page, response, .. } => {
            let count = response.results.len();
            let s = if count == 1 { "" } else { "s" };
            let mut spans = vec![Span::raw(format!(
                " Page {page} · {count} result{s} · {}",
                names(&response.providers)
            ))];
            for warning in warnings(&response.diagnostics) {
                spans.push(Span::styled(" · ", theme.faint));
                spans.push(Span::styled(warning, theme.warning));
            }
            Line::from(spans)
        }
    }
}

/// The keys that work now, leaving out the least needed ones that do not fit
/// in `width` columns.
fn key_line(app: &App, width: usize, theme: &Theme) -> Line<'static> {
    // Each key's rank: the lowest is left out first.
    let keys: &[(&str, &str, u8)] = match app.mode {
        Mode::Editing if app.has_results_view() => &[
            ("enter", "search", 3),
            ("esc", "back", 1),
            ("ctrl-c", "quit", 2),
        ],
        Mode::Editing => &[("enter", "search", 2), ("esc", "quit", 1)],
        Mode::Browsing if matches!(app.shown, Shown::Nothing) => {
            &[("/", "search", 1), ("q", "quit", 2)]
        }
        Mode::Browsing if app.results().is_empty() => &[
            ("/", "search", 2),
            ("r", "search again", 1),
            ("q", "quit", 3),
        ],
        Mode::Browsing => &[
            ("↑↓", "select", 3),
            ("enter", "open", 6),
            ("y", "copy link", 2),
            ("n/p", "page", 4),
            ("/", "search", 5),
            ("r", "refresh", 1),
            ("q", "quit", 7),
        ],
    };
    let cost = |&(key, action, _): &(&str, &str, u8)| key.width() + action.width() + 3;
    let mut kept: Vec<_> = keys.to_vec();
    while kept.len() > 1 && kept.iter().map(cost).sum::<usize>() > width {
        if let Some(least) = (0..kept.len()).min_by_key(|&i| kept[i].2) {
            kept.remove(least);
        }
    }

    let mut spans = Vec::with_capacity(kept.len() * 2);
    for (key, action, _) in kept {
        spans.push(Span::styled(format!(" {key} "), theme.key));
        spans.push(Span::styled(format!("{action} "), theme.faint));
    }
    Line::from(spans)
}

fn names(providers: &[Provider]) -> String {
    providers
        .iter()
        .map(|provider| provider.name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `text` cut to `width` columns, ending in an ellipsis where it was cut.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let Some(room) = width.checked_sub(1) else {
        return String::new();
    };
    let mut fitted = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        used += grapheme.width();
        if used > room {
            break;
        }
        fitted.push_str(grapheme);
    }
    fitted.truncate(fitted.trim_end().len());
    fitted.push('…');
    fitted
}

/// `text` in lines of at most `width` columns, broken between words where
/// possible. Beyond `max` lines, the last one ends in an ellipsis.
fn wrap(text: &str, width: usize, max: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() <= width {
            line.push(' ');
            line.push_str(word);
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        // A word wider than the line is broken wherever it reaches the edge.
        for grapheme in word.graphemes(true) {
            if line.width() + grapheme.width() > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            line.push_str(grapheme);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }

    if lines.len() > max {
        lines.truncate(max);
        if let Some(last) = lines.last_mut() {
            *last = ellipsize(last, width);
        }
    }
    lines
}

/// `line` followed by an ellipsis in `width` columns, dropping its last word
/// to make room if need be.
fn ellipsize(line: &str, width: usize) -> String {
    if line.width() < width {
        format!("{line}…")
    } else if let Some(space) = line.rfind(' ') {
        format!("{}…", &line[..space])
    } else {
        fit(&format!("{line}…"), width)
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use url::Url;

    use super::{Theme, draw, fit, wrap};
    use crate::providers::searxng::SearXNGProvider;
    use crate::tui::app::App;
    use crate::{Provider, SearchParams, SearchResponse};

    const HEALTHY_BODY: &str = include_str!("../../tests/fixtures/healthy.json");

    fn fixture() -> SearchResponse {
        let provider = SearXNGProvider {
            base_url: Url::parse("http://localhost:8888/").expect("the url is valid"),
        };
        provider
            .parse_body("dns lookup", HEALTHY_BODY)
            .expect("the fixture parses")
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    /// An app whose search for "dns lookup" has gone `outcome`'s way.
    fn after(outcome: anyhow::Result<SearchResponse>) -> App {
        let mut app = searching();
        app.searched(outcome);
        app
    }

    fn searching() -> App {
        let mut app = App::new(vec![Provider::SearXNG]);
        app.search_for(SearchParams {
            query: "dns lookup".to_owned(),
            page: None,
        });
        app
    }

    fn drawn(app: &mut App, width: u16, height: u16, colored: bool) -> Terminal<TestBackend> {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("a test terminal opens");
        terminal
            .draw(|frame| draw(frame, app, &Theme::new(colored)))
            .expect("drawing on a test terminal works");
        terminal
    }

    #[test]
    fn the_start_screen() {
        let terminal = drawn(&mut App::new(vec![Provider::SearXNG]), 60, 8, true);
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn results_with_an_answer() {
        let mut app = after(Ok(fixture()));
        press(&mut app, KeyCode::Down);
        insta::assert_snapshot!(drawn(&mut app, 80, 24, true).backend());
    }

    #[test]
    fn a_narrow_terminal() {
        insta::assert_snapshot!(drawn(&mut after(Ok(fixture())), 40, 20, true).backend());
    }

    #[test]
    fn a_search_under_way() {
        insta::assert_snapshot!(drawn(&mut searching(), 60, 8, true).backend());
    }

    #[test]
    fn a_failed_search() {
        let mut app = after(Err(anyhow!("cannot search SearXNG: no answer within 10s")));
        insta::assert_snapshot!(drawn(&mut app, 60, 8, true).backend());
    }

    #[test]
    fn a_search_that_found_nothing() {
        let mut empty = fixture();
        empty.answer = None;
        empty.results.clear();
        empty.diagnostics = crate::Diagnostics::default();
        insta::assert_snapshot!(drawn(&mut after(Ok(empty)), 60, 8, true).backend());
    }

    #[test]
    fn editing_puts_the_cursor_after_the_query() {
        let mut app = after(Ok(fixture()));
        press(&mut app, KeyCode::Char('/'));
        let mut terminal = drawn(&mut app, 60, 8, true);
        // A border and a column of padding, then the ten of "dns lookup".
        terminal.backend_mut().assert_cursor_position((12, 1));
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn the_selected_result_is_marked_in_the_accent_color() {
        let terminal = drawn(&mut after(Ok(fixture())), 80, 24, true);
        let buffer = terminal.backend().buffer();
        let marks: Vec<_> = buffer
            .content()
            .iter()
            .filter(|cell| cell.symbol() == "▌")
            .collect();
        assert_ne!(marks.len(), 0);
        assert!(marks.iter().all(|cell| cell.fg == Color::LightMagenta));
    }

    #[test]
    fn without_color_nothing_is_colored() {
        let mut app = after(Ok(fixture()));
        press(&mut app, KeyCode::Char('p'));
        let terminal = drawn(&mut app, 80, 24, false);
        let buffer = terminal.backend().buffer();
        assert!(
            buffer
                .content()
                .iter()
                .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
        );
    }

    #[test]
    fn tiny_terminals_are_drawn_without_panicking() {
        for (width, height) in [(0, 0), (1, 1), (3, 2), (10, 4), (20, 6), (30, 10)] {
            drawn(&mut after(Ok(fixture())), width, height, true);
            drawn(&mut App::new(vec![Provider::SearXNG]), width, height, true);
        }
    }

    #[test]
    fn text_that_fits_is_left_alone() {
        assert_eq!(fit("Oslo", 4), "Oslo");
        assert_eq!(wrap("capital of norway", 17, 2), ["capital of norway"]);
    }

    #[test]
    fn cut_text_ends_in_an_ellipsis() {
        assert_eq!(fit("capital of norway", 9), "capital…");
        assert_eq!(fit("日本語の検索", 7), "日本語…");
        assert_eq!(fit("Oslo", 1), "…");
        assert_eq!(fit("Oslo", 0), "");
    }

    #[test]
    fn lines_break_between_words() {
        assert_eq!(
            wrap("the capital of norway is oslo", 14, 3),
            ["the capital of", "norway is oslo"]
        );
    }

    #[test]
    fn a_word_wider_than_the_line_is_broken() {
        assert_eq!(wrap("abcdefghij kl", 4, 4), ["abcd", "efgh", "ij", "kl"]);
    }

    #[test]
    fn text_beyond_the_last_line_is_cut_with_an_ellipsis() {
        assert_eq!(
            wrap("the capital of norway is oslo", 14, 1),
            ["the capital…"]
        );
        assert_eq!(wrap("abcdefghij", 4, 2), ["abcd", "efg…"]);
    }

    #[test]
    fn no_room_means_no_lines() {
        assert_eq!(wrap("oslo", 0, 2), Vec::<String>::new());
        assert_eq!(wrap("", 10, 2), Vec::<String>::new());
    }
}
