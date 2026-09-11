//! The bars a run is drawing, and a log drawn above all of them
//!
//! Both want the same terminal. A bar is one line rewritten in place, and
//! anything else printed to that line lands on top of it, so a sync that logs
//! while a bar is drawing leaves the two shuffled together and the bar
//! wherever it was last overwritten.
//!
//! So the log goes through the bars. `indicatif` redraws them under whatever
//! it is asked to print, which is what keeps them at the bottom and every
//! line of the log above them in the order it was written. What that costs is
//! a lock and a redraw a line, paid only where a terminal is there to draw
//! on.
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
//! Where nothing would be seen -- redirected output, or a run with no bar at
//! all -- the log goes straight to stderr. It has to: a hidden draw target
//! swallows what it is asked to print, so routing through one would lose the
//! log exactly where there is nothing else to read.

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

/// Where a line of the log goes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Above {
    /// Through the bars, which redraw themselves underneath it.
    Bars,
    /// Straight out, because nothing is drawing and nothing would be.
    Stderr,
}

/// Whether a line printed through the bars would be seen.
///
/// The one way this arrangement can lose data. A draw target that is hidden
/// swallows what it is asked to print, so the log must never be routed
/// through one: it would go nowhere, and nowhere is where a log matters
/// most.
fn above() -> Above {
    match worth_drawing() {
        true => Above::Bars,
        false => Above::Stderr,
    }
}

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

        match above() {
            Above::Bars => {
                let _ = BARS.println(said);
            }
            Above::Stderr => {
                let _ = writeln!(stderr(), "{}", said);
            }
        }
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

    /// A hidden draw target is never given the log
    ///
    /// A test process has no terminal, so this is the redirected case: the
    /// bars are hidden, printing through them would swallow the line, and
    /// the log has to go to stderr instead. The other way round wants a
    /// terminal to be true, so it is left to running the thing.
    #[test]
    fn the_log_is_not_printed_through_a_hidden_target() {
        assert_eq!(above(), Above::Stderr);
        assert!(BARS.is_hidden(), "the bars would swallow the log");
    }
}
