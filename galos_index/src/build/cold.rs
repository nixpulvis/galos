//! A cold build: the whole galaxy raised from records, a region at a time.
//!
//! [`crate::build::region`] is the pieces — the cut, what a region offers, the
//! crown over it, a region's own snapshot, the join. This is the sequence
//! they go in. A caller pushes every record it has into [`Build`] and the
//! cut is formed afterwards, from the [`bucket`](crate::build::bucket)s the
//! systems landed in, so the galaxy is read once.
//!
//! Nothing the galaxy's size scales is held. Each system goes straight to
//! its bucket's spill file and each name into a names chunk; each region is
//! then built off the mapping of its spill and its payloads written, so what
//! is held at the end is every cell in the galaxy, which is the index file.
//!
//! No live [`Tree`](crate::Tree) is raised: a watch gets one by resuming
//! from the resume point this leaves.

use crate::build::bucket;
use crate::build::bucket::Buckets;
use crate::build::bucket::Formed;
use crate::build::region;
use crate::build::region::{Crown, Offer};
use crate::build::snapshot::{BuildParams, Snapshot};
use crate::core::record::System;
use crate::format::checkpoint::{By, Checkpoint, Compaction};
use crate::format::layout::INDEX_FILE;
use crate::format::layout::mark_path;
use crate::format::layout::spill_dir;
use crate::format::msgpack::{read_meta, write_meta};
use crate::format::spill::Spilled;
use crate::read::index::Index;
use crate::records::NameEntry;
use crate::store::bodies::Reclaimed;
use crate::store::cells::Swept;
use crate::store::names;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// How many systems one region of a cold build may hold.
///
/// A region is built alone at about 298 B a system, so seven million is the
/// two-gigabyte budget the builder is written against.
const REGION_BUDGET: u64 = 7_000_000;

/// What the environment may say instead, and the memory dial of a cold
/// build: how many systems are held at once. A smaller budget is more
/// regions, each read back off its own spill, built, written and then
/// finished with — a region being disjoint, nothing outside it has anything
/// left to ask it.
///
/// Also how the regional build is exercised on a galaxy that fits in one
/// region.
const BUDGET_VAR: &str = "GALOS_REGION_BUDGET";

/// How many systems a region may hold: [`BUDGET_VAR`] where it is set and
/// readable, [`REGION_BUDGET`] otherwise.
///
/// What every caller of [`Build::begin`] passes as its `budget`, so one
/// setting means one thing whichever half of the program is building the
/// directory.
pub fn region_budget() -> u64 {
    match std::env::var(BUDGET_VAR).ok().and_then(|it| it.parse().ok()) {
        Some(budget) => budget,
        None => REGION_BUDGET,
    }
}

/// How far into its own source a published directory stands.
///
/// A dump is hours of reading and a directory nobody can open until the
/// last line is a directory nobody can open. So a read that is stopped
/// publishes what it has — a smaller galaxy, whole in itself — and writes
/// down where it had got to. The next run takes the systems back out of the
/// resume point that publish left, reads on from here, and publishes again.
///
/// The cursor is the caller's own and is carried rather than read: a dump's
/// place is a byte offset and a database's is nothing at all.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Mark {
    /// The caller's place in its own source, opaque here.
    cursor: Vec<u8>,
    /// Systems the directory published when the mark was written.
    systems: u64,
}

/// What a published directory says about the read behind it.
///
/// Answered by [`left_off`] and handed back to [`Build::begin`] as
/// [`Start::Resuming`].
#[derive(Clone, Debug)]
pub struct LeftOff(Mark);

impl LeftOff {
    /// A resume that says nothing about where the read behind it got to.
    ///
    /// What a directory with **no mark** resumes as, and every directory
    /// a database built is one: a database read has no place in its own
    /// source to write down, so [`left_off`] answers [`None`] for it and
    /// [`Start::Resuming`] could not be spelled at all.
    ///
    /// [`Start::Fresh`] is not the substitute it looks like. It removes
    /// the mark and opens the names table with `names::Writer::writing`,
    /// which starts a table from nothing — so a caller that already holds
    /// the systems and wants the names carried forward has to resume, and
    /// this is how. [`Build::finish`] writes no mark unless
    /// [`Build::mark`] was called, so resuming this way leaves whatever
    /// mark stands exactly as it was found.
    pub fn nowhere() -> LeftOff {
        LeftOff(Mark::default())
    }

    /// Where the caller had read to, in the caller's own terms.
    pub fn cursor(&self) -> &[u8] {
        &self.0.cursor
    }

    /// Systems the directory holds, which a resumed read takes back out of
    /// the resume point rather than reading again.
    pub fn systems(&self) -> u64 {
        self.0.systems
    }
}

/// Where a build starts.
#[derive(Clone, Debug)]
pub enum Start {
    /// From nothing: whatever a previous build left is cleared first, the
    /// directory it publishes standing for what this read reaches and
    /// nothing else.
    Fresh,
    /// From a directory a stopped read published: its systems back out of
    /// the resume point, its names back off the chunks, and the read
    /// carrying on from the mark.
    Resuming(LeftOff),
}

/// What to do with a read that was stopped part way.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Ending {
    /// Publish nothing. The directory stands for a whole galaxy and a
    /// prefix of one would replace it with less than it had — which is
    /// what a rebuild from the database is, the rows being read in
    /// address order and the directory already holding every one of them.
    Abandon,
    /// Publish what was read, and record where the read had got to. A dump
    /// is read in file order and what has been read is a galaxy in itself,
    /// smaller than the file; the next run carries on from the mark.
    Publish,
}

/// How far the read behind the directory beside `checkpoint` got, where
/// anything says.
///
/// [`None`] where no build has published one, or where what is there
/// cannot be read — a mark that will not decode is a read starting over,
/// not a run failing.
pub fn left_off(checkpoint: &Path) -> Option<LeftOff> {
    read_meta::<Mark>(&mark_path(checkpoint)).ok().map(LeftOff)
}

/// A cold build a caller pushes into: the galaxy read once, as it arrives.
///
/// Each system goes straight into the spill of the fixed coarse
/// [`bucket`](crate::build::bucket) its position falls in and each name into a
/// chunk; [`finish`](Self::finish) forms the regions from the buckets and
/// raises the tree off them.
///
/// The records are the caller's: a database's rows, a dump's lines. Neither
/// is an event, and nothing here knows which it is reading.
///
/// ## The stop
///
/// A build over the galaxy is hours, and a run asked to stop must not have
/// to sit through one, nor throw away what it read. `stop` is asked per
/// record in [`push`](Self::push), which is the read and where the time
/// goes; the caller then calls [`finish`](Self::finish), and what that does
/// with a read cut short is the caller's [`Ending`].
///
/// [`Ending::Publish`] raises the tree over what was read and writes the
/// directory, the resume point and the mark, so the galaxy read so far is
/// one a map can open and the next run carries on from. The passes that
/// does — the offers and the raise, each a read of every spill — do not
/// ask `stop` again: a caller that has decided not to wait has the second
/// Ctrl-C, which leaves the directory where it stands rather than half
/// written. [`Ending::Abandon`] is the older answer and still the right
/// one for a derivation whose directory already holds more than the read
/// reached.
///
/// A stop is an answer and not a failure — see [`Built`].
pub struct Build<'a> {
    /// The directory the index and the names table are written to.
    dir: PathBuf,
    /// The resume point this leaves, and where the spills go beside it.
    checkpoint: PathBuf,
    /// The two cuts the tree is built on.
    params: BuildParams,
    /// How many systems one region may hold.
    budget: u64,
    /// Where each system has been spilled, by bucket.
    buckets: Buckets,
    /// The names table, written as the names arrive.
    names: names::Writer,
    /// Where the caller says it has read to, for the mark a publish
    /// writes. See [`mark`](Self::mark).
    place: Vec<u8>,
    /// Whether whoever asked for this build has stopped wanting it.
    stop: &'a dyn Fn() -> bool,
}

/// Whether a build is still taking records.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[must_use]
pub enum Taking {
    /// The record is in. Push the next one.
    More,
    /// The run has been asked to stop, and this build has taken nothing
    /// further. The caller stops reading and calls
    /// [`finish`](Build::finish), which answers [`Built::Stopped`] rather
    /// than raising a tree nobody is waiting for.
    Stopped,
}

/// What a cold build came to.
#[derive(Copy, Clone, Debug)]
pub enum Built {
    /// The directory is published: every cell's payload, the names table,
    /// the index file over them, the resume point beside them and the mark
    /// saying how far the read behind them got.
    ///
    /// A read that was stopped and ended [`Ending::Publish`] comes here
    /// too, the galaxy it published being what it read.
    Index(ColdReport),
    /// Nothing was published: a read stopped with nothing to publish, or
    /// one ended [`Ending::Abandon`]. The directory is as the build found
    /// it.
    Stopped(Abandoned),
}

/// A build that published nothing, and what it had read when it stopped.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Abandoned {
    /// Systems the build had taken, the ones a resume brought back with
    /// it included.
    pub systems: u64,
}

impl Abandoned {
    /// A build that was never begun, the run being already stopping.
    pub fn unstarted() -> Abandoned {
        Abandoned { systems: 0 }
    }
}

impl fmt::Display for Abandoned {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} systems read; nothing was published and the directory is \
             as it was found",
            self.systems,
        )
    }
}

impl<'a> Build<'a> {
    /// Open the scratch the systems spill into and the names table beside
    /// it.
    ///
    /// `dir` is the served directory: the cells, their payloads and the
    /// names table. `checkpoint` is the resume point, which is
    /// server-private; the spills ride in a scratch directory beside it,
    /// cleared as each region is built.
    ///
    /// `budget` is how many systems one region may hold. A region is built
    /// alone at about 298 B a system, and the budget is systems rather than
    /// bytes because that is what a bucket's count answers.
    ///
    /// [`Start::Fresh`] clears whatever a previous build left.
    /// [`Start::Resuming`] takes a directory a stopped read published and
    /// carries on with it: every system back out of the resume point and
    /// into the buckets, and the names table back off its own chunks. That
    /// is a read and a write of 56 bytes a system — eleven gigabytes over
    /// the galaxy — against re-reading the dump those systems came out of,
    /// which is six hundred.
    ///
    /// `stop` is asked as the build goes; see the type's own docs for
    /// where.
    pub fn begin(
        dir: &Path,
        checkpoint: &Path,
        params: BuildParams,
        budget: u64,
        start: Start,
        stop: &'a dyn Fn() -> bool,
    ) -> io::Result<Build<'a>> {
        let scratch = spill_dir(checkpoint);
        let mut buckets = Buckets::create(&scratch)?;
        let names = match start {
            Start::Fresh => {
                let _ = std::fs::remove_file(mark_path(checkpoint));
                names::Writer::writing(dir)?
            }
            Start::Resuming(_) => {
                let published = Checkpoint::read(checkpoint)?;
                for &system in published.base() {
                    buckets.push(system)?;
                }
                for &system in published.deltas() {
                    buckets.push(system)?;
                }
                names::Writer::onto(dir)?
            }
        };
        Ok(Build {
            dir: dir.to_owned(),
            checkpoint: checkpoint.to_owned(),
            params,
            budget,
            buckets,
            names,
            place: Vec::new(),
            stop,
        })
    }

    /// Where the caller has read to, for the mark a publish writes.
    ///
    /// The bytes are the caller's own and are carried rather than read. The
    /// last one given stands; a caller that never gives one publishes a
    /// directory nothing can carry on from, which is what a read of
    /// something with no place in it — a database's cursors — has.
    ///
    /// Free, and nothing is written by it: the mark goes out with the
    /// index, so what it says and what the directory holds cannot come
    /// apart.
    pub fn mark(&mut self, cursor: &[u8]) {
        self.place.clear();
        self.place.extend_from_slice(cursor);
    }

    /// One more system, with the name the names table is to carry.
    ///
    /// Order is the caller's: the names table keeps the order they come in
    /// and the tree does not care.
    ///
    /// The stop is asked here rather than every so many records because
    /// this is the read, which at the galaxy's size is hours of it, and
    /// asking costs a call against a record parsed and spilled. A build
    /// that has answered [`Taking::Stopped`] takes nothing more.
    pub fn push(
        &mut self,
        system: System,
        name: NameEntry,
    ) -> io::Result<Taking> {
        if (self.stop)() {
            return Ok(Taking::Stopped);
        }
        self.names.push(name)?;
        self.buckets.push(system)?;
        Ok(Taking::More)
    }

    /// Form the regions from the buckets, raise the tree off them, and
    /// leave the resume point and the mark behind it.
    ///
    /// `by` is what derived the resume point, and so what `cursor` means; a
    /// source with no clock of its own has none to record. `ending` is what
    /// a read that was stopped part way comes to — see [`Ending`].
    ///
    /// Two passes over the spills, because the crown must be settled before
    /// any region can be built: a region cannot know which of its systems a
    /// cell above it took until every region has offered.
    ///
    /// Held at the end: every cell in the galaxy, which is the index file.
    /// Held during: one region's build, which is the budget.
    ///
    /// Each spill is appended to the resume point's base as its region is
    /// built and then deleted, so the base comes out region-ordered — and
    /// that base is what a resumed build takes its systems back out of.
    ///
    /// Neither pass asks `stop`. A read cut short is either being published
    /// or was abandoned before either of them ran, and a caller unwilling
    /// to wait for the raise has the second Ctrl-C; stopping in the middle
    /// of it would leave payloads with no index over them, which is the one
    /// state this is careful never to publish.
    pub fn finish(
        self,
        by: By,
        cursor: Option<NaiveDateTime>,
        ending: Ending,
    ) -> io::Result<Built> {
        let Build {
            dir,
            checkpoint,
            params,
            budget,
            buckets,
            names,
            place,
            stop,
        } = self;
        let named = names.named();
        let taken = buckets.count();
        let scratch = spill_dir(&checkpoint);

        // Nothing to publish, or a caller whose directory already holds
        // more than this read reached.
        if stop() && (taken == 0 || ending == Ending::Abandon) {
            return stopped(&scratch, names, Abandoned { systems: taken });
        }
        let formed =
            bucket::form(&scratch, buckets.finish()?, budget, &params)?;
        let regions = formed.cut.regions().len();

        let offered = offers(&formed, &params)?;
        let systems: u64 = offered.iter().map(Offer::count).sum();
        let allowed = budget.max(params.leaf_cap as u64);
        let over_budget =
            offered.iter().filter(|offer| offer.count() > allowed).count();
        let crown = Crown::over(&offered, &params);
        drop(offered);

        // The point of no return. A cell's payload is replaced in place and
        // the index file that stood is no longer the tree over the payloads
        // beneath it, so it comes down here and the new one goes in at the
        // end: what a reader finds in between is a directory with no index,
        // which is what it reads as nothing at all.
        if let Err(err) = std::fs::remove_file(dir.join(INDEX_FILE)) {
            if err.kind() != io::ErrorKind::NotFound {
                return Err(err);
            }
        }
        crown.built().write_payloads(&dir)?;
        let mut indexes = vec![crown.built().index.clone()];
        let mut base = Compaction::begin(&checkpoint)?;
        let mut points = crown.built().point_count();

        for &region in formed.cut.regions() {
            let spilled = Spilled::open(&formed.spills[&region])?;
            let built = Snapshot::of_region(
                region,
                spilled.systems(),
                crown.claimed(),
                &params,
            );
            built.write_payloads(&dir)?;
            points += built.point_count();
            indexes.push(built.index);
            for &system in spilled.systems() {
                base.push(system)?;
            }
            drop(spilled);
            std::fs::remove_file(&formed.spills[&region])?;
        }
        let _ = std::fs::remove_dir_all(&scratch);
        // Each of the last four steps writes a whole part of the
        // directory, and an `io::Error` off one of them carries no path —
        // a bare "No such file or directory" out of a build over a galaxy
        // is three hours of not knowing which file. Named, so it says.
        step("the resume point", base.finish(cursor, by))?;

        let index = region::joined(&crown, indexes.iter().skip(1));
        let published = step("the names table", names.finish())?;
        step("the index file", index.write(&dir))?;
        // The cells of whatever tree stood here before this one, which this
        // build neither wrote nor named: see `store::sweep_payloads`. After
        // the index file and never before it, so an interrupted sweep
        // leaves a directory that is merely larger.
        let swept = step(
            "the sweep of the old cells",
            crate::store::cells::sweep_payloads(
                &dir,
                &|id| index.get(id).is_some(),
                true,
            ),
        )?;
        // The dead records the body shards carry, which a re-import leaves
        // one of for every system it rewrote: see
        // [`crate::store::bodies::sweep_bodies`]. After the index file, as the cell
        // sweep is, though less turns on the order — every live record is
        // in hand throughout a compaction, so an interruption here leaves
        // a directory that is merely larger.
        //
        // **The stop is not asked.** None of the publish's passes ask it
        // (see this type's docs), a shard is compacted whole, and a caller
        // that has decided not to wait has the second Ctrl-C, which leaves
        // the shards not yet reached exactly as this build left them.
        let reclaimed = step(
            "the compaction of the body shards",
            crate::store::bodies::sweep_bodies(&dir, &|| false, &|_| {}),
        )?;
        // Last, and only where the caller said where it had read to: the
        // mark stands for a published directory, so it goes out behind the
        // index file rather than in front of it.
        if !place.is_empty() {
            step(
                "the resume mark",
                write_meta(
                    &mark_path(&checkpoint),
                    &Mark { cursor: place, systems: taken },
                ),
            )?;
        }
        Ok(Built::Index(ColdReport::of_index(
            systems as usize,
            points,
            regions,
            over_budget,
            Pass { taken: named, named: published },
            Tidied { cells: swept, bodies: reclaimed },
            &index,
        )))
    }
}

/// Say which part of a publish an error came out of.
///
/// The steps that write a whole part of the directory each touch several
/// files, and `std::fs` errors name none of them. A build is hours, so an
/// error it ends with has to be worth reading.
fn step<T>(what: &str, done: io::Result<T>) -> io::Result<T> {
    done.map_err(|err| io::Error::new(err.kind(), format!("{what}: {err}")))
}

/// A build that published nothing, tidied: the scratch spills and the
/// staged names sections go, there being no reader for either.
///
/// Nothing of the served directory is theirs — the sections were staged in
/// the names directory's own `.building`, and the spills sit beside the
/// resume point — so what stood in the directory still stands.
fn stopped(
    scratch: &Path,
    names: names::Writer,
    abandoned: Abandoned,
) -> io::Result<Built> {
    names.abandon()?;
    let _ = std::fs::remove_dir_all(scratch);
    Ok(Built::Stopped(abandoned))
}

/// What each region offers the crown, read off its own spill.
///
/// One region's systems in memory at a time: an offer is the counts a cell
/// above the region would take from it, not the systems themselves.
fn offers(formed: &Formed, params: &BuildParams) -> io::Result<Vec<Offer>> {
    let mut offered = Vec::with_capacity(formed.cut.regions().len());
    for &region in formed.cut.regions() {
        let spilled = Spilled::open(&formed.spills[&region])?;
        offered.push(Offer::of(
            region,
            spilled.systems().iter().copied(),
            params,
        ));
    }
    Ok(offered)
}

/// What the names table came to, carried through to the report.
struct Pass {
    /// Rows pushed, which counts a system named twice twice.
    taken: usize,
    /// Systems the published table names.
    named: usize,
}

/// What the publish's two sweeps came to, carried through to the report.
///
/// One argument rather than two: they are asked at the same point for the
/// same reason — what stood in the directory before this build that this
/// build does not refer to — and differ only in what they reclaim.
struct Tidied {
    /// Payloads of cells the new tree does not name.
    cells: Swept,
    /// Records in the body shards the pack points at none of.
    bodies: Reclaimed,
}

/// What a cold build came to, for a caller to print and check.
#[derive(Copy, Clone, Debug)]
pub struct ColdReport {
    /// Systems read into a region, which is what the tree was built from.
    pub systems: usize,
    /// Systems owned by a cell, across the whole tree. Equal to `systems`
    /// where the partition holds.
    pub points: usize,
    pub cells: usize,
    pub leaves: usize,
    pub deepest_level: u8,
    /// Systems owned by the widest leaf.
    pub max_leaf_points: usize,
    /// Regions the cut divided the galaxy into.
    pub regions: usize,
    /// Regions holding more than the budget, which nothing divided: a cell
    /// at [`MAX_LEVEL`](crate::core::geometry::MAX_LEVEL), or one whose systems
    /// share a position. Each was built with more than the budget's
    /// memory, so a build answering more than zero here is one that asked
    /// for more than it was given.
    pub over_budget: usize,
    /// Systems the published names table names. Fewer than the rows taken
    /// where a read named the same system more than once.
    pub named: usize,
    /// Rows pushed into the names table as the galaxy was read.
    pub named_rows: usize,
    /// Payload files of cells the published tree does not name, removed
    /// after it was written: whatever the tree that stood here before held
    /// and this one does not. See [`crate::store::cells::sweep_payloads`].
    pub swept: Swept,
    /// Dead records the body shards gave back, compacted once the index
    /// file stood: a re-import appends a fresh record for every system and
    /// the one behind it is dead. See [`crate::store::bodies::sweep_bodies`].
    pub reclaimed: Reclaimed,
}

impl ColdReport {
    /// The report over a tree whose payloads are no longer in hand.
    ///
    /// A regional build drops each region's payloads, so `points` is counted
    /// as it goes and the widest leaf is read off the cells' rank ranges.
    fn of_index(
        systems: usize,
        points: usize,
        regions: usize,
        over_budget: usize,
        pass: Pass,
        tidied: Tidied,
        index: &Index,
    ) -> ColdReport {
        let leaves = index.cells().filter(|c| c.is_leaf()).count();
        let deepest_level =
            index.cells().map(|c| c.id.level).max().unwrap_or(0);
        let max_leaf_points = index
            .cells()
            .filter(|c| c.is_leaf())
            .map(|c| c.slice_len() as usize)
            .max()
            .unwrap_or(0);
        ColdReport {
            systems,
            points,
            cells: index.len(),
            leaves,
            deepest_level,
            max_leaf_points,
            regions,
            over_budget,
            named: pass.named,
            named_rows: pass.taken,
            swept: tidied.cells,
            reclaimed: tidied.bodies,
        }
    }

    /// Whether every system landed in exactly one cell: the partition holds.
    pub fn is_consistent(&self) -> bool {
        self.points == self.systems
    }
}

impl fmt::Display for ColdReport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} systems -> {} cells ({} leaves, {} internal), \
             deepest level {}, largest leaf {} systems, {} placed{}{}{}{}",
            self.systems,
            self.cells,
            self.leaves,
            self.cells - self.leaves,
            self.deepest_level,
            self.max_leaf_points,
            self.points,
            if self.is_consistent() { "" } else { " (MISMATCH)" },
            match self.over_budget {
                0 => String::new(),
                n => format!(", {n} regions over budget"),
            },
            match self.swept.orphans {
                0 => String::new(),
                n => format!(
                    ", swept {n} orphaned payloads ({} MB)",
                    self.swept.bytes / 1_000_000
                ),
            },
            match self.reclaimed.shards {
                0 => String::new(),
                n => format!(
                    ", compacted {n} body shards ({} MB of dead record)",
                    self.reclaimed.bytes / 1_000_000
                ),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::layout::HEAD_FILE;
    use crate::format::layout::NAMES_DIR;
    use crate::store::names::Names;
    use std::cell::Cell;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Somewhere to build into, removed with the value.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let at = std::env::temp_dir().join(format!(
                "galos-cold-{}-{}-{}",
                name,
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            let _ = std::fs::remove_dir_all(&at);
            std::fs::create_dir_all(&at).expect("a scratch directory");
            Scratch(at)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn coord(&mut self) -> f64 {
            (self.next() % 40_000) as f64 - 20_000.0
        }
    }

    /// A galaxy lumpy enough that a cut by count is not a cut by level.
    fn galaxy(n: u64) -> Vec<System> {
        let mut rng = Rng(0x5EED);
        let clumps: Vec<[f64; 3]> =
            (0..6).map(|_| [rng.coord(), rng.coord(), rng.coord()]).collect();
        (1..=n)
            .map(|id| {
                let near = clumps[(rng.next() % clumps.len() as u64) as usize];
                let spread = if id % 5 == 0 { 4_000.0 } else { 200.0 };
                let off = |rng: &mut Rng| {
                    (rng.next() % 2_000) as f64 / 1_000.0 * spread
                        - spread / 2.0
                };
                System {
                    id64: id,
                    position: [
                        near[0] + off(&mut rng),
                        near[1] + off(&mut rng),
                        near[2] + off(&mut rng),
                    ],
                    absolute_magnitude: (rng.next() % 2_000) as f64 / 100.0
                        - 5.0,
                    temperature: 3_000.0 + (rng.next() % 20_000) as f64,
                    age_bucket: (rng.next() % 8) as u32,
                    updated_at: 1_700_000_000 + (id as u32 % 1_000),
                    kind: crate::core::record::StarKind::G,
                }
            })
            .collect()
    }

    /// The name a test galaxy carries for a system.
    fn entry(system: &System) -> NameEntry {
        NameEntry {
            address: system.id64 as i64,
            name: format!("Sys {}", system.id64).into(),
            position: [
                system.position[0] as f32,
                system.position[1] as f32,
                system.position[2] as f32,
            ],
        }
    }

    /// A galaxy lumpier than [`galaxy`]: three quarters of it inside one
    /// bucket, so a bucket overflows and has to be split; a disc around
    /// it; and a scattering of lone systems out to the corners of the
    /// cube, so buckets holding a handful have to be grouped instead.
    fn lumpy(n: u64) -> Vec<System> {
        /// Anywhere in the cube.
        fn wide(rng: &mut Rng) -> f64 {
            (rng.next() % 120_001) as f64 - 60_000.0
        }
        /// Within 60 ly, which is a thousandth of a bucket's edge.
        fn tight(rng: &mut Rng) -> f64 {
            (rng.next() % 1_001) as f64 / 1_000.0 * 120.0 - 60.0
        }

        let mut rng = Rng(0xC0FFEE);
        (1..=n)
            .map(|id| {
                let position = if id % 40 == 0 {
                    [wide(&mut rng), wide(&mut rng), wide(&mut rng)]
                } else if id % 4 == 0 {
                    [rng.coord(), rng.coord() / 10.0, rng.coord()]
                } else {
                    [
                        1_000.0 + tight(&mut rng),
                        200.0 + tight(&mut rng),
                        5_000.0 + tight(&mut rng),
                    ]
                };
                System {
                    id64: id,
                    position,
                    absolute_magnitude: (rng.next() % 2_000) as f64 / 100.0
                        - 5.0,
                    temperature: 3_000.0 + (rng.next() % 20_000) as f64,
                    age_bucket: (rng.next() % 8) as u32,
                    updated_at: 1_700_000_000 + (id as u32 % 1_000),
                    kind: crate::core::record::StarKind::G,
                }
            })
            .collect()
    }

    /// Every system pushed once, as a source hands them over, and the
    /// regions formed from what arrived. Nothing asks this to stop.
    fn built(
        at: &Scratch,
        name: &str,
        params: BuildParams,
        budget: u64,
        systems: &[System],
    ) -> io::Result<ColdReport> {
        let never = || false;
        let mut build = Build::begin(
            &at.join(name),
            &at.join(&format!("{name}.checkpoint")),
            params,
            budget,
            Start::Fresh,
            &never,
        )?;
        for &system in systems {
            assert_eq!(build.push(system, entry(&system))?, Taking::More);
        }
        match build.finish(By::Database, None, Ending::Abandon)? {
            Built::Index(report) => Ok(report),
            Built::Stopped(abandoned) => {
                panic!("nothing asked it to stop: {}", abandoned)
            }
        }
    }

    /// Hold `dir` to the directory a whole build would have written: the
    /// same cells owning the same systems, and a names table over the same
    /// galaxy.
    ///
    /// The pieces are held to this by
    /// `region::tests::a_regional_build_is_the_whole_build`; this holds the
    /// sequence they go in to it, payloads on disk and all.
    fn is_the_whole_build(
        at: &Scratch,
        dir: &Path,
        params: BuildParams,
        systems: &[System],
    ) {
        let whole_dir = at.join("whole");
        let whole = Snapshot::build(systems, &params);
        whole.write(&whole_dir).expect("the whole build written");

        let built = Index::read(dir).expect("the cold index");
        assert_eq!(built.len(), whole.index.len());
        for cell in whole.index.cells() {
            let built = built
                .get(cell.id)
                .unwrap_or_else(|| panic!("{:?} is missing", cell.id));
            assert_eq!(
                (built.rank_lo, built.rank_hi, built.child_mask),
                (cell.rank_lo, cell.rank_hi, cell.child_mask),
                "{:?} differs",
                cell.id,
            );
            assert_eq!(
                Index::read_payload(dir, cell.id).expect("a cold payload"),
                Index::read_payload(&whole_dir, cell.id)
                    .expect("a whole payload"),
                "{:?} owns different systems",
                cell.id,
            );
        }

        let names = Names::open(dir).expect("the names table");
        assert_eq!(names.len(), systems.len());
        names.base().audit().expect("a table a cold build wrote");
    }

    /// A cold build writes the directory a whole build would have written,
    /// over a galaxy lumpy enough that a cut by count is not a cut by
    /// level.
    #[test]
    fn a_cold_build_is_the_whole_build() {
        let params = BuildParams::default();
        let systems = galaxy(40_000);
        let at = Scratch::new("cold");

        let report =
            built(&at, "cold", params, 6_000, &systems).expect("a cold build");

        assert_eq!(report.systems, systems.len());
        assert!(report.is_consistent(), "{report}");
        assert_eq!(report.named, systems.len());
        assert!(report.regions > 1, "a budget of 6,000 left one region");

        is_the_whole_build(&at, &at.join("cold"), params, &systems);
    }

    /// And over a galaxy lumpy enough that buckets both overflow and are
    /// left nearly empty, so the cut formed from them is neither the
    /// buckets nor one level of them.
    #[test]
    fn a_lumpy_galaxy_is_cut_both_ways() {
        let params = BuildParams::default();
        let systems = lumpy(40_000);
        let at = Scratch::new("lumpy");

        let report =
            built(&at, "lumpy", params, 6_000, &systems).expect("a cold build");

        assert_eq!(report.systems, systems.len());
        assert!(report.is_consistent(), "{report}");
        assert_eq!(report.named, systems.len());
        assert_eq!(report.over_budget, 0, "{report}");
        assert!(report.regions > 1, "a budget of 6,000 left one region");

        // And it is lumpy in both directions: the core overflowed its
        // bucket and was split below it, the outliers were grouped into
        // regions above it. Formed again here, the cut being the one thing
        // the report does not carry.
        let scratch = at.join("levels");
        let mut buckets =
            Buckets::create(&scratch).expect("a scratch directory");
        for &system in &systems {
            buckets.push(system).expect("a system spilled");
        }
        let formed = bucket::form(
            &scratch,
            buckets.finish().expect("the buckets flushed"),
            6_000,
            &params,
        )
        .expect("regions formed");
        let levels: HashSet<u8> =
            formed.cut.regions().iter().map(|cell| cell.level).collect();
        assert!(
            levels.iter().any(|&level| level > bucket::BUCKET_LEVEL),
            "no bucket overflowed: regions at levels {levels:?}",
        );
        assert!(
            levels.iter().any(|&level| level < bucket::BUCKET_LEVEL),
            "no buckets were grouped: regions at levels {levels:?}",
        );

        is_the_whole_build(&at, &at.join("lumpy"), params, &systems);
    }

    /// A region no split divides is reported over budget rather than
    /// passed off as one that fits. Systems on one point are that region:
    /// every level puts them in the same cell.
    #[test]
    fn an_indivisible_region_over_budget_is_reported() {
        let params = BuildParams { internal_slice: 8, leaf_cap: 32 };
        let at = Scratch::new("stuck");
        let mut rng = Rng(0x5EED);
        let on = [1_000.0, 200.0, 5_000.0];
        let systems: Vec<System> = (1..=400)
            .map(|id| System {
                id64: id,
                position: on,
                absolute_magnitude: (rng.next() % 2_000) as f64 / 100.0 - 5.0,
                temperature: 3_000.0 + (rng.next() % 20_000) as f64,
                age_bucket: (rng.next() % 8) as u32,
                updated_at: 1_700_000_000,
                kind: crate::core::record::StarKind::G,
            })
            .collect();

        let report =
            built(&at, "stuck", params, 100, &systems).expect("a cold build");

        assert_eq!(report.regions, 1);
        assert_eq!(report.over_budget, 1, "{report}");
        assert!(
            format!("{report}").contains("over budget"),
            "the report does not say so: {report}",
        );
        assert_eq!(report.systems, systems.len());
        assert_eq!(report.named, systems.len());
    }

    /// Every file of `dir` by its path within it, and what it holds.
    fn contents(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(root: &Path, at: &Path, into: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(at).expect("a directory") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    walk(root, &path, into);
                    continue;
                }
                let within = path.strip_prefix(root).expect("inside the root");
                into.insert(
                    within.to_owned(),
                    std::fs::read(&path).expect("a file"),
                );
            }
        }

        let mut found = BTreeMap::new();
        walk(dir, dir, &mut found);
        found
    }

    /// A build asked to stop while it is reading leaves the directory it is
    /// building over exactly as it found it, and takes its scratch with it.
    ///
    /// The read is where a build asked to stop nearly always is, the galaxy
    /// being what takes the hour. More than one names chunk is pushed
    /// before the stop, so the table it staged is one that would otherwise
    /// have been written over the published one a chunk at a time.
    #[test]
    fn a_stopped_build_leaves_the_directory_alone() {
        let params = BuildParams::default();
        let at = Scratch::new("stopped");
        let (dir, checkpoint) =
            (at.join("served"), at.join("served.checkpoint"));

        // A directory a client is being served from.
        built(&at, "served", params, 6_000, &galaxy(5_000))
            .expect("a published index");
        let before = contents(&dir);
        let resume_point = std::fs::read(&checkpoint).expect("a base");
        assert!(before.contains_key(Path::new(INDEX_FILE)));

        // Another galaxy pushed over it, stopped part way through the read.
        // 66,000 is more than the 64Ki entries a names chunk holds.
        let systems = lumpy(70_000);
        let pushed = Cell::new(0u64);
        let stop = || pushed.get() >= 66_000;
        let mut build =
            Build::begin(&dir, &checkpoint, params, 6_000, Start::Fresh, &stop)
                .expect("a build");
        for &system in &systems {
            match build.push(system, entry(&system)).expect("a push") {
                Taking::More => pushed.set(pushed.get() + 1),
                Taking::Stopped => break,
            }
        }
        assert_eq!(pushed.get(), 66_000);

        let stopped =
            build.finish(By::Database, None, Ending::Abandon).expect("a build");
        let Built::Stopped(abandoned) = stopped else {
            panic!("it ran to its end: {:?}", stopped)
        };
        assert_eq!(abandoned, Abandoned { systems: 66_000 });
        assert_eq!(contents(&dir), before, "the directory was written to");
        assert_eq!(
            std::fs::read(&checkpoint).expect("a base"),
            resume_point,
            "the resume point was written over",
        );
        assert!(!spill_dir(&checkpoint).exists(), "the spills are still there",);
        assert!(
            !checkpoint.with_extension("tmp").exists(),
            "a base nothing will read is still there",
        );

        // And again with nothing asking it to stop: the same directory,
        // built, the names table the new galaxy's.
        let report = built(&at, "served", params, 6_000, &systems)
            .expect("a cold build");
        assert_eq!(report.systems, systems.len());
        assert!(report.is_consistent(), "{report}");
        assert_eq!(
            Names::open(&dir).expect("the names table").len(),
            systems.len(),
        );
        assert!(dir.join(INDEX_FILE).exists(), "no index file was written");
    }

    /// A stopped read publishes what it read, and carrying on from it comes
    /// to the directory a build that was never stopped comes to
    ///
    /// Two claims in one, and both are silent failures in a directory. The
    /// first is that [`Ending::Publish`] leaves an index a map can open
    /// over exactly the systems that were read. The second is that
    /// [`Start::Resuming`] takes every one of them back out of the resume
    /// point — lose one and it is a name the map can find and never draw,
    /// take one twice and it is a system in two cells, which is not a tree.
    ///
    /// The payloads and the names chunks are compared byte for byte; the
    /// index file by its cells' integers, `rank_lo`, `rank_hi`,
    /// `child_mask` and the count, since the floats beside them are summed
    /// in the order a cell map iterates and move in the last bit between
    /// one build and the next whatever is done to them.
    #[test]
    fn a_resumed_build_is_the_build_that_was_never_stopped() {
        let params = BuildParams { internal_slice: 8, leaf_cap: 32 };
        let at = Scratch::new("resumed");
        // More than the 64Ki entries a names chunk holds, so the chunk the
        // stop published part-filled is one the resume fills the rest of.
        let systems = lumpy(70_000);

        built(&at, "whole", params, 6_000, &systems).expect("a cold build");

        let (dir, checkpoint) =
            (at.join("halved"), at.join("halved.checkpoint"));
        let pushed = Cell::new(0u64);
        let stop = || pushed.get() >= 40_000;
        let mut build =
            Build::begin(&dir, &checkpoint, params, 6_000, Start::Fresh, &stop)
                .expect("a build");
        let mut took = 0usize;
        for &system in &systems {
            match build.push(system, entry(&system)).expect("a push") {
                Taking::More => {
                    pushed.set(pushed.get() + 1);
                    took += 1;
                }
                // The record was refused, so the mark stands for what went
                // in before it — which is what the caller counted.
                Taking::Stopped => {
                    build.mark(took.to_string().as_bytes());
                    break;
                }
            }
        }
        let Built::Index(part) = build
            .finish(By::Database, None, Ending::Publish)
            .expect("a stopped build publishes")
        else {
            panic!("a stopped read published nothing")
        };
        assert_eq!(part.systems, 40_000, "{part}");
        assert!(part.is_consistent(), "{part}");
        assert!(
            dir.join(INDEX_FILE).exists(),
            "a stopped read left no index to open",
        );
        assert_eq!(Names::open(&dir).expect("the names table").len(), 40_000,);

        let left = left_off(&checkpoint).expect("a mark to take up");
        assert_eq!(left.systems(), 40_000);
        let from: usize = std::str::from_utf8(left.cursor())
            .expect("the cursor is the caller's")
            .parse()
            .expect("a count");
        assert_eq!(from, took);

        let never = || false;
        let mut build = Build::begin(
            &dir,
            &checkpoint,
            params,
            6_000,
            Start::Resuming(left),
            &never,
        )
        .expect("a resumed build");
        for &system in &systems[from..] {
            assert_eq!(
                build.push(system, entry(&system)).expect("a push"),
                Taking::More,
            );
        }
        let Built::Index(report) =
            build.finish(By::Database, None, Ending::Publish).expect("a build")
        else {
            panic!("the resumed build stopped")
        };
        assert_eq!(report.systems, systems.len(), "{report}");
        assert!(report.is_consistent(), "{report}");

        let whole = at.join("whole");
        // The names generation is a counter and not content: the stopped
        // read published generation zero and the resume wrote the next,
        // where a build that was never stopped wrote only the first. So a
        // section is compared under a path with the number taken out, and
        // `head.bin` — which carries the number in its bytes — is compared
        // by the table it describes instead, below.
        let generational = |path: &Path| -> PathBuf {
            let mut parts = path.components();
            let Some(head) = parts.next() else {
                return path.to_owned();
            };
            if head.as_os_str() != NAMES_DIR {
                return path.to_owned();
            }
            match parts.next() {
                Some(number)
                    if number
                        .as_os_str()
                        .to_str()
                        .is_some_and(|it| it.parse::<u64>().is_ok()) =>
                {
                    Path::new(NAMES_DIR).join("gen").join(parts.as_path())
                }
                _ => path.to_owned(),
            }
        };
        let named = |dir: &Path| {
            let held = Names::open(dir).expect("a names table");
            held.base().audit().expect("a base a build wrote");
            held.addresses()
                .filter_map(|address| held.entry_of(address))
                .collect::<Vec<_>>()
        };
        assert_eq!(named(&dir), named(&whole), "the names tables differ");

        let over = |dir: &Path| -> BTreeMap<PathBuf, Vec<u8>> {
            contents(dir)
                .into_iter()
                .map(|(path, bytes)| (generational(&path), bytes))
                .collect()
        };
        let mine = over(&dir);
        let theirs = over(&whole);
        for (path, bytes) in &theirs {
            if path == Path::new(INDEX_FILE)
                || path == &Path::new(NAMES_DIR).join(HEAD_FILE)
            {
                continue;
            }
            assert_eq!(
                mine.get(path).map(Vec::len),
                Some(bytes.len()),
                "{}: the resumed build wrote it differently",
                path.display(),
            );
            assert!(
                mine[path] == *bytes,
                "{}: the resumed build wrote other bytes",
                path.display(),
            );
        }
        assert_eq!(
            mine.keys().collect::<Vec<_>>(),
            theirs.keys().collect::<Vec<_>>(),
            "the two builds wrote different files",
        );

        let counted = |dir: &Path| {
            let index = Index::read(dir).expect("an index file");
            let mut cells: Vec<(u8, u64, u64, u64, u8, u64)> = index
                .cells()
                .map(|cell| {
                    (
                        cell.id.level,
                        cell.id.morton(),
                        cell.rank_lo,
                        cell.rank_hi,
                        cell.child_mask,
                        cell.aggregate.count(),
                    )
                })
                .collect();
            cells.sort_unstable();
            cells
        };
        assert_eq!(counted(&dir), counted(&whole), "the trees differ");
    }

    /// Bodies large enough that a shard's dead records are worth sweeping
    ///
    /// The sweep's bar is a mebibyte of dead record in a shard, which a
    /// galaxy reaches with hundreds of thousands of systems in it and this
    /// reaches with barycenters.
    fn padded() -> crate::records::SystemBodies {
        let at = "2026-08-08T12:00:00Z".parse().expect("a moment");
        crate::records::SystemBodies {
            barycenters: (0..8_000)
                .map(|id| crate::records::Barycenter {
                    system_address: 1,
                    id,
                    updated_at: at,
                    updated_by: "a test".into(),
                    orbit: None,
                })
                .collect(),
            ..crate::records::SystemBodies::default()
        }
    }

    /// A publish gives the body shards' dead records back
    ///
    /// The bodies are not a build's to write — a dump's reader writes them
    /// as it reads — but the leftovers are its to reclaim, and this is the
    /// one place that knows the read is over. An import over a directory
    /// that already holds the galaxy appends a fresh record for every
    /// system and leaves the one behind it dead, and nothing on the write
    /// path reaches those: a shard is folded when its tail passes a bound
    /// an import leaves it well under. Measured on a re-imported galaxy
    /// before this ran here: 161.1 GB.
    #[test]
    fn a_publish_reclaims_the_body_shards() {
        let at = Scratch::new("reclaimed");
        let dir = at.join("served");
        let inside = padded();
        // One shard, so the dead records pile up in one data file rather
        // than a kilobyte each across four thousand of them.
        let shard = crate::format::layout::body_shard(1);
        let addresses: Vec<i64> = (1i64..)
            .filter(|&it| crate::format::layout::body_shard(it) == shard)
            .take(4)
            .collect();

        // Written twice, which is what a re-import does to every system in
        // the galaxy: the second record is what a reader answers and the
        // first is bytes nothing points at.
        for _ in 0..2 {
            let rows: HashMap<i64, crate::records::SystemBodies> =
                addresses.iter().map(|&it| (it, inside.clone())).collect();
            assert!(crate::store::bodies::write(&dir, rows).failed.is_none());
        }
        let dead =
            crate::store::bodies::weigh(&dir, &|| false).expect("a weighing");
        assert!(dead.reclaimable > 0, "the re-import left nothing to reclaim");

        let report =
            built(&at, "served", BuildParams::default(), 6_000, &lumpy(1_000))
                .expect("a cold build");
        assert_eq!(
            (report.reclaimed.shards, report.reclaimed.bytes),
            (1, dead.reclaimable),
            "the publish left the dead records where they were: {report}",
        );
        assert!(report.reclaimed.finished);

        // And every system's own bodies came through the rewrite, which is
        // the half of a compaction that cannot be got wrong quietly.
        for &address in &addresses {
            assert_eq!(
                crate::store::bodies::find(&dir, address)
                    .expect("the pack reads"),
                crate::store::bodies::Found::Bodies(inside.clone()),
                "system {address} did not survive the publish",
            );
        }
    }
}
