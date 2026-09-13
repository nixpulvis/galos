//! Everything read, written to an index directory.
//!
//! The DB-free half of the program. `galos_db::index` derives an index from
//! Postgres; this derives one from the events themselves, which is what a
//! commander following their own journal, or a session of EDDN, gets: a
//! directory the map reads with no server anywhere.
//!
//! Three things held together:
//!
//! - [`Galaxy`] — the events, accumulated into the index's vocabulary,
//!   shared with `galos_index::galaxy`.
//! - [`Tree`] — the cell tree, held open and edited. `Tree::upsert` touches
//!   the handful of cells on one system's path, so a scan costs the depth of
//!   the tree rather than its size. `galos_map`'s journal source rebuilds
//!   instead; this cannot, taking EDDN against a tree that for a resumed
//!   `.galos_index` is the galaxy.
//! - [`Tables`] — the metadata sidecars, resumed off the directory and
//!   patched per pass.
//!
//! ## The resume point
//!
//! [`Checkpoint`], the same file and format the database-side builder
//! writes: the served payload carries a downcast magnitude and a bucketed
//! temperature, so the editable tree cannot be rebuilt from the directory it
//! published.
//!
//! What differs is the cursor, which is what [`By`] records. With a database
//! under the run the cursor is a database clock, sampled *before* the batch
//! it stands for is applied — sound because the database sink wrote those
//! entries before this sink was handed them — so a restart's catch-up covers
//! exactly what the index missed. Without one the checkpoint carries `None`
//! and says [`By::Events`] wrote it. The two derivations refuse to resume
//! onto each other's work; see [`one_hand`].
//!
//! ## Where the scanned bodies live
//!
//! In `bodies/<address>.bin`, not in memory: a reach is the far edge over
//! every body of a system together and the file is written whole, so holding
//! them would mean [`Galaxy`] keeping every body the feed ever carried. See
//! `galos_index::bodies`. What is held is what has been scanned and not
//! yet written, which [`Sink::flush`] clears as it publishes.

use crate::sink::tables::{Tables, Wrote};
use crate::sink::{Reporter, Sink, SystemReport};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use elite_journal::entry::market::{BlackMarket, Market, Outfitting, Shipyard};
use elite_journal::entry::{Entry, Event};
use galos_db::Database;
use galos_index::{
    BuildParams, By, Checkpoint, Index as ServedIndex, Pending, System, Tree,
};
use galos_index::galaxy::UNKNOWN;
use galos_index::{Galaxy, Published};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

/// Where an index is written when `--index` names no directory.
pub const INDEX_DIR: &str = ".galos_index";

/// What a resume point is named beside the directory it resumes.
///
/// Derived from the directory, the file carrying the whole editable tree of
/// *that* directory; beside it rather than inside it, holding every system
/// at full precision, which no client should be served.
pub const CHECKPOINT_SUFFIX: &str = ".checkpoint";

/// Whether there is anything to edit a published directory from.
///
/// The one thing a run cannot repair. Everything a directory serves is a
/// lossy projection, so the full-precision inputs live in the resume point
/// and nowhere else, and a directory serving systems with no resume point
/// beside it can only be started over.
///
/// Everything else [`Index::open`] repairs, trimming a resume point and a
/// directory that disagree to what both hold.
fn agrees(
    dir: &Path,
    checkpoint: &Path,
    served: u64,
    resumable: usize,
) -> Result<(), String> {
    if served > 0 && resumable == 0 {
        return Err(format!(
            "{} already serves {served} systems and {} is missing or \
             unreadable, so there is no way to edit what it holds: delete \
             {} and {} to start the directory over, or write to a different \
             one with --index DIR",
            dir.display(),
            checkpoint.display(),
            dir.display(),
            checkpoint.display(),
        ));
    }
    Ok(())
}

/// Whether a resume point was written by the derivation now opening it.
///
/// A database-derived directory holds every system Postgres has and resumes
/// from its cursor; an event-derived one holds whatever a feed has said
/// since somebody started it and has no cursor. Opening one as the other
/// resumes at the wrong size and says nothing about it, so it is refused
/// with what to pass instead. A directory serving nothing has nothing to
/// lose, and the stale checkpoint is ignored.
fn one_hand(
    dir: &Path,
    checkpoint: &Path,
    served: u64,
    wrote: By,
    ours: By,
) -> Result<(), String> {
    if served == 0 || wrote == ours {
        return Ok(());
    }
    let (held, wanted) = match ours {
        By::Events => ("a database", "--db, to resume it as one"),
        By::Database => (
            "a feed",
            "--index DIR on a directory of its own, or delete both to \
             build from the database",
        ),
    };
    Err(format!(
        "{} was derived from {held} and this run would keep it current \
         the other way, which resumes at the wrong size without saying \
         so: pass {wanted} ({} says which wrote it)",
        dir.display(),
        checkpoint.display(),
    ))
}

/// What went wrong, said as the run's own failure.
///
/// A free function rather than a closure per call site: the closure has to
/// outlive the borrow of the message it carries.
fn failed(what: &'static str) -> impl Fn(std::io::Error) -> String {
    move |err| format!("{what}: {err}")
}

/// A [`Sink`] onto an index directory.
pub struct Index {
    dir: PathBuf,
    checkpoint: PathBuf,
    galaxy: Galaxy,
    tree: Tree,
    tables: Tables,
    /// Systems changed since the last [`Sink::flush`].
    touched: HashSet<i64>,
    /// Entries and rows taken, for the line at the end of a run.
    took: u64,
    /// Systems written into the tree, counted once per pass each moved in.
    published: u64,
    /// The database this run is also writing, where there is one.
    ///
    /// Held for one thing: the clock a resume point's cursor is, sampled
    /// once per publish. Nothing is ever read back out of it.
    db: Option<Database>,
}

impl Index {
    /// A sink onto `dir`, resuming whatever it already publishes.
    ///
    /// A directory that is not there is written from nothing, which is the
    /// ordinary first use. `db` decides two things: whether a resume point
    /// written here carries a cursor, and which derivation this is for
    /// [`one_hand`].
    pub fn open(
        dir: &Path,
        checkpoint: &Path,
        db: Option<Database>,
    ) -> Result<Index, String> {
        // The layout first, before anything reads or writes a file: a
        // directory published before `bodies/` and `cells/` were sharded
        // still holds the flat files.
        match galos_index::source::reshard_bodies(dir) {
            Ok(0) => {}
            Ok(moved) => info!(
                files = moved,
                dir = %dir.display(),
                "moved the body files into their shards"
            ),
            Err(err) => return Err(format!("{}: {err}", dir.display())),
        }
        match galos_index::store::reshard_cells(dir) {
            Ok(0) => {}
            Ok(moved) => info!(
                files = moved,
                dir = %dir.display(),
                "moved the cell payloads into their shards"
            ),
            Err(err) => return Err(format!("{}: {err}", dir.display())),
        }

        // What the directory currently serves, which is what the resume
        // point has to agree with. A directory that is not there serves
        // nothing; anything else that stopped the read is a directory this
        // must not publish over.
        let served = match ServedIndex::read(dir) {
            Ok(index) => index.root().map_or(0, |root| root.aggregate.count()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
            Err(err) => return Err(format!("{}: {err}", dir.display())),
        };

        let ours = by(&db);
        let resumed = match Checkpoint::read(checkpoint) {
            Ok(it) => {
                one_hand(dir, checkpoint, served, it.by, ours)?;
                (it.by == ours).then_some(it)
            }
            Err(_) => None,
        };
        // What the resume point stood at, kept for the repair below: a trim
        // applies no events, so the moment it was current as of stands.
        let resumed_at = resumed.as_ref().and_then(|it| it.cursor);
        // The base built in one batch and the log applied over it, later
        // winning, which is the order it was published in.
        let (base, deltas) = resumed
            .as_ref()
            .map_or((&[][..], &[][..]), |it| (it.base(), it.deltas()));
        let replayed = deltas.len();
        let mut tree = Tree::build(base, &BuildParams::default());
        tree.apply(deltas);

        let mut tables = Tables::resume(dir)
            .map_err(|err| format!("{}: {err}", dir.display()))?;

        agrees(dir, checkpoint, served, tree.len())?;

        // The two halves of the directory, trimmed to what they agree on. A
        // system the names table has no row for cannot be asked about; a
        // name whose system is not in the tree is never drawn. Both are what
        // a run killed mid-publish leaves, and both come back from the feed.
        let named = tables.named();
        let nameless: Vec<u64> = tree
            .inputs()
            .filter(|system| !named.contains(&(system.id64 as i64)))
            .map(|system| system.id64)
            .collect();
        let unnamed = nameless.len();
        for id64 in nameless {
            tree.forget(id64);
        }
        let drawn: HashSet<i64> =
            tree.inputs().map(|system| system.id64 as i64).collect();
        let orphaned = tables.forget_names(&drawn);

        // A directory whose halves had to be trimmed, or which serves what
        // no resume point can edit, is written whole here: the run that
        // reads it next is entitled to find both halves standing for the
        // same systems.
        if unnamed > 0 || orphaned > 0 || served != tree.len() as u64 {
            warn!(
                systems = tree.len(),
                served = served,
                unnamed = unnamed,
                orphaned = orphaned,
                dir = %dir.display(),
                "the directory's halves stood for different systems; \
                 trimmed to what both hold",
            );
            tree.write(dir)
                .map_err(failed("the cell tree could not be repaired"))?;
            tables
                .write(dir, Wrote::EVERYTHING)
                .map_err(failed("the metadata could not be repaired"))?;
            Checkpoint::compact(checkpoint, resumed_at, ours, tree.inputs())
                .map_err(failed("the resume point could not be repaired"))?;
        }

        if tree.len() > 0 {
            info!(
                systems = tree.len(),
                names = tables.names(),
                served = served,
                replayed = replayed,
                checkpoint = %checkpoint.display(),
                "resumed the index",
            );
        } else {
            info!(dir = %dir.display(), "writing a new index");
        }

        Ok(Index {
            dir: dir.to_owned(),
            checkpoint: checkpoint.to_owned(),
            // The published body files are the store, not a second copy in
            // memory beside them; see `galos_index::bodies`.
            galaxy: Galaxy::keeping(
                chrono::Utc::now(),
                Box::new(Published::new(dir)),
            ),
            tree,
            tables,
            touched: HashSet::new(),
            took: 0,
            published: 0,
            db,
        })
    }

    /// Where the resume point of an index in `dir` goes.
    ///
    /// Whatever `--checkpoint` named, and `<dir>.checkpoint` otherwise — see
    /// [`CHECKPOINT_SUFFIX`]. Asked before a sink exists, so it is an
    /// associated function.
    pub fn checkpoint(dir: &Path, named: Option<&Path>) -> PathBuf {
        if let Some(named) = named {
            return named.to_owned();
        }
        // Appended to the directory's own name rather than through
        // `set_extension`, which would replace the one `--index galos.d`
        // carries and have two directories sharing a file.
        let mut name = dir.file_name().unwrap_or_default().to_owned();
        name.push(CHECKPOINT_SUFFIX);
        dir.with_file_name(name)
    }

    /// Note that the accumulator has moved, and which systems.
    ///
    /// The galaxy tracks what it touched; this takes it and clears it, so a
    /// flush sees everything since the last flush rather than the last entry.
    fn took(&mut self) {
        self.took += 1;
        self.touched.extend(self.galaxy.touched().iter().copied());
        self.galaxy.settle();
    }

    /// What a resume point written now would resume from.
    ///
    /// [`None`] where there is no database, an event-derived directory
    /// having nothing to catch up from. Where there is one, the clock is the
    /// *database's* and is read before the batch this pass is about to apply
    /// — see the module header. A sample that fails answers the error and
    /// the publish records no cursor rather than a wrong one.
    async fn cursor(&self) -> Result<Option<NaiveDateTime>, String> {
        match &self.db {
            None => Ok(None),
            Some(db) => match db.now().await {
                Ok(now) => Ok(Some(now.naive_utc())),
                Err(err) => Err(format!("{err}")),
            },
        }
    }

    /// Record what this publish put in the directory, saying so where it
    /// could not be.
    ///
    /// A frame on the log, which costs what moved — and the whole base
    /// behind it where the log has outgrown it, which is what
    /// [`Pending::append`] answers. Without the frame a restart rebuilds the
    /// tree short of what the directory serves and publishes the shortfall
    /// over it. Not fatal: the directory is published regardless.
    fn record(&mut self, cursor: Option<NaiveDateTime>, moved: &[System]) {
        let folding =
            match Pending::append(&self.checkpoint, cursor, moved) {
                Ok(folding) => folding,
                Err(err) => {
                    warn!(
                        file = %Pending::path(&self.checkpoint).display(),
                        error = %err,
                        "what this publish wrote could not be logged; a \
                         restart would not see it",
                    );
                    return;
                }
            };
        if !folding {
            return;
        }
        if let Err(err) = Checkpoint::compact(
            &self.checkpoint,
            cursor,
            by(&self.db),
            self.tree.inputs(),
        ) {
            warn!(
                file = %self.checkpoint.display(),
                error = %err,
                "the resume point could not be compacted; the log beside it \
                 stands and keeps growing",
            );
        }
    }
}

/// Which derivation a run with this database under it is.
///
/// With a database, a resume point carries a cursor and may be resumed by a
/// catch-up; without one it carries nothing and may not.
fn by(db: &Option<Database>) -> By {
    match db {
        Some(_) => By::Database,
        None => By::Events,
    }
}

#[async_trait]
impl Sink for Index {
    /// Filed under the commander the *source* named, and under nobody where
    /// it could not name one.
    ///
    /// The galaxy tracks a commander from `Commander` and `LoadGame` events,
    /// which is not enough here: one worker takes every source and EDDN
    /// carries neither. An uploader id is an anonymised sender, and would
    /// say the map knows who scanned a body.
    async fn entry(&mut self, entry: Arc<Entry<Event>>, by: Reporter<'_>) {
        self.galaxy.flying(by.commander().unwrap_or(UNKNOWN));
        self.galaxy.read(&entry);
        self.took();
    }

    /// Everything anything has said about a system, merged into the galaxy.
    ///
    /// A report and not a row, so there is nothing to refuse: a system is a
    /// record in a map keyed by address, brought into being by whatever
    /// names it first. A name is kept anyway, a system with no name being
    /// published by neither derivation.
    ///
    /// `user` is dropped: the galaxy files what it holds under whoever the
    /// journal's own `Commander` event named.
    async fn system(&mut self, report: &SystemReport, _user: &str) {
        self.galaxy.hear(report.clone());
        self.took();
    }

    /// Nothing. A market is a station's stock and the index has no station in
    /// it. These four are the database's to keep.
    async fn market(&mut self, _at: DateTime<Utc>, _user: &str, _: &Market) {}
    async fn outfitting(
        &mut self,
        _at: DateTime<Utc>,
        _user: &str,
        _: &Outfitting,
    ) {
    }
    async fn shipyard(
        &mut self,
        _at: DateTime<Utc>,
        _user: &str,
        _: &Shipyard,
    ) {
    }
    async fn black_market(
        &mut self,
        _at: DateTime<Utc>,
        _user: &str,
        _: &BlackMarket,
    ) {
    }

    /// Move what has been read into the tree, and publish what that changed.
    ///
    /// The systems touched since the last flush and no others: each is asked
    /// of the galaxy for its photometry and place, upserted into the tree,
    /// and the tables patched to match. `Tree::publish` then writes the index
    /// file and exactly the cells whose payloads differ.
    ///
    /// A pass that touched nothing publishes and records nothing, which is
    /// what lets a follower call this on every beat.
    async fn flush(&mut self) -> Result<(), String> {
        // Recency is against now, not whenever the process started: a run
        // left up for a week would date every system by a week-old clock.
        self.galaxy.dated(Utc::now());

        // Sampled before a single one of this pass's systems reaches the
        // tree, never after the publish. Every entry the batch below holds
        // was in Postgres before this sink was handed it; a clock read after
        // the publish would cover rows this index has never seen.
        let resumable = match self.cursor().await {
            Ok(cursor) => cursor,
            Err(err) => {
                warn!(
                    error = %err,
                    "the database clock would not be read; this publish \
                     records no cursor and the last one stands",
                );
                None
            }
        };
        // What the directory can name as well as draw. `Galaxy::name_of`
        // answers nothing for a system nothing named -- a nav beacon carries
        // a place and an optional name -- and a cell tree standing over a
        // system the names table has no row for is what [`agrees`] refuses
        // to reopen a directory over.
        let touched: HashSet<i64> = std::mem::take(&mut self.touched)
            .into_iter()
            .filter(|&address| self.galaxy.name_of(address).is_some())
            .collect();
        if touched.is_empty() {
            return Ok(());
        }
        let start = Instant::now();

        // Only the systems that have been placed. One named by an event that
        // carried no `StarPos` joins the tree when something places it.
        let mut moving = Vec::with_capacity(touched.len());
        for &address in &touched {
            if let Some(system) = self.galaxy.system_of(address) {
                self.tree.upsert(system);
                moving.push(system);
            }
        }
        let placed = moving.len();

        self.tree
            .publish(&self.dir)
            .map_err(failed("the cell tree could not be published"))?;
        // The body files first: the store has been holding what the pass
        // scanned, and the tables' reaches are read back through it.
        let bodies = self
            .galaxy
            .settle_bodies()
            .map_err(failed("the body files could not be written"))?;
        let moved = self
            .tables
            .patch(&self.galaxy, &touched)
            .map_err(failed("the metadata could not be published"))?;
        let wrote = self
            .tables
            .write(&self.dir, moved)
            .map_err(failed("the metadata could not be published"))?;

        // What this publish put in the directory, at full precision, on the
        // log beside the resume point — and the whole base behind it where
        // the log has outgrown one. See [`Pending`].
        self.record(resumable, &moving);

        self.published += placed as u64;
        info!(
            touched = touched.len(),
            placed = placed,
            systems = self.tree.len(),
            chunks = wrote.name_chunks,
            bodies = bodies,
            cursor = resumable.is_some(),
            elapsed = ?start.elapsed(),
            // Which directory, for a log that carries the collect side's
            // lines as well.
            dir = %self.dir.display(),
            "index published",
        );
        Ok(())
    }

    /// Every part of the directory, written whole. See
    /// [`publish_whole`](Index::publish_whole).
    ///
    /// The cursor is sampled in the same order [`Sink::flush`] samples one.
    async fn finish(&mut self) -> Result<(), String> {
        let cursor = self.cursor().await.unwrap_or_else(|err| {
            warn!(
                error = %err,
                "the database clock would not be read; the directory is \
                 written whole and its resume point carries no cursor, so \
                 the next catch-up rebuilds",
            );
            None
        });
        self.publish_whole(cursor)
    }

    fn said(&self) -> String {
        format!(
            "{} messages read, {} system writes to {} ({} systems in the \
             tree)",
            self.took,
            self.published,
            self.dir.display(),
            self.tree.len(),
        )
    }
}

impl Index {
    /// Write every part of the directory whole, whatever has changed.
    ///
    /// What a one-shot run finishes with. [`Sink::flush`] writes only what a
    /// pass moved, which is wrong for a directory written from nothing: the
    /// factions table and the three whole-file tables would never be written
    /// if nothing in them happened to change during the run.
    ///
    /// `cursor` is the caller's, reading it being a query where this is not
    /// async. A test writing a directory by hand passes [`None`].
    pub fn publish_whole(
        &mut self,
        cursor: Option<NaiveDateTime>,
    ) -> Result<(), String> {
        // Against now, as [`Sink::flush`] dates its own pass: an hour-long
        // import would otherwise file every system by its starting clock.
        self.galaxy.dated(Utc::now());

        // Everything the galaxy has placed and named, whether or not a flush
        // has taken it: a whole publish is not a delta. The name is asked
        // for the reason [`Sink::flush`] asks it — a tree standing over a
        // system the names table has no row for will not reopen.
        let placed: Vec<_> = self
            .galaxy
            .systems()
            .into_iter()
            .filter(|system| self.galaxy.name_of(system.id64 as i64).is_some())
            .collect();
        let all: HashSet<i64> =
            placed.iter().map(|system| system.id64 as i64).collect();
        for system in placed {
            self.tree.upsert(system);
        }
        self.touched.clear();
        self.galaxy.settle();

        let bodies = self
            .galaxy
            .settle_bodies()
            .map_err(failed("the body files could not be written"))?;
        self.tree
            .write(&self.dir)
            .map_err(failed("the cell tree could not be written"))?;
        let _ = self
            .tables
            .patch(&self.galaxy, &all)
            .map_err(failed("the metadata could not be written"))?;
        let wrote = self
            .tables
            .write(&self.dir, Wrote::EVERYTHING)
            .map_err(failed("the metadata could not be written"))?;
        self.published = self.tree.len() as u64;

        // A whole directory is written, so a whole resume point goes with
        // it: the log is superseded and dropped by the compaction.
        Checkpoint::compact(
            &self.checkpoint,
            cursor,
            by(&self.db),
            self.tree.inputs(),
        )
        .map_err(failed("the resume point could not be written"))?;

        info!(
            systems = self.tree.len(),
            chunks = wrote.name_chunks,
            bodies = bodies,
            dir = %self.dir.display(),
            "index written",
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elite_journal::entry::Entry;
    use elite_journal::system::Coordinate;
    use galos_index::{FsSource, Source as _};
    use std::path::PathBuf;

    /// A scratch pair of paths of this test's own: a directory and a resume
    /// file beside it.
    fn scratch(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir()
            .join(format!("galos_sync_index_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");
        (root.join("index"), root.join("checkpoint"))
    }

    fn jump(system: &str, address: i64, at: [f64; 3]) -> Arc<Entry<Event>> {
        let json = format!(
            r#"{{"timestamp":"2026-08-08T12:00:00Z","event":"FSDJump","StarSystem":"{}","SystemAddress":{},"StarPos":[{},{},{}]}}"#,
            system, address, at[0], at[1], at[2],
        );
        Arc::new(serde_json::from_str(&json).expect("the entry should parse"))
    }

    /// How many systems a published directory stands over, read back the way
    /// the map reads it.
    fn published(dir: &Path) -> u64 {
        let read = FsSource::new(dir);
        pollster::block_on(read.index())
            .expect("the index reads")
            .root()
            .map_or(0, |root| root.aggregate.count())
    }

    fn names(dir: &Path) -> Vec<String> {
        let read = FsSource::new(dir);
        let mut said: Vec<String> = pollster::block_on(read.names())
            .expect("the names read")
            .into_iter()
            .map(|it| it.name)
            .collect();
        said.sort();
        said
    }

    /// A jump that names who runs a system, as the game writes one.
    fn settled(system: &str, address: i64, at: [f64; 3]) -> Arc<Entry<Event>> {
        let json = format!(
            r#"{{"timestamp":"2026-08-08T12:00:00Z","event":"FSDJump",
                "StarSystem":"{}","SystemAddress":{},"StarPos":[{},{},{}],
                "Population":22780919531,"SystemAllegiance":"Federation",
                "SystemGovernment":"$government_Democracy;",
                "SystemSecurity":"$SYSTEM_SECURITY_high;",
                "SystemEconomy":"$economy_Refinery;"}}"#,
            system, address, at[0], at[1], at[2],
        );
        Arc::new(serde_json::from_str(&json).expect("the entry should parse"))
    }

    /// A route plotted through `system`, which says nothing about it but
    /// where it is and what burns in the middle of it.
    fn routed(system: &str, address: i64, at: [f64; 3]) -> Arc<Entry<Event>> {
        let json = format!(
            r#"{{"timestamp":"2026-08-08T12:05:00Z","event":"NavRoute",
                "Route":[{{"StarSystem":"{}","SystemAddress":{},
                "StarPos":[{},{},{}],"StarClass":"G"}}]}}"#,
            system, address, at[0], at[1], at[2],
        );
        Arc::new(serde_json::from_str(&json).expect("the entry should parse"))
    }

    /// What the directory says about who runs a system.
    fn politics(
        dir: &Path,
        address: i64,
    ) -> Option<galos_index::meta::PopulatedSystem> {
        let read = FsSource::new(dir);
        pollster::block_on(read.populated())
            .expect("the populated table reads")
            .into_iter()
            .find(|it| it.address == address)
    }

    /// A system named by a passing route keeps the politics it had
    ///
    /// A `NavRoute` says nothing about who lives in the systems it names, so
    /// the accumulator answers nothing for one. Withdrawing is for a system
    /// that has *stopped* being populated, which an arrival says and a route
    /// cannot.
    #[test]
    fn a_route_through_a_system_does_not_empty_it() {
        let (dir, checkpoint) = scratch("routed");
        let mut sink = Index::open(&dir, &checkpoint, None).expect("it opens");
        pollster::block_on(sink.entry(
            settled("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        sink.publish_whole(None).expect("the first run writes");
        drop(sink);
        let stood = politics(&dir, 10477373803).expect("Sol is populated");
        assert_eq!(stood.population, 22780919531);

        // A second run, which knows nothing of Sol until a route names it.
        // The politics are on disk and not in the accumulator.
        let mut sink = Index::open(&dir, &checkpoint, None).expect("resumed");
        pollster::block_on(sink.entry(
            routed("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        pollster::block_on(sink.flush()).expect("the publish lands");
        assert_eq!(
            politics(&dir, 10477373803),
            Some(stood),
            "a passing route took the politics off a populated system",
        );

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// An arrival keeps the faction ids a build gave the system
    ///
    /// A journal names factions and numbers none of them, so the event path
    /// publishes an empty faction list. Written over a published row that
    /// had them, that takes the map's faction colouring and filtering off
    /// every populated system a feed mentions.
    #[test]
    fn an_arrival_keeps_what_only_a_build_could_say() {
        let (dir, checkpoint) = scratch("thinned");
        let mut sink = Index::open(&dir, &checkpoint, None).expect("it opens");
        pollster::block_on(sink.entry(
            settled("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        sink.publish_whole(None).expect("the first run writes");
        drop(sink);

        // What a database build leaves behind and an event never can: the
        // faction ids, and the body counts an arrival does not carry.
        let mut table = pollster::block_on(FsSource::new(&dir).populated())
            .expect("the populated table reads");
        table[0].factions = vec![968, 1047];
        table[0].body_count = Some(9);
        galos_index::source::write_meta(
            &galos_index::source::populated_path(&dir),
            &table,
        )
        .expect("the richer table writes");

        let mut sink = Index::open(&dir, &checkpoint, None).expect("resumed");
        pollster::block_on(sink.entry(
            settled("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        pollster::block_on(sink.flush()).expect("the publish lands");

        let stood = politics(&dir, 10477373803).expect("Sol is populated");
        assert_eq!(
            stood.factions,
            vec![968, 1047],
            "an arrival took the faction ids off a published system",
        );
        assert_eq!(stood.body_count, Some(9), "and the body count with them");

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// Nor does an arrival that says nothing about it
    ///
    /// `Population` is absent from an arrival in an unpopulated system and
    /// `zero_is_none` turns a reported zero into the same absence, so this
    /// side cannot tell a system that has emptied from one nobody has
    /// mentioned. A row that should go is withdrawn by the derivation that
    /// can tell, reading `population > 0` off the row.
    #[test]
    fn an_arrival_that_says_nothing_leaves_the_politics_alone() {
        let (dir, checkpoint) = scratch("silent");
        let mut sink = Index::open(&dir, &checkpoint, None).expect("it opens");
        pollster::block_on(sink.entry(
            settled("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        sink.publish_whole(None).expect("the first run writes");
        drop(sink);
        let stood = politics(&dir, 10477373803).expect("Sol is populated");

        let mut sink = Index::open(&dir, &checkpoint, None).expect("resumed");
        pollster::block_on(sink.entry(
            jump("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        pollster::block_on(sink.flush()).expect("the publish lands");
        assert_eq!(
            politics(&dir, 10477373803),
            Some(stood),
            "a bare arrival took the politics off a populated system",
        );

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// One read fills every sink it was given
    ///
    /// `--to db --index DIR` is the invocation this is for; two index
    /// directories drive the same [`Fan`](crate::sink::Fan) without Postgres.
    #[test]
    fn a_fan_writes_every_sink_it_holds() {
        let (here, here_resume) = scratch("fanned_here");
        let (there, there_resume) = scratch("fanned_there");
        let mut fan = crate::sink::Fan::of(vec![
            Box::new(
                Index::open(&here, &here_resume, None).expect("one opens"),
            ),
            Box::new(
                Index::open(&there, &there_resume, None).expect("two opens"),
            ),
        ]);

        pollster::block_on(fan.entry(
            jump("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        pollster::block_on(fan.finish()).expect("both close out");

        assert_eq!(published(&here), 1, "the first directory was written");
        assert_eq!(published(&there), 1, "and so was the second");
        assert_eq!(names(&here), vec!["SOL".to_string()]);
        assert_eq!(names(&there), vec!["SOL".to_string()]);
        assert_eq!(
            fan.said_each().len(),
            2,
            "each sink says what it took, rather than one line for both",
        );

        for dir in [&here, &there] {
            let _ =
                std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
        }
    }

    /// A followed directory publishes the tables it has nothing for
    ///
    /// To a client a missing table means "this index cannot say" rather than
    /// "nowhere supercharges", and a flush is all a follower ever does, so a
    /// flush has to write them.
    #[test]
    fn a_flush_publishes_the_tables_the_directory_lacks() {
        let (dir, checkpoint) = scratch("sidecars");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");
        // A jump and nothing else: nothing populated, nothing scanned,
        // nothing supercharging.
        pollster::block_on(sink.entry(
            jump("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        pollster::block_on(sink.flush()).expect("the publish lands");

        let read = FsSource::new(&dir);
        assert_eq!(
            pollster::block_on(read.boosts()).expect("the table reads"),
            Some(Vec::new()),
            "a published empty table is an answer; a missing one is not",
        );
        assert!(
            pollster::block_on(read.populated()).is_ok(),
            "and the rest of the sidecars are there to be read",
        );

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// Events go in and a directory the map can open comes out
    ///
    /// Read back through [`FsSource`], the reader the map itself uses, so
    /// what is asked is whether the *directory* says it.
    #[test]
    fn events_become_a_readable_directory() {
        let (dir, checkpoint) = scratch("published");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");
        pollster::block_on(async {
            sink.entry(
                jump("Sol", 10477373803, [0.0; 3]),
                Reporter::Commander("cmdr"),
            )
            .await;
            sink.entry(
                jump("Alpha Centauri", 22, [3.0, 0.0, 3.0]),
                Reporter::Commander("cmdr"),
            )
            .await;
        });
        sink.publish_whole(None).expect("the index should be written");

        assert_eq!(published(&dir), 2);
        assert_eq!(names(&dir), vec!["ALPHA CENTAURI", "SOL"]);

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A second run onto the same directory adds to it rather than replacing
    /// it
    ///
    /// The resume point is how that works: the served payload carries a
    /// downcast magnitude and a bucketed temperature, so a sink that did not
    /// read the checkpoint back would publish the second run's systems as
    /// the entire galaxy.
    #[test]
    fn a_second_run_resumes_the_first() {
        let (dir, checkpoint) = scratch("resumed");

        let mut first =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");
        pollster::block_on(first.entry(
            jump("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        first.publish_whole(None).expect("the first run should write");
        assert_eq!(published(&dir), 1);

        let mut second =
            Index::open(&dir, &checkpoint, None).expect("it reopens");
        pollster::block_on(second.entry(
            jump("Alpha Centauri", 22, [3.0, 0.0, 3.0]),
            Reporter::Commander("cmdr"),
        ));
        second.publish_whole(None).expect("the second run should write");

        assert_eq!(published(&dir), 2, "the first run's system was dropped");
        assert_eq!(names(&dir), vec!["ALPHA CENTAURI", "SOL"]);

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A flush publishes what has arrived since the last one, and a quiet
    /// pass writes nothing
    ///
    /// What a follower calls on every beat. A flush that wrote regardless
    /// would rewrite the index file and every changed cell once a second for
    /// as long as the process ran.
    #[test]
    fn a_flush_publishes_only_what_arrived() {
        let (dir, checkpoint) = scratch("flushed");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");

        pollster::block_on(async {
            sink.entry(
                jump("Sol", 10477373803, [0.0; 3]),
                Reporter::Commander("cmdr"),
            )
            .await;
            sink.flush().await.expect("the first flush");
        });
        assert_eq!(published(&dir), 1);

        let index = dir.join("index.bin");
        let written = || {
            index
                .metadata()
                .expect("the index file")
                .modified()
                .expect("a modification time")
        };
        let before = written();
        pollster::block_on(sink.flush()).expect("a quiet flush");
        assert_eq!(
            written(),
            before,
            "a pass that read nothing republished the index",
        );

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A run killed after one flush leaves a directory that reopens
    ///
    /// A directory serving systems with no resume point beside it is one
    /// [`agrees`] refuses for good and nothing can re-derive, so the first
    /// flush of a run has to write one.
    #[test]
    fn a_run_killed_after_one_flush_reopens() {
        let (dir, checkpoint) = scratch("killed");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");
        pollster::block_on(async {
            sink.entry(
                jump("Sol", 10477373803, [0.0; 3]),
                Reporter::Commander("cmdr"),
            )
            .await;
            sink.flush().await.expect("the first flush");
        });
        drop(sink);

        assert_eq!(published(&dir), 1);
        if let Err(said) = Index::open(&dir, &checkpoint, None) {
            panic!("a directory flushed once was orphaned: {}", said);
        }

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A nav beacon that named nothing, which is a place and no name.
    fn beacon(address: i64, at: [f64; 3]) -> Arc<Entry<Event>> {
        let json = format!(
            r#"{{"timestamp":"2026-08-08T12:02:00Z","event":"NavBeaconScan",
            "SystemAddress":{},"StarPos":[{},{},{}],"NumBodies":5}}"#,
            address, at[0], at[1], at[2],
        );
        Arc::new(serde_json::from_str(&json).expect("the beacon should parse"))
    }

    /// A system nothing named is left out of the tree, not published nameless
    ///
    /// `NavBeaconScan` carries a place and an optional name, so a system can
    /// be placed and nameless, and `Galaxy::name_of` answers nothing for one.
    /// A tree standing over a system the names table has no row for is what
    /// [`agrees`] refuses to reopen a directory over, so the point waits
    /// until something names it.
    #[test]
    fn a_system_nothing_named_is_not_published() {
        let (dir, checkpoint) = scratch("nameless");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");
        pollster::block_on(async {
            sink.entry(
                jump("Sol", 10477373803, [0.0; 3]),
                Reporter::Commander("cmdr"),
            )
            .await;
            sink.entry(
                beacon(42, [1.0, 2.0, 3.0]),
                Reporter::Commander("cmdr"),
            )
            .await;
            sink.flush().await.expect("the flush");
        });
        sink.publish_whole(None).expect("the run should write");
        drop(sink);

        assert_eq!(published(&dir), 1, "a system with no name was published");
        assert_eq!(names(&dir), vec!["SOL"]);
        if let Err(said) = Index::open(&dir, &checkpoint, None) {
            panic!("the directory disagreed with itself: {}", said);
        }

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A reported system is placed, and one with no coordinates is not
    ///
    /// EDSM and EDDB hand over reports rather than events, and a report with
    /// nowhere to be is the one thing the tree cannot take. It is still held
    /// — whatever places it later merges onto it — and not published.
    #[test]
    fn a_reported_system_is_placed_where_it_has_a_place() {
        let (dir, checkpoint) = scratch("dumped");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");

        let report =
            |address: i64, name: &str, at: Option<Coordinate>| SystemReport {
                name: Some(name.to_string()),
                position: at,
                population: Some(1000),
                ..SystemReport::new(
                    address,
                    "2026-08-08T12:00:00Z".parse().expect("a moment"),
                )
            };
        pollster::block_on(async {
            sink.system(
                &report(
                    1,
                    "Somewhere",
                    Some(Coordinate { x: 10.0, y: 20.0, z: 30.0 }),
                ),
                "a test",
            )
            .await;
            sink.system(&report(2, "Nowhere", None), "a test").await;
        });
        sink.publish_whole(None).expect("the index should be written");

        assert_eq!(published(&dir), 1, "the placeless row was placed");
        assert_eq!(names(&dir), vec!["SOMEWHERE"]);

        // And its politics reached the populated table, which is what a dump
        // is mostly for.
        let read = FsSource::new(&dir);
        let populated =
            pollster::block_on(read.populated()).expect("the table reads");
        assert_eq!(populated.len(), 1);
        assert_eq!(populated[0].population, 1000);

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A scan of a star, as the game writes one.
    fn scan(address: i64, body: i16, class: &str) -> Arc<Entry<Event>> {
        let json = format!(
            r#"{{"timestamp":"2026-08-08T12:01:00Z","event":"Scan",
            "ScanType":"Detailed","StarSystem":"Sol","SystemAddress":{address},
            "StarPos":[0.0,0.0,0.0],"BodyName":"Sol {body}","BodyID":{body},
            "StarType":"{class}","Subclass":2,"StellarMass":1.0,
            "Radius":696000000.0,"AbsoluteMagnitude":4.83,"Age_MY":4600,
            "SurfaceTemperature":5778.0,"Luminosity":"V",
            "RotationPeriod":2000000.0,"AxialTilt":0.0,
            "DistanceFromArrivalLS":{body}.0,
            "WasDiscovered":true,"WasMapped":false}}"#
        );
        Arc::new(serde_json::from_str(&json).expect("the scan should parse"))
    }

    /// A scan after a flush joins what is already on the disk
    ///
    /// A system is scanned body by body over minutes and the file is written
    /// whole, so the second scan has to read back what the first wrote and
    /// add to it. Replacing instead would leave every system holding only
    /// whatever was scanned since the last publish, and the reach — the far
    /// edge over all of them — wrong with it.
    #[test]
    fn a_scan_after_a_flush_joins_what_is_on_the_disk() {
        let (dir, checkpoint) = scratch("merged");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");

        pollster::block_on(async {
            sink.entry(
                jump("Sol", 10477373803, [0.0; 3]),
                Reporter::Commander("cmdr"),
            )
            .await;
            sink.entry(scan(10477373803, 0, "G"), Reporter::Commander("cmdr"))
                .await;
            sink.flush().await.expect("the first flush");

            // Minutes later, the next body of the same system.
            sink.entry(scan(10477373803, 4, "M"), Reporter::Commander("cmdr"))
                .await;
            sink.flush().await.expect("the second flush");
        });

        let read = FsSource::new(&dir);
        let inside = pollster::block_on(read.bodies(10477373803))
            .expect("the body file reads");
        assert_eq!(
            inside.stars.len(),
            2,
            "the second scan replaced the file rather than joining it",
        );

        // And the reach is over both, which a replaced file would have got
        // wrong without the count ever looking wrong.
        let reaches =
            pollster::block_on(read.reaches()).expect("the table reads");
        let reach = reaches
            .iter()
            .find(|it| it.address == 10477373803)
            .expect("a scanned system should have a reach");
        assert!(reach.reach > 0.0);

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// Nothing is held once a flush has been asked for
    ///
    /// Asserted through the store rather than by measuring memory: what a
    /// feed grows by is the systems it has scanned and not yet written, and
    /// after a flush that is none of them.
    #[test]
    fn a_flush_leaves_no_bodies_held() {
        let (dir, checkpoint) = scratch("unheld");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");
        pollster::block_on(async {
            sink.entry(
                jump("Sol", 10477373803, [0.0; 3]),
                Reporter::Commander("cmdr"),
            )
            .await;
            for body in 0..8 {
                sink.entry(
                    scan(10477373803, body, "G"),
                    Reporter::Commander("cmdr"),
                )
                .await;
            }
            sink.flush().await.expect("the flush");
        });

        assert_eq!(
            sink.galaxy.settle_bodies().expect("a second settle"),
            0,
            "the store was still holding what it had already written",
        );
        // And what it let go of is still there to be read.
        assert_eq!(sink.galaxy.bodies(10477373803).stars.len(), 8);

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A directory whose resume point has gone is refused, not started over
    ///
    /// The one failure the format cannot notice: a cell tree of fifty
    /// systems beside a names table describing a galaxy reads as a valid
    /// directory and is not. This has nowhere to re-derive from, so it stops.
    #[test]
    fn a_directory_without_its_resume_point_is_refused() {
        let (dir, checkpoint) = scratch("orphaned");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");
        pollster::block_on(sink.entry(
            jump("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        sink.publish_whole(None).expect("the first run should write");
        drop(sink);

        std::fs::remove_file(&checkpoint).expect("the resume point goes");
        // `expect_err` wants the `Ok` side to be `Debug`, and a sink holding
        // a tree of the galaxy is not a thing to derive that on.
        let Err(said) = Index::open(&dir, &checkpoint, None) else {
            panic!("an orphaned directory should be refused")
        };
        assert!(
            said.contains("no way to edit"),
            "the refusal should say why: {}",
            said,
        );
        assert!(
            said.contains("--index DIR"),
            "the refusal should say what to do: {}",
            said,
        );

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// The gate itself, without a directory to build one
    #[test]
    fn a_directory_needs_something_to_edit_it_from() {
        let (dir, checkpoint) = (Path::new("d"), Path::new("c"));
        assert!(
            agrees(dir, checkpoint, 0, 0).is_ok(),
            "a first run has nothing to disagree with",
        );
        assert!(
            agrees(dir, checkpoint, 100, 100).is_ok(),
            "a directory that matches its resume point resumes",
        );
        assert!(
            agrees(dir, checkpoint, 100, 0).is_err(),
            "a directory serving systems with no resume point was accepted",
        );
    }

    /// Neither derivation resumes onto the other's directory
    ///
    /// A directory a catch-up wrote is the galaxy; one a feed wrote is
    /// whatever the feed has said since somebody started it. Opened as the
    /// other, the tree comes back the wrong size and the next publish writes
    /// that over the directory. A directory serving nothing has nothing to
    /// lose, and the stale resume point is ignored.
    #[test]
    fn neither_derivation_resumes_onto_the_other() {
        let (dir, checkpoint) = (Path::new("d"), Path::new("c"));
        assert!(
            one_hand(dir, checkpoint, 100, By::Database, By::Database).is_ok(),
            "a catch-up should resume what a catch-up wrote",
        );
        assert!(
            one_hand(dir, checkpoint, 0, By::Database, By::Events).is_ok(),
            "an empty directory has nothing to resume wrongly",
        );

        let Err(said) =
            one_hand(dir, checkpoint, 100, By::Database, By::Events)
        else {
            panic!("a database-derived directory was opened by the feed")
        };
        assert!(said.contains("--db"), "should say what to pass: {}", said);

        let Err(said) =
            one_hand(dir, checkpoint, 100, By::Events, By::Database)
        else {
            panic!("an event-derived directory was opened for a catch-up")
        };
        assert!(said.contains("--index"), "should say what to pass: {}", said,);
    }

    /// A publish after the last whole checkpoint survives a kill
    ///
    /// A publish appends a log frame rather than rewriting the base, so a
    /// restart that read the base alone would bring the tree back short of
    /// the names table beside it, write the shortfall over the directory,
    /// and leave nothing able to open it again. [`Pending`] is what closes
    /// that.
    #[test]
    fn a_publish_after_the_last_checkpoint_is_not_lost() {
        let (dir, checkpoint) = scratch("lagging");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");

        // The first flush of a run writes a resume point and clears the log.
        pollster::block_on(sink.entry(
            jump("Sol", 10477373803, [0.0; 3]),
            Reporter::Commander("cmdr"),
        ));
        pollster::block_on(sink.flush()).expect("the first publish lands");
        assert!(!Pending::path(&checkpoint).exists(), "a whole one clears it");

        // The second appends to the log rather than folding the base, so
        // what it publishes lives there until the next whole checkpoint.
        pollster::block_on(sink.entry(
            jump("Alpha Centauri", 3161824266978, [3.0, 0.0, 3.0]),
            Reporter::Commander("cmdr"),
        ));
        pollster::block_on(sink.flush()).expect("the second publish lands");
        assert_eq!(sink.tree.len(), 2, "both systems are in the tree");
        assert!(Pending::path(&checkpoint).exists(), "the log has the second");
        drop(sink);

        let reopened =
            Index::open(&dir, &checkpoint, None).expect("it reopens");
        assert_eq!(
            reopened.tree.len(),
            2,
            "the system published after the last checkpoint came back",
        );
        assert_eq!(
            reopened.tables.names(),
            2,
            "and the names table was not trimmed to make the halves agree",
        );

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A feed's scan is not filed under whoever is flying locally
    ///
    /// One index worker takes every source, so `--from eddn --from
    /// journal=DIR` hands this sink a commander's own scans and a stranger's
    /// down the same channel. A scan the source cannot name a commander for
    /// is filed under nobody.
    #[test]
    fn a_scan_nobody_can_be_named_for_is_filed_under_nobody() {
        let (dir, checkpoint) = scratch("attributed");
        let mut sink =
            Index::open(&dir, &checkpoint, None).expect("a sink opens");

        pollster::block_on(async {
            // The journal's own `Commander` event, which is what the galaxy
            // tracks a commander from when it is read on its own.
            let flying: Arc<Entry<Event>> = Arc::new(
                serde_json::from_str(
                    r#"{"timestamp":"2026-08-08T12:00:00Z",
                        "event":"Commander","Name":"cmdr","FID":"F1"}"#,
                )
                .expect("the commander should parse"),
            );
            sink.entry(flying, Reporter::Commander("cmdr")).await;
            sink.entry(
                jump("Sol", 10477373803, [0.0; 3]),
                Reporter::Commander("cmdr"),
            )
            .await;
            sink.entry(scan(10477373803, 1, "G"), Reporter::Commander("cmdr"))
                .await;
            // The same galaxy, told by a feed that names only an anonymised
            // sender. The source saying so is the whole of what keeps the
            // two apart.
            sink.entry(scan(10477373803, 2, "M"), Reporter::Nobody).await;
        });

        let inside = sink.galaxy.bodies(10477373803);
        let filed = |id: i16| {
            inside
                .stars
                .iter()
                .find(|star| star.id == id)
                .map(|star| star.updated_by.clone())
                .expect("the scan should be on record")
        };
        assert_eq!(filed(1), "cmdr", "the commander's own scan lost its name");
        assert_eq!(
            filed(2),
            "unknown",
            "a stranger's scan was filed under the commander flying",
        );

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }
}
