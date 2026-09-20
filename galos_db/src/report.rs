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
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
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

/// Both of a table's clocks, as the database holds them.
///
/// Kept as a pair per table rather than as one galaxy clock standing over
/// three arrivals, because only the pair is comparable. `stars` going a
/// minute without an arrival while `systems` keeps changing is an ordinary
/// minute on a live feed, and a report that set those two side by side
/// would have called it a skew several times an hour.
pub struct Clocks {
    /// Which table these were read off.
    pub table: &'static str,

    /// Newest `updated_at`: when the newest thing here happened out in the
    /// galaxy, as the report that carried it says. [`None`] only where the
    /// table holds no rows at all, the column being `NOT NULL` on all
    /// three.
    pub changed: Option<DateTime<Utc>>,

    /// Newest `received_at`: when this database last wrote a row here.
    ///
    /// The clock a watch's cursor and the index's checkpoint are
    /// comparable to. [`None`] against a table that has rows means not one
    /// of them carries an arrival: `20260816000050` left the rows it found
    /// null on purpose rather than invent one for them, and the partial
    /// indexes it created are `WHERE received_at IS NOT NULL`, so a watch
    /// cannot see those rows either. That is
    /// [`Self::predates_arrivals`], and it is a different thing from an
    /// empty table however alike the two look in a column of timestamps.
    pub arrived: Option<DateTime<Utc>>,
}

impl Clocks {
    /// How long after the reading the row carrying it was written.
    ///
    /// The delivery latency, which is a second or so off the live feed and
    /// as long as the import ran for a database fed from a journal. It is
    /// positive in the ordinary case and that is not a fault: the write
    /// cannot precede the event it records. Negative is
    /// [`Self::ahead`], and is the one direction that is wrong.
    pub fn lag(&self) -> Option<Duration> {
        Some(self.arrived? - self.changed?)
    }

    /// How far the newest reading here is dated past the moment this
    /// database wrote it down, where it is dated past it at all.
    ///
    /// A row's `updated_at` is `GREATEST(old, $stamp)` and its
    /// `received_at` is `clock_timestamp() AT TIME ZONE 'utc'` at the
    /// write, so `updated_at <= received_at` holds on every row whose
    /// sender's clock was not ahead of this server's, and the maxima
    /// inherit it: the row holding `max(updated_at)` has a `received_at`
    /// no larger than `max(received_at)`. The maxima crossing therefore
    /// says a stamp came out of the future — a host clock running fast, or
    /// a journal carrying a bad timestamp.
    pub fn ahead(&self) -> Option<Duration> {
        self.lag().filter(|lag| *lag < Duration::zero()).map(|lag| -lag)
    }

    /// Rows here, but not one of them arrived after the column existed.
    pub fn predates_arrivals(&self) -> bool {
        self.changed.is_some() && self.arrived.is_none()
    }

    /// No rows here at all, so neither clock has anything to say.
    pub fn is_empty(&self) -> bool {
        self.changed.is_none()
    }
}

/// How current the database is and roughly how much of it there is.
///
/// Both clocks of every table that carries them, and the server's own time
/// to measure them against, because a single timestamp on a line answers
/// nothing. `updated_at` is the galaxy's clock — when the reading happened
/// out there — and `received_at` is this database's, when the row was
/// written in here. The second following the first by the delivery latency
/// is the ordinary case, and reporting the two apart made it read as a
/// contradiction; they are reported as a pair and a [`Clocks::lag`] now,
/// so that the orderings that *are* wrong are the ones that stand out.
///
/// Three of those, and [`fmt::Display`] names each one:
///
/// - Nothing arriving. [`Self::quiet`] against [`Self::now`] is the number
///   that says the feed is down or nothing is ingesting, and it is the
///   only one of the three an operator watches for.
/// - A reading dated in the future, [`Clocks::ahead`].
/// - Rows older than the arrival column, [`Clocks::predates_arrivals`].
pub struct Status {
    /// The newest `_sqlx_migrations` row, from [`migrate::applied`]. [`None`]
    /// is an unmigrated database, not a failure to ask.
    pub version: Option<i64>,
    pub described: Option<String>,

    /// The server's clock, read in the same statement as [`Self::clocks`].
    ///
    /// The server's and not this process's, for the reason
    /// [`Database::now`] gives at length: a caller comparing a row's stamp
    /// against its own clock is trusting two clocks to agree. Read in the
    /// same round trip rather than through that method so that "how long
    /// since anything arrived" subtracts two numbers taken at one instant
    /// off one clock. [`None`] on an unmigrated database, which is not
    /// asked anything.
    pub now: Option<DateTime<Utc>>,

    /// Both clocks per table, for the three that carry a `received_at`.
    pub clocks: Vec<Clocks>,

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
            now: None,
            clocks: Vec::new(),
            estimates: Vec::new(),
            factions: 0,
        }
    }

    /// How long it has been since anything arrived anywhere.
    ///
    /// The freshness that matters: a galaxy clock says when the newest
    /// reading this database holds was taken, which a database nobody has
    /// fed for a week still answers perfectly well. This one says whether
    /// anything is being written, and nothing else here does.
    ///
    /// [`None`] where no table has an arrival at all, which is a database
    /// that has not been written to since `received_at` was added rather
    /// than a database that was written to infinitely long ago.
    pub fn quiet(&self) -> Option<Duration> {
        let newest = self.clocks.iter().filter_map(|it| it.arrived).max()?;
        Some(self.now? - newest)
    }
}

/// Ask the database what it is.
///
/// Two round trips: one for the clocks, the server's own time and the
/// exact faction count, one for the row estimates. The estimates come from
/// `pg_class.reltuples`, which is what `ANALYZE` last wrote, falling back
/// to `pg_stat_user_tables.n_live_tup` for a table Postgres records as
/// never analysed (`reltuples = -1`) — the live-tuple counter is the only
/// figure a freshly restored table has. Both are estimates and are
/// labelled as such in the report: a `count(*)` over `systems` is a
/// two-gigabyte sequential scan today and a hundred-odd gigabyte one at
/// the galaxy target, which is not a thing a status verb may do.
///
/// # What the six clocks cost
///
/// Four of them come off an index. The other two do not: only `systems`
/// has an index on `updated_at`, so `max(updated_at)` on `stars` and on
/// `system_factions` are parallel sequential scans — **53 ms** over 1.1 M
/// stars and **10 ms** over 429 k faction rows, warm, on the 3.4 M-system
/// development server. That is paid rather than dropped because the
/// cheaper report — every arrival held against `systems.updated_at` — is
/// wrong: `stars` going one second longer than the feed's latency without
/// a write puts its arrival behind the galaxy clock, and a skew reported
/// several times an hour on a healthy database is a skew nobody reads.
/// The two scans grow with their tables and are the only part of `status`
/// that does.
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
        Option<NaiveDateTime>,
        Option<NaiveDateTime>,
        i64,
        NaiveDateTime,
    ) = sqlx::query_as(
        "SELECT (SELECT max(updated_at) FROM systems), \
                (SELECT max(received_at) FROM systems), \
                (SELECT max(updated_at) FROM stars), \
                (SELECT max(received_at) FROM stars), \
                (SELECT max(updated_at) FROM system_factions), \
                (SELECT max(received_at) FROM system_factions), \
                (SELECT count(*)::bigint FROM factions), \
                now() AT TIME ZONE 'utc'",
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
        now: Some(clocks.7.and_utc()),
        clocks: vec![
            Clocks {
                table: "systems",
                changed: clocks.0.map(|it| it.and_utc()),
                arrived: clocks.1.map(|it| it.and_utc()),
            },
            Clocks {
                table: "stars",
                changed: clocks.2.map(|it| it.and_utc()),
                arrived: clocks.3.map(|it| it.and_utc()),
            },
            Clocks {
                table: "system_factions",
                changed: clocks.4.map(|it| it.and_utc()),
                arrived: clocks.5.map(|it| it.and_utc()),
            },
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
        factions: clocks.6,
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
                     yet. `galos db migrate` puts one in."
                )
            }
        }

        // The zone is said once in the heading and left off every stamp
        // below it: three stamps to a line, and " UTC" on each of them is
        // twelve columns spent saying the same thing three times.
        writeln!(f, "\nclocks (UTC):")?;
        writeln!(
            f,
            "  updated_at is the galaxy's own clock — when the reading\n  \
             happened out there. received_at is this database's — when the\n  \
             row was written in here. A write comes after the event it\n  \
             records, so an arrival behind its own reading by the delivery\n  \
             latency is the ordinary case and not a contradiction.\n"
        )?;
        writeln!(
            f,
            "  {:<17} {:<20} {:<20} lag",
            "table", "updated_at", "received_at"
        )?;
        for clocks in &self.clocks {
            writeln!(
                f,
                "  {:<17} {:<20} {:<20} {}",
                clocks.table,
                when(clocks.changed),
                when(clocks.arrived),
                behind(clocks.lag())
            )?;
        }

        match self.quiet() {
            Some(quiet) => writeln!(
                f,
                "\n  nothing has arrived in {}; the server's clock reads {}",
                span(quiet),
                when(self.now)
            )?,
            None => writeln!(
                f,
                "\n  nothing has arrived since `received_at` was added; a watch \
                 following that clock has no cursor to move"
            )?,
        }

        // Each of these is a line only when it has something to say, so
        // an ordinary database prints the block above and nothing else.
        for clocks in &self.clocks {
            if clocks.is_empty() {
                writeln!(
                    f,
                    "  {} holds no rows at all, so neither clock has \
                     anything to say for it",
                    clocks.table
                )?;
            } else if clocks.predates_arrivals() {
                writeln!(
                    f,
                    "  {} holds rows but not one of them carries a \
                     received_at: they were written before migration \
                     20260816000050 added the column, and the partial index \
                     it created — the one a watch reads — cannot see them",
                    clocks.table
                )?;
            }
            if let Some(ahead) = clocks.ahead() {
                writeln!(
                    f,
                    "  the newest reading in {} is dated {} after the moment \
                     this database wrote it down: a reading out of the \
                     future, which is a host clock running fast or a journal \
                     carrying a bad timestamp, and not a feed that is early",
                    clocks.table,
                    span(ahead)
                )?;
            }
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
///   it. Deriving the index from the rows filters `position IS NOT NULL`,
///   so these are simply not served; they are not wrong.
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
///
/// Without the zone, which the block it prints into says once in its
/// heading. Whole seconds: `received_at` carries microseconds from
/// `clock_timestamp()` and `updated_at` does not, and six digits of
/// precision on one of two columns being compared invites a reader to
/// compare them at a precision only one of them has.
fn when(at: Option<DateTime<Utc>>) -> String {
    match at {
        Some(at) => at.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => "never".to_string(),
    }
}

/// A length of time from its largest non-zero unit and the one under it.
///
/// What the reader of a lag wants is the order of magnitude — whether an
/// arrival is a second behind its reading or four days behind it — and
/// `367583s` does not answer that at a glance. Rounded down, and never
/// signed: which way it points is a word in the sentence around it,
/// because `-4d 3h behind` is not a thing anybody can read.
fn span(length: Duration) -> String {
    let seconds = length.num_seconds().abs();
    let (days, hours) = (seconds / 86_400, seconds / 3_600 % 24);
    let (minutes, rest) = (seconds / 60 % 60, seconds % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {rest}s")
    } else {
        format!("{rest}s")
    }
}

/// Which way a [`Clocks::lag`] points, in the one column it gets.
///
/// `behind` is the ordinary direction and says nothing more than how far;
/// `ahead` is the anomaly, and the line under the table says what it
/// means. A table missing either clock has no lag to report at all.
fn behind(lag: Option<Duration>) -> String {
    match lag {
        Some(lag) if lag < Duration::zero() => format!("{} ahead", span(lag)),
        Some(lag) => format!("{} behind", span(lag)),
        None => "—".to_string(),
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

    /// One of the two pieces of arithmetic here that are not the
    /// database's.
    #[test]
    fn counts_are_grouped_in_threes() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(3_412_771), "3,412,771");
        assert_eq!(grouped(-1_234), "-1,234");
    }

    /// The other, and the one with edges: every unit boundary is a place
    /// the reported magnitude can jump a whole unit the wrong way.
    #[test]
    fn a_span_carries_its_largest_unit_and_the_one_under_it() {
        assert_eq!(span(Duration::seconds(0)), "0s");
        assert_eq!(span(Duration::seconds(59)), "59s");
        assert_eq!(span(Duration::seconds(60)), "1m 0s");
        assert_eq!(span(Duration::seconds(3_599)), "59m 59s");
        assert_eq!(span(Duration::seconds(3_600)), "1h 0m");
        assert_eq!(span(Duration::seconds(86_399)), "23h 59m");
        assert_eq!(span(Duration::seconds(86_400)), "1d 0h");
        assert_eq!(span(Duration::seconds(356_400)), "4d 3h");
    }

    /// Which way round the clocks read, which is the whole point of
    /// reporting them as a pair: the ordinary case is an arrival behind
    /// the reading it carries, and only the other direction is a fault.
    #[test]
    fn an_arrival_after_its_reading_is_not_the_anomaly() {
        let at = |secs| Some(DateTime::from_timestamp(secs, 0).unwrap());
        let delivered =
            Clocks { table: "systems", changed: at(1_000), arrived: at(1_001) };
        assert_eq!(delivered.lag(), Some(Duration::seconds(1)));
        assert_eq!(delivered.ahead(), None, "a write follows its event");

        let skewed =
            Clocks { table: "systems", changed: at(1_060), arrived: at(1_000) };
        assert_eq!(
            skewed.ahead(),
            Some(Duration::seconds(60)),
            "a reading stamped after its own write came from the future",
        );

        let older_than_the_column =
            Clocks { table: "stars", changed: at(1_000), arrived: None };
        assert!(older_than_the_column.predates_arrivals());
        assert!(!older_than_the_column.is_empty(), "rows are here");
        assert_eq!(older_than_the_column.lag(), None);
    }

    /// What is unfinished is forgiven; what a constraint should have
    /// stopped is not.
    ///
    /// The numbers are the two databases this was measured against: a
    /// development server restored with its triggers disabled, and a
    /// database migrated from nothing and fed fifteen seconds of the live
    /// feed. The second is what the first version of this rule got wrong
    /// — it called that database unsound, and every database that has
    /// read the feed with it.
    #[test]
    fn what_is_merely_unfinished_is_not_unsound() {
        let fed_for_fifteen_seconds = Verified {
            systems_without_position: 10,
            markets_without_system: 7,
            stations_without_type: 4,
            body_signals_without_body: 6,
            stations_without_body: 3,
            bodies_with_dangling_parents: 134,
            stars_with_dangling_parents: 13,
            system_factions_without_faction: 0,
        };
        assert!(
            fed_for_fifteen_seconds.is_sound(),
            "a database reading the feed is not damaged by reading it",
        );

        let restored_without_triggers = Verified {
            system_factions_without_faction: 76_791,
            ..fed_for_fifteen_seconds
        };
        assert!(
            !restored_without_triggers.is_sound(),
            "a faction id naming no faction is a foreign key not enforced",
        );
    }
}
