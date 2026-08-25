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

use crate::barycenters::Barycenter;
use crate::bodies::{composition, Body, Parent, Surface};
use crate::stars::Star;
use crate::{orbit, Database, Result};
use elite_journal::body::{Discovery, Material, Orbit, Spin};
use galos_index::source::write_meta;
use galos_index::{meta, source, BuildParams, Snapshot, System, Tree};
use galos_photometry::{class_light, combined_magnitude};
use sqlx::postgres::PgRow;
use sqlx::Row;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::{debug, info};

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
        None => sqlx::query(
            "SELECT system_address, absolute_magnitude, temperature FROM stars",
        )
        .fetch_all(&db.pool)
        .await?,
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

/// Build the index from the database and write it to `dir`, then the metadata
/// sidecars beside it: the cell tree the map draws from and the records a click
/// reads, written into one directory so a single transport serves both.
pub async fn build_to_dir(db: &Database, dir: &Path) -> Result<BuildReport> {
    let inputs = read_inputs(db).await?;
    let built = Snapshot::build(&inputs, &BuildParams::default());
    built.write(dir)?;
    let meta = write_metadata(db, dir, None).await?;
    Ok(BuildReport::of(inputs.len(), &built, meta))
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
///
/// The metadata sidecars are refreshed the same pass the cells are. The three
/// tables are derived wholesale from the current database, so each is rewritten
/// whole rather than patched, and the per-system body files are rewritten for
/// exactly the addresses that changed. Rewriting `names.bin` whole every pass is
/// a known interim cost: it holds every positioned system, so a single changed
/// system reserializes the lot, and the price is paid until the transport grows
/// a way to publish a delta into it.
pub async fn watch(
    db: &Database,
    dir: &Path,
    interval: Duration,
) -> Result<()> {
    let mut since = db.now().await?.naive_utc();
    info!(dir = %dir.display(), "building initial index (reading every system)");
    let start = Instant::now();
    let inputs = read_inputs(db).await?;
    let mut tree = Tree::build(&inputs, &BuildParams::default());
    tree.write(dir)?;
    write_metadata(db, dir, None).await?;
    info!(
        systems = inputs.len(),
        cells = tree.len(),
        elapsed = ?start.elapsed(),
        "initial index built"
    );
    info!(
        dir = %dir.display(),
        interval_secs = interval.as_secs(),
        "watching for changes"
    );

    loop {
        async_std::task::sleep(interval).await;
        let now = db.now().await?.naive_utc();
        let changed = read_changed(db, since).await?;
        if changed.is_empty() {
            debug!(since = %since, "polled, no changes");
        } else {
            let start = Instant::now();
            tree.apply(&changed);
            tree.publish(dir)?;
            let touched = changed_addresses(db, since).await?;
            write_metadata(db, dir, Some(&touched)).await?;
            info!(
                changed = changed.len(),
                systems = tree.len(),
                elapsed = ?start.elapsed(),
                "index updated"
            );
        }
        since = now;
    }
}

/// The metadata sidecars, and how much of each was written.
///
/// The four artifacts a click into the map reads, counted so the build tool
/// prints proof that each was written: the populated table, the name-and-place
/// table, the faction names, and the per-system body files.
#[derive(Copy, Clone, Debug)]
pub struct MetaReport {
    pub populated: usize,
    pub names: usize,
    pub factions: usize,
    pub body_files: usize,
}

/// Write the four metadata artifacts beside the cell tree in `dir`.
///
/// The populated, names and faction tables are each derived wholesale from the
/// current database and written whole. `bodies_for` decides the body files:
/// [`None`] rebuilds every system's, which is what a full build wants, and
/// [`Some`] rewrites only the given addresses', which is what a watch pass
/// wants once it knows what changed.
async fn write_metadata(
    db: &Database,
    dir: &Path,
    bodies_for: Option<&[i64]>,
) -> Result<MetaReport> {
    Ok(MetaReport {
        populated: write_populated(db, dir).await?,
        names: write_names(db, dir).await?,
        factions: write_factions(db, dir).await?,
        body_files: write_bodies(db, dir, bodies_for).await?,
    })
}

/// Write `populated.bin`: the dynamic set the map colours and navigates by.
///
/// Every system with a population and a place, with the political columns a
/// filter reads and the ids of the named factions present in it, gathered in
/// one query rather than one per system. Only factions with a row in
/// `factions` are carried: `system_factions` holds ids EDDN has reported a
/// system for but never named, and the client can neither name nor filter by
/// one, so it would only stand as an unreadable line in a panel. A population
/// without a position is left out: the map only ever colours a system it
/// draws, and it draws only positioned ones, so a [`meta::PopulatedSystem`]
/// carries a fixed `[f32; 3]` and never an absent one.
/// The reach is the far edge of what is on record, read for the whole set at
/// once by [`reaches`].
async fn write_populated(db: &Database, dir: &Path) -> Result<usize> {
    let rows = sqlx::query(
        "SELECT address, name, \
                ST_X(position) AS x, ST_Y(position) AS y, ST_Z(position) AS z, \
                population, security, government, allegiance, \
                primary_economy, secondary_economy, body_count, non_body_count, \
                COALESCE( \
                    (SELECT array_agg(sf.faction_id) FROM system_factions sf \
                     WHERE sf.system_address = systems.address \
                       AND EXISTS ( \
                           SELECT 1 FROM factions f WHERE f.id = sf.faction_id \
                       )), \
                    ARRAY[]::integer[] \
                ) AS factions \
         FROM systems WHERE population > 0 AND position IS NOT NULL",
    )
    .fetch_all(&db.pool)
    .await?;

    let addresses: Vec<i64> =
        rows.iter().map(|row| row.get::<i64, _>("address")).collect();
    let reach = reaches(db, &addresses).await?;

    let populated = rows
        .iter()
        .map(|row| -> Result<meta::PopulatedSystem> {
            let address: i64 = row.try_get("address")?;
            let x: f64 = row.try_get("x")?;
            let y: f64 = row.try_get("y")?;
            let z: f64 = row.try_get("z")?;
            let population: i64 = row.try_get("population")?;
            Ok(meta::PopulatedSystem {
                address,
                name: row.try_get("name")?,
                position: [x as f32, y as f32, z as f32],
                population: population as u64,
                security: row.try_get("security")?,
                government: row.try_get("government")?,
                allegiance: row.try_get("allegiance")?,
                primary_economy: row.try_get("primary_economy")?,
                secondary_economy: row.try_get("secondary_economy")?,
                factions: row.try_get("factions")?,
                body_count: row.try_get("body_count")?,
                non_body_count: row.try_get("non_body_count")?,
                reach: reach.get(&address).copied(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    write_meta(&source::populated_path(dir), &populated)?;
    Ok(populated.len())
}

/// Write `names.bin`: the name and place of every positioned system, which is
/// the search index and the routing graph in one. Every positioned system, not
/// just the populated ones, since a search reaches any name and a route steps
/// between any two places.
async fn write_names(db: &Database, dir: &Path) -> Result<usize> {
    let rows = sqlx::query(
        "SELECT address, name, \
                ST_X(position) AS x, ST_Y(position) AS y, ST_Z(position) AS z \
         FROM systems WHERE position IS NOT NULL",
    )
    .fetch_all(&db.pool)
    .await?;

    let names = rows
        .iter()
        .map(|row| -> Result<meta::NameEntry> {
            let x: f64 = row.try_get("x")?;
            let y: f64 = row.try_get("y")?;
            let z: f64 = row.try_get("z")?;
            Ok(meta::NameEntry {
                address: row.try_get("address")?,
                name: row.try_get("name")?,
                position: [x as f32, y as f32, z as f32],
            })
        })
        .collect::<Result<Vec<_>>>()?;

    write_meta(&source::names_path(dir), &names)?;
    Ok(names.len())
}

/// Write `factions.bin`: the whole faction id-to-name table, small and read
/// whole by the client that looks a system's faction ids up in it.
async fn write_factions(db: &Database, dir: &Path) -> Result<usize> {
    let rows = sqlx::query("SELECT id, name FROM factions")
        .fetch_all(&db.pool)
        .await?;

    let factions = rows
        .iter()
        .map(|row| -> Result<meta::Faction> {
            Ok(meta::Faction {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    write_meta(&source::factions_path(dir), &factions)?;
    Ok(factions.len())
}

/// Write `bodies/<address>.bin`: one [`meta::SystemBodies`] per system that has
/// any stars, bodies or barycenters on record.
///
/// The three kinds are read in bulk, ordered by system and grouped in memory,
/// so a system with a hundred bodies costs one row per body of one query rather
/// than a query of its own. `addresses` is [`None`] for a full build, which
/// reads every system, and [`Some`] for a watch pass, which reads only what
/// changed and removes the file of any changed address left with nothing, so a
/// system whose last scan was withdrawn stops reading as one that still has it.
async fn write_bodies(
    db: &Database,
    dir: &Path,
    addresses: Option<&[i64]>,
) -> Result<usize> {
    let mut grouped: HashMap<i64, meta::SystemBodies> = HashMap::new();
    for star in all_stars(db, addresses).await? {
        grouped
            .entry(star.system_address)
            .or_default()
            .stars
            .push(meta_star(star));
    }
    for body in all_bodies(db, addresses).await? {
        grouped
            .entry(body.system_address)
            .or_default()
            .bodies
            .push(meta_body(body));
    }
    for barycenter in all_barycenters(db, addresses).await? {
        grouped
            .entry(barycenter.system_address)
            .or_default()
            .barycenters
            .push(meta_barycenter(barycenter));
    }

    std::fs::create_dir_all(dir.join(source::BODIES_DIR))?;
    for (address, system_bodies) in &grouped {
        write_meta(&source::bodies_path(dir, *address), system_bodies)?;
    }

    if let Some(addresses) = addresses {
        for &address in addresses {
            if !grouped.contains_key(&address) {
                let path = source::bodies_path(dir, address);
                if path.exists() {
                    std::fs::remove_file(path)?;
                }
            }
        }
    }

    Ok(grouped.len())
}

/// How far each of `addresses` reaches from its arrival star, in metres: the
/// far edge of the furthest thing on record, over bodies, stars and the points
/// a close pair goes round. One grouped query rather than one per system, the
/// same shape [`crate::systems`] reads a drawn region's reaches with.
async fn reaches(
    db: &Database,
    addresses: &[i64],
) -> Result<HashMap<i64, f32>> {
    let rows = sqlx::query(
        "SELECT system_address AS address, \
                MAX(GREATEST(away, apoapsis) + radius) AS reach \
         FROM ( \
             SELECT system_address, \
                    (COALESCE(distance_from_arrival, 0) * 299792458)::real \
                        AS away, \
                    (semi_major_axis \
                        * (1 + LEAST(eccentricity, 0.99)))::real AS apoapsis, \
                    radius \
             FROM bodies WHERE system_address = ANY($1) \
           UNION ALL \
             SELECT system_address, \
                    (distance_from_arrival_ls * 299792458)::real, \
                    (COALESCE(semi_major_axis, 0) \
                        * (1 + LEAST(COALESCE(eccentricity, 0), 0.99)))::real, \
                    radius \
             FROM stars WHERE system_address = ANY($1) \
           UNION ALL \
             SELECT system_address, \
                    0::real, \
                    (COALESCE(semi_major_axis, 0) \
                        * (1 + LEAST(COALESCE(eccentricity, 0), 0.99)))::real, \
                    0::real \
             FROM barycenters WHERE system_address = ANY($1) \
         ) reaching \
         GROUP BY system_address",
    )
    .bind(addresses)
    .fetch_all(&db.pool)
    .await?;

    rows.iter()
        .map(|row| Ok((row.try_get("address")?, row.try_get("reach")?)))
        .collect()
}

/// Every star, grouped under its system by the caller: all of them for a full
/// build, or those of `addresses` for a watch pass. Read in bulk and mapped the
/// same way [`crate::stars`] maps a single-system read, so a star lands here
/// exactly as it would through [`crate::stars::Star::fetch_all`].
async fn all_stars(
    db: &Database,
    addresses: Option<&[i64]>,
) -> Result<Vec<Star>> {
    let rows = match addresses {
        None => {
            sqlx::query("SELECT * FROM stars ORDER BY system_address")
                .fetch_all(&db.pool)
                .await?
        }
        Some(addresses) => {
            sqlx::query(
                "SELECT * FROM stars WHERE system_address = ANY($1) \
             ORDER BY system_address",
            )
            .bind(addresses)
            .fetch_all(&db.pool)
            .await?
        }
    };
    rows.iter().map(star_from_row).collect()
}

/// Every body, with what it is made of gathered alongside, grouped under its
/// system by the caller. The materials join and the grouping are [`crate::bodies`]'s
/// own; only the `WHERE` differs, being over a set of systems rather than one.
async fn all_bodies(
    db: &Database,
    addresses: Option<&[i64]>,
) -> Result<Vec<Body>> {
    let select = "SELECT b.*, \
                COALESCE(ARRAY_AGG(m.name ORDER BY m.name) \
                    FILTER (WHERE m.name IS NOT NULL), '{}') AS material_names, \
                COALESCE(ARRAY_AGG(m.percent ORDER BY m.name) \
                    FILTER (WHERE m.name IS NOT NULL), '{}') AS material_percents \
         FROM bodies b \
         LEFT JOIN body_materials m \
             ON m.system_address = b.system_address AND m.body_id = b.id";
    let rows = match addresses {
        None => {
            sqlx::query(&format!(
            "{select} GROUP BY b.system_address, b.id ORDER BY b.system_address"
        ))
            .fetch_all(&db.pool)
            .await?
        }
        Some(addresses) => {
            sqlx::query(&format!(
                "{select} WHERE b.system_address = ANY($1) \
             GROUP BY b.system_address, b.id ORDER BY b.system_address"
            ))
            .bind(addresses)
            .fetch_all(&db.pool)
            .await?
        }
    };
    rows.iter().map(body_from_row).collect()
}

/// Every barycenter, grouped under its system by the caller, mapped as
/// [`crate::barycenters::Barycenter::fetch_all`] maps one system's.
async fn all_barycenters(
    db: &Database,
    addresses: Option<&[i64]>,
) -> Result<Vec<Barycenter>> {
    let rows = match addresses {
        None => {
            sqlx::query("SELECT * FROM barycenters ORDER BY system_address")
                .fetch_all(&db.pool)
                .await?
        }
        Some(addresses) => {
            sqlx::query(
                "SELECT * FROM barycenters WHERE system_address = ANY($1) \
             ORDER BY system_address",
            )
            .bind(addresses)
            .fetch_all(&db.pool)
            .await?
        }
    };
    rows.iter().map(barycenter_from_row).collect()
}

/// One `stars` row as a [`Star`], reading the columns by name where the checked
/// query reads them by macro. The orbit is whole or absent by [`orbit::read`],
/// the primary going round nothing.
fn star_from_row(row: &PgRow) -> Result<Star> {
    let parent_ids: Option<Vec<i16>> = row.try_get("parent_ids")?;
    let parent_types: Option<Vec<String>> = row.try_get("parent_types")?;
    let updated_at: chrono::NaiveDateTime = row.try_get("updated_at")?;
    Ok(Star {
        system_address: row.try_get("system_address")?,
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        parents: Parent::rows(parent_ids, parent_types),
        updated_at: updated_at.and_utc(),
        updated_by: row.try_get("updated_by")?,
        absolute_magnitude: row.try_get("absolute_magnitude")?,
        age_my: row.try_get("age_my")?,
        distance_from_arrival_ls: row.try_get("distance_from_arrival_ls")?,
        luminosity: row.try_get("luminosity")?,
        star_class: row.try_get("star_class")?,
        stellar_mass: row.try_get("stellar_mass")?,
        subclass: row.try_get("subclass")?,
        orbit: orbit::read(
            row.try_get("semi_major_axis")?,
            row.try_get("eccentricity")?,
            row.try_get("orbital_inclination")?,
            row.try_get("periapsis")?,
            row.try_get("orbital_period")?,
            row.try_get("ascending_node")?,
            row.try_get("mean_anomaly")?,
        ),
        spin: Spin {
            period: row.try_get("rotation_period")?,
            tilt: row.try_get("axial_tilt")?,
        },
        radius: row.try_get("radius")?,
        temperature: row.try_get("temperature")?,
        discovery: Discovery {
            discovered: row.try_get("was_discovered")?,
            mapped: row.try_get("was_mapped")?,
        },
    })
}

/// One `bodies` row, with its materials already gathered into the two arrays
/// this reads, as a [`Body`]. The surface reads as absent where a gas giant
/// carries none, and the body's orbit is always present, a body going round
/// something by definition.
fn body_from_row(row: &PgRow) -> Result<Body> {
    let parent_ids: Option<Vec<i16>> = row.try_get("parent_ids")?;
    let parent_types: Option<Vec<String>> = row.try_get("parent_types")?;
    let updated_at: chrono::NaiveDateTime = row.try_get("updated_at")?;
    let body_type: Option<String> = row.try_get("body_type")?;
    let material_names: Vec<String> = row.try_get("material_names")?;
    let material_percents: Vec<f64> = row.try_get("material_percents")?;
    let materials = material_names
        .into_iter()
        .zip(material_percents)
        .map(|(name, percent)| Material { name, percent })
        .collect();
    Ok(Body {
        system_address: row.try_get("system_address")?,
        id: row.try_get("id")?,
        parents: Parent::rows(parent_ids, parent_types),
        name: row.try_get("name")?,
        body_type: body_type.map(|ty| ty.as_str().into()),
        distance_from_arrival: row.try_get("distance_from_arrival")?,
        updated_at: updated_at.and_utc(),
        updated_by: row.try_get("updated_by")?,
        planet_class: row.try_get("planet_class")?,
        tidal_lock: row.try_get("tidal_lock")?,
        surface: Surface::read(
            row.try_get("atmosphere_type")?,
            row.try_get("surface_pressure")?,
            composition(
                row.try_get("composition_ice")?,
                row.try_get("composition_rock")?,
                row.try_get("composition_metal")?,
            ),
            row.try_get("landable")?,
            row.try_get("atmosphere")?,
            row.try_get("volcanism")?,
            row.try_get("terraform_state")?,
            materials,
        ),
        mass: row.try_get("mass")?,
        radius: row.try_get("radius")?,
        gravity: row.try_get("gravity")?,
        temperature: row.try_get("temperature")?,
        orbit: Orbit {
            semi_major_axis: row.try_get("semi_major_axis")?,
            eccentricity: row.try_get("eccentricity")?,
            orbital_inclination: row.try_get("orbital_inclination")?,
            periapsis: row.try_get("periapsis")?,
            orbital_period: row.try_get("orbital_period")?,
            ascending_node: row.try_get("ascending_node")?,
            mean_anomaly: row.try_get("mean_anomaly")?,
        },
        spin: Spin {
            period: row.try_get("rotation_period")?,
            tilt: row.try_get("axial_tilt")?,
        },
        discovery: Discovery {
            discovered: row.try_get("was_discovered")?,
            mapped: row.try_get("was_mapped")?,
        },
    })
}

/// One `barycenters` row as a [`Barycenter`], its orbit whole or absent, the
/// one at the root of a multi-star system going round nothing.
fn barycenter_from_row(row: &PgRow) -> Result<Barycenter> {
    let updated_at: chrono::NaiveDateTime = row.try_get("updated_at")?;
    Ok(Barycenter {
        system_address: row.try_get("system_address")?,
        id: row.try_get("id")?,
        updated_at: updated_at.and_utc(),
        updated_by: row.try_get("updated_by")?,
        orbit: orbit::read(
            row.try_get("semi_major_axis")?,
            row.try_get("eccentricity")?,
            row.try_get("orbital_inclination")?,
            row.try_get("periapsis")?,
            row.try_get("orbital_period")?,
            row.try_get("ascending_node")?,
            row.try_get("mean_anomaly")?,
        ),
    })
}

/// A [`Parent`] as its field-identical [`meta::Parent`]. The two say the same
/// thing on either side of the transport and differ only in which crate names
/// the type.
fn meta_parent(parent: Parent) -> meta::Parent {
    meta::Parent { ty: parent.ty, id: parent.id }
}

/// A [`Surface`] as its field-identical [`meta::Surface`]. The `elite_journal`
/// types it carries are shared, so they pass through unchanged.
fn meta_surface(surface: Surface) -> meta::Surface {
    meta::Surface {
        atmosphere_type: surface.atmosphere_type,
        pressure: surface.pressure,
        composition: surface.composition,
        landable: surface.landable,
        atmosphere: surface.atmosphere,
        volcanism: surface.volcanism,
        terraform_state: surface.terraform_state,
        materials: surface.materials,
    }
}

/// A [`Star`] as its field-identical [`meta::Star`].
fn meta_star(star: Star) -> meta::Star {
    meta::Star {
        system_address: star.system_address,
        id: star.id,
        name: star.name,
        parents: star.parents.into_iter().map(meta_parent).collect(),
        updated_at: star.updated_at,
        updated_by: star.updated_by,
        absolute_magnitude: star.absolute_magnitude,
        age_my: star.age_my,
        distance_from_arrival_ls: star.distance_from_arrival_ls,
        luminosity: star.luminosity,
        star_class: star.star_class,
        stellar_mass: star.stellar_mass,
        subclass: star.subclass,
        orbit: star.orbit,
        spin: star.spin,
        radius: star.radius,
        temperature: star.temperature,
        discovery: star.discovery,
    }
}

/// A [`Body`] as its field-identical [`meta::Body`].
fn meta_body(body: Body) -> meta::Body {
    meta::Body {
        system_address: body.system_address,
        id: body.id,
        parents: body.parents.into_iter().map(meta_parent).collect(),
        name: body.name,
        body_type: body.body_type,
        distance_from_arrival: body.distance_from_arrival,
        updated_at: body.updated_at,
        updated_by: body.updated_by,
        planet_class: body.planet_class,
        tidal_lock: body.tidal_lock,
        mass: body.mass,
        radius: body.radius,
        gravity: body.gravity,
        temperature: body.temperature,
        surface: body.surface.map(meta_surface),
        orbit: body.orbit,
        spin: body.spin,
        discovery: body.discovery,
    }
}

/// A [`Barycenter`] as its field-identical [`meta::Barycenter`].
fn meta_barycenter(barycenter: Barycenter) -> meta::Barycenter {
    meta::Barycenter {
        system_address: barycenter.system_address,
        id: barycenter.id,
        updated_at: barycenter.updated_at,
        updated_by: barycenter.updated_by,
        orbit: barycenter.orbit,
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

    /// A system's bodies survive the trip out to disk and back through the
    /// same path helpers, format and reader the client uses.
    ///
    /// No database: the galos_db values are built by hand, converted to their
    /// metadata form, written with [`write_meta`] under [`source::bodies_path`],
    /// and read back through a [`FsSource`], the way a click into the map reads
    /// them. It proves the conversion is faithful and the two halves of the
    /// transport agree on the layout and the encoding.
    ///
    /// The body carried through the full disk round trip has neither a body
    /// type nor a surface, because [`BodyType`] and [`AtmosphereType`] carry an
    /// untagged `Unknown(String)` variant, and an untagged variant can only be
    /// deserialized from a self-describing format, which postcard is not. Those
    /// two fields serialize but do not read back, which the surfaced body below
    /// covers as far as this crate can: the write and the conversion, not the
    /// read, which is [`galos_index`]'s half.
    #[async_std::test]
    async fn a_systems_bodies_round_trip_through_the_fs_source() {
        use elite_journal::body::{AtmosphereType, BodyType, Composition};
        use galos_index::{FsSource, Source};

        let at = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let address = 0x1234_5678_9abc_def0_i64;

        let star = Star {
            system_address: address,
            id: 0,
            name: "Test A".to_string(),
            parents: vec![],
            updated_at: at,
            updated_by: "test".to_string(),
            absolute_magnitude: 4.83,
            age_my: 4600,
            distance_from_arrival_ls: 0.0,
            luminosity: "V".to_string(),
            star_class: "G".to_string(),
            stellar_mass: 1.0,
            subclass: 2,
            orbit: None,
            spin: Spin { period: 25.0, tilt: 0.1 },
            radius: 6.96e8,
            temperature: 5772.0,
            discovery: Discovery { discovered: true, mapped: false },
        };

        let barycenter = Barycenter {
            system_address: address,
            id: 1,
            updated_at: at,
            updated_by: "test".to_string(),
            orbit: None,
        };

        // A gas giant: no surface and no body type, so nothing here leans on an
        // untagged enum, and the whole thing reads back.
        let gas_giant = Body {
            system_address: address,
            id: 4,
            parents: vec![Parent { ty: Some("Star".to_string()), id: 0 }],
            name: "Test A 4".to_string(),
            body_type: None,
            distance_from_arrival: Some(2000.0),
            updated_at: at,
            updated_by: "test".to_string(),
            planet_class: "Gas giant".to_string(),
            tidal_lock: false,
            mass: 317.8,
            radius: 7.1e7,
            gravity: 24.8,
            temperature: Some(165.0),
            surface: None,
            orbit: Orbit {
                semi_major_axis: 7.8e11,
                eccentricity: 0.048,
                orbital_inclination: 1.3,
                periapsis: 275.0,
                orbital_period: 3.7e8,
                ascending_node: Some(100.0),
                mean_anomaly: Some(20.0),
            },
            spin: Spin { period: 0.4, tilt: 3.1 },
            discovery: Discovery { discovered: true, mapped: false },
        };

        let want = meta::SystemBodies {
            stars: vec![meta_star(star)],
            bodies: vec![meta_body(gas_giant)],
            barycenters: vec![meta_barycenter(barycenter)],
        };

        let dir = std::env::temp_dir().join(format!(
            "galos_db_index_test_{}_{}",
            std::process::id(),
            at.timestamp_nanos_opt().unwrap_or(0),
        ));
        std::fs::create_dir_all(dir.join(source::BODIES_DIR)).unwrap();
        write_meta(&source::bodies_path(&dir, address), &want).unwrap();

        let fs = FsSource::new(&dir);
        let got = fs.bodies(address).await.unwrap();
        assert_eq!(got, want);

        // A system with no file reads as one with nothing, not an error.
        let empty = fs.bodies(address + 1).await.unwrap();
        assert_eq!(empty, meta::SystemBodies::default());

        // A surfaced body converts field-for-field and serializes; only the
        // read half of the two untagged fields is beyond this crate.
        let surfaced = Body {
            system_address: address,
            id: 3,
            parents: vec![
                Parent { ty: Some("Null".to_string()), id: 1 },
                Parent { ty: Some("Star".to_string()), id: 0 },
            ],
            name: "Test A 3".to_string(),
            body_type: Some(BodyType::from("Planet")),
            distance_from_arrival: Some(499.0),
            updated_at: at,
            updated_by: "test".to_string(),
            planet_class: "Earthlike body".to_string(),
            tidal_lock: false,
            mass: 1.0,
            radius: 6.37e6,
            gravity: 9.81,
            temperature: Some(288.0),
            surface: Surface::read(
                Some("Oxygen".to_string()),
                Some(1.0),
                Some(Composition { ice: 0.1, rock: 0.7, metal: 0.2 }),
                true,
                None,
                None,
                Some("Terraformable".to_string()),
                vec![Material { name: "iron".to_string(), percent: 12.5 }],
            ),
            orbit: Orbit {
                semi_major_axis: 1.5e11,
                eccentricity: 0.017,
                orbital_inclination: 0.0,
                periapsis: 114.0,
                orbital_period: 3.15e7,
                ascending_node: Some(-11.0),
                mean_anomaly: Some(358.0),
            },
            spin: Spin { period: 1.0, tilt: 23.4 },
            discovery: Discovery { discovered: true, mapped: true },
        };
        let meta_surfaced = meta_body(surfaced);
        assert_eq!(meta_surfaced.body_type, Some(BodyType::from("Planet")));
        let surface = meta_surfaced.surface.as_ref().unwrap();
        assert_eq!(surface.atmosphere_type, AtmosphereType::from("Oxygen"));
        assert_eq!(
            surface.composition,
            Some(Composition { ice: 0.1, rock: 0.7, metal: 0.2 }),
        );
        assert_eq!(
            surface.materials,
            vec![Material { name: "iron".to_string(), percent: 12.5 }],
        );
        assert_eq!(surface.terraform_state.as_deref(), Some("Terraformable"));
        assert_eq!(meta_surfaced.parents[0].ty.as_deref(), Some("Null"));
        let surfaced_bodies = meta::SystemBodies {
            bodies: vec![meta_surfaced],
            ..Default::default()
        };
        write_meta(&source::bodies_path(&dir, address + 2), &surfaced_bodies)
            .expect(
                "a surfaced body serializes even where its read half does not",
            );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
