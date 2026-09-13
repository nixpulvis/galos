//! A cold build: the whole galaxy raised from records, a region at a time.
//!
//! [`crate::region`] is the pieces — the cut, what a region offers, the
//! crown over it, a region's own snapshot, the join. This is the sequence
//! they go in. A caller pushes every record it has into [`Build`] and the
//! cut is formed afterwards, from the [`bucket`](crate::bucket)s the
//! systems landed in, so the galaxy is read once.
//!
//! Nothing the galaxy's size scales is held. Each system goes straight to
//! its bucket's spill file and each name into a names chunk; each region is
//! then built off the mapping of its spill and its payloads written, so what
//! is held at the end is every cell in the galaxy, which is the index file.
//!
//! No live [`Tree`](crate::Tree) is raised: a watch gets one by resuming
//! from the resume point this leaves.

use crate::bucket::{self, Buckets, Formed};
use crate::checkpoint::{By, Compaction};
use crate::geometry::CellId;
use crate::meta::NameEntry;
use crate::names::Chunks;
use crate::region::{self, Crown, Offer};
use crate::source::{read_meta, write_meta};
use crate::spill::Spilled;
use crate::store::INDEX_FILE;
use crate::tree::{BuildParams, Snapshot, System};
use crate::walk::Index;
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

/// What a build has read, so that a stopped one can be taken up.
///
/// The spills are the work: every system read, on disk, in the bucket it
/// belongs to. What they do not say is where in its own source the caller
/// had got to, and without that a resumed read either starts over or
/// spills a second copy of what it already has.
///
/// So a mark is the caller's place and the cut the spills stand at: the
/// bytes of each bucket's file at the moment the mark was taken, and the
/// names chunks beside them. The buffers flush on their own, so a spill
/// runs on past its figure between one mark and the next; the figures are
/// what a resume cuts back to. See [`Buckets::resume`](crate::bucket::Buckets::resume).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Mark {
    /// The caller's place in its own source, opaque here.
    cursor: Vec<u8>,
    /// Each bucket, as level and Morton key, and the bytes of its spill.
    buckets: Vec<(u8, u64, u64)>,
    /// Complete name chunks, and entries in the tail staged beside them.
    chunks: usize,
    tail: usize,
    /// Systems the mark stands for.
    systems: u64,
}

/// A stopped build's work, and where its caller had read to.
///
/// Answered by [`left_off`] and handed back to [`Build::begin`] as
/// [`Start::Resuming`]. The cursor is the caller's own: this carries it and
/// does not read it.
#[derive(Clone, Debug)]
pub struct LeftOff(Mark);

impl LeftOff {
    /// What the caller wrote when it last marked its place.
    pub fn cursor(&self) -> &[u8] {
        &self.0.cursor
    }

    /// Systems already spilled, which a resumed read does not read again.
    pub fn systems(&self) -> u64 {
        self.0.systems
    }
}

/// Where a build starts.
#[derive(Clone, Debug)]
pub enum Start {
    /// From nothing: whatever a previous build left is cleared first.
    Fresh,
    /// From what a stopped build left, at the cut its mark names.
    Resuming(LeftOff),
}

/// What a stopped build left beside `checkpoint`, where it left anything.
///
/// [`None`] where no build has stopped part way, where one finished (the
/// scratch goes with it), or where what is there cannot be read — a mark
/// that will not decode is a build starting over, not a run failing.
pub fn left_off(checkpoint: &Path) -> Option<LeftOff> {
    read_meta::<Mark>(&mark_path(&spill_dir(checkpoint))).ok().map(LeftOff)
}

/// Where a build's mark sits: in the scratch, so clearing one clears both.
fn mark_path(scratch: &Path) -> PathBuf {
    scratch.join("mark.bin")
}

/// Where a build's own scratch is: beside the resume point, cleared when
/// the build finishes and left standing by a stop that can be taken up.
///
/// A caller with working files of its own — the rows of the tables a
/// record fills, which are cut at the same place the spills are — puts
/// them here, so that one directory removed is the whole of a build's
/// scratch and a fresh build cannot read a stopped one's leavings.
pub fn scratch(checkpoint: &Path) -> PathBuf {
    spill_dir(checkpoint)
}

/// A cold build a caller pushes into: the galaxy read once, as it arrives.
///
/// Each system goes straight into the spill of the fixed coarse
/// [`bucket`](crate::bucket) its position falls in and each name into a
/// chunk; [`finish`](Self::finish) forms the regions from the buckets and
/// raises the tree off them.
///
/// The records are the caller's: a database's rows, a dump's lines. Neither
/// is an event, and nothing here knows which it is reading.
///
/// ## The stop
///
/// A build over the galaxy is an hour, and a run asked to stop must not
/// have to sit through one. `stop` is asked at the three places the time
/// goes: per record in [`push`](Self::push), which is the read; per region
/// in the offer pass, which reads every spill back; and per region in the
/// raise, which reads every spill back and writes every payload. Between
/// them is [`bucket::form`], which is not interruptible: it rewrites the
/// buckets that are over budget and nothing else, where the three above are
/// each the galaxy.
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
    names: Chunks,
    /// Systems this build took up from a stopped one, of what it holds.
    kept: u64,
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
    /// the index file over them and the resume point beside them.
    Index(ColdReport),
    /// The run was asked to stop first. Nothing was published and there is
    /// no resume point to take up, so the next run builds from nothing.
    Stopped(Abandoned),
}

/// How far a build got before it was asked to stop, and what it left.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Abandoned {
    /// Systems the build had taken.
    pub systems: u64,
    /// Regions raised, of [`regions`](Self::regions).
    pub raised: usize,
    /// Regions the cut divided the galaxy into. Zero where the stop came
    /// while the galaxy was still being read, there being no cut yet.
    pub regions: usize,
    /// Whether the directory is as the build found it.
    ///
    /// True for a stop up to and including the offer pass: the spills are
    /// scratch and the names table was staged, so neither is published.
    /// False from the first payload written: those payloads are this
    /// build's and nothing here holds the galaxy it would take to put the
    /// others back, so the index file that stood over them comes down with
    /// them and the directory serves nothing until a build finishes.
    ///
    /// It says nothing about the body files, which are the caller's and
    /// are written as the read goes.
    pub intact: bool,
    /// Systems the spills hold for the next run, which it does not read
    /// again.
    ///
    /// What the build's last [`mark`](Build::mark) stands for, and zero
    /// where the read has to start over: a caller that never marked, or a
    /// stop past the point where the buckets are consumed.
    pub kept: u64,
}

impl Abandoned {
    /// A build that was never begun, the run being already stopping.
    pub fn unstarted() -> Abandoned {
        Abandoned { systems: 0, raised: 0, regions: 0, intact: true, kept: 0 }
    }
}

impl fmt::Display for Abandoned {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} systems read", self.systems)?;
        if self.regions > 0 {
            write!(
                f,
                ", {} of {} regions raised",
                self.raised, self.regions
            )?;
        }
        match self.intact {
            true => write!(f, "; the directory is as it was found")?,
            false => write!(f, "; the directory serves nothing until a \
                                build finishes")?,
        }
        match self.kept {
            0 => write!(f, " and the next run reads from the start"),
            kept => write!(f, " and the next run takes up {kept} systems"),
        }
    }
}

impl<'a> Build<'a> {
    /// Open the scratch the systems spill into and the names table beside
    /// it, both emptied of anything a previous build left.
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
        let (buckets, names, kept) = match start {
            Start::Fresh => {
                (Buckets::create(&scratch)?, Chunks::writing(dir), 0)
            }
            Start::Resuming(LeftOff(mark)) => {
                let cut: Vec<(CellId, u64)> = mark
                    .buckets
                    .iter()
                    .map(|&(level, morton, bytes)| {
                        (CellId::from_morton(level, morton), bytes)
                    })
                    .collect();
                (
                    Buckets::resume(&scratch, &cut)?,
                    Chunks::resuming(dir, mark.chunks, mark.tail)?,
                    mark.systems,
                )
            }
        };
        Ok(Build {
            dir: dir.to_owned(),
            checkpoint: checkpoint.to_owned(),
            params,
            budget,
            buckets,
            names,
            kept,
            stop,
        })
    }

    /// Write down where the caller has read to, and cut the spills to
    /// match.
    ///
    /// `cursor` is the caller's own place in its own source and is carried
    /// rather than read. Everything pushed before this call is on disk when
    /// it returns, and a build that stops after it leaves a scratch the
    /// next run takes up with [`left_off`] — so what a stop costs is
    /// whatever has been read since the last mark, and a caller that marks
    /// when it is told to stop loses nothing at all.
    ///
    /// Not free: every buffer is flushed and the part-filled names chunk is
    /// written, which over a galaxy's worth of buckets is a few thousand
    /// small writes and a megabyte. Marking per record would cost more than
    /// the read. The caller decides how much of a read it is prepared to
    /// lose to a kill.
    pub fn mark(&mut self, cursor: &[u8]) -> io::Result<()> {
        let buckets = self
            .buckets
            .lengths()?
            .into_iter()
            .map(|(id, bytes)| (id.level, id.morton(), bytes))
            .collect();
        let tail = self.names.stage()?;
        let mark = Mark {
            cursor: cursor.to_vec(),
            buckets,
            chunks: self.names.complete(),
            tail,
            systems: self.buckets.count(),
        };
        write_meta(&mark_path(&spill_dir(&self.checkpoint)), &mark)?;
        self.kept = mark.systems;
        Ok(())
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

    /// Form the regions from the buckets, raise the tree off them, and leave
    /// the resume point behind it.
    ///
    /// `by` is what derived the resume point, and so what `cursor` means; a
    /// source with no clock of its own has none to record.
    ///
    /// Two passes over the spills, because the crown must be settled before
    /// any region can be built: a region cannot know which of its systems a
    /// cell above it took until every region has offered.
    ///
    /// Held at the end: every cell in the galaxy, which is the index file.
    /// Held during: one region's build, which is the budget.
    ///
    /// Each spill is appended to the resume point's base as its region is
    /// built and then deleted, so the base comes out region-ordered.
    ///
    /// Nothing of the directory is replaced until the crown's payloads are
    /// written, which is where [`Abandoned::intact`] stops holding.
    ///
    /// A stop before the buckets are formed into regions leaves them, and
    /// the mark beside them, for the next run: see [`Build::mark`]. A stop
    /// after it does not — `form` renames, concatenates and divides the
    /// bucket files, so what is on disk from there on is regions and no
    /// longer the cut a read could be taken up at.
    pub fn finish(
        self,
        by: By,
        cursor: Option<NaiveDateTime>,
    ) -> io::Result<Built> {
        let Build {
            dir,
            checkpoint,
            params,
            budget,
            buckets,
            names,
            kept,
            stop,
        } = self;
        let named = names.named();
        let taken = buckets.count();
        let scratch = spill_dir(&checkpoint);
        let abandoned = |raised, regions, intact, kept| Abandoned {
            systems: taken,
            raised,
            regions,
            intact,
            kept,
        };

        if stop() {
            return stopped(&scratch, names, abandoned(0, 0, true, kept));
        }
        let formed =
            bucket::form(&scratch, buckets.finish()?, budget, &params)?;
        let regions = formed.cut.regions().len();

        let Some(offered) = offers(&formed, &params, stop)? else {
            return stopped(&scratch, names, abandoned(0, regions, true, 0));
        };
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

        for (raised, &region) in formed.cut.regions().iter().enumerate() {
            if stop() {
                base.abandon()?;
                return stopped(
                    &scratch,
                    names,
                    abandoned(raised, regions, false, 0),
                );
            }
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
        base.finish(cursor, by)?;

        let index = region::joined(&crown, indexes.iter().skip(1));
        let chunks = names.finish()?;
        index.write(&dir)?;
        Ok(Built::Index(ColdReport::of_index(
            systems as usize,
            points,
            regions,
            over_budget,
            Pass { named, chunks },
            &index,
        )))
    }
}

/// What a stopped build leaves on disk, which is what its report says.
///
/// A stop the next run can take up — [`Abandoned::kept`] systems, a mark
/// standing for them — leaves the scratch spills and the staged names
/// exactly where a resume looks for them. Nothing of the served directory
/// is theirs: the chunks are in `.building` and the spills are beside the
/// resume point, so a directory that was being read into still serves what
/// it served.
///
/// A stop nothing can take up removes both, there being no reader for
/// either.
fn stopped(
    scratch: &Path,
    names: Chunks,
    abandoned: Abandoned,
) -> io::Result<Built> {
    if abandoned.kept > 0 {
        return Ok(Built::Stopped(abandoned));
    }
    names.abandon()?;
    let _ = std::fs::remove_dir_all(scratch);
    Ok(Built::Stopped(abandoned))
}

/// What each region offers the crown, read off its own spill, or [`None`]
/// where the run was asked to stop part way through the pass.
///
/// One region's systems in memory at a time: an offer is the counts a cell
/// above the region would take from it, not the systems themselves.
fn offers(
    formed: &Formed,
    params: &BuildParams,
    stop: &dyn Fn() -> bool,
) -> io::Result<Option<Vec<Offer>>> {
    let mut offered = Vec::with_capacity(formed.cut.regions().len());
    for &region in formed.cut.regions() {
        if stop() {
            return Ok(None);
        }
        let spilled = Spilled::open(&formed.spills[&region])?;
        offered.push(Offer::of(
            region,
            spilled.systems().iter().copied(),
            params,
        ));
    }
    Ok(Some(offered))
}

/// Where the systems of each region go while they are being read.
///
/// Beside the resume point rather than in the served directory: scratch,
/// the size of the galaxy, and no client may see them.
fn spill_dir(checkpoint: &Path) -> PathBuf {
    let mut name = checkpoint.as_os_str().to_owned();
    name.push(".regions");
    PathBuf::from(name)
}

/// What the names table came to, carried through to the report.
struct Pass {
    named: usize,
    chunks: usize,
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
    /// at [`MAX_LEVEL`](crate::geometry::MAX_LEVEL), or one whose systems
    /// share a position. Each was built with more than the budget's
    /// memory, so a build answering more than zero here is one that asked
    /// for more than it was given.
    pub over_budget: usize,
    /// Systems in the names table.
    pub named: usize,
    /// Chunks the names table is written in.
    pub chunks: usize,
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
            chunks: pass.chunks,
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
             deepest level {}, largest leaf {} systems, {} placed{}{}",
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
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::NameTable;
    use std::cell::Cell;
    use std::collections::{BTreeMap, HashSet};
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
                }
            })
            .collect()
    }

    /// The name a test galaxy carries for a system.
    fn entry(system: &System) -> NameEntry {
        NameEntry {
            address: system.id64 as i64,
            name: format!("Sys {}", system.id64),
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
        match build.finish(By::Database, None)? {
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

        let names = NameTable::read(dir).expect("the names table");
        assert_eq!(names.len(), systems.len());
    }

    /// A cold build writes the directory a whole build would have written,
    /// over a galaxy lumpy enough that a cut by count is not a cut by
    /// level.
    #[test]
    fn a_cold_build_is_the_whole_build() {
        let params = BuildParams::default();
        let systems = galaxy(40_000);
        let at = Scratch::new("cold");

        let report = built(&at, "cold", params, 6_000, &systems)
            .expect("a cold build");

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

        let report = built(&at, "lumpy", params, 6_000, &systems)
            .expect("a cold build");

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
        fn walk(
            root: &Path,
            at: &Path,
            into: &mut BTreeMap<PathBuf, Vec<u8>>,
        ) {
            for entry in std::fs::read_dir(at).expect("a directory") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    walk(root, &path, into);
                    continue;
                }
                let within =
                    path.strip_prefix(root).expect("inside the root");
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

        let stopped = build.finish(By::Database, None).expect("a build");
        let Built::Stopped(abandoned) = stopped else {
            panic!("it ran to its end: {:?}", stopped)
        };
        assert_eq!(
            abandoned,
            Abandoned {
                systems: 66_000,
                raised: 0,
                regions: 0,
                intact: true,
                kept: 0,
            },
        );
        assert_eq!(contents(&dir), before, "the directory was written to");
        assert_eq!(
            std::fs::read(&checkpoint).expect("a base"),
            resume_point,
            "the resume point was written over",
        );
        assert!(
            !spill_dir(&checkpoint).exists(),
            "the spills are still there",
        );
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
            NameTable::read(&dir).expect("the names table").len(),
            systems.len(),
        );
        assert!(dir.join(INDEX_FILE).exists(), "no index file was written");
    }

    /// A build stopped once it has begun raising says the directory is no
    /// longer as it found it, and takes the index file down with the
    /// payloads it replaced.
    ///
    /// The one step that cannot be undone. A cell's payload is written in
    /// place, so the index file that stood over the payloads a previous
    /// build wrote is no longer a tree over what is beneath it — and it is
    /// the file every reader and every resume starts from, so it goes and
    /// the next build writes both again. The scratch goes all the same.
    #[test]
    fn a_build_stopped_while_raising_takes_the_index_file_with_it() {
        let params = BuildParams { internal_slice: 8, leaf_cap: 32 };
        let at = Scratch::new("raising");
        let (dir, checkpoint) =
            (at.join("served"), at.join("served.checkpoint"));

        let published = galaxy(4_000);
        built(&at, "served", params, 500, &published)
            .expect("a published index");
        let resume_point = std::fs::read(&checkpoint).expect("a base");
        let table = NameTable::read(&dir).expect("the names table").len();

        // Asked to stop the moment the index file comes down, which is the
        // one observable the point of no return has.
        let systems = galaxy(3_000);
        let stop = || !dir.join(INDEX_FILE).exists();
        let mut build =
            Build::begin(&dir, &checkpoint, params, 500, Start::Fresh, &stop)
                .expect("a build");
        for &system in &systems {
            assert_eq!(
                build.push(system, entry(&system)).expect("a push"),
                Taking::More,
            );
        }

        let stopped = build.finish(By::Database, None).expect("a build");
        let Built::Stopped(abandoned) = stopped else {
            panic!("it ran to its end: {:?}", stopped)
        };
        assert!(!abandoned.intact, "{}", abandoned);
        assert_eq!(abandoned.systems, systems.len() as u64);
        assert_eq!(abandoned.raised, 0);
        assert!(abandoned.regions > 1, "{}", abandoned);
        assert!(
            !dir.join(INDEX_FILE).exists(),
            "an index file over payloads it does not stand for",
        );
        assert!(
            !spill_dir(&checkpoint).exists(),
            "the spills are still there",
        );
        assert!(
            !checkpoint.with_extension("tmp").exists(),
            "a base nothing will read is still there",
        );
        assert_eq!(
            std::fs::read(&checkpoint).expect("a base"),
            resume_point,
            "the resume point was written over",
        );
        // The table is published with the index file and never before it,
        // so the one that stood is still the one on disk.
        assert_eq!(
            NameTable::read(&dir).expect("the names table").len(),
            table,
        );

        // And the next build writes both again.
        built(&at, "served", params, 500, &systems).expect("a cold build");
        assert!(dir.join(INDEX_FILE).exists());
        assert_eq!(
            NameTable::read(&dir).expect("the names table").len(),
            systems.len(),
        );
    }

    /// A stopped read is taken up where it left off, and comes to the
    /// directory a build that was never stopped comes to
    ///
    /// The whole claim of [`Build::mark`], and the one worth a test: the
    /// spills a stop leaves stand for exactly the systems the mark names,
    /// so the second run reads on from there and neither loses a system nor
    /// spills one twice. A system in two cells is not a tree, and a system
    /// in none is a name the map cannot draw, so both failures are silent
    /// in the directory and loud here.
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
        // More than the 64Ki entries a names chunk holds, so the tail
        // chunk the mark stages is a chunk the resume reads back.
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
                    build.mark(took.to_string().as_bytes()).expect("a mark");
                    break;
                }
            }
        }
        let Built::Stopped(abandoned) =
            build.finish(By::Database, None).expect("a stopped build")
        else {
            panic!("it ran to its end")
        };
        assert_eq!(abandoned.kept, 40_000, "{abandoned}");
        assert!(
            spill_dir(&checkpoint).exists(),
            "the spills a resume reads were removed: {abandoned}",
        );

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
            build.finish(By::Database, None).expect("a build")
        else {
            panic!("the resumed build stopped")
        };
        assert_eq!(report.systems, systems.len(), "{report}");
        assert!(report.is_consistent(), "{report}");

        let whole = at.join("whole");
        let mine = contents(&dir);
        let theirs = contents(&whole);
        for (path, bytes) in &theirs {
            if path == Path::new(INDEX_FILE) {
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
}
