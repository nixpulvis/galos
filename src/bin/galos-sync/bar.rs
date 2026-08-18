//! A progress bar that keeps the bottom line, and a log drawn above it
//!
//! Both want the same terminal. A bar is one line rewritten in place, and
//! anything else printed to that line lands on top of it, so a sync that logs
//! while a bar is drawing leaves the two shuffled together and the bar
//! wherever it was last overwritten.
//!
//! So the log goes through the bar. `indicatif` redraws the bar under
//! whatever it is asked to print, which is what keeps the bar at the bottom
//! and every line of the log above it in the order it was written. What that
//! costs is a lock and a redraw a line, paid only while a bar is drawing.
//!
//! Where a bar is not drawing -- redirected output, or a command that has no
//! bar -- the log goes straight to stderr. It has to: a hidden draw target
//! swallows what it is asked to print, so routing through one would lose the
//! log exactly where there is nothing else to read.

use indicatif::ProgressBar;
use std::io::{self, stderr, IsTerminal, Write};
use std::sync::{Mutex, MutexGuard};
use tracing_subscriber::fmt::MakeWriter;

/// The bar the log is being drawn above, where one is drawing
static DRAWING: Mutex<Option<ProgressBar>> = Mutex::new(None);

/// What is drawing, however the thread before left it
///
/// Taken without asking whether a panic poisoned it. What is behind the lock
/// is one handle, no worse for a thread having died holding it, and this is
/// read from inside a `Drop` on the logging path: refusing to hand it over
/// would turn any panic anywhere into a second panic while unwinding.
fn drawing() -> MutexGuard<'static, Option<ProgressBar>> {
    DRAWING.lock().unwrap_or_else(|held| held.into_inner())
}

/// Draw the log above this bar until the guard is dropped
///
/// Answers nothing where the bar is hidden, since a hidden bar cannot print
/// and the log belongs on stderr instead.
pub fn under(bar: &ProgressBar) -> Drawing {
    if !bar.is_hidden() {
        *drawing() = Some(bar.clone());
    }

    Drawing
}

/// What holds the log above a bar, and puts it back on stderr when dropped
///
/// A guard rather than a pair of calls, so a bar that goes away while the
/// sync goes on cannot leave the log printing into a bar nobody is drawing.
#[must_use = "the log is held above the bar only until this is dropped"]
pub struct Drawing;

impl Drop for Drawing {
    fn drop(&mut self) {
        *drawing() = None;
    }
}

/// Where the log goes: above the bar, or to stderr where there is none
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
/// The bar prints a line at a time and the formatter writes an event in
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
        // Taken out of the lock before anything is printed. Printing is a
        // draw on a terminal and another thread's log line is waiting behind
        // it, and a `ProgressBar` is a handle rather than the bar itself, so
        // holding it costs nothing.
        let bar = drawing().clone();

        let said = String::from_utf8_lossy(&self.said);
        let said = said.trim_end_matches('\n');

        match bar {
            Some(bar) => bar.println(said),
            None => {
                let _ = writeln!(stderr(), "{}", said);
            }
        }
    }
}

/// Whether a bar drawn now would be seen
///
/// What a bar draws is one line rewritten in place, and a file of those is
/// not a log of anything, so redirected output gets no bar.
pub fn worth_drawing() -> bool {
    stderr().is_terminal()
}

/// A hidden bar takes nothing, which is what keeps a redirected log readable
///
/// The one way this arrangement can lose data. A draw target that is hidden
/// swallows what it is asked to print, so a bar that is not being drawn must
/// never be given the log: it would go nowhere, and nowhere is where a log
/// matters most. The other way round wants a terminal to be true, so it is
/// left to running the thing.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_bar_is_not_given_the_log() {
        let held = under(&ProgressBar::hidden());

        assert!(drawing().is_none());

        drop(held);
    }
}
