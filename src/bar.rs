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
//!
//! ## One line for every bulk import
//!
//! [`imported`] builds that line and [`Import`] keeps its tally: systems
//! taken in, split by whether the store already had them, plus the records
//! nothing could parse. A source passes a tag, an [`Extent`] and what each
//! reading did; it never formats the message itself. Four sources writing
//! their own gave three different shapes of line for one job.

use crate::sink::Landed;
use indicatif::{
    HumanCount, MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle,
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

/// How much of a source there is to get through
///
/// The one part of an import's line that differs by source: a file read a
/// line at a time knows its size but not its record count, and a file
/// parsed whole before it is walked knows the opposite.
#[derive(Clone, Copy)]
pub enum Extent {
    /// Bytes of a file read a line at a time.
    ///
    /// The size is known at open and bytes only go up, where a count
    /// estimated from the mean record so far walks backwards whenever a
    /// denser stretch arrives.
    Bytes(u64),
    /// Records, for a source counted before it is walked.
    Records(u64),
}

/// The template up to the position field
const BEFORE: &str = "[{elapsed_precise}/{eta_precise}] {bar:40} ";

/// And after it
const AFTER: &str = " ({percent}%) {msg}";

/// The line one bulk import draws
///
/// Every bulk source draws this and nothing else, so a run reading
/// several — `--from eddb=… --from edsm=…` — shows lines that differ only
/// in their numbers and in `tag`, which is what the source calls itself,
/// shard included.
///
/// Added to the run's bars rather than owning the terminal, so each source
/// keeps its own line and the log stays above them.
///
/// EDDN gets none of this: a feed never ends, so there is no total to draw
/// against. It reports itself in the log.
pub fn imported(tag: &str, of: Extent) -> Import {
    let (extent, position) = match of {
        Extent::Bytes(size) => (size, "{bytes}/{total_bytes}"),
        Extent::Records(records) => (records, "{human_pos}/{human_len}"),
    };
    let bar = ProgressBar::new(extent);
    bar.set_style(
        ProgressStyle::default_bar()
            .template(&[BEFORE, position, AFTER].concat())
            .unwrap()
            .progress_chars("##-"),
    );

    let import = Import {
        bar: BARS.add(bar),
        tag: tag.to_owned(),
        systems: 0,
        new: 0,
        updated: 0,
        skipped: 0,
    };
    // Drawn before the first record, so a source that has opened a file
    // and read nothing yet still names itself.
    import.draw();
    import
}

/// One bulk import's line and its tally
///
/// A source reports what each reading did and this renders the message.
/// Sources do not format it themselves; there was one shape per source
/// before.
pub struct Import {
    bar: ProgressBar,
    /// What the source calls itself, shard included.
    tag: String,
    /// Systems the source has stated, however they landed.
    systems: u64,
    /// Of those, how many the store did not have and how many it did.
    ///
    /// These need not sum to `systems`: a stale reading is in neither, and
    /// so is one handed to a sink that only passes it along.
    new: u64,
    updated: u64,
    /// Records nothing could parse, so systems missed.
    skipped: u64,
}

impl Import {
    /// One system the source stated, and what the store did with it
    ///
    /// [`None`] means nothing was stored: a report no row could be made
    /// from, a refused write, or a sink that only passes readings along.
    /// It is still a system the run took in.
    pub fn took(&mut self, landed: Option<Landed>) {
        self.systems += 1;
        match landed {
            Some(Landed::New) => self.new += 1,
            Some(Landed::Updated) => self.updated += 1,
            Some(Landed::Stale) | None => {}
        }
        self.draw();
    }

    /// One record nothing could parse.
    pub fn missed(&mut self) {
        self.skipped += 1;
        self.draw();
    }

    /// How much of the source that record was
    ///
    /// Its bytes for a file, one for a record count. This is what the
    /// source had to read to get past it, which is more than what it took
    /// in: lines another shard owns and lines nothing could parse are read
    /// too.
    pub fn through(&self, amount: u64) {
        self.bar.inc(amount);
    }

    /// The source was read to the end.
    ///
    /// Landed on its total rather than wherever the last record left it: a
    /// byte extent is advanced by records, and a file's framing — a JSON
    /// array's brackets — is bytes no record accounts for.
    pub fn done(&self) {
        self.bar.set_position(self.bar.length().unwrap_or(0));
        self.bar.finish();
    }

    /// The read stopped part way, and why
    ///
    /// The tally stays on the line: what the run took in before it stopped
    /// is what the store holds.
    pub fn abandoned(&self, why: &str) {
        self.bar.abandon_with_message(format!("{} ({why})", self.message()));
    }

    /// Show the tally
    ///
    /// One `format!` per record, which is what naming each system cost
    /// before. indicatif decides how often the line is actually drawn.
    fn draw(&self) {
        self.bar.set_message(self.message());
    }

    /// The tag, then the counts.
    fn message(&self) -> String {
        format!(
            "[{}] {} systems, {} new, {} updated, {} skipped",
            self.tag,
            HumanCount(self.systems),
            HumanCount(self.new),
            HumanCount(self.updated),
            HumanCount(self.skipped),
        )
    }
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
