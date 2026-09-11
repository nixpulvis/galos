//! Building the galaxy index from the database.
//!
//! This is the one place the derived index meets the authoritative dataset. It
//! reads every positioned system and its scanned stars, turns them into the
//! photometry the ordering and the glow need, through `galos_photometry`'s
//! fallback chain, since two-thirds of systems carry no scanned star, and
//! hands the result to `galos_index`'s pure builder. Nothing about the tree
//! lives here; this crate knows the database and the builder knows the tree,
//! and they meet at [`System`].
//!
//! The queries are deliberately unchecked `sqlx::query`, not the `query!`
//! macro, so the build tool needs no compile-time database and no cached
//! metadata beyond what the rest of the crate already carries. The columns are
//! read back by name.
//!
//! The tree is here; the metadata that rides beside it — the populated table,
//! the names table, the factions and the body files — is `metadata`, which is
//! also where a watch's incremental publishing of it lives.
//!
//! What a pass reads is paged, in chunks of [`CHANGED_CHUNK`] addresses. A
//! cold catch-up over a million changed addresses is a hundred chunks of nine
//! bounded queries apiece, where it was nine queries each binding a
//! million-element array and holding everything they matched at once. It is
//! more round trips for a bound on what any one of them costs, which is what
//! a directory a week stale and a nightly dump that re-stamps every row it
//! read both need. The three tables written whole are written once for the
//! pass and not once per chunk, so the paging does not multiply the hundred
//! megabytes a publish rewrites.

use crate::{Database, Result};
use galos_index::{
    derive, meta, BuildParams, By, Checkpoint, Index, Pending, Snapshot,
    System, Tree,
};
use galos_photometry::{Magnitude, Temperature};
use metadata::{Metadata, Moved};
use sqlx::Row;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::{debug, info};

mod metadata;
pub use metadata::MetaReport;

/// Which of the index's parts a build writes
///
/// A full build writes all of them and is what a fresh directory wants. One
/// part alone is what a change to how a part is derived wants: the reach
/// table moved to [`galos_index::inside`]'s arithmetic and every published
/// reach was a table stale by that much, with nothing wrong with the cell
/// tree, the names or the factions beside it. Rebuilding those to fix this
/// one is a hundred megabytes of rewriting to say nothing new, and a watch
/// only ever patches the systems the feed reports, so a stale table converges
/// on whatever is being scanned and never on the rest.
///
/// The parts are what a directory holds rather than how it is derived, so
/// each names a file or a set of them: the cell tree and its payloads, the
/// names chunks, `populated.bin`, `reaches.bin`, `factions.bin`, and the
/// per-system body files. What each costs to derive differs wildly — the
/// cells and the names come out of one read of every positioned system, the
/// reaches and the body files out of one read of every scanned thing — and
/// asking for one part reads only what that part needs.
///
/// A part left out is left exactly as it stands in the directory. Nothing
/// here removes a file, so a partial build cannot leave the index short of
/// one: the worst it can do is leave one older than the rest, which is what
/// it was asked for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Parts {
    pub cells: bool,
    pub names: bool,
    pub populated: bool,
    pub reaches: bool,
    pub boosts: bool,
    pub factions: bool,
    pub bodies: bool,
}

impl Parts {
    /// Every part, which is what a build with nothing named writes
    pub const ALL: Parts = Parts {
        cells: true,
        names: true,
        populated: true,
        reaches: true,
        boosts: true,
        factions: true,
        bodies: true,
    };

    /// No part at all, to name them onto
    pub const NONE: Parts = Parts {
        cells: false,
        names: false,
        populated: false,
        reaches: false,
        boosts: false,
        factions: false,
        bodies: false,
    };

    /// Whether anything at all was asked for
    pub fn any(&self) -> bool {
        *self != Parts::NONE
    }

    /// Whether a read of every positioned system is wanted
    ///
    /// The one read the cell tree and the names table both come out of, and
    /// the expensive half of a full build. Skipped outright where neither is
    /// being written.
    fn wants_galaxy(&self) -> bool {
        self.cells || self.names
    }

    /// Whether a read of every scanned thing is wanted
    ///
    /// The rows the body files are written from, the reaches are measured
    /// over and the arrival star is picked out of, which is one read for all
    /// three. The boosts joined it when the arrival star stopped being a
    /// `DISTINCT ON` of its own and became
    /// [`galos_index::derive::arrival_class`] over these same rows, so
    /// `--only boosts` now pays for the whole scanned read where it used to
    /// pay for one ordered pass over `stars`. That is the price of the rule
    /// being written once: a repair of the boost table is the rarest build
    /// there is, and a table derived by a second copy of the rule is what it
    /// would be repairing.
    fn wants_bodies(&self) -> bool {
        self.reaches || self.bodies || self.boosts
    }
}

/// How long a watch goes between resume points, at most.
///
/// A checkpoint is every system at full precision, a hundred megabytes and
/// more, so writing one each pass costs more than everything else a pass does
/// put together. It is written on this timer instead. What that gives up is
/// paid back by the replay: a restart asks for the changes since the cursor the
/// checkpoint carries, applying one twice is idempotent, and the price of a
/// throttled checkpoint is a minute of the feed read a second time.
const CHECKPOINT_EVERY: Duration = Duration::from_secs(60);

/// How far back a pass looks past its own cursor.
///
/// A row's `received_at` is stamped inside the transaction that writes it and
/// the cursor is read outside any of them, so a report can carry a stamp older
/// than a cursor taken before it committed and be behind the cursor by the time
/// it can be seen. A pass looks back far enough to cover that. Re-reading a
/// system is free: every patch is rebuilt from the current row rather than
/// edited in place, so applying one twice lands exactly where applying it once
/// did. The overlap is measured from each pass's own cursor and the cursor
/// still moves to the clock the pass read, so it does not compound.
const CURSOR_OVERLAP: Duration = Duration::from_secs(2);

/// How many changed addresses one read of a pass covers.
///
/// The changed set is unbounded. A directory a week stale is a million
/// addresses, and one nightly EDSM dump is several million, since the import
/// stamps `received_at` on every row it read whether or not anything about
/// that row changed. Everything a pass does with the set binds it whole to
/// `= ANY($1)` — the systems and the stars behind [`inputs_for`], the names,
/// the populated rows, the bodies and the boosts behind
/// [`Metadata::patch`] — so the set unpaged is a bind parameter of millions,
/// a plan built over it, and every row any of those queries matches held in
/// memory at once.
///
/// Ten thousand is chosen for what one chunk holds rather than for the round
/// trips it costs. The body rows are the heavy read: a scanned system is tens
/// of them, each carrying its materials, so a chunk is on the order of a
/// hundred thousand rows at its worst and that is the high-water mark a
/// catch-up of any length runs against. Smaller pays for a plan and a round
/// trip more often without lowering the mark that matters; larger walks back
/// towards the unbounded case a chunk at a time.
const CHANGED_CHUNK: usize = 10_000;

/// Every scanned star grouped under its system: its visual `(absolute
/// magnitude, temperature)`, for the given addresses, or all systems when
/// `None`.
///
/// Each star's scanned magnitude is bolometric — its whole output as one figure
/// — so it is turned into the visual magnitude the sky sees by
/// [`Magnitude::visual`] before it is grouped. A star missing a magnitude or a
/// temperature cannot be summed, so it is left out and its system falls to the
/// class fallback like any other.
async fn stars_by_system(
    db: &Database,
    addresses: Option<&[i64]>,
) -> Result<HashMap<i64, Vec<(f64, f64)>>> {
    let rows = match addresses {
        None => {
            sqlx::query(
                "SELECT system_address, absolute_magnitude, temperature \
                 FROM stars",
            )
            .fetch_all(&db.pool)
            .await?
        }
        Some(addresses) => {
            sqlx::query(
                "SELECT system_address, absolute_magnitude, temperature \
                 FROM stars WHERE system_address = ANY($1)",
            )
            .bind(addresses)
            .fetch_all(&db.pool)
            .await?
        }
    };
    let mut stars: HashMap<i64, Vec<(f64, f64)>> = HashMap::new();
    for row in rows {
        let address: i64 = row.try_get("system_address")?;
        let magnitude: Option<f32> = row.try_get("absolute_magnitude")?;
        let temperature: Option<f32> = row.try_get("temperature")?;
        // Elite's scanned magnitude is bolometric — a star's whole output as if
        // all of it were visible — so convert it to the visual magnitude the
        // sky reads. This is where a white dwarf keeps its faint scanned
        // brightness and a neutron star or black hole falls to nothing, with no
        // per-class figure. See [`Magnitude::visual`].
        if let (Some(m), Some(t)) = (magnitude, temperature) {
            let t = t as f64;
            stars
                .entry(address)
                .or_default()
                .push((Magnitude(m as f64).visual(Temperature(t)).0, t));
        }
    }
    Ok(stars)
}

/// One `systems` row turned into build input through the photometry fallback.
///
/// The row carries `address`, the three `ST_?` coordinates, `primary_star_class`
/// and `updated_at`; `now` dates the Recency reading and `stars` supplies any
/// scan.
///
/// Both derived facts are [`galos_index::derive`]'s and neither is this
/// crate's to decide: the photometry fallback chain, which is the scanned
/// stars' light added and the brightest one's tint, or the class the system
/// is named for where nothing has been scanned; and the two forms of one
/// reading of `updated_at`, the Recency bucket the cell aggregates count by
/// and the Unix second the payload carries. A system built from a row and
/// the same system built from a journal entry land in the same place because
/// both go through those.
fn input_from_row(
    row: &sqlx::postgres::PgRow,
    stars: &HashMap<i64, Vec<(f64, f64)>>,
    now: chrono::NaiveDateTime,
) -> Result<System> {
    let address: i64 = row.try_get("address")?;
    let x: f64 = row.try_get("x")?;
    let y: f64 = row.try_get("y")?;
    let z: f64 = row.try_get("z")?;
    let class: Option<String> = row.try_get("primary_star_class")?;
    let at: chrono::NaiveDateTime = row.try_get("updated_at")?;
    let scanned = stars.get(&address).map(Vec::as_slice).unwrap_or(&[]);
    let (absolute_magnitude, temperature) =
        derive::lit(scanned.iter().copied(), class.as_deref().unwrap_or(""));
    let (age_bucket, updated_at) = derive::updated(at, now);
    Ok(System {
        id64: address as u64,
        position: [x, y, z],
        absolute_magnitude,
        temperature,
        age_bucket,
        updated_at,
    })
}

/// The addresses of systems reported since `since`: those whose own row
/// arrived, those whose stars did, since a scan re-magnitudes a system without
/// touching its row, and those whose factions did, since a faction is reported
/// for a system beside its row rather than in it.
///
/// A body scan is followed through the system row the sync writes beside it
/// rather than through `bodies` itself: that table is two million rows with no
/// index on when a report arrived, and every scan message names the system it
/// is in, so the row is written with the scan.
///
/// The column read is `received_at` and not `updated_at`, which is the trap
/// this walked into. `updated_at` is the timestamp off the journal entry, the
/// time the event describes out in the galaxy, and `systems.updated_at` is
/// merged with `GREATEST` so that a late message cannot move it backwards. The
/// cursor is the database's own clock, read when a pass looks. Comparing those
/// two compares an event's time against the time somebody happened to look, so
/// a report carrying an event timestamp behind the previous pass's cursor was
/// never asked for: a star scanned seconds before a pass, a system row already
/// carried forward by a later message, an entire journal import of timestamps
/// years old. `received_at` is stamped by the upsert as the report is written,
/// so this compares one clock against itself.
///
/// Rows that arrived before that column existed hold `NULL` and are not in the
/// result. They are the dataset as it stood, which a full build publishes and a
/// watch has no reason to publish again.
async fn changed_addresses(
    db: &Database,
    since: chrono::NaiveDateTime,
) -> Result<Vec<i64>> {
    let rows = sqlx::query(
        "SELECT address FROM systems \
         WHERE received_at > $1 AND position IS NOT NULL \
         UNION \
         SELECT DISTINCT system_address FROM stars WHERE received_at > $1 \
         UNION \
         SELECT DISTINCT system_address FROM system_factions \
         WHERE received_at > $1",
    )
    .bind(since)
    .fetch_all(&db.pool)
    .await?;
    Ok(rows.iter().map(|row| row.get::<i64, _>("address")).collect())
}

/// Read every positioned system as build input, and its name and place
/// alongside.
///
/// One read for both, because the two must agree. The cell tree and the names
/// table stand for the same set of systems, and a feed that writes a system
/// between two reads of `systems` would leave them a few apart with nothing to
/// close the gap — which is also the consistency a `--watch` restart tests the
/// served directory by. The row carries what each side needs and is turned into
/// both here.
async fn read_galaxy(
    db: &Database,
) -> Result<(Vec<System>, Vec<meta::NameEntry>)> {
    let now = db.now().await?.naive_utc();
    let stars = stars_by_system(db, None).await?;
    let rows = sqlx::query(
        "SELECT address, name, \
                ST_X(position) AS x, ST_Y(position) AS y, ST_Z(position) AS z, \
                primary_star_class, updated_at \
         FROM systems WHERE position IS NOT NULL",
    )
    .fetch_all(&db.pool)
    .await?;
    let mut inputs = Vec::with_capacity(rows.len());
    let mut names = Vec::with_capacity(rows.len());
    for row in &rows {
        inputs.push(input_from_row(row, &stars, now)?);
        names.push(metadata::name_from_row(row)?);
    }
    Ok((inputs, names))
}

/// Build the parts of the index `parts` names and write them to `dir`: the cell
/// tree the map draws from and the records a click reads, in one directory so a
/// single transport serves both.
///
/// [`Parts::ALL`] is a full build and what a fresh directory wants. Anything
/// narrower reads only what those parts need and leaves every other file in the
/// directory exactly as it stands.
///
/// A build that read the whole galaxy writes `checkpoint` beside it, as
/// [`catch_up`] does after its own initial build, and says the database
/// derived it: the directory is otherwise a published index nothing can
/// edit, since the served payloads carry a downcast magnitude and a bucketed
/// temperature, so the full-precision inputs are here or nowhere and nothing
/// following the same directory can resume onto one without them. A narrowed
/// build writes none: it never read the systems a resume point is made of,
/// and a short one would be worse than an absent one.
pub async fn build_to_dir(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    parts: Parts,
) -> Result<BuildReport> {
    // The one read the cell tree and the names table both come out of, and the
    // whole of what a build asking for neither can skip.
    let since = db.now().await?.naive_utc();
    let galaxy =
        if parts.wants_galaxy() { Some(read_galaxy(db).await?) } else { None };

    let cells = galaxy
        .as_ref()
        .filter(|_| parts.cells)
        .map(|(inputs, _)| {
            let built = Snapshot::build(inputs, &BuildParams::default());
            built.write(dir).map(|()| TreeReport::of(inputs.len(), &built))
        })
        .transpose()?;

    if cells.is_some() {
        if let Some((inputs, _)) = &galaxy {
            record(checkpoint, since, inputs)?;
        }
    }

    let names = galaxy.map(|(_, names)| names);
    let meta = metadata::write_parts(db, dir, names, parts).await?;
    Ok(BuildReport { cells, meta })
}

/// The systems of `addresses` as build input.
///
/// Each is rebuilt whole from its current record through the same fallback
/// [`read_galaxy`] uses, so a system applied incrementally lands exactly where a
/// full rebuild would put it. An address with no positioned row is not in the
/// result: there is nothing for the tree to hold.
async fn inputs_for(db: &Database, addresses: &[i64]) -> Result<Vec<System>> {
    if addresses.is_empty() {
        return Ok(Vec::new());
    }
    let now = db.now().await?.naive_utc();
    let stars = stars_by_system(db, Some(addresses)).await?;
    let rows = sqlx::query(
        "SELECT address, \
                ST_X(position) AS x, ST_Y(position) AS y, ST_Z(position) AS z, \
                primary_star_class, updated_at \
         FROM systems WHERE address = ANY($1) AND position IS NOT NULL",
    )
    .bind(addresses)
    .fetch_all(&db.pool)
    .await?;
    rows.iter().map(|row| input_from_row(row, &stars, now)).collect()
}

/// Bring `dir` level with the database and answer the clock it is level at.
///
/// This is what a directory needs before anything else can maintain it: it is
/// missing, or it is a week behind, and either way the index has to stand for
/// the database before it starts taking live events beside it. It resumes
/// from `checkpoint`, or builds the whole galaxy afresh where it cannot, and
/// then runs delta passes until one has little enough left to hand over.
///
/// The clock is read before each pass reads what changed, never after, so the
/// cursor answered is one nothing can hide behind: a write racing a pass's
/// read is asked for again by whoever follows the cursor rather than missed
/// by everyone. That discipline is the whole of what makes the handoff sound,
/// and each pass reads back a further [`CURSOR_OVERLAP`] to catch a write
/// that committed after the cursor was taken. Applying a system twice is
/// idempotent, since every patch is rebuilt from the current row rather than
/// edited in place, so the overlap costs a little work and no correctness.
///
/// A pass ends the catch-up when what it found is smaller than one
/// [`CHANGED_CHUNK`], which an empty set is the floor of. Waiting on an empty
/// one is waiting for a quiet database, and the caller wanting this is
/// usually the same process writing to it — thirty messages a second of its
/// own feed, which a pass would chase forever and never go live. The residue
/// is not rows at risk: everything received since that process started is
/// already in its handoff buffer, so the systems between "under a chunk" and
/// "nothing" are applied again from there, which is duplicate work and
/// idempotent.
///
/// What it leaves on disk is a whole resume point standing for exactly the
/// systems the directory now serves. The throttle that keeps a long watch
/// from writing the galaxy every pass does not apply to the last word: the
/// caller opens this directory for editing against the checkpoint beside it,
/// and one short of what is served cannot be opened at all.
///
/// `parts` narrower than [`Parts::ALL`] is the repair case — one part derived
/// by a rule that has since changed — and is a one-shot [`build_to_dir`] of
/// those parts rather than a catch-up. There is nothing to resume onto when
/// the parts of a directory disagree about when each was derived, and nothing
/// to follow afterwards. The cursor answered is the clock read before the
/// build read anything, which is behind the one the build wrote and so asks
/// whoever follows it for a little more than it has to.
pub async fn catch_up(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    parts: Parts,
    stop: &Stop<'_>,
) -> Result<chrono::NaiveDateTime> {
    if parts != Parts::ALL {
        let since = db.now().await?.naive_utc();
        let report = build_to_dir(db, dir, checkpoint, parts).await?;
        info!(dir = %dir.display(), %report, "index parts derived");
        return Ok(since);
    }
    Ok(bring_level(db, dir, checkpoint, stop).await?.cursor)
}

/// Whether whoever asked for this has stopped wanting it.
///
/// A catch-up over a galaxy is an hour and a watch never ends at all, so a
/// caller that can be asked to stop — a Ctrl-C on the sync — needs a way to
/// say so that does not involve killing the process and leaving the
/// directory wherever the last write left it. Asked between chunks and
/// between passes, which is as often as there is a whole thing to leave
/// behind; a single query is not interrupted, so the longest this can take
/// to be obeyed is the longest read a pass makes.
///
/// A borrowed predicate rather than a token type of its own, since the only
/// thing this crate does with it is ask.
pub type Stop<'a> = dyn Fn() -> bool + Send + Sync + 'a;

/// Nothing ever asks this to stop, which is what a one-shot caller wants.
pub fn never() -> &'static Stop<'static> {
    &|| false
}

/// Bring `dir` level with the database, then keep it there as the feed writes,
/// publishing what each round of changes touched.
///
/// This rides on top of the collection rather than beside it: a sync writes
/// systems to the database in real time and this follows the rows those
/// writes leave behind, two processes or two machines with the database the
/// only thing between them. It is [`catch_up`] and then a poll every
/// `interval`, which reads those systems changed since the previous pass,
/// moves each in the live [`Tree`] (a handful of cells apiece, not a rebuild),
/// and writes only the cells that changed.
///
/// The metadata beside the cells is kept current the same pass the cells are,
/// and the same way: `Metadata` holds the four tables open, a pass patches in
/// the systems that changed, and only what that moved is written — the names
/// chunks the arrivals landed in, the populated table when a political column
/// really did change, the factions when a new one is named, and a body file
/// per changed system. Nothing here reads the whole database twice.
///
/// The checkpoint rides outside `dir`, is never served, and is written after
/// the initial build and every [`CHECKPOINT_EVERY`] thereafter — not every
/// pass, which would cost more than the publishing does.
pub async fn watch(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    interval: Duration,
    stop: &Stop<'_>,
) -> Result<()> {
    let mut level = bring_level(db, dir, checkpoint, stop).await?;
    info!(
        dir = %dir.display(),
        interval_secs = interval.as_secs(),
        "watching for changes"
    );
    while !stop() {
        // In ticks rather than one sleep: `--watch 3600` is a poll an hour
        // apart, and a stop that waited for the next one would be a run
        // nobody could stop.
        waited(interval, stop).await;
        if stop() {
            break;
        }
        pass(db, dir, checkpoint, &mut level).await?;
    }
    info!(dir = %dir.display(), "asked to stop watching");
    Ok(())
}

/// Sleep `interval`, or until asked to stop.
async fn waited(interval: Duration, stop: &Stop<'_>) {
    const TICK: Duration = Duration::from_millis(200);
    let until = Instant::now() + interval;
    while Instant::now() < until && !stop() {
        async_std::task::sleep(TICK.min(until - Instant::now())).await;
    }
}

/// A directory level with the database, and everything following it further
/// asks for: the tree and the tables as they stand in the directory, the
/// clock the last pass covered, when the last resume point was written, and
/// whether the directory has moved past it since.
///
/// Held together because a pass moves all of it at once, and because a
/// catch-up handing over to a watch hands over exactly this — the tables are
/// derived once at the start and then only ever patched.
struct Level {
    tree: Tree,
    meta: Metadata,
    cursor: chrono::NaiveDateTime,
    checkpointed: Instant,
    /// Whether a pass has published something the resume point on disk does
    /// not stand for. Set by every pass the throttle skipped, cleared by
    /// every write, and what [`bring_level`] settles before it hands over.
    unrecorded: bool,
}

/// Resume `dir` or build it whole, then run delta passes until one finds
/// little enough left to hand over. The one copy of the build-or-resume that
/// both [`catch_up`] and [`watch`] start from.
async fn bring_level(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    stop: &Stop<'_>,
) -> Result<Level> {
    let params = BuildParams::default();
    let mut level = match resume(dir, checkpoint, &params) {
        Some((tree, meta, cursor)) => {
            info!(
                systems = tree.len(),
                cursor = %cursor,
                checkpoint = %checkpoint.display(),
                "resumed from checkpoint"
            );
            Level {
                tree,
                meta,
                cursor,
                checkpointed: Instant::now(),
                unrecorded: false,
            }
        }
        None => {
            let since = db.now().await?.naive_utc();
            info!(dir = %dir.display(), "building initial index (reading every system)");
            let start = Instant::now();
            let (inputs, names) = read_galaxy(db).await?;
            let mut tree = Tree::build(&inputs, &params);
            tree.write(dir)?;
            let (meta, report) = Metadata::build(db, dir, names).await?;
            record(checkpoint, since, &inputs)?;
            info!(
                systems = tree.len(),
                chunks = report.name_chunks,
                elapsed = ?start.elapsed(),
                "initial index built"
            );
            Level {
                tree,
                meta,
                cursor: since,
                checkpointed: Instant::now(),
                unrecorded: false,
            }
        }
    };
    while !stop()
        && pass(db, dir, checkpoint, &mut level).await? >= CHANGED_CHUNK
    {}

    // What the passes published since the last throttled write. The caller is
    // about to hand this directory to something that opens it against the
    // resume point beside it, and a directory serving more than its
    // checkpoint stands for is one that cannot be opened for editing at all —
    // so the timer, which exists to keep a long watch from writing the galaxy
    // every pass, does not get to decide what is on disk when a catch-up
    // returns.
    if level.unrecorded {
        let start = Instant::now();
        record(checkpoint, level.cursor, &level.tree.to_inputs())?;
        level.checkpointed = Instant::now();
        level.unrecorded = false;
        info!(
            systems = level.tree.len(),
            cursor = %level.cursor,
            checkpoint = %checkpoint.display(),
            elapsed = ?start.elapsed(),
            "caught up, resume point written"
        );
    }
    Ok(level)
}

/// Write the resume point for a directory this crate has just published, and
/// drop any pending log beside it.
///
/// Every checkpoint this crate writes is the database's own derivation, read
/// up to `cursor`, and is whole: it stands for every system in the directory
/// rather than for a difference. The event side's [`Pending`] log is the
/// difference since the last whole one, so a whole one supersedes it — and a
/// log left beside a checkpoint that already holds what it holds would be
/// replayed over the directory at the next open, publishing systems twice or,
/// worse, systems this build never read.
fn record(
    checkpoint: &Path,
    cursor: chrono::NaiveDateTime,
    inputs: &[System],
) -> Result<()> {
    Checkpoint::write_from(checkpoint, Some(cursor), By::Database, inputs)?;
    Pending::clear(checkpoint)?;
    Ok(())
}

/// One pass: read what has changed since the cursor, apply it a chunk at a
/// time, publish what that moved, and move the cursor on to the clock the
/// read covered. Answers how many addresses it found.
///
/// The clock comes first, before the read, so a write that commits while the
/// pass runs is asked for by the next one rather than passed over by both.
///
/// The chunks are what keeps a pass over a million addresses from being one
/// query per table with a million-element array in it; see [`CHANGED_CHUNK`].
/// The cells and the three tables written whole are written once, after the
/// last chunk, since each is a file rewritten entire and a chunk of ten
/// thousand addresses has no more claim on when that happens than the pass
/// does. Only the body files and the names chunks are written as the chunks
/// go, being per system and per neighbourhood of systems respectively.
async fn pass(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    level: &mut Level,
) -> Result<usize> {
    let now = db.now().await?.naive_utc();
    let touched = changed_addresses(db, level.cursor - CURSOR_OVERLAP).await?;
    if touched.is_empty() {
        debug!(since = %level.cursor, "polled, no changes");
        level.cursor = now;
        return Ok(0);
    }

    let start = Instant::now();
    let mut changed = 0;
    let mut moved = Moved::default();
    for chunk in touched.chunks(CHANGED_CHUNK) {
        let inputs = inputs_for(db, chunk).await?;
        changed += inputs.len();
        level.tree.apply(&inputs);
        moved.absorb(level.meta.patch(db, dir, chunk).await?);
    }
    level.tree.publish(dir)?;
    let report = level.meta.publish_pass(db, dir, moved).await?;

    let resumed_from = level.checkpointed.elapsed() >= CHECKPOINT_EVERY;
    if resumed_from {
        record(checkpoint, now, &level.tree.to_inputs())?;
        level.checkpointed = Instant::now();
    }
    level.unrecorded = !resumed_from;
    info!(
        changed,
        pages = touched.len().div_ceil(CHANGED_CHUNK),
        systems = level.tree.len(),
        chunks = report.name_chunks,
        bodies = report.body_files,
        checkpointed = resumed_from,
        elapsed = ?start.elapsed(),
        "index updated"
    );
    level.cursor = now;
    Ok(touched.len())
}

/// Rebuild the live tree and the metadata tables from a checkpoint and the
/// directory they were published to, if the two still read as each other's.
/// Returns the tree, the tables and the cursor to follow from, or [`None`] to
/// build from scratch.
///
/// The checkpoint may lag the directory, being written on a timer while every
/// pass publishes: the last passes before a stop stand in the directory and not
/// in it. That is what the replay is for. The directory can differ from the
/// checkpoint only through systems that changed after the cursor, and every one
/// of those is read and applied again on the first pass, so the publishes that
/// follow rewrite exactly what the lag left behind.
///
/// Hence the three gates. The checkpoint has to be the database's own. An
/// event-derived directory stands for what the feed reported since it was
/// opened and not for the galaxy — a few thousand systems where the database
/// holds a hundred million — and adopting one as the tree to follow from
/// leaves a catch-up patching changes into an index that was never the
/// dataset, passing both counting gates below because the directory really is
/// internally consistent about the little it holds. It is refused outright,
/// and the full build that follows is what makes the directory the database's
/// again.
///
/// Then the directory must be internally consistent, its cell tree and its
/// names table standing for exactly the same systems, which is what
/// [`read_galaxy`] reading both out of one row makes an invariant and what a
/// half-written or older-layout directory fails. And it must be at or ahead
/// of the checkpoint, since a delta publish repairs only what the next
/// changes touch, not what is already wrong.
fn resume(
    dir: &Path,
    path: &Path,
    params: &BuildParams,
) -> Option<(Tree, Metadata, chrono::NaiveDateTime)> {
    let checkpoint = Checkpoint::read(path).ok()?;
    if checkpoint.by != By::Database {
        info!(
            by = ?checkpoint.by,
            checkpoint = %path.display(),
            "checkpoint was written from the event feed, which stands for \
             what the feed reported and not for the galaxy; building afresh"
        );
        return None;
    }
    let Some(cursor) = checkpoint.cursor else {
        debug!(
            checkpoint = %path.display(),
            "checkpoint carries no cursor to follow from; building afresh"
        );
        return None;
    };
    let tree = Tree::build(&checkpoint.inputs, params);
    let count = Index::read(dir).ok()?.root()?.aggregate.count();
    let meta = Metadata::resume(dir).ok()?;
    if meta.names() as u64 != count || count < tree.len() as u64 {
        debug!(
            names = meta.names(),
            served = count,
            checkpointed = tree.len(),
            "checkpoint does not match the served directory; building afresh"
        );
        return None;
    }
    Some((tree, meta, cursor))
}

/// What a build of the cell tree came to, for the binary to print and check.
#[derive(Copy, Clone, Debug)]
pub struct TreeReport {
    pub systems: usize,
    pub points: usize,
    pub cells: usize,
    pub leaves: usize,
    pub deepest_level: u8,
    pub max_leaf_points: usize,
}

impl TreeReport {
    fn of(systems: usize, built: &Snapshot) -> TreeReport {
        let leaves = built.index.cells().filter(|c| c.is_leaf()).count();
        let deepest_level =
            built.index.cells().map(|c| c.id.level).max().unwrap_or(0);
        let max_leaf_points = built
            .index
            .cells()
            .filter(|c| c.is_leaf())
            .map(|c| built.payload(c.id).len())
            .max()
            .unwrap_or(0);
        TreeReport {
            systems,
            points: built.point_count(),
            cells: built.index.len(),
            leaves,
            deepest_level,
            max_leaf_points,
        }
    }

    /// Whether every system landed in exactly one cell: the partition holds.
    fn is_consistent(&self) -> bool {
        self.points == self.systems
    }
}

impl fmt::Display for TreeReport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} systems -> {} cells ({} leaves, {} internal), \
             deepest level {}, largest leaf {} systems, {} placed{}",
            self.systems,
            self.cells,
            self.leaves,
            self.cells - self.leaves,
            self.deepest_level,
            self.max_leaf_points,
            self.points,
            if self.is_consistent() { "" } else { " (MISMATCH)" },
        )
    }
}

/// A summary of a build, for the binary to print and check.
///
/// Each part is what this build wrote rather than what stands in the
/// directory, so a part it was not asked for says so instead of reading as a
/// count of nothing. A build that wrote no cells has nothing to say about the
/// partition either.
#[derive(Copy, Clone, Debug)]
pub struct BuildReport {
    /// The cell tree, where this build wrote one.
    pub cells: Option<TreeReport>,
    /// The metadata sidecars written beside the tree.
    pub meta: MetaReport,
}

impl BuildReport {
    /// Whether every system landed in exactly one cell: the partition holds.
    ///
    /// True where no tree was written, there being no partition this build
    /// could have got wrong.
    pub fn is_consistent(&self) -> bool {
        self.cells.is_none_or(|cells| cells.is_consistent())
    }
}

/// What was written, part by part, with the parts left alone named as kept.
impl fmt::Display for BuildReport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.cells {
            Some(cells) => write!(f, "{cells}")?,
            None => write!(f, "cells kept")?,
        }
        write!(f, "; metadata: {}", self.meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address the write below owns, which nothing else here writes
    ///
    /// Well outside anything the game hands out and outside the block
    /// `tests/write_path.rs` keeps for itself, so the two files can run at the
    /// same time against one database.
    const WATCHED: i64 = 900_001_000;

    /// The address the catch-up below owns, which nothing else here writes
    const CAUGHT: i64 = 900_001_001;

    /// The address the resume point test below owns
    const RECORDED: i64 = 900_001_002;

    /// A system at `position`, lit as the class `G` names rather than by a
    /// scan, which is what the two tests below want a [`System`] for and
    /// neither of them is about.
    fn input(address: i64, position: [f64; 3]) -> System {
        let (absolute_magnitude, temperature) =
            derive::lit(std::iter::empty(), "G");
        System {
            id64: address as u64,
            position,
            absolute_magnitude,
            temperature,
            age_bucket: 0,
            updated_at: 0,
        }
    }

    /// A report whose event timestamp is a year old still reads as newly
    /// arrived
    ///
    /// This is the bug the `received_at` column exists for, and it fails
    /// without it: the row's `updated_at` is the timestamp off the entry, a
    /// year in the past, so it is never greater than a cursor taken today and
    /// the pass that should have published the system never asked for it. A
    /// journal import is exactly this, hours or years of entries at once, and a
    /// live scan is the same thing by a few seconds. `received_at` is stamped
    /// by the upsert as the report is written, so the age of what the report
    /// describes has nothing to do with whether a watch sees it arrive.
    ///
    /// This needs a database of its own, named by `TEST_DATABASE_URL`, for the
    /// reason `tests/write_path.rs` sets out: the database being filled from
    /// EDDN is one `cargo test` must not be able to reach. It stands down when
    /// nothing says where to write, so CI passes with no database at all.
    #[async_std::test]
    async fn an_old_event_timestamp_still_reads_as_newly_arrived() {
        dotenv::dotenv().ok();
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("no TEST_DATABASE_URL: standing down");
            return;
        };
        let db = Database::from_url(&url)
            .await
            .expect("TEST_DATABASE_URL should connect");

        // Nothing else writes this address, so the row can be dropped and the
        // write below is the only thing that could put it back.
        for statement in [
            "DELETE FROM stars WHERE system_address = $1",
            "DELETE FROM system_factions WHERE system_address = $1",
            "DELETE FROM systems WHERE address = $1",
        ] {
            sqlx::query(statement)
                .bind(WATCHED)
                .execute(&db.pool)
                .await
                .expect("the address should be clearable");
        }

        // The cursor a watch would be holding: the database's clock, read
        // before anything is written.
        let before = db.now().await.expect("the clock should read").naive_utc();

        // What a journal import looks like, and what a late message looks like
        // with the numbers made obvious.
        let happened = chrono::Utc::now() - chrono::TimeDelta::days(365);
        let system = elite_journal::system::System {
            pos: Some(elite_journal::system::Coordinate {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            }),
            ..elite_journal::system::System::new(WATCHED, "TEST WATCHED SYSTEM")
        };
        crate::systems::System::from_journal(&db, happened, "test", &system)
            .await
            .expect("the system should write");

        let touched = changed_addresses(&db, before)
            .await
            .expect("the changed addresses should read");
        assert!(
            touched.contains(&WATCHED),
            "a system reported now is changed since a cursor taken before it, \
             whatever the age of the event it reports",
        );
    }

    /// A resume point the event feed wrote is refused, whatever it counts up
    /// to
    ///
    /// The two derivations publish the same directory and write the same kind
    /// of resume point, and only one of them stands for the galaxy. An
    /// event-derived directory is what a sink saw reported since it was
    /// opened — a few thousand systems against a hundred million — and it is
    /// internally consistent about exactly that, so it passes the counting
    /// gates a half-written directory fails. Adopted, it would be followed
    /// from its cursor forever and the database would never be read: the
    /// index would be the feed's recent memory published as the galaxy. So
    /// what wrote a checkpoint is read before anything is counted.
    ///
    /// No database: the directory is two systems written by hand through the
    /// same writers a build uses, and the only thing that differs between the
    /// two halves of the test is which derivation the checkpoint names.
    #[test]
    fn a_resume_point_written_from_events_is_refused() {
        let dir = std::env::temp_dir()
            .join(format!("galos_db_by_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.with_extension("checkpoint");

        // A directory that is every gate's idea of consistent: the cell tree
        // and the names table standing for the same two systems.
        let params = BuildParams::default();
        let inputs: Vec<System> = [(1i64, 0.0f64), (2, 10.0)]
            .iter()
            .map(|&(address, x)| input(address, [x, 0.0, 0.0]))
            .collect();
        let mut tree = Tree::build(&inputs, &params);
        tree.write(&dir).expect("the tree should write");
        galos_index::NameTable::from_entries(
            inputs
                .iter()
                .map(|system| meta::NameEntry {
                    address: system.id64 as i64,
                    name: format!("TEST {}", system.id64),
                    position: [system.position[0] as f32, 0.0, 0.0],
                })
                .collect(),
        )
        .publish(&dir)
        .expect("the names should write");
        let empty: Vec<u8> = Vec::new();
        for table in [
            galos_index::source::populated_path(&dir),
            galos_index::source::reaches_path(&dir),
            galos_index::source::factions_path(&dir),
        ] {
            galos_index::source::write_meta(&table, &empty)
                .expect("a table should write");
        }

        let cursor = chrono::DateTime::from_timestamp(1_757_260_000, 0)
            .expect("a moment")
            .naive_utc();
        Checkpoint::write_from(&path, Some(cursor), By::Database, &inputs)
            .expect("a resume point should write");
        assert!(
            resume(&dir, &path, &params).is_some(),
            "a directory the database derived, matching its own resume point, \
             was refused",
        );

        Checkpoint::write_from(&path, Some(cursor), By::Events, &inputs)
            .expect("a resume point should write");
        assert!(
            resume(&dir, &path, &params).is_none(),
            "a resume point the event feed wrote was adopted as the galaxy's",
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&path);
    }

    /// A poll an hour away is still a poll a run can be stopped between
    ///
    /// `watch` sleeps the interval between passes, and a run asked to stop
    /// has to be obeyed before the next one: `--watch 3600` slept as one
    /// call meant a Ctrl-C the process answered an hour later, which is a
    /// run nobody can stop. It waits in ticks now, and this is that.
    #[async_std::test]
    async fn a_long_wait_ends_the_moment_it_is_asked_to() {
        let start = Instant::now();
        waited(Duration::from_secs(3600), &|| true).await;
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "an hour's sleep outlasted the question: {:?}",
            start.elapsed(),
        );

        // And it still waits where nothing is asking.
        let start = Instant::now();
        waited(Duration::from_millis(300), &|| false).await;
        assert!(
            start.elapsed() >= Duration::from_millis(300),
            "the wait was skipped: {:?}",
            start.elapsed(),
        );
    }

    /// A catch-up levels a directory and answers a cursor to follow from, and
    /// a second one finds the directory it left with nothing to do
    ///
    /// The two halves of the handoff. The cursor has to be at or past the
    /// moment the systems it read were written, since whoever follows it asks
    /// only for what came after; and the directory has to be one a catch-up
    /// will adopt, since a catch-up that refused its own work would read the
    /// whole galaxy every time a process started and the handoff would never
    /// be worth taking.
    ///
    /// `populated.bin` is the witness for the second half. It is written
    /// whole or not at all: a full build writes it always, and a resumed pass
    /// writes it only where a political column really moved. So a file left
    /// exactly as the first call wrote it says the second call resumed and
    /// published nothing, and a file rewritten says it built the galaxy
    /// afresh. Any system another test writes meanwhile is unpopulated and
    /// moves the names and the cells rather than this table, so the witness
    /// holds with the rest of the suite running beside it.
    ///
    /// This needs a database of its own, named by `TEST_DATABASE_URL`, and
    /// stands down without one exactly as the test above does.
    #[async_std::test]
    async fn a_catch_up_levels_a_directory_and_leaves_one_to_resume() {
        dotenv::dotenv().ok();
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("no TEST_DATABASE_URL: standing down");
            return;
        };
        let db = Database::from_url(&url)
            .await
            .expect("TEST_DATABASE_URL should connect");

        let dir = std::env::temp_dir()
            .join(format!("galos_db_catch_up_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let checkpoint = dir.with_extension("checkpoint");
        let _ = std::fs::remove_file(&checkpoint);

        // A system written before the catch-up looks, which is what the
        // catch-up owes the directory.
        for statement in [
            "DELETE FROM stars WHERE system_address = $1",
            "DELETE FROM system_factions WHERE system_address = $1",
            "DELETE FROM systems WHERE address = $1",
        ] {
            sqlx::query(statement)
                .bind(CAUGHT)
                .execute(&db.pool)
                .await
                .expect("the address should be clearable");
        }
        let before = db.now().await.expect("the clock should read").naive_utc();
        let system = elite_journal::system::System {
            pos: Some(elite_journal::system::Coordinate {
                x: 4.0,
                y: 5.0,
                z: 6.0,
            }),
            ..elite_journal::system::System::new(CAUGHT, "TEST CAUGHT SYSTEM")
        };
        crate::systems::System::from_journal(
            &db,
            chrono::Utc::now(),
            "test",
            &system,
        )
        .await
        .expect("the system should write");

        let cursor = catch_up(&db, &dir, &checkpoint, Parts::ALL, never())
            .await
            .expect("the catch-up should run");
        assert!(
            cursor > before,
            "a cursor at {} is behind the write at {} it covered, so whoever \
             follows it would ask for the write again and nothing before it",
            cursor,
            before,
        );
        let served = Index::read(&dir)
            .expect("the index should read")
            .root()
            .expect("the tree should have a root")
            .aggregate
            .count();
        assert!(served >= 1, "the catch-up published an empty directory");

        // Past the look-back, so a pass has nothing to ask for rather than the
        // overlap's worth of what it just read.
        let published =
            std::fs::metadata(galos_index::source::populated_path(&dir))
                .expect("the populated table should stand")
                .modified()
                .expect("a modification time");
        async_std::task::sleep(CURSOR_OVERLAP + Duration::from_secs(1)).await;

        let again = catch_up(&db, &dir, &checkpoint, Parts::ALL, never())
            .await
            .expect("the second catch-up should run");
        assert!(again >= cursor, "the cursor went backwards");
        let after =
            std::fs::metadata(galos_index::source::populated_path(&dir))
                .expect("the populated table should stand")
                .modified()
                .expect("a modification time");
        assert_eq!(
            published, after,
            "a catch-up rebuilt a directory it had just brought level, so its \
             own resume point was refused",
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&checkpoint);
    }

    /// A catch-up leaves a resume point standing for what the directory
    /// serves, whatever the throttle says
    ///
    /// The throttle is there so a watch running for a week does not write a
    /// hundred megabytes of full-precision systems every pass; a restart pays
    /// for the lag by replaying from the cursor. Nothing replays for the
    /// caller of a catch-up. It opens the directory for editing against the
    /// checkpoint beside it, and a checkpoint short of what is served is not
    /// a resume that costs a little extra work — the directory cannot be
    /// opened at all, since there is no way to say what the systems it
    /// already serves were built from. A cold start is exactly this: the
    /// initial build wrote a checkpoint for an empty galaxy, the delta pass
    /// that followed published the systems written since, and the timer had
    /// not come round.
    ///
    /// The pending log goes with it. It is the event side's difference since
    /// the last whole checkpoint, and one written here holds everything it
    /// held, so a log left standing would be replayed over a directory that
    /// already has it.
    ///
    /// `TEST_DATABASE_URL`-gated like the two above.
    #[async_std::test]
    async fn a_catch_up_leaves_a_checkpoint_for_what_it_serves() {
        dotenv::dotenv().ok();
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("no TEST_DATABASE_URL: standing down");
            return;
        };
        let db = Database::from_url(&url)
            .await
            .expect("TEST_DATABASE_URL should connect");

        let dir = std::env::temp_dir()
            .join(format!("galos_db_recorded_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let checkpoint = dir.with_extension("checkpoint");
        let _ = std::fs::remove_file(&checkpoint);
        let _ = Pending::clear(&checkpoint);

        // The address is cleared first, so the build below cannot already
        // hold it and the pass that publishes it really does grow the tree
        // past what the build's resume point stands for.
        for statement in [
            "DELETE FROM stars WHERE system_address = $1",
            "DELETE FROM system_factions WHERE system_address = $1",
            "DELETE FROM systems WHERE address = $1",
        ] {
            sqlx::query(statement)
                .bind(RECORDED)
                .execute(&db.pool)
                .await
                .expect("the address should be clearable");
        }

        // A directory brought level, and then a system written after it was:
        // the pass that publishes this one is well inside CHECKPOINT_EVERY,
        // so it is the pass the throttle skips.
        catch_up(&db, &dir, &checkpoint, Parts::ALL, never())
            .await
            .expect("the first catch-up should run");
        let system = elite_journal::system::System {
            pos: Some(elite_journal::system::Coordinate {
                x: 7.0,
                y: 8.0,
                z: 9.0,
            }),
            ..elite_journal::system::System::new(
                RECORDED,
                "TEST RECORDED SYSTEM",
            )
        };
        crate::systems::System::from_journal(
            &db,
            chrono::Utc::now(),
            "test",
            &system,
        )
        .await
        .expect("the system should write");

        // A log from an earlier event run, which the whole checkpoint below
        // supersedes.
        Pending::append(&checkpoint, &[input(RECORDED, [7.0, 8.0, 9.0])])
            .expect("a pending log should write");

        catch_up(&db, &dir, &checkpoint, Parts::ALL, never())
            .await
            .expect("the second catch-up should run");

        let served = Index::read(&dir)
            .expect("the index should read")
            .root()
            .expect("the tree should have a root")
            .aggregate
            .count();
        let written = Checkpoint::read(&checkpoint)
            .expect("the resume point should read");
        assert_eq!(
            written.inputs.len() as u64,
            served,
            "the directory serves {} systems and its resume point stands for \
             {}, so nothing can open it to edit what it holds",
            served,
            written.inputs.len(),
        );
        assert!(written.cursor.is_some(), "a resume point with no cursor");
        assert!(
            !Pending::path(&checkpoint).exists(),
            "a pending log outlived the whole checkpoint that holds it, and \
             would be replayed over a directory that already has it",
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&checkpoint);
    }
}
