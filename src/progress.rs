//! Progress of a long job, told the way a package manager tells it: a line for
//! each step and each change, scrolling above one bar for the job as a whole.
//! Where a line cannot be redrawn, such as in a log file, only the lines are
//! written.
//!
//! Writing is best effort throughout: a closed stderr is no reason to abandon
//! an installation.

use std::fmt::Write as _;
use std::io::Write;
use std::time::{Duration, Instant};

use anstyle::{AnsiColor, Color, Style};
use unicode_width::UnicodeWidthStr;

use crate::output::fit;

/// How often [`Progress::tick`] redraws the bar.
pub const TICK: Duration = Duration::from_millis(100);

const WHOLE: u64 = 1000;
const MIN_BAR: u64 = 10;
const MAX_BAR: u64 = 20;
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];
const MIB: u64 = 1024 * 1024;

/// Width assumed for a terminal that will not say.
const FALLBACK_WIDTH: usize = 80;

const HEADING: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Blue)));
const STEP: Style = Style::new().bold();
const NOTE: Style = Style::new().dimmed();
const WARNING: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const ERROR: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Red)));

/// Reports a job to `out`, which belongs on stderr so that piped search output
/// stays clean.
#[derive(Debug)]
pub struct Progress<W: Write> {
    out: W,
    /// How wide the terminal is, where there is a bar to fit into it.
    width: Option<fn() -> usize>,
    weights: Vec<u64>,
    begun: usize,
    title: String,
    /// How much of the current step is done, in thousandths.
    share: u64,
    detail: String,
    since: Instant,
    /// When the bar was last drawn, while it is on screen.
    drawn: Option<Instant>,
}

impl<W: Write> Progress<W> {
    /// Reports to a terminal, with a bar, if `terminal`; else only in lines.
    pub fn new(out: W, terminal: bool) -> Self {
        if terminal {
            Self::with_width(out, terminal_width)
        } else {
            Self::plain(out)
        }
    }

    /// Reports in lines alone.
    pub fn plain(out: W) -> Self {
        Self::build(out, None)
    }

    fn with_width(out: W, width: fn() -> usize) -> Self {
        Self::build(out, Some(width))
    }

    fn build(out: W, width: Option<fn() -> usize>) -> Self {
        Self {
            out,
            width,
            weights: Vec::new(),
            begun: 0,
            title: String::new(),
            share: 0,
            detail: String::new(),
            since: Instant::now(),
            drawn: None,
        }
    }

    /// A line of text, as it is.
    pub fn line(&mut self, text: &str) {
        self.above_bar(text);
    }

    /// Announces a job.
    pub fn heading(&mut self, text: &str) {
        self.above_bar(&format!("\n{HEADING}::{HEADING:#} {STEP}{text}{STEP:#}"));
    }

    /// Starts a job of steps taking these shares of the bar.
    pub fn plan(&mut self, weights: &[u64]) {
        self.clear();
        self.weights = weights.to_vec();
        self.begun = 0;
    }

    /// Finishes the current step and starts the next.
    pub fn step(&mut self, title: &str) {
        self.begun += 1;
        title.clone_into(&mut self.title);
        self.share = 0;
        self.detail.clear();
        self.since = Instant::now();

        let line = format!("{STEP}{}{title}{STEP:#}", self.counter());
        self.above_bar(&line);
    }

    /// Reports something done within the current step, such as a file added.
    pub fn note(&mut self, text: &str) {
        self.above_bar(&format!("      {NOTE}{text}{NOTE:#}"));
    }

    /// Reports something that went wrong without stopping the job.
    pub fn warn(&mut self, text: &str) {
        self.above_bar(&format!("{WARNING}warning:{WARNING:#} {text}"));
    }

    /// Reports what stopped the job.
    pub fn error(&mut self, text: &str) {
        self.above_bar(&format!("{ERROR}error:{ERROR:#} {text}"));
    }

    /// Records `done` out of `total` of the current step.
    pub fn share(&mut self, done: u64, total: u64) {
        if let Some(share) = (done.min(total) * WHOLE).checked_div(total) {
            self.share = share;
        }
        self.tick();
    }

    /// Records `done` bytes of a download of `total`, if the size is known.
    pub fn bytes(&mut self, done: u64, total: Option<u64>) {
        let speed = speed(done, self.since.elapsed());
        self.detail = match total {
            Some(total) => format!("{}/{}, {speed}", amount(done), size(total)),
            None => format!("{}, {speed}", size(done)),
        };
        if let Some(total) = total {
            self.share(done, total);
        } else {
            self.tick();
        }
    }

    /// Redraws the bar, if it is due, so that the time shown keeps moving.
    pub fn tick(&mut self) {
        if self.drawn.is_none_or(|drawn| drawn.elapsed() >= TICK) {
            self.draw();
        }
    }

    /// Ends the job, taking the bar off the screen.
    pub fn finish(&mut self) {
        self.clear();
        self.weights.clear();
        self.begun = 0;
    }

    fn counter(&self) -> String {
        if self.weights.len() > 1 {
            format!("({}/{}) ", self.begun, self.weights.len())
        } else {
            String::new()
        }
    }

    /// Thousandths of the whole job done.
    fn done(&self) -> u64 {
        let total: u64 = self.weights.iter().sum();
        if total == 0 {
            return 0;
        }
        let finished: u64 = self.weights.iter().take(self.begun.saturating_sub(1)).sum();
        let current = self
            .begun
            .checked_sub(1)
            .and_then(|index| self.weights.get(index))
            .copied()
            .unwrap_or(0);
        (finished * WHOLE + current * self.share) / total
    }

    fn draw(&mut self) {
        let Some(width) = self.width else { return };
        if self.begun == 0 {
            return;
        }
        let width = width();
        let elapsed = self.since.elapsed();
        let frame = usize::try_from(elapsed.as_millis() / TICK.as_millis()).unwrap_or(0);
        let spinner = SPINNER[frame % SPINNER.len()];

        // The title goes last, as the part to lose in a narrow terminal.
        let mut text = format!("{} {spinner} {}", clock(elapsed), self.title);
        if !self.detail.is_empty() {
            let _ = write!(text, ", {}", self.detail);
        }

        let line = bar_line(self.done(), &text, width);
        let padding = width.saturating_sub(1 + line.width());
        let _ = write!(self.out, "\r{line}{:padding$}", "");
        let _ = self.out.flush();
        self.drawn = Some(Instant::now());
    }

    fn clear(&mut self) {
        if self.drawn.take().is_some()
            && let Some(width) = self.width
        {
            let blank = width().saturating_sub(1);
            let _ = write!(self.out, "\r{:blank$}\r", "");
            let _ = self.out.flush();
        }
    }

    fn above_bar(&mut self, line: &str) {
        self.clear();
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
        self.draw();
    }
}

impl Progress<Vec<u8>> {
    /// What has been reported so far, for showing or testing.
    #[must_use]
    pub fn written(&self) -> String {
        anstream::adapter::strip_str(&String::from_utf8_lossy(&self.out)).to_string()
    }
}

/// Runs when an error ends a job early, so that the message reporting it does
/// not start in the middle of the bar.
impl<W: Write> Drop for Progress<W> {
    fn drop(&mut self) {
        self.clear();
    }
}

/// One line for the bar: `done` in thousandths, then what is happening.
fn bar_line(done: u64, text: &str, width: usize) -> String {
    let done = done.min(WHOLE);
    let slots = u64::try_from(width / 5)
        .unwrap_or(MAX_BAR)
        .clamp(MIN_BAR, MAX_BAR);
    let filled = usize::try_from(slots * done / WHOLE).unwrap_or(0);
    let empty = usize::try_from(slots).unwrap_or(0).saturating_sub(filled);
    let line = format!(
        "[{:>3}%] [{}{}] {text}",
        done / 10,
        "#".repeat(filled),
        "-".repeat(empty)
    );
    fit(&line, width.saturating_sub(1))
}

fn terminal_width() -> usize {
    crossterm::terminal::size().map_or(FALLBACK_WIDTH, |(columns, _)| columns.into())
}

/// `bytes` for reading, such as `12.3 MiB`.
#[must_use]
pub fn size(bytes: u64) -> String {
    if bytes < MIB {
        return format!("{} KiB", bytes / 1024);
    }
    let tenths = bytes * 10 / MIB;
    format!("{}.{} MiB", tenths / 10, tenths % 10)
}

/// `bytes` as [`size`] gives it, without the unit, to read `12.3/45.6 MiB`.
fn amount(bytes: u64) -> String {
    let tenths = bytes * 10 / MIB;
    format!("{}.{}", tenths / 10, tenths % 10)
}

fn speed(bytes: u64, elapsed: Duration) -> String {
    let millis = elapsed.as_millis().max(1);
    let per_second = u128::from(bytes) * 1000 / millis;
    format!("{}/s", size(u64::try_from(per_second).unwrap_or(u64::MAX)))
}

fn clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use unicode_width::UnicodeWidthStr;

    use super::{Progress, bar_line, clock, size, speed};

    fn forty() -> usize {
        40
    }

    #[test]
    fn plain_progress_is_lines_alone() {
        let mut progress = Progress::plain(Vec::new());
        progress.plan(&[1, 1]);
        progress.step("Downloading");
        progress.share(1, 2);
        progress.note("added a file");
        progress.step("Installing");
        progress.finish();

        assert_eq!(
            progress.written(),
            "(1/2) Downloading\n      added a file\n(2/2) Installing\n"
        );
    }

    #[test]
    fn a_job_of_one_step_is_not_counted() {
        let mut progress = Progress::plain(Vec::new());
        progress.plan(&[1]);
        progress.step("Starting SearXNG");
        assert_eq!(progress.written(), "Starting SearXNG\n");
    }

    #[test]
    fn the_bar_shows_the_whole_job_not_the_step() {
        let mut progress = Progress::plain(Vec::new());
        progress.plan(&[1, 3]);
        progress.step("first");
        assert_eq!(progress.done(), 0);
        progress.share(1, 2);
        assert_eq!(progress.done(), 125);
        progress.step("second");
        assert_eq!(progress.done(), 250);
        progress.share(1, 1);
        assert_eq!(progress.done(), 1000);
    }

    #[test]
    fn a_line_lands_above_the_bar_and_the_bar_comes_back() {
        let mut progress = Progress::with_width(Vec::new(), forty);
        progress.plan(&[1]);
        progress.step("Working");
        progress.note("added a file");
        let written = progress.written();

        let note = written.find("added a file").expect("the note is written");
        let bars: Vec<_> = written.match_indices("[  0%]").collect();
        assert_eq!(bars.len(), 2, "drawn after each line: {written:?}");
        assert!(bars[0].0 < note && note < bars[1].0, "{written:?}");
    }

    #[test]
    fn finishing_clears_the_bar() {
        let mut progress = Progress::with_width(Vec::new(), forty);
        progress.plan(&[1]);
        progress.step("Working");
        progress.finish();

        let written = progress.written();
        assert!(written.ends_with('\r'), "{written:?}");
        assert!(progress.drawn.is_none());
    }

    #[test]
    fn nothing_is_drawn_before_the_first_step() {
        let mut progress = Progress::with_width(Vec::new(), forty);
        progress.line("Welcome");
        assert_eq!(progress.written(), "Welcome\n");
    }

    #[test]
    fn the_bar_never_outgrows_the_terminal() {
        for width in [1, 10, 25, 40, 80, 200] {
            let line = bar_line(500, &"a long title ".repeat(30), width);
            assert!(
                line.width() < width.max(2),
                "{} columns in a terminal of {width}",
                line.width()
            );
        }
    }

    #[test]
    fn the_bar_fills_with_the_job() {
        assert_eq!(bar_line(0, "x", 30), "[  0%] [----------] x");
        assert_eq!(bar_line(500, "x", 30), "[ 50%] [#####-----] x");
        assert_eq!(bar_line(1000, "x", 30), "[100%] [##########] x");
    }

    #[test]
    fn a_step_is_never_done_past_its_end() {
        let mut progress = Progress::plain(Vec::new());
        progress.plan(&[1]);
        progress.step("only");
        progress.share(5, 2);
        assert_eq!(progress.done(), 1000);
    }

    #[test]
    fn a_download_of_unknown_size_still_reports_what_arrived() {
        let mut progress = Progress::plain(Vec::new());
        progress.plan(&[1]);
        progress.step("Downloading");
        progress.bytes(3 * 1024 * 1024, None);
        assert_eq!(progress.done(), 0);
        assert!(
            progress.detail.starts_with("3.0 MiB, "),
            "{}",
            progress.detail
        );
        progress.bytes(3 * 1024 * 1024, Some(6 * 1024 * 1024));
        assert_eq!(progress.done(), 500);
        assert!(progress.detail.starts_with("3.0/6.0 MiB, "));
    }

    #[test]
    fn sizes_read_like_a_package_manager_s() {
        assert_eq!(size(512), "0 KiB");
        assert_eq!(size(100 * 1024), "100 KiB");
        assert_eq!(size(12_900_000), "12.3 MiB");
    }

    #[test]
    fn speed_survives_no_time_having_passed() {
        assert_eq!(speed(0, Duration::ZERO), "0 KiB/s");
        assert_eq!(speed(2 * 1024 * 1024, Duration::from_secs(1)), "2.0 MiB/s");
    }

    #[test]
    fn the_clock_counts_minutes_and_seconds() {
        assert_eq!(clock(Duration::from_secs(5)), "0:05");
        assert_eq!(clock(Duration::from_secs(75)), "1:15");
    }
}
