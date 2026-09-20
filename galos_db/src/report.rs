//! What the database is, whether it is sound, and what it holds
//!
//! Three questions an operator asks of a galaxy they cannot read: how
//! current is it, is anything in it broken, and what is in it. They are
//! answered here rather than in the binary for the same reason
//! [`crate::catalog::report`] is — the judgement about what is worth saying
//! is a judgement about the schema, and the schema lives on this side.
//!
//! # What these verbs may cost
//!
//! `systems` is 3.4 million rows and 2.2 GB on the development server, and
//! the galaxy target is 200 million. That rules out `count(*)` as a way of
//! answering "how many", so [`status`] asks the planner's statistics
//! instead and says that it did. It also means every check in [`verify`]
//! had to be measured rather than guessed at; the timings below are from
//! that 3.4 M-system database, warm, at the server's default `work_mem` of
//! 4 MB.

use crate::migrate;
use crate::{Database, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::Row;
use std::fmt;

/// The tables [`status`] estimates, in the order it reports them.
///
/// The spine of the galaxy first, then what is hung off a system, then the
/// trade side. Not every table in the schema: `outfitting`, `commodities`
/// and `body_materials` are each larger than `systems` and none of them
/// says anything about how far along an import is, which is the question
/// `status` exists to answer. They turn up in [`stats`] instead, where
/// size is the point.
const WATCHED: &[&str] = &[
    "systems",
    "bodies",
    "stars",
    "barycenters",
    "body_signals",
    "system_signals",
    "stations",
    "markets",
    "system_factions",
];

/// How current the database is and roughly how much of it there is.
///
/// The two clocks are kept apart because they are different questions.
/// `changed` is the newest `systems.updated_at`, which is when the galaxy
/// last did something as far as this database knows. `arrived` is the newest
/// `received_at` on each of the three tables that carry one, which is when a
/// report last reached us — the clock a watch's cursor and the index's
/// checkpoint are comparable to. A database fed by a journal import moves
/// the first one backwards and the second one forwards, and only the second
/// says whether the feed is alive.
pub struct Status {
    /// The newest `_sqlx_migrations` row, from [`migrate::applied`]. [`None`]
    /// is an unmigrated database, not a failure to ask.
    pub version: Option<i64>,
    pub described: Option<String>,

    /// Newest `systems.updated_at`: when the galaxy last changed.
    pub changed: Option<DateTime<Utc>>,

    /// Newest `received_at` per table: when a report last reached us.
    ///
    /// [`None`] against a table means nothing there has arrived since the
    /// column was added, since the migration left existing rows null on
    /// purpose rather than invent an arrival for them.
    pub arrived: Vec<(&'static str, Option<DateTime<Utc>>)>,

    /// Estimated live rows per table of [`WATCHED`], in that order. [`None`]
    /// where the table is not in this database at all, which is how a
    /// half-migrated one shows itself.
    pub estimates: Vec<(&'static str, Option<i64>)>,

    /// `factions`, counted exactly.
    ///
    /// Seventy-odd thousand rows, so the scan is nothing, and it is the one
    /// number here somebody might want to compare against a published
    /// figure.
    pub factions: i64,
}

impl Status {
    /// What an unmigrated database answers: a version of [`None`] and
    /// nothing else.
    ///
    /// `status` is the first verb anybody runs, and on a database the
    /// migrations have never touched every table it would ask about is
    /// missing. Reporting that as `relation "systems" does not exist` is
    /// a raw error where the answer is simply "there is no schema here
    /// yet" — which the report says, and names the verb that fixes it.
    fn unmigrated() -> Status {
        Status {
            version: None,
            described: None,
            changed: None,
            arrived: Vec::new(),
            estimates: Vec::new(),
            factions: 0,
        }
    }
}

/// Ask the database what it is.
///
/// Two round trips: one for the clocks and the exact faction count, one for
/// the row estimates. The estimates come from `pg_class.reltuples`, which is
/// what `ANALYZE` last wrote, falling back to `pg_stat_user_tables.n_live_tup`
/// for a table Postgres records as never analysed (`reltuples = -1`) — the
/// live-tuple counter is the only figure a freshly restored table has. Both
/// are estimates and are labelled as such in the report: a `count(*)` over
/// `systems` is a two-gigabyte sequential scan today and a hundred-odd
/// gigabyte one at the galaxy target, which is not a thing a status verb may
/// do.
pub async fn status(db: &Database) -> Result<Status> {
    // A database with no `_sqlx_migrations` has none of the tables below
    // either, and asking about them would answer with the first one
    // Postgres missed rather than with the thing that is actually wrong.
    let Some(newest) = migrate::applied(db).await? else {
        return Ok(Status::unmigrated());
    };

    let clocks: (
        Option<NaiveDateTime>,
        Option<NaiveDateTime>,
        Option<NaiveDateTime>,
        Option<NaiveDateTime>,
        i64,
    ) = sqlx::query_as(
        "SELECT (SELECT max(updated_at) FROM systems), \
                (SELECT max(received_at) FROM systems), \
                (SELECT max(received_at) FROM stars), \
                (SELECT max(received_at) FROM system_factions), \
                (SELECT count(*)::bigint FROM factions)",
    )
    .fetch_one(&db.pool)
    .await?;

    let named: Vec<String> = WATCHED.iter().map(|it| it.to_string()).collect();
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT c.relname::text, \
                CASE WHEN c.reltuples < 0 \
                     THEN COALESCE(s.n_live_tup, 0) \
                     ELSE c.reltuples::bigint END \
           FROM pg_class c \
           JOIN pg_namespace n ON n.oid = c.relnamespace \
           LEFT JOIN pg_stat_user_tables s ON s.relid = c.oid \
          WHERE n.nspname = 'public' \
            AND c.relkind = 'r' \
            AND c.relname = ANY($1)",
    )
    .bind(&named)
    .fetch_all(&db.pool)
    .await?;

    Ok(Status {
        version: Some(newest.0),
        described: Some(newest.1),
        changed: clocks.0.map(|it| it.and_utc()),
        arrived: vec![
            ("systems", clocks.1.map(|it| it.and_utc())),
            ("stars", clocks.2.map(|it| it.and_utc())),
            ("system_factions", clocks.3.map(|it| it.and_utc())),
        ],
        estimates: WATCHED
            .iter()
            .map(|table| {
                let found = rows
                    .iter()
                    .find(|(name, _)| name == table)
                    .map(|(_, rows)| *rows);
                (*table, found)
            })
            .collect(),
        factions: clocks.4,
    })
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match (self.version, &self.described) {
            (Some(version), Some(description)) => {
                writeln!(f, "migration {version} ({description})")?
            }
            // The tables were never asked about, so there is nothing
            // below to print: see `Status::unmigrated`.
            _ => {
                return writeln!(
                    f,
                    "no migrations on record: this database has no schema \
                     yet. `galos-db migrate` puts one in."
                )
            }
        }

        writeln!(f, "\nclocks:")?;
        writeln!(f, "  {:<24} {}", "galaxy changed", when(self.changed))?;
        for (table, at) in &self.arrived {
            writeln!(f, "  {:<24} {}", format!("{table} arrived"), when(*at))?;
        }
        if self.arrived.iter().all(|(_, at)| at.is_none()) {
            writeln!(
                f,
                "  nothing has arrived since `received_at` was added; a watch \
                 following that clock has no cursor to move"
            )?;
        }

        writeln!(f, "\nrows (estimated from the planner's statistics):")?;
        for (table, rows) in &self.estimates {
            match rows {
                Some(rows) => {
                    writeln!(f, "  {:<24} {:>15}", table, grouped(*rows))?
                }
                None => writeln!(f, "  {:<24} {:>15}", table, "absent")?,
            }
        }
        writeln!(
            f,
            "  {:<24} {:>15}  (exact)",
            "factions",
            grouped(self.factions)
        )?;

        Ok(())
    }
}

/// What is wrong with the database that a constraint is not already
/// stopping.
///
/// Nothing here re-checks something the schema guarantees. Every
/// `system_address` on every child table is a foreign key to `systems`,
/// every table has its primary key, `barycenters` has its
/// `barycenters_orbit_whole` check, and asking after any of those would be
/// asking Postgres whether Postgres works. What is left is the joins the
/// schema cannot express and the columns that were made nullable on purpose.
///
/// # Damage against backlog
///
/// Three of these counts are a database waiting for data it has not been
/// sent yet, and [`Self::is_sound`] does not fail on them:
///
/// - `systems_without_position`. A system known by name from a market or a
///   faction report but never visited, so nobody has sent coordinates for
///   it. The index build filters `position IS NOT NULL`, so these are
///   simply not served; they are not wrong.
/// - `markets_without_system`. Market data that arrived before its system
///   did. `markets.system_address` was made nullable exactly so this could
///   be held rather than dropped, and `markets_waiting_on_system` is the
///   partial index that finds them again when the system lands.
/// - `stations_without_type`. The same phenomenon and the same migration:
///   `20260802080000_accept_market_data` made `stations.ty` nullable in
///   the same breath as `markets.system_address`, because market data
///   names a station before anything has docked there to report what kind
///   it is. 25,110 of them on a development database of 3.4 M systems —
///   ordinary operation, so failing on it would be a `verify` that exits
///   1 on every healthy live database and can therefore never be run
///   from anything.
///
/// The rest are damage: a reference that names a row that does not exist,
/// where the code that follows it assumes it will.
pub struct Verified {
    /// Backlog. Systems no index can serve, for want of coordinates.
    pub systems_without_position: i64,

    /// Backlog. Market data still waiting on its system row.
    pub markets_without_system: i64,

    /// Backlog. Placeholder stations: a name and a system from market
    /// data, with no station type, because nothing has docked there and
    /// reported one.
    pub stations_without_type: i64,

    /// `body_signals` rows naming a body that is not on record.
    ///
    /// `body_signals` has a foreign key to `systems` and none to `bodies` —
    /// the signal usually arrives from an FSS honk before the body is
    /// scanned, so the key could not be there. Nothing repairs these once
    /// the scan lands, so a count that only grows is the feed dropping body
    /// scans.
    pub body_signals_without_body: i64,

    /// `stations.body_id` naming a body that is not on record. Same shape,
    /// same absent key, same reading.
    pub stations_without_body: i64,

    /// `system_factions` rows naming a faction that is not on record.
    ///
    /// The foreign key says this cannot happen. `index/metadata.rs`'s
    /// `POPULATED_SELECT` filters its faction array with
    /// `EXISTS (SELECT 1 FROM factions …)` as though it could. One of the
    /// two is stale and this count decides which: **zero means the filter
    /// is the stale one** and can go, and nonzero means the foreign key is
    /// not being enforced on the rows that are actually there — which is
    /// what a `pg_restore --disable-triggers` leaves behind, and what the
    /// development server shows today at seventy-six thousand rows against
    /// a constraint Postgres lists as present.
    pub system_factions_without_faction: i64,

    /// `bodies.parent_ids` entries naming nothing in their own system.
    pub bodies_with_dangling_parents: i64,

    /// `stars.parent_ids` entries naming nothing in their own system.
    pub stars_with_dangling_parents: i64,
}

impl Verified {
    /// Whether anything in here is wrong rather than merely unfinished.
    ///
    /// **One count decides it**: `system_factions_without_faction`. There
    /// is a foreign key on that column, so a nonzero count does not mean
    /// "a report arrived out of order" — it means the constraint was not
    /// enforced on the rows that are actually there, which is what
    /// `pg_restore --disable-triggers` leaves behind. Nothing else here
    /// can say that: every other reference names a body or a station type
    /// that Postgres was never asked to guarantee, because the report
    /// that names it routinely arrives before the report that defines it.
    ///
    /// **Measured, because the first version of this was wrong.** A
    /// database migrated from nothing and fed fifteen seconds of the live
    /// feed answers 73 dangling body parents, 7 dangling star parents, 3
    /// stations naming no body and 3 stations with no type — ordinary
    /// in-flight state, 1.78 M of the 1.92 M on the development server
    /// naming body id 0, the arrival star every scan names as a parent
    /// and which is written as a row only when it is itself scanned. A
    /// rule that failed on those is a `verify` that exits 1 on every
    /// database that has ever read the feed, which is a verb no cron can
    /// run and therefore a verb nobody runs.
    ///
    /// The dangling counts are still worth reading, and the reason they
    /// are reported rather than forgiven: they should hover, not climb. A
    /// count that grows run over run is the feed dropping body scans,
    /// which no constraint will ever tell anybody.
    pub fn is_sound(&self) -> bool {
        self.system_factions_without_faction == 0
    }
}

/// Run every check.
///
/// Two statements. The first carries the six cheap counts as scalar
/// subqueries so they cost one round trip; five of them are index or
/// small-table work and `systems WHERE position IS NULL` is a five-second
/// sequential scan over 3.4 M rows, there being no index that answers
/// `IS NULL` on a GiST-indexed geometry.
///
/// The second is the ancestry check, and it is the one that had to be
/// written carefully. The obvious form — three correlated `NOT EXISTS`
/// lookups per unnested parent — takes **12 seconds** over 5 M parent
/// references, because it is 15 M index probes. Unioning the three id
/// spaces into one subquery instead lets the planner build a single hash of
/// the 5 M `(system_address, id)` pairs and stream the references past it,
/// which is **1.3 seconds** for both tables at once. That is inside what a
/// verb can ask for, so it is not bounded or sampled; the same shape at the
/// 200 M-system target is a hash that no longer fits `work_mem` and will
/// need a bound, but writing one now would be inventing a limit for numbers
/// nobody has measured.
///
/// The check exists because `orbit`'s ancestry walk assumes a `parent_ids`
/// entry resolves to a body, a star or a barycenter in the same system. The
/// development database has 1.9 M body references and 750 k star references
/// that do not, nearly all of them to id 0 — the arrival star, which is
/// named as a parent by every scan in the system and written as a row only
/// when it is itself scanned.
pub async fn verify(db: &Database) -> Result<Verified> {
    let cheap: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT count(*)::bigint FROM systems WHERE position IS NULL), \
           (SELECT count(*)::bigint FROM markets \
             WHERE system_address IS NULL), \
           (SELECT count(*)::bigint FROM stations WHERE ty IS NULL), \
           (SELECT count(*)::bigint FROM body_signals g \
             WHERE NOT EXISTS (SELECT 1 FROM bodies b \
                                WHERE b.system_address = g.system_address \
                                  AND b.id = g.body_id)), \
           (SELECT count(*)::bigint FROM stations s \
             WHERE s.body_id IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM bodies b \
                                WHERE b.system_address = s.system_address \
                                  AND b.id = s.body_id)), \
           (SELECT count(*)::bigint FROM system_factions sf \
             WHERE NOT EXISTS (SELECT 1 FROM factions f \
                                WHERE f.id = sf.faction_id))",
    )
    .fetch_one(&db.pool)
    .await?;

    let ancestry: (i64, i64) = sqlx::query_as(
        "WITH refs AS ( \
             SELECT 0 AS kind, b.system_address AS sa, p.parent AS parent \
               FROM bodies b \
               CROSS JOIN LATERAL unnest(b.parent_ids) AS p(parent) \
             UNION ALL \
             SELECT 1, s.system_address, p.parent \
               FROM stars s \
               CROSS JOIN LATERAL unnest(s.parent_ids) AS p(parent) \
         ) \
         SELECT count(*) FILTER (WHERE kind = 0)::bigint, \
                count(*) FILTER (WHERE kind = 1)::bigint \
           FROM refs d \
          WHERE NOT EXISTS ( \
            SELECT 1 FROM ( \
              SELECT system_address, id FROM bodies \
              UNION ALL SELECT system_address, id FROM stars \
              UNION ALL SELECT system_address, id FROM barycenters \
            ) k WHERE k.system_address = d.sa AND k.id = d.parent)",
    )
    .fetch_one(&db.pool)
    .await?;

    Ok(Verified {
        systems_without_position: cheap.0,
        markets_without_system: cheap.1,
        stations_without_type: cheap.2,
        body_signals_without_body: cheap.3,
        stations_without_body: cheap.4,
        system_factions_without_faction: cheap.5,
        bodies_with_dangling_parents: ancestry.0,
        stars_with_dangling_parents: ancestry.1,
    })
}

impl fmt::Display for Verified {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "waiting on data that has not arrived:")?;
        for (count, what) in [
            (self.systems_without_position, "systems with no position"),
            (self.markets_without_system, "markets with no system"),
            (self.stations_without_type, "stations with no type yet"),
        ] {
            writeln!(f, "  {:>15}  {}", grouped(count), what)?;
        }

        writeln!(
            f,
            "\nreferences to bodies nobody has scanned yet (these should \
             hover, not climb):"
        )?;
        for (count, what) in [
            (self.body_signals_without_body, "body signals with no body"),
            (self.stations_without_body, "stations naming no body"),
            (
                self.bodies_with_dangling_parents,
                "body parent ids naming nothing in their system",
            ),
            (
                self.stars_with_dangling_parents,
                "star parent ids naming nothing in their system",
            ),
        ] {
            writeln!(f, "  {:>15}  {}", grouped(count), what)?;
        }

        writeln!(f, "\nwhat the schema should have prevented:")?;
        writeln!(
            f,
            "  {:>15}  {}",
            grouped(self.system_factions_without_faction),
            "system factions naming no faction",
        )?;

        match self.is_sound() {
            true => writeln!(
                f,
                "\nsound. Every faction id resolves, so the foreign key on \
                 that column is being enforced — and the \
                 `EXISTS (SELECT 1 FROM factions …)` filter in \
                 index/metadata.rs is guarding against what it already \
                 forbids. Everything above it is data not sent yet."
            )?,
            false => writeln!(
                f,
                "\nnot sound. A faction id names no faction, against a \
                 foreign key Postgres lists as present: the constraint was \
                 not enforced on the rows that are there, which is what a \
                 restore with its triggers disabled leaves behind."
            )?,
        }

        Ok(())
    }
}

/// One row of a breakdown: how many systems, and how many people in them.
pub struct Group {
    /// The enum label, or [`None`] for the systems that have none on record
    /// — which is most of the galaxy, and worth showing rather than
    /// filtering out.
    pub value: Option<String>,
    pub systems: i64,
    pub population: i64,
}

/// The galaxy as this database holds it.
///
/// Only from columns that exist. `sql/stats/power.sql` asks after `power`
/// and `power_state`, which no migration in this tree ever created, so it
/// is not lifted here; `sql/stats/economy.sql` and `sql/table_sizes.sql`
/// are, and this is now where those two live.
pub struct Stats {
    pub economies: Vec<Group>,
    pub governments: Vec<Group>,
    pub allegiances: Vec<Group>,
    pub securities: Vec<Group>,
    /// From `systems.primary_star_class`, not from `stars`: the column on
    /// `systems` is already one row per system, and counting `stars` would
    /// weight a system by how many of its stars have been scanned.
    pub star_classes: Vec<Group>,
    /// Largest relations by `pg_total_relation_size`, in bytes, largest
    /// first.
    pub tables: Vec<(String, i64)>,
}

/// How many relations [`stats`] lists. Enough to reach past the four tables
/// that dominate every Elite database and show what comes after them.
const LARGEST: i64 = 12;

/// Read the galaxy's shape.
///
/// The five breakdowns are one statement and one sequential scan of
/// `systems`, not five. `GROUP BY GROUPING SETS` is what makes that
/// possible: five independent groupings computed in a single pass, 1.6
/// seconds over 3.4 M rows against roughly five times that for five
/// separate `GROUP BY`s. `GROUPING()` says which set a row came from, and
/// because only the grouped column is non-null in its own set, one
/// `COALESCE` recovers the label whichever set it is.
pub async fn stats(db: &Database) -> Result<Stats> {
    let rows = sqlx::query(
        "SELECT \
           CASE WHEN GROUPING(primary_economy) = 0 THEN 'economy' \
                WHEN GROUPING(government) = 0 THEN 'government' \
                WHEN GROUPING(allegiance) = 0 THEN 'allegiance' \
                WHEN GROUPING(security) = 0 THEN 'security' \
                ELSE 'star_class' END AS dimension, \
           COALESCE(primary_economy::text, government::text, \
                    allegiance::text, security::text, \
                    primary_star_class) AS value, \
           count(*)::bigint AS systems, \
           COALESCE(sum(population), 0)::bigint AS population \
         FROM systems \
         GROUP BY GROUPING SETS ((primary_economy), (government), \
                                 (allegiance), (security), \
                                 (primary_star_class))",
    )
    .fetch_all(&db.pool)
    .await?;

    let mut stats = Stats {
        economies: Vec::new(),
        governments: Vec::new(),
        allegiances: Vec::new(),
        securities: Vec::new(),
        star_classes: Vec::new(),
        tables: Vec::new(),
    };

    for row in &rows {
        let dimension: String = row.try_get("dimension")?;
        let group = Group {
            value: row.try_get("value")?,
            systems: row.try_get("systems")?,
            population: row.try_get("population")?,
        };
        match dimension.as_str() {
            "economy" => stats.economies.push(group),
            "government" => stats.governments.push(group),
            "allegiance" => stats.allegiances.push(group),
            "security" => stats.securities.push(group),
            _ => stats.star_classes.push(group),
        }
    }

    // Sorted here and not in SQL because the two kinds of breakdown want
    // different orders out of one query: the four civic ones are about
    // where people are, and the star classes are a histogram of a galaxy
    // that is almost entirely empty, where population would sort every
    // class that matters to the bottom.
    for breakdown in [
        &mut stats.economies,
        &mut stats.governments,
        &mut stats.allegiances,
        &mut stats.securities,
    ] {
        breakdown.sort_by_key(|g| (-g.population, -g.systems));
    }
    stats.star_classes.sort_by_key(|g| -g.systems);

    // Lifted from `sql/table_sizes.sql`, which asked `pg_size_pretty` for a
    // string; the bytes are kept instead so a caller can compare them and
    // the report does its own rounding.
    stats.tables = sqlx::query_as(
        "SELECT (n.nspname || '.' || c.relname)::text, \
                pg_total_relation_size(c.oid)::bigint AS bytes \
           FROM pg_class c \
           JOIN pg_namespace n ON n.oid = c.relnamespace \
          WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') \
            AND n.nspname !~ '^pg_toast' \
            AND c.relkind = 'r' \
          ORDER BY bytes DESC LIMIT $1",
    )
    .bind(LARGEST)
    .fetch_all(&db.pool)
    .await?;

    Ok(stats)
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for (title, breakdown) in [
            ("economy", &self.economies),
            ("government", &self.governments),
            ("allegiance", &self.allegiances),
            ("security", &self.securities),
        ] {
            writeln!(f, "by {title}:")?;
            writeln!(f, "  {:<24} {:>13} {:>18}", "", "systems", "population")?;
            for group in breakdown.iter() {
                writeln!(
                    f,
                    "  {:<24} {:>13} {:>18}",
                    group.value.as_deref().unwrap_or("(none recorded)"),
                    grouped(group.systems),
                    grouped(group.population),
                )?;
            }
            writeln!(f)?;
        }

        writeln!(f, "by primary star class:")?;
        for group in &self.star_classes {
            writeln!(
                f,
                "  {:<24} {:>13}",
                group.value.as_deref().unwrap_or("(none recorded)"),
                grouped(group.systems),
            )?;
        }

        writeln!(f, "\nlargest relations:")?;
        for (relation, bytes) in &self.tables {
            writeln!(f, "  {:<32} {:>10}", relation, size(*bytes))?;
        }

        Ok(())
    }
}

/// A timestamp as a reader wants it, or a word for not having one.
fn when(at: Option<DateTime<Utc>>) -> String {
    match at {
        Some(at) => at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => "never".to_string(),
    }
}

/// A count with thousands separators.
///
/// Every number in these reports is somewhere between a handful and a few
/// hundred million, and the difference between 3412771 and 34127710 is not
/// one an eye catches without them.
fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A byte count in the largest unit that leaves it above one.
///
/// `pg_size_pretty` would do this server-side, as `sql/table_sizes.sql`
/// had it, but then the struct would carry a string nobody can compare.
fn size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one piece of arithmetic here that is not the database's.
    #[test]
    fn counts_are_grouped_in_threes() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(3_412_771), "3,412,771");
        assert_eq!(grouped(-1_234), "-1,234");
    }

    /// Backlog is reported and forgiven; a dangling reference is not.
    #[test]
    fn backlog_is_not_damage() {
        let mut verified = Verified {
            systems_without_position: 26_213,
            markets_without_system: 167,
            stations_without_type: 0,
            body_signals_without_body: 0,
            stations_without_body: 0,
            system_factions_without_faction: 0,
            bodies_with_dangling_parents: 0,
            stars_with_dangling_parents: 0,
        };
        assert!(verified.is_sound());

        verified.stars_with_dangling_parents = 1;
        assert!(!verified.is_sound());
    }
}
