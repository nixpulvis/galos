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

use crate::{Database, Result};
use galos_index::{
    meta, BuildParams, Checkpoint, Index, Snapshot, System, Tree,
};
use galos_photometry::{ClassLight, Magnitude, Temperature};
use metadata::Metadata;
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
    /// The rows the body files are written from and the reaches are measured
    /// over, which is one read for both.
    fn wants_bodies(&self) -> bool {
        self.reaches || self.bodies
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

/// The edges between the eight Recency buckets, in days since a system was last
/// written. Updated today lands in bucket 0, untouched for a decade in bucket 7.
const AGE_EDGES: [i64; 7] = [1, 7, 30, 90, 365, 1095, 3650];

/// Which Recency bucket an age in days falls in, `0..8`.
fn age_bucket(days: i64) -> usize {
    AGE_EDGES.iter().filter(|&&edge| days >= edge).count()
}

/// One system's photometry, by the fallback chain: its scanned stars summed if
/// it has any, else the class it is named for, else a default.
///
/// `stars` is the `(absolute magnitude, temperature)` of every scanned star.
/// Their light adds, so the magnitudes combine to one figure and the tint is
/// the brightest star's, which dominates it. With no stars the primary class
/// stands in, and with no class the default M dwarf does.
///
/// `age_bucket` and `updated_at` are the two forms of one fact, as [`updated`]
/// settles them: the binned one the cell aggregates count by and the exact one
/// the payload carries.
fn system_input(
    address: i64,
    position: [f64; 3],
    primary_star_class: Option<&str>,
    stars: &[(f64, f64)],
    (age_bucket, updated_at): (usize, u32),
) -> System {
    let (absolute_magnitude, temperature) =
        match Magnitude::combine(stars.iter().map(|&(m, _)| Magnitude(m))) {
            Some(combined) => {
                let tint = stars
                    .iter()
                    .copied()
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, temperature)| temperature)
                    .expect("a combined magnitude means at least one star");
                (combined.0, tint)
            }
            None => {
                let light = ClassLight::of(primary_star_class.unwrap_or(""));
                (light.absolute_magnitude.0, light.temperature.0)
            }
        };
    System {
        id64: address as u64,
        position,
        absolute_magnitude,
        temperature,
        age_bucket,
        updated_at,
    }
}

/// How lately a system was updated, in the two forms the index wants it: the
/// Recency bucket the cell aggregates count by and the Unix second the payload
/// carries.
///
/// One reading of `updated_at`, so the two cannot disagree about a system. The
/// bucket alone was what the index carried and the buckets are days wide, which
/// answers a Recency span of thirty days and none of the five spans shorter
/// than a day — the end of the control the map is actually used at. So the
/// second goes on the payload point beside it.
///
/// `u32`, which is Unix seconds to 2106 and four bytes rather than eight on a
/// record of thirty-five. Clamped rather than wrapped: `updated_at` is a
/// timestamp off a journal entry and a client can write whatever it likes
/// there, and a year outside the `u32` range should read as the far end of the
/// axis rather than fold back into the middle of it.
fn updated(
    updated_at: chrono::NaiveDateTime,
    now: chrono::NaiveDateTime,
) -> (usize, u32) {
    (
        age_bucket((now - updated_at).num_days()),
        updated_at.and_utc().timestamp().clamp(0, u32::MAX as i64) as u32,
    )
}

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
    let updated_at: chrono::NaiveDateTime = row.try_get("updated_at")?;
    let system_stars = stars.get(&address).map(Vec::as_slice).unwrap_or(&[]);
    Ok(system_input(
        address,
        [x, y, z],
        class.as_deref(),
        system_stars,
        updated(updated_at, now),
    ))
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
pub async fn build_to_dir(
    db: &Database,
    dir: &Path,
    parts: Parts,
) -> Result<BuildReport> {
    // The one read the cell tree and the names table both come out of, and the
    // whole of what a build asking for neither can skip.
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

/// Build the index once, then keep it current as the feed writes to the
/// database, publishing what each round of changes touched.
///
/// This rides on top of the other sources rather than beside them: `galos-sync
/// eddn` writes systems to the database in real time, and `galos-sync db
/// --watch` follows the rows those writes leave behind. Two processes, or two
/// machines — the database is the only thing between them. The subcommands sit
/// in one program because they are the same sentence, not because a run of one
/// is a run of the other. It applies whatever is waiting since the cursor at once, then
/// every `interval` reads those changed since the previous pass, moves each in
/// the live [`Tree`] (a handful of cells apiece, not a rebuild), and writes only
/// the cells that changed. The clock is read before each query, so a write
/// racing the query is asked for again next pass rather than missed, and each
/// pass reads back a further [`CURSOR_OVERLAP`] to catch a write that committed
/// after the cursor was taken. Applying one twice is idempotent, so the overlap
/// costs a little work and no correctness.
///
/// The metadata beside the cells is kept current the same pass the cells are,
/// and the same way: `Metadata` holds the three tables open, a pass patches in
/// the systems that changed, and only what that moved is written — the names
/// chunks the arrivals landed in, the populated table when a political column
/// really did change, the factions when a new one is named, and a body file per
/// changed system. Nothing here reads the whole database twice.
///
/// On start it resumes from `checkpoint` when one is present and still reads as
/// the served directory's: the tree is rebuilt in memory from the checkpoint's
/// inputs and the cursor followed from there, so a restart costs a rebuild in
/// memory rather than a fresh read of the whole database and a rewrite of every
/// file. A missing, unreadable, or stale checkpoint falls back to a full build.
/// The checkpoint rides outside `dir`, is never served, and is written after the
/// initial build and every `CHECKPOINT_EVERY` thereafter — not every pass,
/// which would cost more than the publishing does.
pub async fn watch(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    interval: Duration,
) -> Result<()> {
    let params = BuildParams::default();
    let (mut tree, mut meta, mut since) = match resume(dir, checkpoint, &params)
    {
        Some((tree, meta, cursor)) => {
            info!(
                systems = tree.len(),
                cursor = %cursor,
                checkpoint = %checkpoint.display(),
                "resumed from checkpoint"
            );
            (tree, meta, cursor)
        }
        None => {
            let since = db.now().await?.naive_utc();
            info!(dir = %dir.display(), "building initial index (reading every system)");
            let start = Instant::now();
            let (inputs, names) = read_galaxy(db).await?;
            let mut tree = Tree::build(&inputs, &params);
            tree.write(dir)?;
            let (meta, report) = Metadata::build(db, dir, names).await?;
            Checkpoint { cursor: since, inputs }.write(checkpoint)?;
            info!(
                systems = tree.len(),
                chunks = report.name_chunks,
                elapsed = ?start.elapsed(),
                "initial index built"
            );
            (tree, meta, since)
        }
    };
    info!(
        dir = %dir.display(),
        interval_secs = interval.as_secs(),
        "watching for changes"
    );

    let mut checkpointed = Instant::now();
    loop {
        let now = db.now().await?.naive_utc();
        let touched = changed_addresses(db, since - CURSOR_OVERLAP).await?;
        if touched.is_empty() {
            debug!(since = %since, "polled, no changes");
        } else {
            let start = Instant::now();
            let changed = inputs_for(db, &touched).await?;
            tree.apply(&changed);
            tree.publish(dir)?;
            let report = meta.follow(db, dir, &touched).await?;
            let resumed_from = checkpointed.elapsed() >= CHECKPOINT_EVERY;
            if resumed_from {
                Checkpoint { cursor: now, inputs: tree.to_inputs() }
                    .write(checkpoint)?;
                checkpointed = Instant::now();
            }
            info!(
                changed = changed.len(),
                systems = tree.len(),
                chunks = report.name_chunks,
                bodies = report.body_files,
                checkpointed = resumed_from,
                elapsed = ?start.elapsed(),
                "index updated"
            );
        }
        since = now;
        async_std::task::sleep(interval).await;
    }
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
/// Hence the two gates. The directory must be internally consistent, its cell
/// tree and its names table standing for exactly the same systems, which is
/// what [`read_galaxy`] reading both out of one row makes an invariant and what
/// a half-written or older-layout directory fails. And it must be at or ahead
/// of the checkpoint, since a delta publish repairs only what the next changes
/// touch, not what is already wrong.
fn resume(
    dir: &Path,
    path: &Path,
    params: &BuildParams,
) -> Option<(Tree, Metadata, chrono::NaiveDateTime)> {
    let checkpoint = Checkpoint::read(path).ok()?;
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
    Some((tree, meta, checkpoint.cursor))
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

    /// Scanned stars sum to one magnitude and take the brightest's tint.
    #[test]
    fn scanned_stars_combine_and_take_the_brightest_tint() {
        // Two equal stars are about 0.75 mag brighter together than either.
        let stars = [(4.83, 5772.0), (4.83, 3000.0)];
        let s = system_input(42, [0.0; 3], Some("G"), &stars, (0, 0));
        assert!((s.absolute_magnitude - (4.83 - 0.7526)).abs() < 0.01);
        assert_eq!(s.id64, 42);

        // A distinct brightest star pins the tint to its temperature.
        let stars = [(2.0, 9000.0), (5.0, 3000.0)];
        let s = system_input(42, [0.0; 3], Some("G"), &stars, (0, 0));
        assert_eq!(s.temperature, 9000.0);
    }

    /// A starless system takes its named class.
    #[test]
    fn a_starless_system_falls_back_to_its_class() {
        let s = system_input(1, [0.0; 3], Some("M"), &[], (0, 0));
        let m = ClassLight::of("M");
        assert_eq!(s.absolute_magnitude, m.absolute_magnitude.0);
        assert_eq!(s.temperature, m.temperature.0);
    }

    /// No stars and no class is the default dwarf.
    #[test]
    fn no_stars_and_no_class_is_the_default_dwarf() {
        let s = system_input(1, [0.0; 3], None, &[], (0, 0));
        assert_eq!(
            s.absolute_magnitude,
            galos_photometry::ClassLight::DEFAULT.absolute_magnitude.0
        );
    }

    /// The Recency bucket climbs with the days since an update.
    #[test]
    fn age_buckets_climb_with_the_days() {
        assert_eq!(age_bucket(0), 0);
        assert_eq!(age_bucket(1), 1);
        assert_eq!(age_bucket(6), 1);
        assert_eq!(age_bucket(7), 2);
        assert_eq!(age_bucket(10_000), 7);
    }

    /// An update is carried to the second as well as binned
    ///
    /// The second is what the Recency filter tests, and its spans run from a
    /// minute to thirty days: five of the eight are shorter than the finest
    /// bucket, so the bucket cannot stand in for the stamp. Both come out of one
    /// reading of one column, so the cell aggregates and the payload cannot say
    /// different things about the same system.
    #[test]
    fn an_update_is_kept_to_the_second_as_well_as_binned() {
        let now = chrono::DateTime::from_timestamp(1_757_260_000, 0)
            .expect("a moment")
            .naive_utc();

        // Five minutes ago and now are the same bucket, and the stamp is the
        // whole of what tells them apart.
        let live = now - chrono::TimeDelta::minutes(5);
        assert_eq!(updated(live, now), (0, live.and_utc().timestamp() as u32));

        let week = now - chrono::TimeDelta::days(8);
        assert_eq!(updated(week, now), (2, week.and_utc().timestamp() as u32));

        // A journal entry can carry any timestamp its client cared to write.
        // Beyond the range of the stamp it reads as the far end of the axis
        // rather than folding back into the middle of it.
        let absurd = chrono::DateTime::from_timestamp(1i64 << 40, 0)
            .expect("a moment")
            .naive_utc();
        assert_eq!(updated(absurd, now), (0, u32::MAX));
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
}
