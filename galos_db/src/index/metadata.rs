//! The serving metadata beside the cell tree, and how a watch keeps it current.
//!
//! The cells carry what the map draws. These carry what a click wants: the
//! populated table the map colors and filters by, the names table a search and
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
use crate::index::Parts;
use crate::stars::Star;
use crate::{orbit, Database, Result};
use elite_journal::body::{Material, Orbit, Spin};
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
///
/// Nothing where the publish was not asked for that part — see
/// [`super::Parts`] — so a part left alone reads as left alone rather than as
/// a count of nothing.
#[derive(Copy, Clone, Debug, Default)]
pub struct MetaReport {
    pub populated: Option<usize>,
    pub names: Option<usize>,
    pub factions: Option<usize>,
    /// How many systems have a reach on record, which is every scanned one.
    pub reaches: Option<usize>,
    /// How many systems can supercharge a drive, which is four in a hundred.
    pub boosts: Option<usize>,
    pub body_files: Option<usize>,
    /// How many of the names table's chunks were written.
    pub name_chunks: usize,
}

/// What was written, table by table, with the tables left alone named as kept.
impl std::fmt::Display for MetaReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let said = |what: &str, count: Option<usize>| match count {
            Some(count) => format!("{count} {what}"),
            None => format!("{what} kept"),
        };
        write!(
            f,
            "{}, {}, {}, {}, {}, {}",
            said("populated", self.populated),
            said("names", self.names),
            said("reaches", self.reaches),
            said("boosts", self.boosts),
            said("factions", self.factions),
            said("body files", self.body_files),
        )
    }
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
    /// Which systems can supercharge a drive, by address. Keyed like the two
    /// above, and the quietest of the three: a system's main star class
    /// arrives with the scan that first names it and then stands, so a pass
    /// moves this only where it has met a neutron star or a white dwarf it had
    /// not met before.
    boosts: HashMap<i64, meta::Boost>,
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
        let grouped = bodies_of(db, None).await?;
        let reaches = reaches_of(&grouped);
        let boosts = boosts_of(db, None).await?.into_iter().collect();
        let factions = factions_above(db, 0).await?;
        let high = factions.last().map(|f| f.id).unwrap_or(0);
        let mut metadata =
            Metadata { names, populated, reaches, boosts, factions, high };
        let report =
            metadata.publish(dir, &grouped, None, true, true, true, true)?;
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
        // A directory published before this table existed has none, and the
        // first pass over a system with a jet cone puts it back. Only that:
        // read as an empty table, a corrupt or truncated one would be
        // republished from the handful of addresses one pass touches, and the
        // hundred thousand rows already published would be gone with no error
        // anywhere. Its siblings above say the same by using `?`.
        let boosts: Vec<meta::SystemBoost> =
            match read_meta(&source::boosts_path(dir)) {
                Ok(table) => table,
                Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(e),
            };
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
            boosts: boosts
                .into_iter()
                .map(|it| (it.address, it.boost))
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
        // reads exactly as the one held and writes nothing. Worked out from
        // the rows the body files are written from, which is what keeps the
        // reach the map sizes a system by and the arrangement it draws inside
        // that system the same answer.
        let grouped = bodies_of(db, Some(touched)).await?;
        let reached = reaches_of(&grouped);
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

        // The star class of each system reported, which says whether its
        // arrival star can supercharge a drive.
        let boosting = boosts_of(db, Some(touched)).await?;
        let mut charged = HashSet::with_capacity(boosting.len());
        let mut lit = false;
        for (address, boost) in boosting {
            charged.insert(address);
            if self.boosts.get(&address) != Some(&boost) {
                self.boosts.insert(address, boost);
                lit = true;
            }
        }
        for address in touched {
            if !charged.contains(address)
                && self.boosts.remove(address).is_some()
            {
                lit = true;
            }
        }

        let named = factions_above(db, self.high).await?;
        let reported = !named.is_empty();
        if let Some(highest) = named.last() {
            self.high = highest.id;
            self.factions.extend(named);
        }

        self.publish(dir, &grouped, Some(touched), moved, grew, lit, reported)
    }

    /// Write the dirty names chunks, whichever whole tables changed, and the
    /// body files of `grouped` — every system's for a full build, the changed
    /// ones' for a watch pass.
    ///
    /// A watch always has the four tables in hand, so what it writes is
    /// decided by what moved rather than by what was asked for. A build that
    /// wants one part alone holds none of them and goes through
    /// [`write_parts`] instead.
    fn publish(
        &mut self,
        dir: &Path,
        grouped: &HashMap<i64, meta::SystemBodies>,
        bodies_for: Option<&[i64]>,
        populated: bool,
        reaches: bool,
        boosts: bool,
        factions: bool,
    ) -> Result<MetaReport> {
        let name_chunks = self.names.publish(dir)?;
        if populated {
            write_populated(dir, &self.populated)?;
        }
        if reaches {
            write_reaches(dir, &self.reaches)?;
        }
        if boosts {
            write_boosts(dir, &self.boosts)?;
        }
        if factions {
            write_meta(&source::factions_path(dir), &self.factions)?;
        }
        Ok(MetaReport {
            populated: Some(self.populated.len()),
            names: Some(self.names.len()),
            factions: Some(self.factions.len()),
            reaches: Some(self.reaches.len()),
            boosts: Some(self.boosts.len()),
            body_files: Some(write_bodies(dir, grouped, bodies_for)?),
            name_chunks,
        })
    }
}

/// Derive and write the metadata parts `parts` names, and nothing else.
///
/// What a build asking for one part goes through, where a watch goes through
/// [`Metadata::publish`]. The difference is what is in hand: a watch holds the
/// tables and writes whichever moved, and this holds nothing, so each part it
/// was asked for is read fresh and each part it was not is never read at all.
/// The reaches and the body files share one read of every scanned thing, which
/// is the expensive half of a build and the half neither of them can skip.
///
/// `names` is the other half of the read the cell tree comes out of, and is
/// [`None`] exactly where the names table was not asked for.
pub(super) async fn write_parts(
    db: &Database,
    dir: &Path,
    names: Option<Vec<meta::NameEntry>>,
    parts: Parts,
) -> Result<MetaReport> {
    let mut report = MetaReport::default();

    if parts.names {
        let entries = names.expect("the names read for the names table");
        let mut table = NameTable::from_entries(entries);
        report.name_chunks = table.publish(dir)?;
        report.names = Some(table.len());
    }

    if parts.populated {
        let populated: HashMap<i64, meta::PopulatedSystem> =
            populated_of(db, None)
                .await?
                .into_iter()
                .map(|system| (system.address, system))
                .collect();
        report.populated = Some(write_populated(dir, &populated)?);
    }

    if parts.wants_bodies() {
        let grouped = bodies_of(db, None).await?;
        if parts.reaches {
            report.reaches = Some(write_reaches(dir, &reaches_of(&grouped))?);
        }
        if parts.bodies {
            report.body_files = Some(write_bodies(dir, &grouped, None)?);
        }
    }

    if parts.boosts {
        let boosts: HashMap<i64, meta::Boost> =
            boosts_of(db, None).await?.into_iter().collect();
        report.boosts = Some(write_boosts(dir, &boosts)?);
    }

    if parts.factions {
        let factions = factions_above(db, 0).await?;
        write_meta(&source::factions_path(dir), &factions)?;
        report.factions = Some(factions.len());
    }

    Ok(report)
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
/// A population without a position is left out: the map only ever colors a
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

/// Write `populated.bin`: the dynamic set the map colors and navigates by, in
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

/// Write `boosts.bin`: which systems can supercharge a drive, in address order
/// so the same table is always the same bytes.
fn write_boosts(
    dir: &Path,
    boosts: &HashMap<i64, meta::Boost>,
) -> Result<usize> {
    let mut table: Vec<meta::SystemBoost> = boosts
        .iter()
        .map(|(&address, &boost)| meta::SystemBoost { address, boost })
        .collect();
    table.sort_unstable_by_key(|it| it.address);
    write_meta(&source::boosts_path(dir), &table)?;
    Ok(table.len())
}

/// Which of the positioned systems can supercharge a drive, or those of
/// `addresses` alone.
///
/// The arrival star's class, which is the one a ship can reach the jet cone
/// of without crossing the system. Two places say what that is and the
/// scanned one wins: a `stars` row is something somebody looked at, where
/// `systems.primary_star_class` is only ever written by a plotted route
/// (`galos-sync`'s `nav_route`), which names the class of a system nobody
/// has necessarily been to. Reading the column alone published no
/// supercharge at all for a neutron star a commander had flown to and
/// scanned — the scan writes `stars`, and nothing writes that column back.
///
/// Nearest the drop point, ties broken by body id, which is
/// `galos_journal::Galaxy::arrival_class`'s rule: the two derivations of
/// this table have to answer the same thing about the same galaxy.
///
/// The classification is [`meta::Boost::of`] and only that: a class that
/// supercharges nothing is not in the result, and the caller takes such a
/// system out of the table it stands in.
///
/// So the query narrows by what it needs — a class to read at all — and not
/// by which classes those are. Written out in SQL as well, the two would
/// agree until [`meta::Boost::of`] was widened, and then a full build would
/// publish a short table that every watch pass patched systems into: one
/// directory disagreeing with itself about the same galaxy depending on how
/// it was produced.
async fn boosts_of(
    db: &Database,
    addresses: Option<&[i64]>,
) -> Result<Vec<(i64, meta::Boost)>> {
    let rows = match addresses {
        // One ordered pass over the stars for the whole galaxy, rather than
        // a lookup per system: a full build is reading every star anyway.
        None => {
            sqlx::query(
                "SELECT s.address, \
                        COALESCE(arrival.star_class, s.primary_star_class) \
                            AS class \
                 FROM systems s \
                 LEFT JOIN ( \
                     SELECT DISTINCT ON (system_address) \
                            system_address, star_class \
                     FROM stars \
                     ORDER BY system_address, distance_from_arrival_ls, id \
                 ) arrival ON arrival.system_address = s.address \
                 WHERE s.position IS NOT NULL \
                   AND (arrival.star_class IS NOT NULL \
                        OR s.primary_star_class IS NOT NULL)",
            )
            .fetch_all(&db.pool)
            .await?
        }
        // A handful of systems: the nearest star of each off the index the
        // reaches are read through.
        Some(addresses) => {
            sqlx::query(
                "SELECT s.address, \
                        COALESCE(arrival.star_class, s.primary_star_class) \
                            AS class \
                 FROM systems s \
                 LEFT JOIN LATERAL ( \
                     SELECT star_class FROM stars st \
                     WHERE st.system_address = s.address \
                     ORDER BY st.distance_from_arrival_ls, st.id \
                     LIMIT 1 \
                 ) arrival ON true \
                 WHERE s.address = ANY($1) AND s.position IS NOT NULL",
            )
            .bind(addresses)
            .fetch_all(&db.pool)
            .await?
        }
    };
    let mut boosts = Vec::new();
    for row in rows {
        let address: i64 = row.try_get("address")?;
        let class: Option<String> = row.try_get("class")?;
        if let Some(boost) = class.as_deref().and_then(meta::Boost::of) {
            boosts.push((address, boost));
        }
    }
    Ok(boosts)
}

/// Group `bodies/<address>.bin`'s worth of rows: one [`meta::SystemBodies`] per
/// system that has any stars, bodies or barycenters on record.
///
/// The three kinds are read in bulk, ordered by system and grouped in memory,
/// so a system with a hundred bodies costs one row per body of one query rather
/// than a query of its own. `addresses` is [`None`] for a full build, which
/// reads every system, and [`Some`] for a watch pass, which reads only what
/// changed.
///
/// Read once and used twice: [`write_bodies`] writes these out and
/// [`reaches_of`] measures them. They were two passes over the same rows until
/// the reach stopped being its own query.
async fn bodies_of(
    db: &Database,
    addresses: Option<&[i64]>,
) -> Result<HashMap<i64, meta::SystemBodies>> {
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
    Ok(grouped)
}

/// Write the body files, and remove the file of any changed address left with
/// nothing — so a system whose last scan was withdrawn stops reading as one
/// that still has it.
fn write_bodies(
    dir: &Path,
    grouped: &HashMap<i64, meta::SystemBodies>,
    addresses: Option<&[i64]>,
) -> Result<usize> {
    std::fs::create_dir_all(dir.join(source::BODIES_DIR))?;
    for (address, system_bodies) in grouped {
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

/// How far each system reaches from its arrival star, in metres, over the rows
/// already grouped for the body files.
///
/// [`galos_index::inside`]'s answer and nobody else's. The map sizes every
/// system in the sky by this table and draws the one it descends into from the
/// same rows, and the two have to agree: a shell drawn smaller than the orbits
/// inside it is the one thing a reach cannot be. This was a query of its own
/// for a while — a `GREATEST(away, apoapsis) + radius` maxed per system — and
/// it did not agree. It never added the displacement of what a thing goes
/// round, so a star whose own orbit is measured about a point ten billion
/// kilometres off came back reaching only as far as that orbit, and the shell
/// cut through the far half of its own ellipse. Written twice, in two
/// languages, it was never going to hold.
///
/// A system with nothing to say comes back with no entry at all, which is what
/// leaves it out of the table: the map reads an absent reach as a system whose
/// size is not on record and stands in for it.
fn reaches_of(grouped: &HashMap<i64, meta::SystemBodies>) -> HashMap<i64, f32> {
    grouped
        .iter()
        .filter_map(|(&address, rows)| Some((address, rows.extent(address)?)))
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
    let discovered_at: Option<chrono::NaiveDateTime> =
        row.try_get("discovered_at")?;
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
        mapped: row.try_get("was_mapped")?,
        discovered_at: discovered_at.map(|at| at.and_utc()),
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
    let discovered_at: Option<chrono::NaiveDateTime> =
        row.try_get("discovered_at")?;
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
        mapped: row.try_get("was_mapped")?,
        discovered_at: discovered_at.map(|at| at.and_utc()),
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
        mapped: star.mapped,
        discovered_at: star.discovered_at,
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
        mapped: body.mapped,
        discovered_at: body.discovered_at,
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
            mapped: false,
            discovered_at: None,
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
            mapped: false,
            discovered_at: None,
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
            mapped: true,
            discovered_at: None,
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

    /// A corrupt supercharge table stops a resume rather than emptying it
    ///
    /// [`Metadata::resume`] reads the table back so a watch pass can patch it
    /// instead of rebuilding it, and a directory published before the table
    /// existed has none — which is why the read tolerates an absence. Only an
    /// absence: read as empty, a truncated file would be republished from the
    /// handful of addresses one pass touches, and the hundred thousand rows
    /// already published would be gone with nothing said. A refused resume is
    /// a full rebuild, which is the recoverable answer.
    #[test]
    fn a_corrupt_boosts_table_refuses_a_resume() {
        let dir = std::env::temp_dir()
            .join(format!("galos_db_resume_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");

        // The tables a resume needs, all present and all empty.
        let empty: Vec<u8> = Vec::new();
        source::write_meta(&source::populated_path(&dir), &empty)
            .expect("populated");
        source::write_meta(&source::reaches_path(&dir), &empty)
            .expect("reaches");
        source::write_meta(&source::factions_path(&dir), &empty)
            .expect("factions");
        galos_index::NameTable::from_entries(Vec::new())
            .publish(&dir)
            .expect("names");

        // Nothing where the table would be: the case the tolerance is for.
        assert!(
            Metadata::resume(&dir).is_ok(),
            "a directory published before the table refused to resume"
        );

        // A table half written, which is what a builder killed mid-pass left
        // before the write became a rename.
        std::fs::write(source::boosts_path(&dir), b"\xdd\xff\xff\xff\xff\x01")
            .expect("a truncated table");
        assert!(
            Metadata::resume(&dir).is_err(),
            "a corrupt table resumed as an empty one"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A scanned neutron star supercharges, whatever a route said
    ///
    /// The bug: `boosts_of` read `systems.primary_star_class`, and the only
    /// thing that writes that column is a plotted route. Fly to a neutron
    /// star and scan it and the `stars` row says `N` while the column stays
    /// null, so the index published no supercharge for a system the
    /// commander had personally stood in — and the map, which refuses a
    /// route for a drive that takes a jet cone without a table, had nothing
    /// to plot by.
    ///
    /// Needs a database of its own, named by `TEST_DATABASE_URL`, for the
    /// reason `tests/write_path.rs` sets out. Stands down when nothing says
    /// where to write, so CI passes with no database at all.
    #[async_std::test]
    async fn a_scanned_arrival_star_is_what_supercharges() {
        dotenv::dotenv().ok();
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("no TEST_DATABASE_URL: standing down");
            return;
        };
        let db = Database::from_url(&url)
            .await
            .expect("TEST_DATABASE_URL should connect");

        // Addresses nothing else writes, cleared so the rows below are the
        // only ones they can hold.
        let scanned = 0x0B00_5700_0000_0001_u64 as i64;
        let routed = 0x0B00_5700_0000_0002_u64 as i64;
        for address in [scanned, routed] {
            for statement in [
                "DELETE FROM stars WHERE system_address = $1",
                "DELETE FROM systems WHERE address = $1",
            ] {
                sqlx::query(statement)
                    .bind(address)
                    .execute(&db.pool)
                    .await
                    .expect("the address should be clearable");
            }
        }

        // One system nobody plotted a route to, holding a scanned neutron
        // star at the drop point and a white dwarf further out.
        sqlx::query(
            "INSERT INTO systems (address, name, position, updated_at, \
                                  updated_by, primary_star_class) \
             VALUES ($1, 'BOOST SCANNED', \
                     ST_MakePoint(1, 2, 3)::geometry, now(), 'test', NULL)",
        )
        .bind(scanned)
        .execute(&db.pool)
        .await
        .expect("the scanned system should write");
        for (id, class, distance) in
            [(1i16, "D", 900.0f32), (0i16, "N", 0.0f32)]
        {
            sqlx::query(
                "INSERT INTO stars (system_address, id, name, updated_at, \
                     updated_by, absolute_magnitude, age_my, \
                     distance_from_arrival_ls, luminosity, star_class, \
                     stellar_mass, subclass, axial_tilt, radius, \
                     rotation_period, temperature, was_mapped) \
                 VALUES ($1, $2, $3, now(), 'test', 4.8, 100, $4, 'V', $5, \
                         1.0, 2, 0, 1.0, 0, 5000, false)",
            )
            .bind(scanned)
            .bind(id)
            .bind(format!("BOOST SCANNED {id}"))
            .bind(distance)
            .bind(class)
            .execute(&db.pool)
            .await
            .expect("the star should write");
        }

        // And one nobody has scanned, known only from a plotted route.
        sqlx::query(
            "INSERT INTO systems (address, name, position, updated_at, \
                                  updated_by, primary_star_class) \
             VALUES ($1, 'BOOST ROUTED', \
                     ST_MakePoint(4, 5, 6)::geometry, now(), 'test', 'D')",
        )
        .bind(routed)
        .execute(&db.pool)
        .await
        .expect("the routed system should write");

        let whole: HashMap<i64, meta::Boost> = boosts_of(&db, None)
            .await
            .expect("a full read")
            .into_iter()
            .collect();
        assert_eq!(
            whole.get(&scanned),
            Some(&meta::Boost::Neutron),
            "a scanned neutron star published no supercharge",
        );
        assert_eq!(
            whole.get(&routed),
            Some(&meta::Boost::WhiteDwarf),
            "a routed class is still what an unscanned system has",
        );

        // A watch pass reads the same systems through the other query and
        // must answer the same, or a directory says different things about
        // one galaxy depending on how it was built.
        let touched: HashMap<i64, meta::Boost> =
            boosts_of(&db, Some(&[scanned, routed]))
                .await
                .expect("a read of what changed")
                .into_iter()
                .collect();
        assert_eq!(touched, whole, "the two reads disagree");
    }
}
