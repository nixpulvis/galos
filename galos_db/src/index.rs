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

use crate::{Database, Result};
use galos_index::{BuildParams, Snapshot, System, Tree};
use galos_photometry::{class_light, combined_magnitude};
use sqlx::Row;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

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
        match combined_magnitude(stars.iter().map(|&(m, _)| m)) {
            Some(combined) => {
                let tint = stars
                    .iter()
                    .copied()
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, temperature)| temperature)
                    .expect("a combined magnitude means at least one star");
                (combined, tint)
            }
            None => {
                let light = class_light(primary_star_class.unwrap_or(""));
                (light.absolute_magnitude, light.temperature)
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

/// Every scanned star grouped under its system: its `(absolute magnitude,
/// temperature)`, for the given addresses, or all systems when `None`.
///
/// A star missing a magnitude or a temperature cannot be summed, so it is left
/// out and its system falls to the class fallback like any other.
async fn stars_by_system(
    db: &Database,
    addresses: Option<&[i64]>,
) -> Result<HashMap<i64, Vec<(f64, f64)>>> {
    let rows = match addresses {
        None => {
            sqlx::query("SELECT system_address, absolute_magnitude, temperature FROM stars")
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
        if let (Some(m), Some(t)) = (magnitude, temperature) {
            stars.entry(address).or_default().push((m as f64, t as f64));
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

/// The addresses of systems changed since `since`: those whose own row moved or
/// whose stars did, since a scan re-magnitudes a system without touching its row.
async fn changed_addresses(
    db: &Database,
    since: chrono::NaiveDateTime,
) -> Result<Vec<i64>> {
    let rows = sqlx::query(
        "SELECT address FROM systems WHERE updated_at > $1 AND position IS NOT NULL \
         UNION \
         SELECT DISTINCT system_address FROM stars WHERE updated_at > $1",
    )
    .bind(since)
    .fetch_all(&db.pool)
    .await?;
    Ok(rows.iter().map(|row| row.get::<i64, _>("address")).collect())
}

/// Read every positioned system, with its stars, as build input.
pub async fn read_inputs(db: &Database) -> Result<Vec<System>> {
    let now = db.now().await?.naive_utc();
    let stars = stars_by_system(db, None).await?;
    let rows = sqlx::query(
        "SELECT address, \
                ST_X(position) AS x, ST_Y(position) AS y, ST_Z(position) AS z, \
                primary_star_class, updated_at \
         FROM systems WHERE position IS NOT NULL",
    )
    .fetch_all(&db.pool)
    .await?;
    rows.iter().map(|row| input_from_row(row, &stars, now)).collect()
}

/// Build the index from the database and write it to `dir`.
pub async fn build_to_dir(db: &Database, dir: &Path) -> Result<BuildReport> {
    let inputs = read_inputs(db).await?;
    let built = Snapshot::build(&inputs, &BuildParams::default());
    built.write(dir)?;
    Ok(BuildReport::of(inputs.len(), &built))
}

/// Read the systems changed since `since`, with their stars, as build input.
///
/// Each is rebuilt whole from its current record through the same fallback
/// [`read_inputs`] uses, so a system applied incrementally lands exactly where a
/// full rebuild would put it.
pub async fn read_changed(
    db: &Database,
    since: chrono::NaiveDateTime,
) -> Result<Vec<System>> {
    let now = db.now().await?.naive_utc();
    let addresses = changed_addresses(db, since).await?;
    if addresses.is_empty() {
        return Ok(Vec::new());
    }
    let stars = stars_by_system(db, Some(&addresses)).await?;
    let rows = sqlx::query(
        "SELECT address, \
                ST_X(position) AS x, ST_Y(position) AS y, ST_Z(position) AS z, \
                primary_star_class, updated_at \
         FROM systems WHERE address = ANY($1) AND position IS NOT NULL",
    )
    .bind(&addresses)
    .fetch_all(&db.pool)
    .await?;
    rows.iter().map(|row| input_from_row(row, &stars, now)).collect()
}

/// Build the index once, then keep it current as the feed writes to the
/// database, publishing what each round of changes touched.
///
/// This rides on top of the sync rather than inside it: `galos-sync` writes
/// systems to the database in real time, and this follows the rows those writes
/// leave behind. Every `interval` it reads the systems changed since the last
/// pass, moves each in the live [`Tree`] (a handful of cells apiece, not a
/// rebuild), and writes only the cells that changed. The clock is read before
/// each query, so a write racing the query is asked for again next pass rather
/// than missed, and applying it twice is idempotent.
pub async fn watch(db: &Database, dir: &Path, interval: Duration) -> Result<()> {
    let mut since = db.now().await?.naive_utc();
    let inputs = read_inputs(db).await?;
    let mut tree = Tree::build(&inputs, &BuildParams::default());
    tree.write(dir)?;
    eprintln!("index built: {} systems, watching for changes", inputs.len());

    loop {
        async_std::task::sleep(interval).await;
        let now = db.now().await?.naive_utc();
        let changed = read_changed(db, since).await?;
        if !changed.is_empty() {
            tree.apply(&changed);
            tree.publish(dir)?;
            eprintln!("index updated: {} changed, {} systems", changed.len(), tree.len());
        }
        since = now;
    }
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
}

impl BuildReport {
    fn of(systems: usize, built: &Snapshot) -> BuildReport {
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
        let m = class_light("M");
        assert_eq!(s.absolute_magnitude, m.absolute_magnitude);
        assert_eq!(s.temperature, m.temperature);
    }

    /// No stars and no class is the default dwarf.
    #[test]
    fn no_stars_and_no_class_is_the_default_dwarf() {
        let s = system_input(1, [0.0; 3], None, &[], 0);
        assert_eq!(
            s.absolute_magnitude,
            galos_photometry::DEFAULT_CLASS_LIGHT.absolute_magnitude
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
