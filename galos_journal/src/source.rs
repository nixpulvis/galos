//! The journal as a transport: a tree raised in memory and served.
//!
//! [`galos_index::FsSource`] reads a directory somebody baked. This reads a
//! directory the *game* wrote, holds what it says in memory, and answers the
//! same trait. Between the two sits [`galos_index::Layered`], which is where
//! the toggle lives; nothing here knows it is being layered.
//!
//! ## Rebuilt, not edited
//!
//! `galos_db`'s watch holds a [`Tree`](galos_index::Tree) open and moves one
//! system at a time, because a full build over a hundred and twenty-nine
//! million systems is an hour and it has to publish every few seconds. A
//! commander's journal is thousands of systems. [`Snapshot::build`] over that
//! is milliseconds, so a change rebuilds the lot — no incremental insert, no
//! dirty set, no checkpoint, no resume. What that buys is that the tree is
//! never in a state a fresh build would not produce, by construction rather
//! than by an oracle test.
//!
//! It is also what makes [`Claimed`] cheap to obey. The layer below is read
//! once at startup and answers which systems it already carries; those are
//! left out of this tree so the two compose exactly (see
//! [`galos_index::layer`]). A claim answered later — the map reads its names
//! table seconds after the window opens — is a rebuild and not a
//! reconciliation.
//!
//! ## What a client sees
//!
//! One number: the generation, bumped whenever the journal moved or the claim
//! changed, and returned as the [`Stamp`] of every part this serves. So a
//! client polling stamps re-reads everything this has whenever anything in it
//! changed, which is right at this size and would not be at the galaxy's.

use crate::follow::Follower;
use crate::galaxy::Galaxy;
use async_trait::async_trait;
use galos_index::cache::Point;
use galos_index::geometry::CellId;
use galos_index::meta::{
    Faction, NameEntry, PopulatedSystem, SystemBodies, SystemBoost, SystemReach,
};
use galos_index::source::{Part, Source, Stamp};
use galos_index::walk::Index;
use galos_index::{BuildParams, Claimed, Snapshot};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, info, warn};

/// How often a watch goes back to the directory, unless told otherwise.
///
/// A second, which is the beat the game writes at: an arrival is one line and
/// a full system scan is a few dozen, and the map's own refresh poll is of the
/// same order. Faster buys nothing a player can see; slower is a jump that
/// shows up late on the map they are flying by.
pub const EVERY: Duration = Duration::from_secs(1);

/// The longest a stopping watch goes on sleeping before it notices.
///
/// The beat is slept in slices of this, so dropping a [`Watch`] — which joins
/// — costs at most one of them rather than one whole beat. A window closing
/// is the case: it joins this thread, and a beat set to a minute would
/// otherwise hold the window open for most of one.
const WAKE: Duration = Duration::from_millis(100);

/// The journal, as read, and the tree standing over it.
///
/// Behind one lock because the whole of it is replaced together: a rebuild
/// produces a tree and five tables that stand for the same set of systems, and
/// a reader holding half of one build and half of the next would draw a sky
/// neither pass produced. The reads are short and the rebuild is milliseconds.
#[derive(Debug)]
struct Held {
    galaxy: Galaxy,
    snapshot: Snapshot,
    names: Vec<NameEntry>,
    reaches: Vec<SystemReach>,
    boosts: Vec<SystemBoost>,
    populated: Vec<PopulatedSystem>,
    /// Bumped on every rebuild, and the [`Stamp`] of every part served.
    generation: u64,
    /// The [`Claimed`] generation this tree was built under, so a claim
    /// answered since is noticed on the next pass.
    claimed_at: u64,
}

impl Held {
    fn empty() -> Held {
        Held {
            galaxy: Galaxy::default(),
            snapshot: Snapshot::default(),
            names: Vec::new(),
            reaches: Vec::new(),
            boosts: Vec::new(),
            populated: Vec::new(),
            generation: 0,
            claimed_at: 0,
        }
    }

    /// Raise the tree and the tables from the galaxy as it now stands.
    ///
    /// The systems the layer below already carries are left out of the tree
    /// and kept in every table: that is the whole shape of the thing. For a
    /// system EDDN has, what this commander scanned is a better reading of it
    /// — a fresher reach, the bodies they mapped, the class they saw — and
    /// putting the system itself into a second tree over the first would draw
    /// the same star twice.
    ///
    /// Nothing the names table cannot name, either. A `NavBeaconScan` places
    /// a system without necessarily naming it (see
    /// [`Galaxy::name_of`](crate::Galaxy::name_of)), and a tree standing over
    /// one is a star the search cannot reach and a published directory whose
    /// two counts disagree, which `galos-sync` refuses to reopen. So a system
    /// nothing has named waits for whatever names it, exactly as a system
    /// nothing has placed waits for whatever places it.
    fn raise(&mut self, claimed: &Claimed) {
        let named = |address: i64| self.galaxy.name_of(address).is_some();
        let systems: Vec<_> = self
            .galaxy
            .systems()
            .into_iter()
            .filter(|system| {
                !claimed.holds(system.id64 as i64) && named(system.id64 as i64)
            })
            .collect();
        self.snapshot = Snapshot::build(&systems, &BuildParams::default());
        self.names = self.galaxy.names();
        self.reaches = self.galaxy.reaches();
        self.boosts = self.galaxy.boosts();
        self.populated = self.galaxy.populated();
        self.generation += 1;
        self.claimed_at = claimed.generation();
        self.galaxy.settle();
    }
}

/// A [`Source`] over a journal directory, read into memory.
///
/// Cloneable: a clone shares the one reading, so the watch thread and the
/// client hold the same journal. Nothing is read until [`Self::pass`] is
/// called or a [`Watch`] is started, so constructing one cannot fail and a
/// directory that is not there is not an error until somebody looks.
#[derive(Clone)]
pub struct JournalSource {
    dir: PathBuf,
    held: Arc<RwLock<Held>>,
    follower: Arc<RwLock<Follower>>,
    claimed: Claimed,
}

/// What one pass over the directory did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// Entries read out of the files.
    pub entries: usize,
    /// Entries that said something about the sky.
    pub kept: usize,
    /// Whole lines that were not entries at all.
    pub unread: usize,
    /// Whether the tree and the tables were raised again.
    pub rebuilt: bool,
}

impl JournalSource {
    /// A source over the journal directory at `dir`, claiming nothing.
    ///
    /// Nothing is claimed, which draws every system the journal names. Right
    /// for a source read on its own and wrong under a published index, where
    /// a system both sides carry would be counted twice; see
    /// [`Self::over`].
    pub fn new(dir: impl Into<PathBuf>) -> JournalSource {
        JournalSource::over(dir, Claimed::none())
    }

    /// A source over `dir`, leaving out whatever `claimed` says is already
    /// carried by the layer below.
    pub fn over(dir: impl Into<PathBuf>, claimed: Claimed) -> JournalSource {
        let dir = dir.into();
        JournalSource {
            follower: Arc::new(RwLock::new(Follower::new(&dir))),
            dir,
            held: Arc::new(RwLock::new(Held::empty())),
            claimed,
        }
    }

    /// The journal directory being read.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What the claim is answered by, for whoever fills it in.
    pub fn claimed(&self) -> Claimed {
        self.claimed.clone()
    }

    /// Which rebuild the reading is on: the [`Stamp`] every part serves.
    pub fn generation(&self) -> u64 {
        self.read().generation
    }

    /// How many systems the journal has named, drawn or not.
    pub fn len(&self) -> usize {
        self.read().galaxy.len()
    }

    /// Whether the journal has named nothing at all.
    pub fn is_empty(&self) -> bool {
        self.read().galaxy.is_empty()
    }

    /// Who the journal says is flying.
    pub fn commander(&self) -> String {
        self.read().galaxy.commander().to_string()
    }

    /// Read whatever has arrived and rebuild if anything did.
    ///
    /// The one operation. A watch is this on a timer, and a one-shot build is
    /// this once.
    ///
    /// Rebuilt when the journal said something *or* when the claim has been
    /// answered since the last build: a map that has just finished reading its
    /// names table has changed which of these systems are this layer's to
    /// draw, and nothing in the journal has to move for that to matter.
    pub fn pass(&self) -> io::Result<Pass> {
        let read = {
            let mut follower =
                self.follower.write().unwrap_or_else(|it| it.into_inner());
            follower.poll()?
        };

        let mut pass = Pass {
            entries: read.entries.len(),
            unread: read.unread,
            ..Pass::default()
        };
        if read.unread > 0 {
            warn!(
                lines = read.unread,
                dir = %self.dir.display(),
                "lines in the journal were not entries",
            );
        }

        let mut held = self.held.write().unwrap_or_else(|it| it.into_inner());
        held.galaxy.dated(chrono::Utc::now());
        for entry in &read.entries {
            if held.galaxy.read(entry) {
                pass.kept += 1;
            }
        }

        let claim_moved = held.claimed_at != self.claimed.generation();
        let said = !held.galaxy.touched().is_empty();
        // The first pass raises the tree even over an empty journal, so a
        // client asking for an index before anything has been flown is
        // answered with an empty one rather than with whatever `Snapshot`
        // defaults to.
        if said || claim_moved || held.generation == 0 {
            held.raise(&self.claimed);
            pass.rebuilt = true;
            debug!(
                entries = pass.entries,
                kept = pass.kept,
                systems = held.galaxy.len(),
                generation = held.generation,
                "journal rebuilt",
            );
        }
        Ok(pass)
    }

    /// Read the directory on a timer, on a thread of its own.
    ///
    /// A thread and not a task: this crate has no runtime and the client that
    /// holds it may have any. The reads are blocking, short and rare — one
    /// `stat` per journal file per beat, and bytes only where the game wrote
    /// some — so a thread asleep between them costs a stack.
    pub fn watch(&self, every: Duration) -> Watch {
        let source = self.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let dir = self.dir.clone();
        let handle = std::thread::Builder::new()
            .name("galos-journal".into())
            .spawn(move || {
                info!(dir = %dir.display(), "following the journal");
                while !stopping.load(Ordering::Relaxed) {
                    if let Err(err) = source.pass() {
                        warn!(
                            dir = %dir.display(),
                            error = %err,
                            "the journal could not be read",
                        );
                    }
                    // In slices, so that dropping the watch is the length of
                    // one slice and not of the beat. A map closing its window
                    // joins this thread, and a beat set to a minute would
                    // hold the window open for most of one.
                    let mut left = every;
                    while left > Duration::ZERO
                        && !stopping.load(Ordering::Relaxed)
                    {
                        let slice = left.min(WAKE);
                        std::thread::sleep(slice);
                        left -= slice;
                    }
                }
                debug!(dir = %dir.display(), "the journal watch stopped");
            })
            .expect("a thread to follow the journal on");
        Watch { stop, handle: Some(handle) }
    }

    /// The reading, for the accessors. Poisoning says a reader panicked
    /// mid-read, which says nothing about whether this one may look.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Held> {
        self.held.read().unwrap_or_else(|it| it.into_inner())
    }
}

/// A running [`JournalSource::watch`], stopped by dropping it.
///
/// Dropping joins, so a client that lets go of the watch is not left with a
/// thread reading a directory nobody is drawing.
pub struct Watch {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    /// Ask the watch to stop, without waiting for it.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[async_trait]
impl Source for JournalSource {
    async fn index(&self) -> io::Result<Index> {
        Ok(self.read().snapshot.index.clone())
    }

    async fn payload(&self, id: CellId) -> io::Result<Vec<Point>> {
        Ok(self.read().snapshot.payload(id).to_vec())
    }

    async fn populated(&self) -> io::Result<Vec<PopulatedSystem>> {
        Ok(self.read().populated.clone())
    }

    async fn names(&self) -> io::Result<Vec<NameEntry>> {
        Ok(self.read().names.clone())
    }

    /// The whole table as chunk zero, and nothing past it.
    ///
    /// The published table is chunked because it is a hundred megabytes and a
    /// publish moves one chunk of it. This one is a commander's own systems
    /// and is written and read whole, so the numbering the layout asks for is
    /// satisfied by having exactly one number in it.
    async fn names_chunk(&self, chunk: usize) -> io::Result<Vec<NameEntry>> {
        if chunk == 0 { self.names().await } else { Ok(Vec::new()) }
    }

    /// No factions, ever. A journal names them and numbers nothing, and the
    /// ids are the database's; see [`crate::galaxy`].
    async fn factions(&self) -> io::Result<Vec<Faction>> {
        Ok(Vec::new())
    }

    async fn reaches(&self) -> io::Result<Vec<SystemReach>> {
        Ok(self.read().reaches.clone())
    }

    /// The supercharging systems this commander has been to.
    ///
    /// Always a table and never [`None`]: a journal that has been read knows
    /// what it has scanned, and an empty table from it means "none of the
    /// systems in here" rather than "this index cannot say". Which is a
    /// different question from whether there is anything here to read at
    /// all — that is [`Self::stamp`], and an empty table has no stamp.
    async fn boosts(&self) -> io::Result<Option<Vec<SystemBoost>>> {
        Ok(Some(self.read().boosts.clone()))
    }

    async fn bodies(&self, address: i64) -> io::Result<SystemBodies> {
        Ok(self.read().galaxy.bodies(address))
    }

    /// The generation, for every part this actually holds something for.
    ///
    /// One number for the lot, since a rebuild rewrites the lot. [`None`]
    /// where this holds nothing for the part — a cell it owns no systems in,
    /// a names chunk past the only one, a table that came out empty — which
    /// is the trait's own meaning of "not there" and is what keeps a client
    /// from re-reading an absence forever.
    ///
    /// The empty tables are where that earns its keep.
    /// [`Layered`](galos_index::Layered) folds the two sides' stamps
    /// together, so a part this answers for moves the composed stamp on
    /// every rebuild — once a jump, once a scan — and has the client re-read
    /// and re-merge the whole of the published table it is layered over.
    /// Factions are empty for good (see [`Self::factions`]) and the other
    /// three are empty until the commander has flown somewhere that fills
    /// them: a journal of pure exploration never populates a system and a
    /// journal with nothing scanned reaches nowhere.
    ///
    /// The index is the exception that is always answered for. A build over
    /// an empty journal still raises a root, and a client reads that as an
    /// empty sky rather than as a layer that cannot say.
    async fn stamp(&self, part: Part) -> io::Result<Option<Stamp>> {
        let held = self.read();
        let here = match part {
            Part::Cell(id) => !held.snapshot.payload(id).is_empty(),
            Part::NamesChunk(chunk) => chunk == 0 && !held.names.is_empty(),
            Part::Index => true,
            Part::Populated => !held.populated.is_empty(),
            Part::Reaches => !held.reaches.is_empty(),
            Part::Boosts => !held.boosts.is_empty(),
            Part::Factions => false,
        };
        Ok(here.then_some(held.generation))
    }
}

impl JournalSource {
    /// Write what has been read as a plain index directory.
    ///
    /// The same layout `galos-sync db` publishes, so a journal can be looked
    /// at through `galos-index info`, or handed to the map on its own as
    /// `GALOS_INDEX_DIR` — which is the sky one commander has personally seen
    /// and nothing else, and is a useful thing to look at once.
    ///
    /// Whole every time. There is no diff to write: the tree is rebuilt from
    /// scratch on every change, so there is no record of which cells moved,
    /// and at this size writing the lot is the cheaper of the two anyway.
    pub fn publish(&self, dir: &Path) -> io::Result<()> {
        use galos_index::NameTable;
        use galos_index::source::{
            bodies_path, boosts_path, factions_path, populated_path,
            reaches_path, write_meta,
        };

        let held = self.read();
        held.snapshot.write(dir)?;
        write_meta(&populated_path(dir), &held.populated)?;
        write_meta(&reaches_path(dir), &held.reaches)?;
        write_meta(&boosts_path(dir), &held.boosts)?;
        write_meta(&factions_path(dir), &Vec::<Faction>::new())?;
        NameTable::from_entries(held.names.clone()).publish(dir)?;
        for address in held.galaxy.scanned() {
            write_meta(
                &bodies_path(dir, address),
                &held.galaxy.bodies(address),
            )?;
        }
        Ok(())
    }

    /// How many systems, names, reaches and so on the reading holds, for the
    /// command line to say and for a test to check.
    pub fn counts(&self) -> HashMap<&'static str, usize> {
        let held = self.read();
        HashMap::from([
            ("systems", held.galaxy.len()),
            (
                "drawn",
                held.snapshot
                    .index
                    .root()
                    .map_or(0, |root| root.aggregate.count() as usize),
            ),
            ("cells", held.snapshot.index.len()),
            ("names", held.names.len()),
            ("reaches", held.reaches.len()),
            ("boosts", held.boosts.len()),
            ("populated", held.populated.len()),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_index::FsSource;

    /// A scratch directory of this test's own, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "galos_journal_source_{name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A journal directory holding one log of `lines`.
    fn journal(name: &str, lines: &[String]) -> PathBuf {
        let dir = scratch(name);
        std::fs::write(
            dir.join("Journal.2026-08-08T120000.01.log"),
            lines.join("\n") + "\n",
        )
        .expect("a journal log");
        dir
    }

    fn jump(system: &str, address: i64, at: [f64; 3]) -> String {
        format!(
            r#"{{"timestamp":"2026-08-08T12:00:00Z","event":"FSDJump","StarSystem":"{}","SystemAddress":{},"StarPos":[{},{},{}]}}"#,
            system, address, at[0], at[1], at[2],
        )
    }

    /// A pass reads the directory, and a quiet directory is not rebuilt
    ///
    /// The generation is the [`Stamp`] of every part this serves, so a
    /// rebuild nothing asked for is every client re-reading everything this
    /// holds for nothing.
    #[test]
    fn a_quiet_journal_is_not_rebuilt() {
        let dir = journal("quiet", &[jump("Sol", 10477373803, [0.0; 3])]);
        let source = JournalSource::new(&dir);

        let first = source.pass().expect("a pass");
        assert!(first.rebuilt, "the first pass did not raise a tree");
        assert_eq!(first.kept, 1);
        let generation = source.generation();

        let second = source.pass().expect("a second pass");
        assert!(!second.rebuilt, "a quiet journal was rebuilt");
        assert_eq!(source.generation(), generation, "the stamp moved anyway");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An empty journal directory is an empty index, not a failure
    #[test]
    fn an_empty_journal_serves_an_empty_index() {
        let dir = scratch("empty");
        let source = JournalSource::new(&dir);
        assert!(source.pass().expect("a pass").rebuilt);
        assert!(source.is_empty());

        // Not an empty *index*: the build raises the root over no systems,
        // as a full build over an empty galaxy would. What is empty is what
        // it stands over, which is what a client reads off it.
        let index = pollster::block_on(source.index()).expect("an index");
        assert_eq!(index.root().map(|root| root.aggregate.count()), Some(0));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A claimed system is left out of the tree and kept in every table
    ///
    /// The whole of how the two layers stay disjoint. Left in the tree it
    /// would be the same star drawn twice and counted twice; left out of the
    /// tables the commander's own reach and their own scans would be thrown
    /// away, which is what they ran this for.
    #[test]
    fn a_claimed_system_is_named_but_not_drawn() {
        let dir = journal(
            "claimed",
            &[
                jump("Sol", 10477373803, [0.0, 0.0, 0.0]),
                jump("Alpha Centauri", 22, [3.03, -0.09, 3.16]),
            ],
        );
        let claimed = Claimed::none();
        let source = JournalSource::over(&dir, claimed.clone());
        source.pass().expect("a pass");

        let drawn = |source: &JournalSource| {
            pollster::block_on(source.index())
                .expect("an index")
                .root()
                .map_or(0, |root| root.aggregate.count())
        };
        assert_eq!(drawn(&source), 2, "both systems should start out drawn");

        // The layer below turns out to carry Sol, as the published index
        // does.
        claimed.set(|address| address == 10477373803);
        let pass = source.pass().expect("a pass after the claim");
        assert!(pass.rebuilt, "an answered claim did not rebuild the tree");
        assert_eq!(drawn(&source), 1, "the claimed system was drawn anyway");

        let names = pollster::block_on(source.names()).expect("the names");
        assert_eq!(names.len(), 2, "the claimed system lost its name");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What is read can be written as an index directory and read back
    ///
    /// The layout is the seam, so a journal written through it is a journal
    /// the map can be pointed at with nothing else running. Read back through
    /// [`FsSource`], which is the reader the map itself uses.
    #[test]
    fn a_journal_publishes_a_readable_index() {
        let dir = journal("published", &[jump("Sol", 10477373803, [0.0; 3])]);
        let out = scratch("published_out");
        let source = JournalSource::new(&dir);
        source.pass().expect("a pass");
        source.publish(&out).expect("the index should write");

        let read = FsSource::new(&out);
        let index = pollster::block_on(read.index()).expect("the index reads");
        assert_eq!(
            index.root().map(|root| root.aggregate.count()),
            Some(1),
            "the published tree lost its system",
        );
        let names = pollster::block_on(read.names()).expect("the names read");
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].name, "SOL");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&out);
    }

    /// A part this holds nothing for stamps as absent
    ///
    /// [`None`] is "not there", and a client compares it against the [`None`]
    /// it recorded at startup. A source answering `Some` for a cell it owns
    /// nothing in would have that client fetching an empty payload on every
    /// poll for the life of the session.
    #[test]
    fn an_absent_part_has_no_stamp() {
        let dir = journal("stamps", &[jump("Sol", 10477373803, [0.0; 3])]);
        let source = JournalSource::new(&dir);
        source.pass().expect("a pass");

        let stamp = |part| {
            pollster::block_on(source.stamp(part)).expect("a stamp answers")
        };
        assert!(stamp(Part::Index).is_some());
        assert!(stamp(Part::NamesChunk(0)).is_some());
        assert_eq!(stamp(Part::NamesChunk(1)), None, "a chunk past the end");
        assert_eq!(
            stamp(Part::Cell(CellId { level: 12, x: 5, y: 6, z: 7 })),
            None,
            "a cell this owns nothing in",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A table that came out empty stamps as absent, and a filled one does
    /// not
    ///
    /// [`Layered`](galos_index::Layered) folds the two sides' stamps, so a
    /// part this answers for moves the composed stamp on every rebuild —
    /// once a jump — and has the map re-read and re-merge the whole of the
    /// published table underneath. A journal that has scanned nothing,
    /// found no jet cone and named no populated system holds nothing for
    /// three of those tables, and never holds anything for the fourth.
    #[test]
    fn an_empty_table_has_no_stamp() {
        let dir = journal("tables", &[jump("Sol", 10477373803, [0.0; 3])]);
        let source = JournalSource::new(&dir);
        source.pass().expect("a pass");

        let stamp = |source: &JournalSource, part| {
            pollster::block_on(source.stamp(part)).expect("a stamp answers")
        };
        assert_eq!(
            stamp(&source, Part::Factions),
            None,
            "a journal numbers no faction and never will",
        );
        assert_eq!(stamp(&source, Part::Reaches), None, "nothing was scanned");
        assert_eq!(stamp(&source, Part::Boosts), None, "no star was scanned");
        assert_eq!(
            stamp(&source, Part::Populated),
            None,
            "an unpopulated system was published as a populated table",
        );

        // The same jump, with the column the game writes where anybody lives
        // in the system. That table is then held, so it is stamped.
        let lived_in = journal(
            "tables_populated",
            &[r#"{"timestamp":"2026-08-08T12:00:00Z","event":"FSDJump","StarSystem":"Sol","SystemAddress":10477373803,"StarPos":[0.0,0.0,0.0],"Population":22780919531}"#
                .to_owned()],
        );
        let source = JournalSource::new(&lived_in);
        source.pass().expect("a pass");
        assert!(
            stamp(&source, Part::Populated).is_some(),
            "a populated system was not stamped",
        );
        assert_eq!(stamp(&source, Part::Factions), None, "still no factions");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&lived_in);
    }

    /// A system nothing named is not drawn either
    ///
    /// The cell tree and the names table are two readings of one set of
    /// systems, and a published directory whose counts disagree is one
    /// `galos-sync` refuses to reopen. A nav beacon scan can place a system
    /// without naming it, which is the one event that can pull the two
    /// apart.
    #[test]
    fn a_nameless_system_is_not_drawn() {
        let dir = journal(
            "nameless",
            &[
                jump("Sol", 10477373803, [0.0; 3]),
                r#"{"timestamp":"2026-08-08T12:00:00Z","event":"NavBeaconScan","SystemAddress":42,"StarPos":[1.0,2.0,3.0],"NumBodies":5}"#
                    .to_owned(),
            ],
        );
        let source = JournalSource::new(&dir);
        source.pass().expect("a pass");
        assert_eq!(source.len(), 2, "the beacon's system was not recorded");

        let index = pollster::block_on(source.index()).expect("an index");
        let names = pollster::block_on(source.names()).expect("the names");
        assert_eq!(
            index.root().map(|root| root.aggregate.count()),
            Some(names.len() as u64),
            "the tree draws what the names table cannot name",
        );
        assert_eq!(names.len(), 1, "the nameless system was named anyway");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
