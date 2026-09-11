//! Everything read, written to an index directory.
//!
//! The DB-free half of the program. `galos_db::index` derives an index from
//! Postgres, which is the right thing when Postgres is what holds the
//! galaxy; this derives one from the events themselves, which is the right
//! thing when nobody wants to stand a database up at all. A commander
//! following their own journal, or a session of EDDN, gets a directory the
//! map reads with no server anywhere.
//!
//! It is three things held together:
//!
//! - [`Galaxy`] — the events, accumulated into the index's vocabulary. Shared
//!   with `galos_journal`, which is where the argument for each derivation
//!   lives and where the three things a journal cannot say are written down.
//! - [`Tree`] — the cell tree, held open and edited. `Tree::upsert` moves one
//!   system and touches the handful of cells on its path, so a scan arriving
//!   costs the depth of the tree rather than its size.
//! - [`Tables`] — the metadata sidecars, resumed off the directory and
//!   patched per pass.
//!
//! ## Why it is incremental where `galos_journal` is not
//!
//! `galos_journal::JournalSource` rebuilds the whole tree whenever the
//! journal moves, and says why: a commander's journal is thousands of systems
//! and a rebuild is milliseconds. This sink takes EDDN as well, where the
//! feed runs at thirty messages a second and the tree it is editing is
//! whatever has been published into the directory — which for a resumed
//! `.galos_index` is the galaxy. Rebuilding that per message is not a thing
//! that could work, so this holds the tree open and publishes deltas, exactly
//! as `galos-sync db --to index` does on the other side.
//!
//! ## The resume point
//!
//! [`Checkpoint`] — the same file and the same format the database-side
//! builder writes, because it is the same problem: the served payload carries
//! a downcast magnitude and a bucketed temperature, so the editable tree
//! cannot be rebuilt from the directory it published. What differs is the
//! cursor. The database's cursor is a database clock and is read back to ask
//! "what changed since"; a feed has no such thing to ask, so this writes the
//! moment of the pass and never reads it. It is kept in the record all the
//! same: a directory carrying a checkpoint whose cursor nothing reads is
//! better than two checkpoint formats.
//!
//! Written on a timer rather than per publish, for the reason stated in
//! `galos_db::index`: a checkpoint is every system at full precision, and
//! writing one each pass costs more than everything else a pass does put
//! together.
//!
//! ## Where the scanned bodies live
//!
//! Not here. A reach is the far edge over every body of a system together, so
//! one more scan means recomputing from all of them, and the published body
//! file is written whole for the same reason — which for a while meant
//! [`Galaxy`] holding every body the feed had ever carried. A `meta::Body` is
//! 376 bytes before its four strings, its parents and its materials, so that
//! was a process growing for as long as it ran.
//!
//! It keeps them in `bodies/<address>.bin` instead — the files it was writing
//! anyway, and the ones the map fetches when a click opens a system. See
//! `galos_journal::bodies`. What is in memory is what has been scanned and
//! not yet written, which [`Sink::flush`] clears on the same beat it
//! publishes on.
//!
//! Which leaves the tree and the names table, and those are what
//! `galos-sync db --to index` holds too. The two sides of this program cost
//! the same thing now; what differs is only where the bodies are read back
//! from, Postgres there and the directory here.

use crate::sink::tables::{Tables, Wrote};
use crate::sink::{Row, Sink};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use elite_journal::entry::market::{BlackMarket, Market, Outfitting, Shipyard};
use elite_journal::entry::{Entry, Event};
use elite_journal::system::Coordinate;
use galos_index::{
    BuildParams, Checkpoint, Index as ServedIndex, Pending, System, Tree,
};
use galos_journal::galaxy::Politics;
use galos_journal::{Galaxy, Published};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// How long a run goes between resume points, at most.
///
/// `galos_db::index`'s figure and its reasoning: a checkpoint is every system
/// at full precision, a hundred megabytes and more, so it rides a timer and
/// not a publish. What that gives up a restart pays back by replaying, and
/// applying an event twice lands exactly where applying it once did.
const CHECKPOINT_EVERY: Duration = Duration::from_secs(60);

/// Whether there is anything to edit a published directory from.
///
/// The one thing a run cannot repair. Everything a directory serves is a
/// lossy projection — a payload point carries a downcast magnitude and a
/// bucketed temperature — so the full-precision inputs live in the resume
/// point and nowhere else, and a directory serving systems with no resume
/// point beside it can only be started over. `galos_db::index` answers the
/// same question by rebuilding, which it can: every row is still in
/// Postgres.
///
/// Everything else *is* repairable and is repaired rather than refused.
/// A resume point short of the directory (a run killed between whole
/// checkpoints, which [`Pending`] now keeps from happening), one ahead of
/// it (a publish that did not land), or two halves of a directory standing
/// for different systems: [`Index::open`] trims them to what both hold and
/// writes the directory whole. What that costs is the systems the trim
/// dropped, which the feed reports again; what a refusal cost was the
/// directory.
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
             one with --to index=DIR",
            dir.display(),
            checkpoint.display(),
            dir.display(),
            checkpoint.display(),
        ));
    }
    Ok(())
}

/// What went wrong, said as the run's own failure.
///
/// A free function rather than a closure per call site: the closure has to
/// outlive the borrow of the message it carries, and a `&'static str` is what
/// every one of these actually passes.
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
    /// Systems written into the tree, counted once per pass each moved in:
    /// a feed reporting the same system twice is two writes.
    published: u64,
    /// When this run last wrote a resume point, and [`None`] until it has.
    checkpointed: Option<Instant>,
}

impl Index {
    /// A sink onto `dir`, resuming whatever it already publishes.
    ///
    /// A directory that is not there is one to be written from nothing, which
    /// is not an error: `galos-sync journal ~/… --to index=.galos_journal_index`
    /// on a machine that has never run this is the ordinary first use.
    pub fn open(dir: &Path, checkpoint: &Path) -> Result<Index, String> {
        let resumed = Checkpoint::read(checkpoint).ok();
        let mut held: HashMap<u64, System> = resumed
            .map(|it| it.inputs)
            .unwrap_or_default()
            .into_iter()
            .map(|system| (system.id64, system))
            .collect();
        // What was published after that checkpoint was written, which the
        // directory holds and the checkpoint does not. Later wins, as an
        // upsert does: these are appended in the order they were published.
        let replayed = Pending::read(checkpoint);
        for system in &replayed {
            held.insert(system.id64, *system);
        }

        let mut tables = Tables::resume(dir)
            .map_err(|err| format!("{}: {err}", dir.display()))?;

        // What the directory currently serves, which is what the resume
        // point has to agree with. A directory that is not there serves
        // nothing, which is the ordinary first run; anything else that
        // stopped the read is a directory this must not publish over.
        let served = match ServedIndex::read(dir) {
            Ok(index) => index.root().map_or(0, |root| root.aggregate.count()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
            Err(err) => return Err(format!("{}: {err}", dir.display())),
        };
        agrees(dir, checkpoint, served, held.len())?;

        // The two halves of the directory, trimmed to what they agree on. A
        // system the names table has no row for cannot be drawn and asked
        // about; a name whose system is not in the tree is a row the map
        // finds and never draws. Both are what a run killed mid-publish
        // leaves, and both are re-derivable from the feed, where the
        // directory is not.
        let named = tables.named();
        let unnamed = held.len();
        held.retain(|&id64, _| named.contains(&(id64 as i64)));
        let unnamed = unnamed - held.len();
        let drawn: HashSet<i64> = held.keys().map(|&id| id as i64).collect();
        let orphaned = tables.forget_names(&drawn);

        let inputs: Vec<System> = held.into_values().collect();
        let mut tree = Tree::build(&inputs, &BuildParams::default());

        // A directory whose halves had to be trimmed, or which serves what
        // no resume point can edit, is written whole here rather than left
        // for the first publish: the run that reads it next is entitled to
        // find the two halves standing for the same systems.
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
            let at = Checkpoint {
                cursor: Utc::now().naive_utc(),
                inputs: tree.to_inputs(),
            };
            at.write(checkpoint)
                .map_err(failed("the resume point could not be repaired"))?;
            let _ = Pending::clear(checkpoint);
        }

        if tree.len() > 0 {
            info!(
                systems = tree.len(),
                names = tables.names(),
                served = served,
                replayed = replayed.len(),
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
            // memory beside them. That is the whole of why a feed into an
            // index does not grow for as long as it runs; see
            // `galos_journal::bodies`.
            galaxy: Galaxy::keeping(
                chrono::Utc::now(),
                Box::new(Published::new(dir)),
            ),
            tree,
            tables,
            touched: HashSet::new(),
            took: 0,
            published: 0,
            checkpointed: None,
        })
    }

    /// Note that the accumulator has moved, and which systems.
    ///
    /// The galaxy tracks what it touched itself; this takes it and clears it,
    /// so a flush sees everything since the last one rather than since the
    /// last entry.
    fn took(&mut self) {
        self.took += 1;
        self.touched.extend(self.galaxy.touched().iter().copied());
        self.galaxy.settle();
    }

    /// Whether this pass owes a resume point.
    ///
    /// The first flush of a run always does. A run killed before it had
    /// written one leaves a directory serving systems with nothing to edit
    /// them from, which [`agrees`] then refuses for good — the same reason
    /// `galos_db::index` writes one the moment its initial build lands.
    /// After that it is the timer, for the reason [`CHECKPOINT_EVERY`]
    /// gives.
    fn owes_a_resume_point(&self) -> bool {
        self.checkpointed.map_or(true, |at| at.elapsed() >= CHECKPOINT_EVERY)
    }

    /// Write where the run has got to, saying so where it could not be.
    ///
    /// Not fatal. The directory is published either way; what a failed
    /// checkpoint costs is a restart that starts over, and taking the run
    /// down would cost the same and the rest of the session besides.
    ///
    /// The whole checkpoint holds everything [`Pending`] was carrying, so
    /// the log goes with it — and only where the checkpoint was written, or
    /// the next run would resume short of what the directory serves.
    fn resume_point(&mut self) {
        let at = Checkpoint {
            cursor: Utc::now().naive_utc(),
            inputs: self.tree.to_inputs(),
        };
        match at.write(&self.checkpoint) {
            Ok(()) => {
                if let Err(err) = Pending::clear(&self.checkpoint) {
                    warn!(
                        file = %Pending::path(&self.checkpoint).display(),
                        error = %err,
                        "the published log could not be cleared",
                    );
                }
            }
            Err(err) => warn!(
                file = %self.checkpoint.display(),
                error = %err,
                "the resume point could not be written",
            ),
        }
        self.checkpointed = Some(Instant::now());
    }
}

#[async_trait]
impl Sink for Index {
    async fn entry(&mut self, entry: &Entry<Event>, _user: &str) {
        // The commander is the galaxy's to track: it reads `Commander` and
        // `LoadGame` and files its scans under whoever the journal named. An
        // EDDN uploader id is not that — it is an anonymised sender, not a
        // commander — and putting one in `updated_by` would say the map knows
        // who scanned a body when it does not.
        self.galaxy.read(entry);
        self.took();
    }

    /// Nothing: an index has no keys and no rows to point at one.
    ///
    /// The reason this method exists at all is Postgres' foreign key onto
    /// `systems`, and the game writing events that name a system before the
    /// arrival that would have created it. Here a system is a record in a map
    /// keyed by address, brought into being by whatever mentions it first, so
    /// there is nothing to ensure and nothing that can be refused.
    ///
    /// `true`, since what the caller is asking is whether to go on.
    async fn ensure_system(
        &mut self,
        _at: DateTime<Utc>,
        _user: &str,
        _address: i64,
        _name: Option<&str>,
        _position: Option<Coordinate>,
        _why: &str,
    ) -> bool {
        true
    }

    async fn system(&mut self, row: &Row) {
        // A dump with no coordinates is a system the tree has nowhere to put.
        // The database keeps such a row for its name and its politics; this
        // has no row to keep.
        let Some(position) = row.position else {
            debug!(system = %row.name, "a dumped system with no position");
            return;
        };
        self.galaxy.place(
            row.updated_at,
            row.address,
            &row.name,
            position,
            Politics {
                population: row.population,
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
            },
        );
        self.took();
    }

    /// Nothing. A market is a station's stock and the index has no station
    /// in it: what a click opens is the bodies inside a system, and what the
    /// map draws is the sky. These four are the database's to keep.
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
    /// of the galaxy for its current photometry and place, upserted into the
    /// tree, and the tables are patched to match. `Tree::publish` then writes
    /// the index file and exactly the cells whose payloads differ.
    ///
    /// A pass that touched nothing publishes nothing at all, which is what
    /// lets a follower call this on every beat. What it still does is the
    /// resume point, since a run that has been quiet for an hour has got
    /// somewhere all the same and a run killed before its first one leaves
    /// a directory nothing can edit.
    async fn flush(&mut self) -> Result<(), String> {
        // Recency is against now, not against whenever the process started.
        // A `--watch` run left up for a week would otherwise be dating every
        // system it hears by a week-old clock.
        self.galaxy.dated(Utc::now());

        let resumable = self.owes_a_resume_point();
        // What the directory can name as well as draw. `Galaxy::name_of`
        // answers nothing for a system nothing named -- a nav beacon
        // carries a place and an optional name -- and a blank row is worse
        // than no row in a table the map searches. A cell tree standing
        // over a system the names table has no row for is the disagreement
        // [`agrees`] refuses to reopen a directory over, so a nameless
        // system waits for whatever names it, exactly as a placeless one
        // waits for whatever places it.
        let touched: HashSet<i64> = std::mem::take(&mut self.touched)
            .into_iter()
            .filter(|&address| self.galaxy.name_of(address).is_some())
            .collect();
        if touched.is_empty() {
            if resumable {
                self.resume_point();
            }
            return Ok(());
        }
        let start = Instant::now();

        // Only the systems that have been placed. One named by an event
        // that carried no `StarPos` is in the galaxy and not in the tree,
        // and will join it when something places it.
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

        // What this publish put in the directory, kept at full precision
        // beside the resume point until a whole one is written. Without it
        // a restart rebuilds the tree short of what the directory serves
        // and publishes the shortfall over it; see [`Pending`].
        if let Err(err) = Pending::append(&self.checkpoint, &moving) {
            warn!(
                file = %Pending::path(&self.checkpoint).display(),
                error = %err,
                "what this publish wrote could not be logged; a restart \
                 before the next resume point would not see it",
            );
        }

        // On a timer, not per publish: see [`CHECKPOINT_EVERY`].
        if resumable {
            self.resume_point();
        }

        self.published += placed as u64;
        info!(
            touched = touched.len(),
            placed = placed,
            systems = self.tree.len(),
            chunks = wrote.name_chunks,
            bodies = bodies,
            checkpointed = resumable,
            elapsed = ?start.elapsed(),
            // Which directory, since `--to` repeats and two index sinks
            // publish on the same beat.
            dir = %self.dir.display(),
            "index published",
        );
        Ok(())
    }

    /// Every part of the directory, written whole. See [`publish_whole`].
    ///
    /// [`publish_whole`]: Index::publish_whole
    async fn finish(&mut self) -> Result<(), String> {
        self.publish_whole()
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
    /// pass moved, which is right for a follower and wrong for a directory
    /// being written from nothing: the factions table and the three
    /// whole-file tables would never be written at all if nothing in them
    /// happened to change during the run.
    pub fn publish_whole(&mut self) -> Result<(), String> {
        // Against now, as [`Sink::flush`] dates its own pass: a run that
        // took an hour to import a journal directory would otherwise file
        // every system in it by the clock it started on.
        self.galaxy.dated(Utc::now());

        // Everything the galaxy has placed and named, whether or not a
        // flush has already taken it: a whole publish is not a delta. The
        // name is asked for the reason [`Sink::flush`] asks it -- a tree
        // standing over a system the names table has no row for is a
        // directory that will not reopen.
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

        let at = Checkpoint {
            cursor: Utc::now().naive_utc(),
            inputs: self.tree.to_inputs(),
        };
        at.write(&self.checkpoint)
            .map_err(failed("the resume point could not be written"))?;
        let _ = Pending::clear(&self.checkpoint);
        self.checkpointed = Some(Instant::now());

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

    fn jump(system: &str, address: i64, at: [f64; 3]) -> Entry<Event> {
        let json = format!(
            r#"{{"timestamp":"2026-08-08T12:00:00Z","event":"FSDJump","StarSystem":"{}","SystemAddress":{},"StarPos":[{},{},{}]}}"#,
            system, address, at[0], at[1], at[2],
        );
        serde_json::from_str(&json).expect("the entry should parse")
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

    /// One read fills every sink it was given
    ///
    /// `--to db --to index=DIR` is the invocation this is for; two index
    /// directories are the half of it a test can drive without Postgres,
    /// and they exercise the same [`Fan`](crate::sink::Fan).
    #[test]
    fn a_fan_writes_every_sink_it_holds() {
        let (here, here_resume) = scratch("fanned_here");
        let (there, there_resume) = scratch("fanned_there");
        let mut fan = crate::sink::Fan::of(vec![
            Box::new(Index::open(&here, &here_resume).expect("one opens")),
            Box::new(Index::open(&there, &there_resume).expect("two opens")),
        ]);

        pollster::block_on(
            fan.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr"),
        );
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
    /// Reported from a map opened on an EDDN sync: no supercharge table,
    /// so a route for a drive that takes a jet cone was refused. A table
    /// that never moved was never written, and to a client a missing one
    /// means "this index cannot say" rather than "nowhere supercharges".
    /// A flush is what a follower ever does, so a flush has to write them.
    #[test]
    fn a_flush_publishes_the_tables_the_directory_lacks() {
        let (dir, checkpoint) = scratch("sidecars");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");
        // A jump and nothing else: nothing populated, nothing scanned,
        // nothing supercharging.
        pollster::block_on(
            sink.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr"),
        );
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
    /// The whole of what this sink is for. Read back through [`FsSource`],
    /// which is the reader the map itself uses, rather than through the
    /// sink's own state — what is being asked is whether the *directory* says
    /// it, not whether the accumulator does.
    #[test]
    fn events_become_a_readable_directory() {
        let (dir, checkpoint) = scratch("published");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");
        pollster::block_on(async {
            sink.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr").await;
            sink.entry(&jump("Alpha Centauri", 22, [3.0, 0.0, 3.0]), "cmdr")
                .await;
        });
        sink.publish_whole().expect("the index should be written");

        assert_eq!(published(&dir), 2);
        assert_eq!(names(&dir), vec!["ALPHA CENTAURI", "SOL"]);

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A second run onto the same directory adds to it rather than replacing
    /// it
    ///
    /// The resume point is the whole of how that works: the served payload
    /// carries a downcast magnitude and a bucketed temperature, so the
    /// editable tree cannot be rebuilt from the directory it published. A
    /// sink that did not read the checkpoint back would publish the second
    /// run's systems as the entire galaxy.
    #[test]
    fn a_second_run_resumes_the_first() {
        let (dir, checkpoint) = scratch("resumed");

        let mut first = Index::open(&dir, &checkpoint).expect("a sink opens");
        pollster::block_on(
            first.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr"),
        );
        first.publish_whole().expect("the first run should write");
        assert_eq!(published(&dir), 1);

        let mut second = Index::open(&dir, &checkpoint).expect("it reopens");
        pollster::block_on(
            second.entry(&jump("Alpha Centauri", 22, [3.0, 0.0, 3.0]), "cmdr"),
        );
        second.publish_whole().expect("the second run should write");

        assert_eq!(published(&dir), 2, "the first run's system was dropped");
        assert_eq!(names(&dir), vec!["ALPHA CENTAURI", "SOL"]);

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A flush publishes what has arrived since the last one, and a quiet
    /// pass writes nothing
    ///
    /// What a follower calls on every beat. A flush that wrote regardless
    /// would rewrite the index file and every changed cell once a second for
    /// as long as the process ran, whatever the feed was doing.
    #[test]
    fn a_flush_publishes_only_what_arrived() {
        let (dir, checkpoint) = scratch("flushed");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");

        pollster::block_on(async {
            sink.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr").await;
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
    /// A `--watch` run publishes on every beat and checkpoints on a timer,
    /// so for the first minute of it the directory serves systems the
    /// resume point does not know about. Killed there with no checkpoint at
    /// all, that directory is one [`agrees`] refuses for good and nothing
    /// can re-derive: the first flush of a run has to write one.
    #[test]
    fn a_run_killed_after_one_flush_reopens() {
        let (dir, checkpoint) = scratch("killed");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");
        pollster::block_on(async {
            sink.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr").await;
            sink.flush().await.expect("the first flush");
        });
        drop(sink);

        assert_eq!(published(&dir), 1);
        if let Err(said) = Index::open(&dir, &checkpoint) {
            panic!("a directory flushed once was orphaned: {}", said);
        }

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A nav beacon that named nothing, which is a place and no name.
    fn beacon(address: i64, at: [f64; 3]) -> Entry<Event> {
        let json = format!(
            r#"{{"timestamp":"2026-08-08T12:02:00Z","event":"NavBeaconScan",
            "SystemAddress":{},"StarPos":[{},{},{}],"NumBodies":5}}"#,
            address, at[0], at[1], at[2],
        );
        serde_json::from_str(&json).expect("the beacon should parse")
    }

    /// A system nothing named is left out of the tree, not published nameless
    ///
    /// `NavBeaconScan` carries a place and an optional name, so a system can
    /// be placed and nameless, and `galos_journal::Galaxy::name_of` answers
    /// nothing for one rather than the blank row the map's search would
    /// list. A tree standing over a system the names table has no row for is
    /// the disagreement [`agrees`] refuses to reopen a directory over, so
    /// the point waits until something names it.
    #[test]
    fn a_system_nothing_named_is_not_published() {
        let (dir, checkpoint) = scratch("nameless");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");
        pollster::block_on(async {
            sink.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr").await;
            sink.entry(&beacon(42, [1.0, 2.0, 3.0]), "cmdr").await;
            sink.flush().await.expect("the flush");
        });
        sink.publish_whole().expect("the run should write");
        drop(sink);

        assert_eq!(published(&dir), 1, "a system with no name was published");
        assert_eq!(names(&dir), vec!["SOL"]);
        if let Err(said) = Index::open(&dir, &checkpoint) {
            panic!("the directory disagreed with itself: {}", said);
        }

        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    /// A dumped system is placed, and one with no coordinates is not
    ///
    /// EDSM and EDDB hand over rows rather than events. A row with nowhere to
    /// be is the one thing the tree cannot take: put at the origin it would
    /// be a system drawn on top of Sol.
    #[test]
    fn a_dumped_row_is_placed_where_it_has_a_place() {
        let (dir, checkpoint) = scratch("dumped");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");

        let row = |address: i64, name: &str, at: Option<Coordinate>| Row {
            address,
            name: name.to_string(),
            position: at,
            population: Some(1000),
            security: None,
            government: None,
            allegiance: None,
            primary_economy: None,
            secondary_economy: None,
            updated_at: "2026-08-08T12:00:00Z".parse().expect("a moment"),
            updated_by: "a test".to_string(),
        };
        pollster::block_on(async {
            sink.system(&row(
                1,
                "Somewhere",
                Some(Coordinate { x: 10.0, y: 20.0, z: 30.0 }),
            ))
            .await;
            sink.system(&row(2, "Nowhere", None)).await;
        });
        sink.publish_whole().expect("the index should be written");

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
    fn scan(address: i64, body: i16, class: &str) -> Entry<Event> {
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
        serde_json::from_str(&json).expect("the scan should parse")
    }

    /// A scan after a flush joins what is already on the disk
    ///
    /// The one thing keeping the bodies in the published files rather than in
    /// memory could have broken. A system is scanned body by body over
    /// minutes and the file is written whole, so the second scan has to read
    /// back what the first wrote and add to it. Replacing instead would leave
    /// every system holding only whatever was scanned since the last publish,
    /// and the reach — the far edge over all of them — wrong with it.
    #[test]
    fn a_scan_after_a_flush_joins_what_is_on_the_disk() {
        let (dir, checkpoint) = scratch("merged");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");

        pollster::block_on(async {
            sink.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr").await;
            sink.entry(&scan(10477373803, 0, "G"), "cmdr").await;
            sink.flush().await.expect("the first flush");

            // Minutes later, the next body of the same system.
            sink.entry(&scan(10477373803, 4, "M"), "cmdr").await;
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

        // And the reach is over both, which is what a replaced file would
        // have got wrong without the count ever looking wrong.
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
    /// What the whole exercise was for. Asserted through the store rather
    /// than by measuring memory: what a feed grows by is the systems it has
    /// scanned and not yet written, and after a flush that is none of them.
    #[test]
    fn a_flush_leaves_no_bodies_held() {
        let (dir, checkpoint) = scratch("unheld");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");
        pollster::block_on(async {
            sink.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr").await;
            for body in 0..8 {
                sink.entry(&scan(10477373803, body, "G"), "cmdr").await;
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
    /// The one failure the format cannot notice: a cell tree of fifty systems
    /// beside a names table describing a galaxy reads as a valid directory
    /// and is not. `galos_db`'s builder answers a mismatch by rebuilding,
    /// which it can — every row is still in Postgres. This has nowhere to
    /// re-derive from, so it has to stop.
    #[test]
    fn a_directory_without_its_resume_point_is_refused() {
        let (dir, checkpoint) = scratch("orphaned");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");
        pollster::block_on(
            sink.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr"),
        );
        sink.publish_whole().expect("the first run should write");
        drop(sink);

        std::fs::remove_file(&checkpoint).expect("the resume point goes");
        // `expect_err` wants the `Ok` side to be `Debug`, and a sink holding
        // a tree of the galaxy is not a thing to derive that on.
        let Err(said) = Index::open(&dir, &checkpoint) else {
            panic!("an orphaned directory should be refused")
        };
        assert!(
            said.contains("no way to edit"),
            "the refusal should say why: {}",
            said,
        );
        assert!(
            said.contains("--to index=DIR"),
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

    /// A publish between two whole checkpoints survives a kill
    ///
    /// The resume point rides a minute's timer and a follower publishes
    /// every few seconds, so all but the first publish of a minute is in
    /// the directory and not in the checkpoint. Rebuilding from the
    /// checkpoint alone brought the tree back short of the names table
    /// beside it, the next publish wrote the shortfall over the directory,
    /// and nothing could open it again. [`Pending`] is what closes that.
    #[test]
    fn a_publish_after_the_last_checkpoint_is_not_lost() {
        let (dir, checkpoint) = scratch("lagging");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");

        // The first flush of a run writes a resume point and clears the log.
        pollster::block_on(
            sink.entry(&jump("Sol", 10477373803, [0.0; 3]), "cmdr"),
        );
        pollster::block_on(sink.flush()).expect("the first publish lands");
        assert!(!Pending::path(&checkpoint).exists(), "a whole one clears it");

        // The second does not: the timer has not come round, so what it
        // publishes lives in the log until the next whole checkpoint.
        pollster::block_on(sink.entry(
            &jump("Alpha Centauri", 3161824266978, [3.0, 0.0, 3.0]),
            "cmdr",
        ));
        pollster::block_on(sink.flush()).expect("the second publish lands");
        assert_eq!(sink.tree.len(), 2, "both systems are in the tree");
        assert!(Pending::path(&checkpoint).exists(), "the log has the second");
        drop(sink);

        let reopened = Index::open(&dir, &checkpoint).expect("it reopens");
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

    /// An index has nothing to ensure, and says so by answering yes
    ///
    /// The method exists for Postgres' foreign key. Answering `false` here
    /// would have callers skip the write that follows, which is the one thing
    /// this sink does.
    #[test]
    fn an_index_ensures_nothing() {
        let (dir, checkpoint) = scratch("ensured");
        let mut sink = Index::open(&dir, &checkpoint).expect("a sink opens");
        assert!(pollster::block_on(sink.ensure_system(
            "2026-08-08T12:00:00Z".parse().expect("a moment"),
            "cmdr",
            42,
            None,
            None,
            "a test",
        )));
        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }
}
