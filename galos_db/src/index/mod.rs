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
//! the names table, the factions and the body files — is [`metadata`], which is
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

/// How long a watch goes between resume points, at most.
///
/// A checkpoint is every system at full precision, a hundred megabytes and
/// more, so writing one each pass costs more than everything else a pass does
/// put together. It is written on this timer instead. What that gives up is
/// paid back by the replay: a restart asks for the changes since the cursor the
/// checkpoint carries, applying one twice is idempotent, and the price of a
/// throttled checkpoint is a minute of the feed read a second time.
const CHECKPOINT_EVERY: Duration = Duration::from_secs(60);

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
fn system_input(
    address: i64,
    position: [f64; 3],
    primary_star_class: Option<&str>,
    stars: &[(f64, f64)],
    age_bucket: usize,
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
    }
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
/// and `updated_at`; `now` dates the Recency bucket and `stars` supplies any scan.
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
    let updated: chrono::NaiveDateTime = row.try_get("updated_at")?;
    let bucket = age_bucket((now - updated).num_days());
    let system_stars = stars.get(&address).map(Vec::as_slice).unwrap_or(&[]);
    Ok(system_input(address, [x, y, z], class.as_deref(), system_stars, bucket))
}

/// The addresses of systems changed since `since`: those whose own row moved,
/// whose stars did, since a scan re-magnitudes a system without touching its
/// row, and those whose factions did, since a faction is reported for a system
/// beside its row rather than in it.
///
/// A body scan is followed through the system row the sync writes beside it
/// rather than through `bodies` itself: that table is two million rows with no
/// index on `updated_at`, and every scan message names the system it is in, so
/// the row moves with the scan. Where the sync refuses such a write for being
/// older than the row it would replace, that system's metadata converges on the
/// next thing that touches it, every patch being rebuilt from the current row
/// rather than edited in place.
async fn changed_addresses(
    db: &Database,
    since: chrono::NaiveDateTime,
) -> Result<Vec<i64>> {
    let rows = sqlx::query(
        "SELECT address FROM systems \
         WHERE updated_at > $1 AND position IS NOT NULL \
         UNION \
         SELECT DISTINCT system_address FROM stars WHERE updated_at > $1 \
         UNION \
         SELECT DISTINCT system_address FROM system_factions \
         WHERE updated_at > $1",
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
pub async fn read_galaxy(
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

/// Build the index from the database and write it to `dir`, then the metadata
/// sidecars beside it: the cell tree the map draws from and the records a click
/// reads, written into one directory so a single transport serves both.
pub async fn build_to_dir(db: &Database, dir: &Path) -> Result<BuildReport> {
    let (inputs, names) = read_galaxy(db).await?;
    let built = Snapshot::build(&inputs, &BuildParams::default());
    built.write(dir)?;
    let (_, meta) = Metadata::build(db, dir, names).await?;
    Ok(BuildReport::of(inputs.len(), &built, meta))
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
/// This rides on top of the sync rather than inside it: `galos-sync` writes
/// systems to the database in real time, and this follows the rows those writes
/// leave behind. It applies whatever is waiting since the cursor at once, then
/// every `interval` reads those changed since the previous pass, moves each in
/// the live [`Tree`] (a handful of cells apiece, not a rebuild), and writes only
/// the cells that changed. The clock is read before each query, so a write
/// racing the query is asked for again next pass rather than missed, and
/// applying it twice is idempotent.
///
/// The metadata beside the cells is kept current the same pass the cells are,
/// and the same way: [`Metadata`] holds the three tables open, a pass patches in
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
/// initial build and every [`CHECKPOINT_EVERY`] thereafter — not every pass,
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
        let touched = changed_addresses(db, since).await?;
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

/// A summary of a build, for the binary to print and check.
#[derive(Copy, Clone, Debug)]
pub struct BuildReport {
    pub systems: usize,
    pub points: usize,
    pub cells: usize,
    pub leaves: usize,
    pub deepest_level: u8,
    pub max_leaf_points: usize,
    /// The metadata sidecars written beside the tree.
    pub meta: MetaReport,
}

impl BuildReport {
    fn of(systems: usize, built: &Snapshot, meta: MetaReport) -> BuildReport {
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
        BuildReport {
            systems,
            points: built.point_count(),
            cells: built.index.len(),
            leaves,
            deepest_level,
            max_leaf_points,
            meta,
        }
    }

    /// Whether every system landed in exactly one cell: the partition holds.
    pub fn is_consistent(&self) -> bool {
        self.points == self.systems
    }
}

impl fmt::Display for BuildReport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} systems -> {} cells ({} leaves, {} internal), \
             deepest level {}, largest leaf {} systems, {} placed{}; \
             metadata: {} populated, {} names, {} factions, {} body files",
            self.systems,
            self.cells,
            self.leaves,
            self.cells - self.leaves,
            self.deepest_level,
            self.max_leaf_points,
            self.points,
            if self.is_consistent() { "" } else { " (MISMATCH)" },
            self.meta.populated,
            self.meta.names,
            self.meta.factions,
            self.meta.body_files,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scanned stars sum to one magnitude and take the brightest's tint.
    #[test]
    fn scanned_stars_combine_and_take_the_brightest_tint() {
        // Two equal stars are about 0.75 mag brighter together than either.
        let stars = [(4.83, 5772.0), (4.83, 3000.0)];
        let s = system_input(42, [0.0; 3], Some("G"), &stars, 0);
        assert!((s.absolute_magnitude - (4.83 - 0.7526)).abs() < 0.01);
        assert_eq!(s.id64, 42);

        // A distinct brightest star pins the tint to its temperature.
        let stars = [(2.0, 9000.0), (5.0, 3000.0)];
        let s = system_input(42, [0.0; 3], Some("G"), &stars, 0);
        assert_eq!(s.temperature, 9000.0);
    }

    /// A starless system takes its named class.
    #[test]
    fn a_starless_system_falls_back_to_its_class() {
        let s = system_input(1, [0.0; 3], Some("M"), &[], 0);
        let m = ClassLight::of("M");
        assert_eq!(s.absolute_magnitude, m.absolute_magnitude.0);
        assert_eq!(s.temperature, m.temperature.0);
    }

    /// No stars and no class is the default dwarf.
    #[test]
    fn no_stars_and_no_class_is_the_default_dwarf() {
        let s = system_input(1, [0.0; 3], None, &[], 0);
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
}
