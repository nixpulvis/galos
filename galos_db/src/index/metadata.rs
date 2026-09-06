//! The serving metadata beside the cell tree, and how a watch keeps it current.
//!
//! The cells carry what the map draws. These carry what a click wants: the
//! populated table the map colours and filters by, the names table a search and
//! a route read, the faction names, and one file of bodies per system.
//!
//! All four were derived wholesale from the database every time the index
//! published, which a full build wants and a watch cannot afford: the names
//! table alone is every positioned system, so a pass that had fifty arrivals to
//! publish read two million rows and rewrote a hundred megabytes to say so. So
//! a watch holds the three tables open here — [`Metadata`] — and
//! [`follow`](Metadata::follow) reads only the systems that changed, patches
//! them in, and writes only what that moved: the names chunks the arrivals
//! landed in, the populated table when a political column really did change,
//! the faction names when a new one is reported. The body files were already
//! written per changed system and stay that way.
//!
//! Patching rests on the changed set being complete, which is
//! [`changed_addresses`](super::changed_addresses)'s business, and on a table
//! being able to tell that a system reported again says nothing new, which is
//! each record's `PartialEq`. Where the feed does slip a change past the cursor
//! — a write refused for being older than the row it would replace, which the
//! sync counts — the system converges the next time anything touches it, since
//! every patch rebuilds its whole record from the current row rather than
//! editing what is held.

use crate::barycenters::Barycenter;
use crate::bodies::{composition, Body, Parent, Surface};
use crate::stars::Star;
use crate::{orbit, Database, Result};
use elite_journal::body::{Discovery, Material, Orbit, Spin};
use galos_index::source::{read_meta, write_meta};
use galos_index::{meta, source, NameTable};
use sqlx::postgres::PgRow;
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

/// The columns a [`meta::NameEntry`] is read from.
const NAMES_SELECT: &str = "SELECT address, name, \
     ST_X(position) AS x, ST_Y(position) AS y, ST_Z(position) AS z \
     FROM systems";

/// The columns a [`meta::PopulatedSystem`] is read from.
///
/// Only factions with a row in `factions` are carried: `system_factions` holds
/// ids EDDN has reported a system for but never named, and the client can
/// neither name nor filter by one, so it would only stand as an unreadable line
/// in a panel. They are gathered here rather than by a query per system.
const POPULATED_SELECT: &str = "SELECT address, name, \
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
     FROM systems";

/// The metadata artifacts, and how much of each a publish wrote.
///
/// The tables are counted whole, since that is what stands in the directory
/// after the publish and what the build tool prints as proof each was written.
/// `name_chunks` and `body_files` are what the publish actually touched, which
/// is the whole point of a watch pass: a few files, not the galaxy.
#[derive(Copy, Clone, Debug)]
pub struct MetaReport {
    pub populated: usize,
    pub names: usize,
    pub factions: usize,
    /// How many systems have a reach on record, which is every scanned one.
    pub reaches: usize,
    pub body_files: usize,
    /// How many of the names table's chunks were written.
    pub name_chunks: usize,
}

/// The four metadata tables, held open across a watch.
///
/// Each is the authority on what stands in the published directory: a patch
/// reads the changed systems' current rows, moves the tables, and writes what
/// moved. Held rather than re-derived, because deriving any of them is a read
/// of the whole `systems` table.
pub(super) struct Metadata {
    names: NameTable,
    /// The populated table by address, materialised in address order when
    /// written. Keyed rather than chunked: it is a fortieth of the names table,
    /// and its rows change with the feed rather than only arriving, so there is
    /// no tail for changes to cluster in.
    populated: HashMap<i64, meta::PopulatedSystem>,
    /// How far each scanned system reaches, by address. Keyed for the same
    /// reason the populated table is, and more so: a reach moves whenever a
    /// body is scanned, and the systems scanned in a pass are scattered across
    /// the whole address space. Chunked alongside the names it would dirty
    /// chunks all over the galaxy every pass, which is what chunking that
    /// table was for.
    reaches: HashMap<i64, f32>,
    /// The faction names, in id order. Ids come from a sequence and a name is
    /// never rewritten (`Faction::create` conflicts onto the name on record),
    /// so this only ever grows, past `high`.
    factions: Vec<meta::Faction>,
    /// The highest faction id read.
    high: i32,
}

impl Metadata {
    /// Derive the tables from the database and write everything: every names
    /// chunk, all three whole tables, and a body file for every system that
    /// has anything on record. What a full build publishes.
    ///
    /// `names` comes from the caller rather than a read of its own, being the
    /// other half of the read the cell tree was built from; see
    /// [`read_galaxy`](super::read_galaxy).
    pub(super) async fn build(
        db: &Database,
        dir: &Path,
        names: Vec<meta::NameEntry>,
    ) -> Result<(Metadata, MetaReport)> {
        let names = NameTable::from_entries(names);
        let populated = populated_of(db, None)
            .await?
            .into_iter()
            .map(|system| (system.address, system))
            .collect();
        let reaches = reaches(db, None).await?;
        let factions = factions_above(db, 0).await?;
        let high = factions.last().map(|f| f.id).unwrap_or(0);
        let mut metadata =
            Metadata { names, populated, reaches, factions, high };
        let report = metadata.publish(db, dir, None, true, true, true).await?;
        Ok((metadata, report))
    }

    /// The tables as `dir` holds them, read back with no database at all.
    ///
    /// What a `--watch` restart resumes onto. The names chunks come back with
    /// their boundaries intact, so the next publish appends where the run before
    /// it left off and rewrites nothing it already wrote.
    pub(super) fn resume(dir: &Path) -> io::Result<Metadata> {
        let names = NameTable::read(dir)?;
        let populated: Vec<meta::PopulatedSystem> =
            read_meta(&source::populated_path(dir))?;
        let reaches: Vec<meta::SystemReach> =
            read_meta(&source::reaches_path(dir))?;
        let factions: Vec<meta::Faction> =
            read_meta(&source::factions_path(dir))?;
        let high = factions.iter().map(|f| f.id).max().unwrap_or(0);
        Ok(Metadata {
            names,
            populated: populated
                .into_iter()
                .map(|system| (system.address, system))
                .collect(),
            reaches: reaches
                .into_iter()
                .map(|it| (it.address, it.reach))
                .collect(),
            factions,
            high,
        })
    }

    /// How many systems the names table stands for: every positioned one, which
    /// is what the served cell tree holds too.
    pub(super) fn names(&self) -> usize {
        self.names.len()
    }

    /// Patch in the systems of `touched` and publish what that moved.
    ///
    /// Every one of `touched` is rebuilt from its current row, so applying the
    /// same address twice lands in the same place and a system that has stopped
    /// qualifying for a table — its position withdrawn, its population gone —
    /// leaves it. A system whose record reads exactly as the one held changes
    /// nothing and writes nothing, which is the common case: the feed reports
    /// the same systems over and over.
    pub(super) async fn follow(
        &mut self,
        db: &Database,
        dir: &Path,
        touched: &[i64],
    ) -> Result<MetaReport> {
        let entries = names_for(db, touched).await?;
        let mut placed = HashSet::with_capacity(entries.len());
        for entry in entries {
            placed.insert(entry.address);
            self.names.upsert(entry);
        }

        let systems = populated_of(db, Some(touched)).await?;
        let mut inhabited = HashSet::with_capacity(systems.len());
        let mut moved = false;
        for system in systems {
            inhabited.insert(system.address);
            match self.populated.get(&system.address) {
                Some(held) if *held == system => {}
                _ => {
                    self.populated.insert(system.address, system);
                    moved = true;
                }
            }
        }

        // A scan is what moves a reach, so this is the table a watch pass
        // really does move: a system reported again with nothing new scanned
        // reads exactly as the one held and writes nothing.
        let reached = reaches(db, Some(touched)).await?;
        let mut scanned = HashSet::with_capacity(reached.len());
        let mut grew = false;
        for (address, reach) in reached {
            scanned.insert(address);
            if self.reaches.get(&address) != Some(&reach) {
                self.reaches.insert(address, reach);
                grew = true;
            }
        }

        for address in touched {
            if !placed.contains(address) {
                self.names.remove(*address);
            }
            if !inhabited.contains(address)
                && self.populated.remove(address).is_some()
            {
                moved = true;
            }
            if !scanned.contains(address)
                && self.reaches.remove(address).is_some()
            {
                grew = true;
            }
        }

        let named = factions_above(db, self.high).await?;
        let reported = !named.is_empty();
        if let Some(highest) = named.last() {
            self.high = highest.id;
            self.factions.extend(named);
        }

        self.publish(db, dir, Some(touched), moved, grew, reported).await
    }

    /// Write the dirty names chunks, whichever whole tables changed, and the
    /// body files of `bodies_for` — every system's for a full build, the
    /// changed ones' for a watch pass.
    async fn publish(
        &mut self,
        db: &Database,
        dir: &Path,
        bodies_for: Option<&[i64]>,
        populated: bool,
        reaches: bool,
        factions: bool,
    ) -> Result<MetaReport> {
        let name_chunks = self.names.publish(dir)?;
        if populated {
            write_populated(dir, &self.populated)?;
        }
        if reaches {
            write_reaches(dir, &self.reaches)?;
        }
        if factions {
            write_meta(&source::factions_path(dir), &self.factions)?;
        }
        Ok(MetaReport {
            populated: self.populated.len(),
            names: self.names.len(),
            factions: self.factions.len(),
            reaches: self.reaches.len(),
            body_files: write_bodies(db, dir, bodies_for).await?,
            name_chunks,
        })
    }
}

/// The name and place of each of `addresses` that has one.
///
/// Every positioned system belongs in the names table, not just the populated
/// ones, since a search reaches any name and a route steps between any two
/// places. An address that comes back with no row is a system that is not
/// positioned, and the caller takes it out of the table it stands in.
async fn names_for(
    db: &Database,
    addresses: &[i64],
) -> Result<Vec<meta::NameEntry>> {
    let rows = sqlx::query(&format!(
        "{NAMES_SELECT} WHERE address = ANY($1) AND position IS NOT NULL"
    ))
    .bind(addresses)
    .fetch_all(&db.pool)
    .await?;

    rows.iter().map(name_from_row).collect()
}

/// One `systems` row as the names table's record of it. The row carries
/// `address`, `name` and the three `ST_?` coordinates.
pub(super) fn name_from_row(row: &PgRow) -> Result<meta::NameEntry> {
    let x: f64 = row.try_get("x")?;
    let y: f64 = row.try_get("y")?;
    let z: f64 = row.try_get("z")?;
    Ok(meta::NameEntry {
        address: row.try_get("address")?,
        name: row.try_get("name")?,
        position: [x as f32, y as f32, z as f32],
    })
}

/// Every populated system with a place, or those of `addresses` alone.
///
/// A population without a position is left out: the map only ever colours a
/// system it draws, and it draws only positioned ones, so a
/// [`meta::PopulatedSystem`] carries a fixed `[f32; 3]` and never an absent one.
/// How far a system reaches has its own table, [`write_reaches`]: most systems
/// with anything scanned in them are not populated at all.
async fn populated_of(
    db: &Database,
    addresses: Option<&[i64]>,
) -> Result<Vec<meta::PopulatedSystem>> {
    let rows = match addresses {
        None => {
            sqlx::query(&format!(
                "{POPULATED_SELECT} \
             WHERE population > 0 AND position IS NOT NULL"
            ))
            .fetch_all(&db.pool)
            .await?
        }
        Some(addresses) => {
            sqlx::query(&format!(
                "{POPULATED_SELECT} WHERE address = ANY($1) \
             AND population > 0 AND position IS NOT NULL"
            ))
            .bind(addresses)
            .fetch_all(&db.pool)
            .await?
        }
    };

    rows.iter()
        .map(|row| {
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
            })
        })
        .collect()
}

/// The factions named since id `above`, in id order.
///
/// Ids come from a sequence, so a new faction is always a higher id than every
/// one already read, and a name on record is never rewritten. The whole table is
/// `above = 0`.
async fn factions_above(
    db: &Database,
    above: i32,
) -> Result<Vec<meta::Faction>> {
    let rows =
        sqlx::query("SELECT id, name FROM factions WHERE id > $1 ORDER BY id")
            .bind(above)
            .fetch_all(&db.pool)
            .await?;

    rows.iter()
        .map(|row| {
            Ok(meta::Faction {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
            })
        })
        .collect()
}

/// Write `populated.bin`: the dynamic set the map colours and navigates by, in
/// address order so the same table is always the same bytes.
fn write_populated(
    dir: &Path,
    populated: &HashMap<i64, meta::PopulatedSystem>,
) -> Result<usize> {
    let mut table: Vec<&meta::PopulatedSystem> = populated.values().collect();
    table.sort_unstable_by_key(|system| system.address);
    write_meta(&source::populated_path(dir), &table)?;
    Ok(table.len())
}

/// Write `reaches.bin`: how far each scanned system reaches, in address order
/// so the same table is always the same bytes.
fn write_reaches(dir: &Path, reaches: &HashMap<i64, f32>) -> Result<usize> {
    let mut table: Vec<meta::SystemReach> = reaches
        .iter()
        .map(|(&address, &reach)| meta::SystemReach { address, reach })
        .collect();
    table.sort_unstable_by_key(|it| it.address);
    write_meta(&source::reaches_path(dir), &table)?;
    Ok(table.len())
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

/// How far each system reaches from its arrival star, in metres: the far edge
/// of the furthest thing on record, over bodies, stars and the points a close
/// pair goes round. All of them for a full build, or those of `addresses` for a
/// watch pass. One grouped query rather than one per system, and the only place
/// the reach is worked out at all: the map reads it out of the published table
/// rather than asking the database how big a system is.
///
/// The far edge is the furthest the thing ever gets, not where it was found:
/// how far from arrival the scan put it or the far end of its orbit, whichever
/// is greater, with its own radius on top. A scan records where a thing stood
/// on the day, so the orbit is what says how far it ever carries, and the
/// recorded distance is what says how far its parent stands from the middle.
///
/// The points a close pair goes round count as well. Nothing stands at one, but
/// the pair rides its ellipse, and a pair scanned near periapsis says nothing
/// about how far that ellipse reaches.
///
/// Eccentricity is held short of one. What is recorded is a scan rather than a
/// solution, and a parabola read literally reaches forever.
///
/// The `299792458` is the metres in a light second, the distances from arrival
/// being recorded in those and everything else in metres.
///
/// A system with nothing scanned in it comes back with no row at all, which is
/// what leaves it out of the table: the map reads an absent reach as a system
/// whose size is not on record and stands in for it.
async fn reaches(
    db: &Database,
    addresses: Option<&[i64]>,
) -> Result<HashMap<i64, f32>> {
    // Every reaching thing, in the terms the outer query maxes over: how far
    // out it stands, how far its own orbit carries it, and how wide it is.
    const REACHING: &str = "SELECT system_address AS address, \
                MAX(GREATEST(away, apoapsis) + radius) AS reach \
         FROM ( \
             SELECT system_address, \
                    (COALESCE(distance_from_arrival, 0) * 299792458)::real \
                        AS away, \
                    (semi_major_axis \
                        * (1 + LEAST(eccentricity, 0.99)))::real AS apoapsis, \
                    radius \
             FROM bodies";
    const AND_STARS: &str = "           UNION ALL \
             SELECT system_address, \
                    (distance_from_arrival_ls * 299792458)::real, \
                    (COALESCE(semi_major_axis, 0) \
                        * (1 + LEAST(COALESCE(eccentricity, 0), 0.99)))::real, \
                    radius \
             FROM stars";
    const AND_CENTERS: &str = "           UNION ALL \
             SELECT system_address, \
                    0::real, \
                    (COALESCE(semi_major_axis, 0) \
                        * (1 + LEAST(COALESCE(eccentricity, 0), 0.99)))::real, \
                    0::real \
             FROM barycenters";
    const GROUPED: &str = ") reaching GROUP BY system_address";

    let rows = match addresses {
        None => {
            sqlx::query(&format!(
                "{REACHING} {AND_STARS} {AND_CENTERS} {GROUPED}"
            ))
            .fetch_all(&db.pool)
            .await?
        }
        Some(addresses) => {
            let of = " WHERE system_address = ANY($1)";
            sqlx::query(&format!(
                "{REACHING}{of} {AND_STARS}{of} {AND_CENTERS}{of} {GROUPED}"
            ))
            .bind(addresses)
            .fetch_all(&db.pool)
            .await?
        }
    };

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A system's bodies survive the trip out to disk and back through the
    /// same path helpers, format and reader the client uses.
    ///
    /// No database: the galos_db values are built by hand, converted to their
    /// metadata form, written with [`write_meta`] under [`source::bodies_path`],
    /// and read back through a [`FsSource`], the way a click into the map reads
    /// them. It proves the conversion is faithful and the two halves of the
    /// transport agree on the layout and the encoding.
    ///
    /// Both bodies make the full trip, the surfaced one on purpose: its
    /// [`BodyType`] and its [`AtmosphereType`] are `#[serde(untagged)]` enums
    /// with an `Unknown(String)` arm, and an untagged variant reads back only
    /// from a self-describing format. MessagePack is one, which is the reason
    /// it is the codec — see `galos_index::source::untagged_enums_round_trip`
    /// — so those two fields are asserted after the read here rather than
    /// only on the way out.
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

        // A gas giant: no surface, having none, and no body type on record,
        // so this is the empty side of both optional fields.
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

        // A surfaced body, the one carrying both untagged enums, through the
        // same trip.
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
        let surfaced_bodies = meta::SystemBodies {
            bodies: vec![meta_body(surfaced)],
            ..Default::default()
        };
        write_meta(&source::bodies_path(&dir, address + 2), &surfaced_bodies)
            .unwrap();
        let read_back = fs.bodies(address + 2).await.unwrap();
        assert_eq!(read_back, surfaced_bodies);

        // And field by field, so a failure says which one the codec dropped
        // rather than only that the two structs differ.
        let body = &read_back.bodies[0];
        assert_eq!(body.body_type, Some(BodyType::from("Planet")));
        let surface = body.surface.as_ref().unwrap();
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
        assert_eq!(body.parents[0].ty.as_deref(), Some("Null"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
