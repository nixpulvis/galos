//! Building a directory from a dump with no tree in memory.
//!
//! One import does not go through the sinks at all. A run that names a
//! finite source and writes only a directory has nothing to fan out to,
//! and the index sink's price for holding one reading open — a live tree
//! and the whole names table, a kilobyte a system — is what stops a two
//! hundred million system dump from being importable at all. Such a run
//! comes here instead, which cuts the galaxy into regions and builds them
//! one at a time, holding one region rather than the sky.
//!
//! That is `galos ingest --from spansh=PATH --index DIR` and nothing
//! names it: no flag and no verb of its own, because the shape of the run
//! already says it — a source with an end, one sink, and no reader
//! waiting on a directory being published as it is written. What the run
//! does say is which route it took, when it starts, since 200 GB of
//! resident memory is not a thing to discover afterwards.
//!
//! [`region_budget`] is the memory dial of that route, and what it bounds
//! is how many systems are held at once: a region's systems are read back
//! off its spill, built, written and dropped. A region can be dropped
//! because a cut is disjoint — every system in it is owned by a cell
//! inside it — so once it is built there is nothing left to ask it.

use crate::read::spansh;
use galos_index::build::cold::{region_budget, Build, Built, Ending, Start};
use galos_index::format::checkpoint::By;
use galos_index::store::sidecars::Rows;
use galos_index::BuildParams;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{info, warn};

/// Build `dir` from a finite source and leave a resume point beside it.
///
/// The regional build: every system is spilled to its bucket's file as the
/// source pushes it, the buckets are formed into regions of
/// [`region_budget`] systems, and the regions are built one at a time. What
/// is held at once is one region's systems and, at the end, every cell in
/// the galaxy — never a live tree over the sky, which is the whole reason
/// this path is not the sink one.
///
/// The resume point says [`By::Events`] and carries no cursor. A cursor is a
/// database clock — what a delta pass reads `received_at` against — and a
/// file published last Tuesday has none to offer; a dump's own newest
/// `updateTime` is a time out in the galaxy and not a time this program's
/// database wrote a row, so recording it as one would have the next
/// catch-up read back from a clock nothing here ever kept. `By::Events`
/// says the directory was derived from records rather than from the
/// database, which is what makes `galos_db::index` rebuild rather than
/// resume from it.
///
/// Ctrl-C during the read is a clean exit that keeps its place: the builder
/// publishes nothing — no cells, no names table, no resume point — but the
/// spills of every system it has read stay where they are, and the next run
/// over the same file takes them up and reads on from the line it stopped
/// at. The body files it wrote as it went stand too, each whole and each
/// filed under its own address. A run over a different dump, or over one
/// that has changed length, reads from the start and says so.
///
/// The metadata tables go in after [`Build::finish`] has answered, which
/// is after the index file. A stop then leaves no table at all: up to the
/// point of no return the directory is as the build found it, and past it
/// the directory serves nothing until a build finishes, so a table written
/// early would either break the first or stand for a galaxy nothing
/// published. And the resume point `finish` leaves is what makes the next
/// run resume rather than build, so tables short of the cells beside them
/// would be a directory nothing ever repairs. `galos_db::index` writes its
/// own parts at the same place, for the same reason.
pub fn cold(
    source: &mut spansh::Galaxy,
    dir: &Path,
    checkpoint: &Path,
) -> Result<bool, String> {
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    let start = Instant::now();
    let budget = region_budget();
    let failed = |err| format!("the index could not be built: {err}");

    // What the directory already stands for, where it stands for part of
    // this same file. The clock comes back with it: a build ages every
    // system against one moment, and a run carrying on with its own would
    // bin half the galaxy's Recency against another.
    let (taking_up, place) = match galos_index::build::cold::left_off(
        checkpoint,
    ) {
        Some(left) => match spansh::Place::of(&left, &source.path) {
            Some(place) => {
                info!(
                    systems = left.systems(),
                    at = place.at(),
                    dir = %dir.display(),
                    "carrying on with the read this directory was published \
                     from",
                );
                source.now = place.now;
                (Start::Resuming(left), Some(place))
            }
            None => (Start::Fresh, None),
        },
        None => (Start::Fresh, None),
    };
    let carrying_on = place.is_some();

    let stop = || source.shutdown.asked();
    let mut build = Build::begin(
        dir,
        checkpoint,
        BuildParams::default(),
        budget,
        taking_up,
        &stop,
    )
    .map_err(failed)?;
    // Beside the build's own scratch rather than in it: `Build::finish`
    // clears that when it publishes, and these have to stand until the
    // tables have been written off them. A run carrying on starts from the
    // tables the directory publishes, those rows being the only copy of
    // what the read before it derived.
    let spill = rows_dir(checkpoint);
    let mut rows = match carrying_on {
        true => Rows::onto(&spill, dir),
        false => Rows::writing(&spill),
    }
    .map_err(failed)?;
    source.read(&mut build, &mut rows, place).map_err(failed)?;

    // A read cut short publishes what it read: a dump is read in file
    // order, so what has been read is a galaxy in itself, and a map can
    // open it while the rest of the file is still to come. The mark the
    // publish leaves is what the next run carries on from.
    let report = match build
        .finish(By::Events, None, Ending::Publish)
        .map_err(failed)?
    {
        Built::Index(report) => report,
        Built::Stopped(abandoned) => {
            info!(
                %abandoned,
                elapsed = ?start.elapsed(),
                dir = %dir.display(),
                "asked to stop before anything was read",
            );
            return Ok(true);
        }
    };

    // Every table the dump can fill, read back off the rows the read
    // spilled and written in address order. Not the factions: the dump's
    // own faction lists are passed over by `spansh::System`, and
    // nothing reading records could number a faction anyway — an empty
    // table would say the galaxy has none where an absent one says this
    // index cannot tell.
    let counts = rows.finish(dir).map_err(failed)?;

    // One line, in the shape the sink's own publish prints, so a directory
    // written by either builder says what happened the same way.
    info!(
        wrote = "whole",
        moved = report.systems,
        systems = report.systems,
        cells = report.cells,
        leaves = report.leaves,
        deepest = report.deepest_level,
        regions = report.regions,
        budget,
        named = report.named,
        rows = report.named_rows,
        populated = counts.populated,
        reaches = counts.reaches,
        boosts = counts.boosts,
        elapsed = ?start.elapsed(),
        dir = %dir.display(),
        "index published",
    );
    if !report.is_consistent() {
        warn!(
            systems = report.systems,
            placed = report.points,
            "the cut did not partition the galaxy",
        );
    }
    Ok(true)
}

/// Where a cold build's table rows are spilled: beside the resume point,
/// as the build's own scratch is.
///
/// A run starting over opens them at zero bytes, which is the clearing,
/// and [`Rows::finish`] removes them once the tables have been written.
pub fn rows_dir(checkpoint: &Path) -> PathBuf {
    let mut name = checkpoint.as_os_str().to_owned();
    name.push(".rows");
    PathBuf::from(name)
}
