//! The serving metadata beside the cell tree, and how a watch keeps it current.
//!
//! The cells carry what the map draws. These carry what a click wants: the
//! populated table the map colors and filters by, the names table a search
//! and a route read, the faction names, and one file of bodies per system.
//!
//! A full build derives all four wholesale; a watch cannot, the names table
//! alone being every positioned system. So a watch holds the tables open
//! here — [`Metadata`] — where [`patch`](Metadata::patch) reads only the
//! systems that changed and [`publish_pass`](Metadata::publish_pass) writes
//! only what that moved, once for the pass rather than once per chunk.
//!
//! Patching rests on the changed set being complete, which is
//! [`changed_addresses`](super::changed_addresses)'s business, and on a
//! record's `PartialEq` telling that a system reported again says nothing
//! new. A change slipped past the cursor converges the next time anything
//! touches the system, every patch rebuilding its record from the current
//! row.

use crate::barycenters::Barycenter;
use crate::bodies::{ancestry, composition, Body, Surface};
use crate::index::Parts;
use crate::stars::Star;
use crate::{orbit, Database, Result};
use async_std::stream::StreamExt;
use elite_journal::body::{Material, Orbit, Spin};
use futures_core::stream::BoxStream;
pub(super) use galos_index::sidecars::Moved;
use galos_index::sidecars::Sidecars;
use galos_index::source::write_meta;
use galos_index::{derive, meta, source};
use sqlx::postgres::PgRow;
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

/// The columns a [`meta::NameEntry`] is read from.
pub(super) const NAMES_SELECT: &str = "SELECT address, name, \
     ST_X(position) AS x, ST_Y(position) AS y, ST_Z(position) AS z \
     FROM systems";

/// The columns a [`meta::PopulatedSystem`] is read from.
///
/// Only factions with a row in `factions` are carried: `system_factions`
/// holds ids EDDN has reported but never named, which the client can neither
/// name nor filter by.
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
/// The tables are counted whole; `name_chunks` and `body_files` are what the
/// publish touched. [`None`] where the publish was not asked for that part —
/// see [`super::Parts`].
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

/// The metadata tables, held open across a watch, and the highest faction id
/// read so far.
///
/// The tables are [`Sidecars`], shared with `galos-sync`'s event-sourced
/// half. What is this crate's is where a row comes from and that a row which
/// has stopped qualifying is *withdrawn*: it re-reads `population > 0` off
/// the row, so it can tell a system that has emptied from one nothing has
/// mentioned.
pub(super) struct Metadata {
    held: Sidecars,
    /// The highest faction id read. Ids come from a sequence and a name is
    /// never rewritten, so one query past this answers a whole pass.
    high: i32,
}

impl Metadata {
    /// The tables as `dir` holds them, read back with no database at all.
    ///
    /// What a `--watch` restart resumes onto. The names chunks come back
    /// with their boundaries intact, so the next publish appends where the
    /// run before it left off. A file missing is read back as empty, and the
    /// first pass over a qualifying system puts it there.
    pub(super) fn resume(dir: &Path) -> io::Result<Metadata> {
        let (held, _absent) = Sidecars::resume(dir)?;
        let high = held.highest_faction();
        Ok(Metadata { held, high })
    }

    /// How many systems the names table stands for: every positioned one,
    /// which is what the served cell tree holds too.
    pub(super) fn names(&self) -> usize {
        self.held.counts().names
    }

    /// Patch in the systems of `touched` — one chunk of a pass — write their
    /// body files, and answer what that moved in the tables written whole.
    ///
    /// Every one of `touched` is rebuilt from its current row, so applying
    /// an address twice lands in the same place and a system that has
    /// stopped qualifying for a table leaves it. A record reading as the one
    /// held writes nothing.
    ///
    /// Nothing is written here but the body files: the tables written whole
    /// are the pass's, through [`publish_pass`](Metadata::publish_pass).
    pub(super) async fn patch(
        &mut self,
        db: &Database,
        dir: &Path,
        touched: &[i64],
    ) -> Result<(Moved, usize)> {
        let entries = names_for(db, touched).await?;
        let mut placed = HashSet::with_capacity(entries.len());
        for entry in entries {
            placed.insert(entry.address);
            self.held.name(entry);
        }

        let systems = populated_of(db, Some(touched)).await?;
        let mut inhabited = HashSet::with_capacity(systems.len());
        let mut moved = Moved::default();
        for system in systems {
            inhabited.insert(system.address);
            moved.populated |= self.held.populate(system);
        }

        // A scan is what moves a reach. Worked out from the rows the body
        // files are written from, so the reach the map sizes a system by and
        // the arrangement it draws inside that system agree.
        let grouped = bodies_of(db, touched).await?;
        let reached: HashMap<i64, f32> = grouped
            .iter()
            .filter_map(|(&address, rows)| {
                Some((address, rows.extent(address)?))
            })
            .collect();
        let mut scanned = HashSet::with_capacity(reached.len());
        for (address, reach) in reached {
            scanned.insert(address);
            moved.reaches |= self.held.reach(address, reach);
        }

        // The star class of each system reported, which says whether its
        // arrival star can supercharge a drive. Read off the same rows the
        // reach was measured over, so the two cannot disagree about which
        // star a ship drops at.
        let boosting = boosts_of(db, touched, &grouped).await?;
        let mut charged = HashSet::with_capacity(boosting.len());
        for (address, boost) in boosting {
            charged.insert(address);
            moved.boosts |= self.held.boost(address, boost);
        }

        for address in touched {
            if !placed.contains(address) {
                self.held.unname(*address);
            }
            if !inhabited.contains(address) {
                moved.populated |= self.held.depopulate(*address);
            }
            if !scanned.contains(address) {
                moved.reaches |= self.held.unreach(*address);
            }
            if !charged.contains(address) {
                moved.boosts |= self.held.unboost(*address);
            }
        }

        Ok((moved, write_bodies(dir, &grouped, touched)?))
    }

    /// Write everything one pass's chunks moved: the names chunks the
    /// arrivals landed in, the whole tables any chunk dirtied, and the
    /// factions named since the pass before.
    ///
    /// The faction sweep is here rather than in [`patch`](Metadata::patch)
    /// because it is not about the addresses that changed: one query past
    /// the highest id read answers the whole pass.
    pub(super) async fn publish_pass(
        &mut self,
        db: &Database,
        dir: &Path,
        mut moved: Moved,
        body_files: usize,
    ) -> Result<MetaReport> {
        let named = factions_above(db, self.high).await?;
        if let Some(highest) = named.last() {
            self.high = highest.id;
            moved.factions |= self.held.add_factions(named);
        }
        self.publish(dir, moved, body_files)
    }

    /// Write the dirty names chunks and whichever whole tables `moved` names.
    ///
    /// A watch has the tables in hand, so what it writes is decided by what
    /// moved; a build wanting one part goes through [`write_parts`]. The
    /// body files are written by whoever moved the tables.
    fn publish(
        &mut self,
        dir: &Path,
        moved: Moved,
        body_files: usize,
    ) -> Result<MetaReport> {
        let name_chunks = self.held.write(dir, moved)?;
        let counts = self.held.counts();
        Ok(MetaReport {
            populated: Some(counts.populated),
            names: Some(counts.names),
            factions: Some(counts.factions),
            reaches: Some(counts.reaches),
            boosts: Some(counts.boosts),
            body_files: Some(body_files),
            name_chunks,
        })
    }
}

/// Derive and write the metadata parts `parts` names, and nothing else.
///
/// What a build asking for one part goes through, where a watch goes through
/// [`Metadata::publish`]. This holds no tables, so each part asked for is
/// read fresh. The reaches, the body files and the boosts share one read of
/// every scanned thing.
///
/// The names table is not among them: it comes out of the same read of
/// `systems` the cell tree does, a chunk at a time, a galaxy of name entries
/// held to be published afterwards being tens of gigabytes.
pub(super) async fn write_parts(
    db: &Database,
    dir: &Path,
    parts: Parts,
) -> Result<MetaReport> {
    let mut report = MetaReport::default();

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
        // One pass over everything scanned, in address order, holding one
        // system's rows at a time. The two tables written whole are gathered
        // as it goes; the body files go out as each system's rows arrive.
        let mut reaches = HashMap::new();
        let mut boosts = HashMap::new();
        let mut body_files = 0;
        each_scanned(db, |scanned| {
            if parts.reaches {
                if let Some(reach) = scanned.inside.extent(scanned.address) {
                    reaches.insert(scanned.address, reach);
                }
            }
            if parts.boosts {
                if let Some(boost) = scanned.boost() {
                    boosts.insert(scanned.address, boost);
                }
            }
            if parts.bodies && scanned.anything() {
                write_meta(
                    &source::bodies_path(dir, scanned.address),
                    &scanned.inside,
                )?;
                body_files += 1;
            }
            Ok(())
        })
        .await?;
        if parts.reaches {
            report.reaches = Some(write_reaches(dir, &reaches)?);
        }
        if parts.bodies {
            report.body_files = Some(body_files);
        }
        if parts.boosts {
            report.boosts = Some(write_boosts(dir, &boosts)?);
        }
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
/// Every positioned system belongs in the names table, a search reaching any
/// name and a route stepping between any two places. An address that comes
/// back with no row is not positioned, and the caller takes it out of the
/// table it stands in.
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
/// A population without a position is left out: the map colors only what it
/// draws, so a [`meta::PopulatedSystem`] carries a fixed `[f32; 3]`. How far
/// a system reaches has its own table, [`write_reaches`].
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
/// Ids come from a sequence and a name on record is never rewritten, so a
/// new faction is always a higher id. The whole table is `above = 0`.
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

/// Which of the positioned systems can supercharge a drive, over the rows
/// already grouped for the body files.
///
/// The arrival star's class, which is the one a ship can reach the jet cone
/// of without crossing the system. Two places say what that is and the
/// scanned one wins: `systems.primary_star_class` is only ever written by a
/// plotted route, naming the class of a system nobody has necessarily been
/// to.
///
/// Which star that is, is [`derive::arrival_class`] and not a query, over
/// the rows the caller has already read for the body files and the reaches.
/// SQL says only which systems are eligible: positioned, and with a class to
/// read at all. The classification is [`meta::Boost::of`], so a class that
/// supercharges nothing is left out and the caller takes such a system out
/// of the table it stands in.
async fn boosts_of(
    db: &Database,
    addresses: &[i64],
    grouped: &HashMap<i64, meta::SystemBodies>,
) -> Result<Vec<(i64, meta::Boost)>> {
    // The systems with nothing to say are dropped below rather than by the
    // query.
    let rows = sqlx::query(
        "SELECT address, primary_star_class FROM systems \
         WHERE address = ANY($1) AND position IS NOT NULL",
    )
    .bind(addresses)
    .fetch_all(&db.pool)
    .await?;
    let mut boosts = Vec::new();
    for row in rows {
        let address: i64 = row.try_get("address")?;
        let routed: Option<String> = row.try_get("primary_star_class")?;
        let inside = grouped.get(&address);
        let class = inside
            .and_then(derive::arrival_class)
            .or(routed.as_deref());
        if let Some(boost) = class.and_then(meta::Boost::of) {
            boosts.push((address, boost));
        }
    }
    Ok(boosts)
}

/// Group `bodies/<address>.bin`'s worth of rows for `addresses`: one
/// [`meta::SystemBodies`] per system of them with anything on record.
///
/// The three kinds are read in bulk, ordered by system and grouped in
/// memory, so a system with a hundred bodies costs one row per body rather
/// than a query of its own.
///
/// A watch pass's read, bounded by the chunk it is given; a full build goes
/// through [`each_scanned`]. Read once and used three times: the body files
/// are written out of it, the reach measured over it, the arrival star's
/// class read off it.
async fn bodies_of(
    db: &Database,
    addresses: &[i64],
) -> Result<HashMap<i64, meta::SystemBodies>> {
    let mut grouped: HashMap<i64, meta::SystemBodies> = HashMap::new();
    for star in all_stars(db, addresses).await? {
        grouped.entry(star.system_address).or_default().stars.push(star.into());
    }
    for body in all_bodies(db, addresses).await? {
        grouped
            .entry(body.system_address)
            .or_default()
            .bodies
            .push(body.into());
    }
    for barycenter in all_barycenters(db, addresses).await? {
        grouped
            .entry(barycenter.system_address)
            .or_default()
            .barycenters
            .push(barycenter.into());
    }
    Ok(grouped)
}

/// Write the body files of a pass's chunk, and remove the file of any
/// changed address left with nothing — so a system whose last scan was
/// withdrawn stops reading as one that still has it. Only a pass knows which
/// addresses it asked about.
///
/// The removal goes through [`source::remove_bodies`], a directory published
/// before the sharding still holding a flat file a read falls back onto.
fn write_bodies(
    dir: &Path,
    grouped: &HashMap<i64, meta::SystemBodies>,
    addresses: &[i64],
) -> Result<usize> {
    for (address, system_bodies) in grouped {
        write_meta(&source::bodies_path(dir, *address), system_bodies)?;
    }

    for &address in addresses {
        if !grouped.contains_key(&address) {
            source::remove_bodies(dir, address)?;
        }
    }

    Ok(grouped.len())
}

/// Everything one system has on record, as the three things read off it
/// want it: the file to write, the reach to measure, the arrival star to
/// classify.
struct Scanned {
    address: i64,
    inside: meta::SystemBodies,
    /// Whether anything has placed this system. A supercharge is published
    /// only for one that has; see [`boosts_of`].
    placed: bool,
    /// What a plotted route said the system's primary is: the fallback where
    /// nothing has been scanned.
    routed: Option<String>,
}

impl Scanned {
    /// Whether there is anything to write a body file out of. A system that
    /// is only here because a route named its class has no file.
    fn anything(&self) -> bool {
        !self.inside.stars.is_empty()
            || !self.inside.bodies.is_empty()
            || !self.inside.barycenters.is_empty()
    }

    /// Whether this system's arrival star can supercharge a drive, by
    /// [`boosts_of`]'s rule: the scanned arrival star, else the route's
    /// class, and nothing for a system nothing has placed.
    fn boost(&self) -> Option<meta::Boost> {
        if !self.placed {
            return None;
        }
        derive::arrival_class(&self.inside)
            .or(self.routed.as_deref())
            .and_then(meta::Boost::of)
    }
}

/// The columns each of the four ordered reads is made of, shared with the
/// paged reads above so a build and a pass read a row the same way.
const STARS_SELECT: &str = "SELECT * FROM stars";
const BARYCENTERS_SELECT: &str = "SELECT * FROM barycenters";
const BODIES_SELECT: &str = "SELECT b.*, \
     COALESCE(ARRAY_AGG(m.name ORDER BY m.name) \
         FILTER (WHERE m.name IS NOT NULL), '{}') AS material_names, \
     COALESCE(ARRAY_AGG(m.percent ORDER BY m.name) \
         FILTER (WHERE m.name IS NOT NULL), '{}') AS material_percents \
     FROM bodies b \
     LEFT JOIN body_materials m \
         ON m.system_address = b.system_address AND m.body_id = b.id";
/// Every positioned system with either source of an arrival class. The
/// semi-join keeps a full build from carrying back the systems with neither.
const BOOSTABLE_SELECT: &str =
    "SELECT address AS system_address, primary_star_class FROM systems s \
     WHERE s.position IS NOT NULL \
       AND (s.primary_star_class IS NOT NULL \
            OR EXISTS (SELECT 1 FROM stars st \
                       WHERE st.system_address = s.address))";

/// One ordered cursor over a table keyed by system, with the row read past
/// the system it belongs to held back.
///
/// What merging costs in memory: one row per stream.
struct ByAddress<'a, T> {
    rows: BoxStream<'a, sqlx::Result<PgRow>>,
    read: fn(&PgRow) -> Result<T>,
    ahead: Option<(i64, T)>,
}

impl<'a, T> ByAddress<'a, T> {
    /// Open the cursor and read its first row.
    async fn open(
        db: &'a Database,
        query: &'a str,
        read: fn(&PgRow) -> Result<T>,
    ) -> Result<ByAddress<'a, T>> {
        let mut cursor = ByAddress {
            rows: sqlx::query(query).fetch(&db.pool),
            read,
            ahead: None,
        };
        cursor.step().await?;
        Ok(cursor)
    }

    /// The system the held row belongs to, or [`None`] at the end.
    fn at(&self) -> Option<i64> {
        self.ahead.as_ref().map(|&(address, _)| address)
    }

    /// Take everything this cursor holds for `address`.
    async fn take(&mut self, address: i64, into: &mut Vec<T>) -> Result<()> {
        while self.at() == Some(address) {
            let (_, row) = self.ahead.take().expect("a held row");
            into.push(row);
            self.step().await?;
        }
        Ok(())
    }

    /// Read one more row.
    async fn step(&mut self) -> Result<()> {
        self.ahead = match self.rows.next().await {
            None => None,
            Some(row) => {
                let row = row?;
                let address: i64 = row.try_get("system_address")?;
                Some((address, (self.read)(&row)?))
            }
        };
        Ok(())
    }
}

/// Every system with anything scanned in it, or any class to read off it,
/// handed over one at a time in address order.
///
/// Four ordered cursors merged: the stars, the bodies with their materials,
/// the barycenters, and the systems a supercharge could be published for.
/// Each is a `fetch` ordered by the system column, so what is held at any
/// moment is one system's rows and one row of each cursor beyond it.
///
/// A watch pass still reads its chunk through [`bodies_of`]: it is bounded
/// by the chunk, and it needs the whole group to say which addresses came
/// back with *nothing*.
async fn each_scanned<F>(db: &Database, mut each: F) -> Result<()>
where
    F: FnMut(Scanned) -> Result<()>,
{
    // The merge wants each cursor ascending by system, and the reader wants
    // a system's members in the order the game numbers them: a body file is
    // written in the order the rows arrive, and heap order is not an order --
    // an updated row moves, so a republish rewrites files whose contents did
    // not change and the two derivations of one system disagree about the
    // order of what is in it.
    let stars_sql = format!("{STARS_SELECT} ORDER BY system_address, id");
    let barycenters_sql =
        format!("{BARYCENTERS_SELECT} ORDER BY system_address, id");
    let bodies_sql = format!(
        "{BODIES_SELECT} GROUP BY b.system_address, b.id \
         ORDER BY b.system_address, b.id"
    );
    // One row per system, `systems.address` being its key, so the system is
    // the whole of the order there is.
    let placed_sql = format!("{BOOSTABLE_SELECT} ORDER BY system_address");

    let mut stars =
        ByAddress::open(db, &stars_sql, |row| star_from_row(row).map(Into::into))
            .await?;
    let mut bodies = ByAddress::open(db, &bodies_sql, |row| {
        body_from_row(row).map(Into::into)
    })
    .await?;
    let mut barycenters = ByAddress::open(db, &barycenters_sql, |row| {
        barycenter_from_row(row).map(Into::into)
    })
    .await?;
    let mut placed = ByAddress::open(db, &placed_sql, |row| {
        Ok(row.try_get::<Option<String>, _>("primary_star_class")?)
    })
    .await?;

    let mut routed = Vec::new();
    loop {
        // By value and not `into_iter`, this crate being on the 2018
        // edition, where an array's `into_iter` is the reference's.
        let next = IntoIterator::into_iter([
            stars.at(),
            bodies.at(),
            barycenters.at(),
            placed.at(),
        ])
        .flatten()
        .min();
        let Some(address) = next else { return Ok(()) };

        let mut inside = meta::SystemBodies::default();
        stars.take(address, &mut inside.stars).await?;
        bodies.take(address, &mut inside.bodies).await?;
        barycenters.take(address, &mut inside.barycenters).await?;
        routed.clear();
        placed.take(address, &mut routed).await?;

        each(Scanned {
            address,
            inside,
            placed: !routed.is_empty(),
            routed: routed.first().cloned().flatten(),
        })?;
    }
}

/// Every star of `addresses`, grouped under its system by the caller, mapped
/// as [`crate::stars::Star::fetch_all`] maps one system's.
///
/// By system and then by id, as [`each_scanned`] reads them: a pass and a
/// build write the same body file for a system and cannot order what is in
/// it differently.
async fn all_stars(db: &Database, addresses: &[i64]) -> Result<Vec<Star>> {
    let rows = sqlx::query(&format!(
        "{STARS_SELECT} WHERE system_address = ANY($1) \
         ORDER BY system_address, id"
    ))
    .bind(addresses)
    .fetch_all(&db.pool)
    .await?;
    rows.iter().map(star_from_row).collect()
}

/// Every body of `addresses`, with what it is made of gathered alongside,
/// grouped under its system by the caller. The materials join and the
/// grouping are [`crate::bodies`]'s own; only the `WHERE` differs.
async fn all_bodies(db: &Database, addresses: &[i64]) -> Result<Vec<Body>> {
    let rows = sqlx::query(&format!(
        "{BODIES_SELECT} WHERE b.system_address = ANY($1) \
         GROUP BY b.system_address, b.id \
         ORDER BY b.system_address, b.id"
    ))
    .bind(addresses)
    .fetch_all(&db.pool)
    .await?;
    rows.iter().map(body_from_row).collect()
}

/// Every barycenter of `addresses`, grouped under its system by the caller,
/// mapped as [`crate::barycenters::Barycenter::fetch_all`] maps one
/// system's.
async fn all_barycenters(
    db: &Database,
    addresses: &[i64],
) -> Result<Vec<Barycenter>> {
    let rows = sqlx::query(&format!(
        "{BARYCENTERS_SELECT} WHERE system_address = ANY($1) \
         ORDER BY system_address, id"
    ))
    .bind(addresses)
    .fetch_all(&db.pool)
    .await?;
    rows.iter().map(barycenter_from_row).collect()
}

/// One `stars` row as a [`Star`], read by column name. The orbit is whole or
/// absent by [`orbit::read`], the primary going round nothing.
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
        parents: ancestry(parent_ids, parent_types),
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
/// this reads, as a [`Body`]. The surface is absent where a gas giant
/// carries none; the orbit is always present.
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
        parents: ancestry(parent_ids, parent_types),
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

// The four projections below are `From` impls rather than private helpers
// because the same projection is what `galos_db`'s conformance tests compare
// a merged row against: the rule for merging a scan is stated once in
// `galos_index::merge`.

/// A [`Surface`] as its field-identical [`meta::Surface`].
impl From<Surface> for meta::Surface {
    fn from(surface: Surface) -> meta::Surface {
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
}

/// A [`Star`] as its field-identical [`meta::Star`].
impl From<Star> for meta::Star {
    fn from(star: Star) -> meta::Star {
        meta::Star {
            system_address: star.system_address,
            id: star.id,
            name: star.name,
            parents: star.parents,
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
}

/// A [`Body`] as its field-identical [`meta::Body`].
impl From<Body> for meta::Body {
    fn from(body: Body) -> meta::Body {
        meta::Body {
            system_address: body.system_address,
            id: body.id,
            parents: body.parents,
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
            surface: body.surface.map(Into::into),
            orbit: body.orbit,
            spin: body.spin,
            mapped: body.mapped,
            discovered_at: body.discovered_at,
        }
    }
}

/// A [`Barycenter`] as its field-identical [`meta::Barycenter`].
impl From<Barycenter> for meta::Barycenter {
    fn from(barycenter: Barycenter) -> meta::Barycenter {
        meta::Barycenter {
            system_address: barycenter.system_address,
            id: barycenter.id,
            updated_at: barycenter.updated_at,
            updated_by: barycenter.updated_by,
            orbit: barycenter.orbit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use galos_index::Parent;

    /// A system's bodies survive the trip out to disk and back through the
    /// same path helpers, format and reader the client uses.
    ///
    /// No database: the values are built by hand, written with [`write_meta`]
    /// under [`source::bodies_path`], and read back through a [`FsSource`].
    ///
    /// The surfaced body is here on purpose: its [`BodyType`] and its
    /// [`AtmosphereType`] are `#[serde(untagged)]` enums with an
    /// `Unknown(String)` arm, which read back only from a self-describing
    /// format, so both fields are asserted after the read.
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
            stars: vec![star.into()],
            bodies: vec![gas_giant.into()],
            barycenters: vec![barycenter.into()],
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
            bodies: vec![surfaced.into()],
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
    /// [`Metadata::resume`] tolerates an absent table, a directory published
    /// before it existed having none. Only an absence: read as empty, a
    /// truncated file would be republished from the handful of addresses one
    /// pass touches and the rows already published would be gone. A refused
    /// resume is a full rebuild, which is recoverable.
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
    /// The only thing that writes `systems.primary_star_class` is a plotted
    /// route, so a scanned neutron star has `N` in `stars` and a null column.
    /// Reading the column alone published no supercharge for it.
    ///
    /// Needs a server to reach, named by `TEST_DATABASE_URL`, and stands
    /// down without one.
    #[async_std::test]
    async fn a_scanned_arrival_star_is_what_supercharges() {
        let Some(db) = Scratch::new().await else { return };

        let scanned = 0x0B00_5700_0000_0001_u64 as i64;
        let routed = 0x0B00_5700_0000_0002_u64 as i64;

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

        // Over the rows a full build has in hand, which is every scanned
        // thing there is, handed over a system at a time.
        let mut whole: HashMap<i64, meta::Boost> = HashMap::new();
        each_scanned(&db, |system| {
            if let Some(boost) = system.boost() {
                whole.insert(system.address, boost);
            }
            Ok(())
        })
        .await
        .expect("a full read");
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
        let some = bodies_of(&db, &[scanned, routed])
            .await
            .expect("the scanned things of two systems");
        let touched: HashMap<i64, meta::Boost> =
            boosts_of(&db, &[scanned, routed], &some)
                .await
                .expect("a read of what changed")
                .into_iter()
                .collect();
        for address in [scanned, routed] {
            assert_eq!(
                touched.get(&address),
                whole.get(&address),
                "the two reads disagree about {address}",
            );
        }

        db.done().await;
    }

    /// A system's members come back in the order the game numbers them
    ///
    /// `ORDER BY system_address` alone leaves the rows within one system in
    /// whatever order the heap holds, which is not an order: an updated row
    /// is written somewhere else. A body file is written in the order the
    /// rows arrive, so a republish rewrote files whose contents had not
    /// changed, and a directory raised from the dump, where the rows are
    /// read in ascending `bodyId`, disagreed with one raised from here
    /// about the order of a system's barycenters.
    ///
    /// Written highest id first and then updated, so heap order and id
    /// order are not the same order.
    ///
    /// Needs a server to reach, named by `TEST_DATABASE_URL`, and stands
    /// down without one.
    #[async_std::test]
    async fn a_systems_members_come_back_in_id_order() {
        let Some(db) = Scratch::new().await else { return };

        let address = 0x0B00_5700_0000_0003_u64 as i64;
        sqlx::query(
            "INSERT INTO systems (address, name, position, updated_at, \
                                  updated_by) \
             VALUES ($1, 'ORDERED', ST_MakePoint(7, 8, 9)::geometry, \
                     now(), 'test')",
        )
        .bind(address)
        .execute(&db.pool)
        .await
        .expect("the system should write");

        for id in [3i16, 2, 1] {
            sqlx::query(
                "INSERT INTO stars (system_address, id, name, updated_at, \
                     updated_by, absolute_magnitude, age_my, \
                     distance_from_arrival_ls, luminosity, star_class, \
                     stellar_mass, subclass, axial_tilt, radius, \
                     rotation_period, temperature, was_mapped) \
                 VALUES ($1, $2, $3, now(), 'test', 4.8, 100, 0, 'V', 'G', \
                         1.0, 2, 0, 1.0, 0, 5000, false)",
            )
            .bind(address)
            .bind(id)
            .bind(format!("ORDERED {id}"))
            .execute(&db.pool)
            .await
            .expect("the star should write");
        }
        for id in [6i16, 5, 4] {
            sqlx::query(
                "INSERT INTO barycenters (system_address, id, updated_at, \
                     updated_by) \
                 VALUES ($1, $2, now(), 'test')",
            )
            .bind(address)
            .bind(id)
            .execute(&db.pool)
            .await
            .expect("the barycenter should write");
        }
        for id in [9i16, 8, 7] {
            sqlx::query(
                "INSERT INTO bodies (system_address, id, name, updated_at, \
                     updated_by, planet_class, tidal_lock, landable, mass, \
                     radius, gravity, semi_major_axis, eccentricity, \
                     orbital_inclination, periapsis, orbital_period, \
                     rotation_period, axial_tilt, was_mapped) \
                 VALUES ($1, $2, $3, now(), 'test', 'Rocky body', false, \
                         false, 1.0, 1.0, 1.0, 1.0, 0, 0, 0, 1.0, 1.0, 0, \
                         false)",
            )
            .bind(address)
            .bind(id)
            .bind(format!("ORDERED {id}"))
            .execute(&db.pool)
            .await
            .expect("the body should write");
        }

        // What moves a row: the first one written is rewritten, so the
        // order the rows were laid down in is no longer the order they lie
        // in.
        for statement in [
            "UPDATE stars SET temperature = 6000 \
             WHERE system_address = $1 AND id = 3",
            "UPDATE barycenters SET updated_by = 'again' \
             WHERE system_address = $1 AND id = 6",
            "UPDATE bodies SET gravity = 2 \
             WHERE system_address = $1 AND id = 9",
        ] {
            sqlx::query(statement)
                .bind(address)
                .execute(&db.pool)
                .await
                .expect("the row should update");
        }

        // What a full build reads, and what a watch pass reads: the same
        // system, and the same order within it.
        let mut built = meta::SystemBodies::default();
        each_scanned(&db, |system| {
            if system.address == address {
                built = system.inside;
            }
            Ok(())
        })
        .await
        .expect("a full read");
        let mut passed = bodies_of(&db, &[address])
            .await
            .expect("the scanned things of one system");
        let passed = passed.remove(&address).expect("the system was read");

        for inside in [&built, &passed] {
            let stars: Vec<i16> =
                inside.stars.iter().map(|star| star.id).collect();
            let bodies: Vec<i16> =
                inside.bodies.iter().map(|body| body.id).collect();
            let barycenters: Vec<i16> =
                inside.barycenters.iter().map(|it| it.id).collect();
            assert_eq!(stars, vec![1, 2, 3], "the stars are in heap order");
            assert_eq!(bodies, vec![7, 8, 9], "the bodies are in heap order");
            assert_eq!(
                barycenters,
                vec![4, 5, 6],
                "the barycenters are in heap order",
            );
        }

        db.done().await;
    }
}
