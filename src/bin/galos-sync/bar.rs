//! The bars a run is drawing, and a log drawn above all of them
//!
//! Both want the same terminal. A bar is one line rewritten in place, and
//! anything else printed to that line lands on top of it, so a sync that logs
//! while a bar is drawing leaves the two shuffled together and the bar
//! wherever it was last overwritten.
//!
//! So the log is written between draws. [`MultiProgress::suspend`] clears
//! whatever is on the screen, hands the terminal over for one line, and
//! redraws underneath it, which is what keeps the bars at the bottom and
//! every line of the log above them in the order it was written. What that
//! costs is a lock and a redraw a line, paid only where a bar is drawing.
//!
//! The line itself goes to stderr as a line — `writeln!`, one `\n`, nothing
//! else. That is worth saying because the obvious arrangement,
//! [`MultiProgress::println`], is not it: it prints *as a bar prints*, which
//! means padded to the width of the terminal and positioned rather than
//! newline-terminated. On a screen with no bar on it that turns every log
//! line into a row of trailing spaces with the next line run on after it,
//! which is what a redirected copy of the output shows and what a reader
//! sees the moment the terminal is narrower than a line.
//!
//! ## Why it is a `MultiProgress` and not a bar
//!
//! Because a run draws more than one now. `--from eddb=… --from edsm=…` is
//! two dumps read at once, each with a bar of its own, and the arrangement
//! this replaced held exactly one: a single-slot static that the second
//! drawer overwrote and whose `Drop` cleared it while the first was still
//! drawing, which left that bar's lines landing on top of the log for the
//! rest of the run. A [`MultiProgress`] owns the terminal for all of them and
//! there is nothing to hand back.
//!
//! Redirected output and a run with no bar at all need nothing special:
//! suspending a hidden or empty set of bars is the write on its own.

use indicatif::{
    MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle,
};
use std::io::{self, stderr, IsTerminal, Write};
use std::sync::LazyLock;
use tracing_subscriber::fmt::MakeWriter;

/// Every bar this run is drawing, and what the log is printed through.
///
/// One per process, built the first time anything asks for a bar or writes a
/// line. Hidden where there is no terminal, which is what leaves the log on
/// stderr: see [`above`].
static BARS: LazyLock<MultiProgress> = LazyLock::new(|| {
    let bars = MultiProgress::new();
    if !worth_drawing() {
        bars.set_draw_target(ProgressDrawTarget::hidden());
    }
    bars
});

/// Where the log goes: above the bars, or to stderr where none are drawn
#[derive(Clone, Copy)]
pub struct Log;

impl<'a> MakeWriter<'a> for Log {
    type Writer = Line;

    fn make_writer(&'a self) -> Line {
        Line { said: Vec::new() }
    }
}

/// One event of the log, held until it is whole
///
/// The bars print a line at a time and the formatter writes an event in
/// several pieces, so the pieces are gathered here and printed when the
/// writer is dropped, which is the end of the event.
pub struct Line {
    said: Vec<u8>,
}

impl Write for Line {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.said.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        let said = String::from_utf8_lossy(&self.said);
        let said = said.trim_end_matches('\n');

        // A line, written as a line. `suspend` is what the bars are for:
        // they come off the screen, this goes out, and they draw again
        // under it. With no bar drawing it is the write on its own.
        BARS.suspend(|| {
            let _ = writeln!(stderr(), "{}", said);
        });
    }
}

/// How far along one feed is, where there is someone to show
///
/// Added to the run's bars rather than owning the terminal itself, so two
/// sources reading at once each keep a line of their own and the log keeps
/// the ones above them.
pub fn progress(steps: u64) -> ProgressBar {
    let bar = ProgressBar::new(steps);
    bar.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}/{eta_precise}] {bar:40} {pos:>7}/{len:7} ({percent}%) {msg}")
        .unwrap()
        .progress_chars("##-"));

    BARS.add(bar)
}

/// Whether a bar drawn now would be seen
///
/// What a bar draws is one line rewritten in place, and a file of those is
/// not a log of anything, so redirected output gets no bar.
fn worth_drawing() -> bool {
    stderr().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line is written whether anything is drawing or not
    ///
    /// The way this goes wrong is silence: the arrangement before this one
    /// printed *through* the bars, and a hidden draw target swallows what
    /// it is asked to print, so a redirected run lost its log entirely
    /// unless the writer knew to go around. Suspending does not have that
    /// failure — the closure runs either way — and a test process has no
    /// terminal, so this is the hidden case.
    #[test]
    fn a_hidden_set_of_bars_still_hands_the_line_over() {
        assert!(BARS.is_hidden(), "a test process should have no terminal");

        let mut wrote = 0;
        BARS.suspend(|| wrote += 1);
        assert_eq!(wrote, 1, "the line would have gone nowhere");
    }
}
