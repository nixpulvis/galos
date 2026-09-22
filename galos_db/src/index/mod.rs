//! Building the galaxy index from the database.
//!
//! Reads every positioned system and its scanned stars, turns them into
//! photometry through `galos_photometry`'s fallback chain, and hands the
//! result to `galos_index`'s pure builder. The two crates meet at [`System`].
//! The metadata beside the tree is `metadata`.
//!
//! The queries are unchecked `sqlx::query`; the columns are read back by
//! name. A pass reads in chunks of [`CHANGED_CHUNK`] addresses, and the
//! three tables written whole are written once for the pass.

use crate::{Database, Result};
use async_std::stream::StreamExt;
use futures_core::stream::BoxStream;
use galos_index::{
    derive, Abandoned, Build, BuildParams, Built, By, Checkpoint, ColdReport,
    Ending, Index, Pending, Start, System, Taking, Tree,
};
use galos_photometry::{Magnitude, Temperature};
use metadata::{Metadata, Moved};
use sqlx::Row;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

mod metadata;
pub use metadata::MetaReport;

/// Which of the index's parts a build writes
///
/// A full build writes all of them; one part alone is what a change to how
/// that part is derived wants. Each names a file or a set of them, and
/// asking for one reads only what it needs. A part left out is left as it
/// stands; nothing here removes a file.
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

    /// Whether a read of every scanned thing is wanted
    ///
    /// One read serves all three: the body files are written from those rows,
    /// the reaches measured over them, the arrival star picked out of them.
    fn wants_bodies(&self) -> bool {
        self.reaches || self.bodies || self.boosts
    }
}

/// How far back a pass looks past its own cursor.
///
/// `received_at` is stamped inside the transaction that writes a row while
/// the cursor is read outside one, so a report can be behind a cursor taken
/// before it committed. Re-reading a system is free, every patch being
/// rebuilt from the current row, and the overlap does not compound.
const CURSOR_OVERLAP: Duration = Duration::from_secs(2);

/// How many changed addresses one read of a pass covers.
///
/// The changed set is unbounded and every read binds it whole to `= ANY($1)`.
/// Ten thousand is what one chunk holds: a scanned system is tens of body
/// rows, so a chunk is a hundred thousand rows at its worst.
const CHANGED_CHUNK: usize = 10_000;

/// One `stars` row as the light the index reads off it: the system it is in
/// and its visual `(absolute magnitude, temperature)`.
///
/// The scanned magnitude is bolometric, so [`Magnitude::visual`] turns it
/// into what the sky sees. A star missing either figure is [`None`] and its
/// system falls to the class fallback. Shared with [`build_cells`].
fn star_light(row: &sqlx::postgres::PgRow) -> Result<Option<(i64, f64, f64)>> {
    let address: i64 = row.try_get("system_address")?;
    let magnitude: Option<f32> = row.try_get("absolute_magnitude")?;
    let temperature: Option<f32> = row.try_get("temperature")?;
    let (Some(m), Some(t)) = (magnitude, temperature) else {
        return Ok(None);
    };
    let t = t as f64;
    Ok(Some((address, Magnitude(m as f64).visual(Temperature(t)).0, t)))
}

/// Every scanned star of `addresses`, grouped under its system.
///
/// The paged path's read, bounded by [`CHANGED_CHUNK`]. A cold build goes
/// through [`build_cells`], which merges two ordered reads instead.
async fn stars_by_system(
    db: &Database,
    addresses: &[i64],
) -> Result<HashMap<i64, Vec<(f64, f64)>>> {
    let rows = sqlx::query(
        "SELECT system_address, absolute_magnitude, temperature \
         FROM stars WHERE system_address = ANY($1)",
    )
    .bind(addresses)
    .fetch_all(&db.pool)
    .await?;
    let mut stars: HashMap<i64, Vec<(f64, f64)>> = HashMap::new();
    for row in rows {
        if let Some((address, magnitude, temperature)) = star_light(&row)? {
            stars.entry(address).or_default().push((magnitude, temperature));
        }
    }
    Ok(stars)
}

/// One `systems` row turned into build input through the photometry fallback.
///
/// The row carries `address`, the three `ST_?` coordinates,
/// `primary_star_class` and `updated_at`; `now` dates the Recency reading
/// and `scanned` is this system's stars.
///
/// Both derived facts are [`galos_index::derive`]'s, so a system built from
/// a row and one built from a journal entry land in the same place.
fn input_from_row(
    row: &sqlx::postgres::PgRow,
    scanned: &[(f64, f64)],
    now: chrono::NaiveDateTime,
) -> Result<System> {
    let address: i64 = row.try_get("address")?;
    let x: f64 = row.try_get("x")?;
    let y: f64 = row.try_get("y")?;
    let z: f64 = row.try_get("z")?;
    let class: Option<String> = row.try_get("primary_star_class")?;
    let at: chrono::NaiveDateTime = row.try_get("updated_at")?;
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
        // The arrival star's own class, off the same column the photometry
        // fallback reads. A row with none reads as nothing having been
        // said, which is what most of the galaxy is: see
        // [`galos_index::StarKind`].
        kind: class
            .as_deref()
            .map_or(galos_index::StarKind::Unknown, galos_index::StarKind::of),
    })
}

/// The addresses of systems reported since `since`: those whose own row
/// arrived, those whose stars did, since a scan re-magnitudes a system
/// without touching its row, and those whose factions did.
///
/// A body scan is followed through the system row the sync writes beside it:
/// `bodies` has no index on when a report arrived.
///
/// The column read is `received_at`, not `updated_at`: `updated_at` is the
/// time the event describes out in the galaxy, where `received_at` is
/// stamped by the upsert, so this compares the database's clock against
/// itself. Rows older than that column hold `NULL` and are left out.
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

/// Build the cell tree and the names table of every positioned system into
/// `dir`, and leave the resume point beside them.
///
/// One read, and a row pushed into [`Build`] as it comes: the photometry
/// and the name out of two cursors ordered by address and merged rather
/// than indexed. One read for the cell tree and the names table both,
/// because the two must agree — they stand for the same set of systems, and
/// a feed writing a system between two reads would leave them a few apart.
///
/// Nothing the galaxy's size scales is held: a row at a time, one star read
/// past the system it belongs to, and one region's systems while that
/// region is built.
///
/// `now` is both what dates the Recency reading, so every system in the
/// build is aged against the same moment, and what the resume point is
/// current as of. It is read before this reads anything.
///
/// `stop` is asked per row, which is where the read's time goes, and again
/// inside [`Build::finish`]. A build that was stopped published nothing:
/// see [`Built`].
///
/// Always [`Start::Fresh`] and [`Ending::Abandon`]: the directory this
/// publishes stands for every row Postgres has, so a read cut short must
/// not replace it with the prefix it reached — and a read taken up again
/// is a re-read of the cursors, which is minutes over a database where it
/// is hours over a 610 GB dump.
async fn build_cells(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    params: BuildParams,
    budget: u64,
    now: chrono::NaiveDateTime,
    stop: &Stop<'_>,
    told: &Told<'_>,
) -> Result<Built> {
    // Before the read rather than during it: what a bar is drawn against
    // has to be there when the first row arrives, and this is one index
    // scan of `pg_class`.
    let of = estimated(db, "systems").await;
    let asked = || stop();
    let mut build =
        Build::begin(dir, checkpoint, params, budget, Start::Fresh, &asked)?;

    let mut stars = sqlx::query(
        "SELECT system_address, absolute_magnitude, temperature \
         FROM stars ORDER BY system_address",
    )
    .fetch(&db.pool);
    // The one star read past the system it belongs to, which is what
    // merging two cursors costs in memory.
    let mut ahead = next_star(&mut stars).await?;

    let mut rows = sqlx::query(
        "SELECT address, name, \
                ST_X(position) AS x, ST_Y(position) AS y, \
                ST_Z(position) AS z, \
                primary_star_class, updated_at \
         FROM systems WHERE position IS NOT NULL ORDER BY address",
    )
    .fetch(&db.pool);

    let mut scanned: Vec<(f64, f64)> = Vec::new();
    let mut read = 0u64;
    while let Some(row) = rows.next().await {
        let row = row?;
        let address: i64 = row.try_get("address")?;
        read += 1;
        if read % TOLD_EVERY == 0 {
            told(Progress { step: step::SYSTEMS, done: read, of });
        }
        scanned.clear();
        while let Some((at, magnitude, temperature)) = ahead {
            // A star of a system this read will never reach: one whose
            // system has no position, or none at all.
            if at > address {
                break;
            }
            if at == address {
                scanned.push((magnitude, temperature));
            }
            ahead = next_star(&mut stars).await?;
        }
        if build.push(
            input_from_row(&row, &scanned, now)?,
            metadata::name_from_row(&row)?,
        )? == Taking::Stopped
        {
            break;
        }
    }
    told(Progress { step: step::SYSTEMS, done: read, of: Some(read) });
    Ok(build.finish(By::Database, Some(now), Ending::Abandon)?)
}

/// The next star of the ordered read, as [`star_light`] reads one, skipping
/// those with nothing to light a system by.
async fn next_star(
    stars: &mut BoxStream<'_, sqlx::Result<sqlx::postgres::PgRow>>,
) -> Result<Option<(i64, f64, f64)>> {
    while let Some(row) = stars.next().await {
        if let Some(star) = star_light(&row?)? {
            return Ok(Some(star));
        }
    }
    Ok(None)
}

/// Say what [`galos_index::migrate`] moved in `dir`, on this side's log.
///
/// The migration itself is shared with the sink's own open, which says the
/// same lines; what is here is the saying of them.
fn migrate(dir: &Path, stop: &Stop<'_>) -> Result<()> {
    let asked = || stop();
    let done = galos_index::migrate(dir, &asked)?;
    // Nothing was moved and nothing can be until the payloads are brought
    // forward, which is not something an open does: said at `warn` rather
    // than `info` because every read after this one fails, and the message
    // is the only place the remedy appears before it does.
    if let Some(found) = done.upgrade {
        tracing::warn!(
            found,
            reads = galos_index::INDEX_VERSION,
            dir = %dir.display(),
            "the directory's payloads are of another layout; run \
             `galos index migrate` over it",
        );
        return Ok(());
    }
    if done.bodies.moved > 0 {
        info!(
            files = done.bodies.moved,
            dir = %dir.display(),
            "moved the body files into their shards"
        );
    }
    if !done.bodies.finished {
        info!(
            files = done.bodies.moved,
            dir = %dir.display(),
            "asked to stop part way through sharding the body files; the \
             next run continues it"
        );
    }
    if let Some(cells) = done.cells {
        if cells.moved > 0 {
            info!(
                files = cells.moved,
                dir = %dir.display(),
                "moved the cell payloads into their shards"
            );
        }
        if !cells.finished {
            info!(
                files = cells.moved,
                dir = %dir.display(),
                "asked to stop part way through sharding the cell payloads; \
                 the next run continues it"
            );
        }
    }
    if let Some(named) = done.names {
        info!(
            named,
            dir = %dir.display(),
            "folded the names chunks into a mapped base"
        );
    }
    Ok(())
}

/// Build the parts of the index `parts` names and write them to `dir`: the
/// cell tree the map draws from and the records a click reads, in one
/// directory so a single transport serves both.
///
/// [`Parts::ALL`] is a full build; anything narrower reads only what those
/// parts need and leaves every other file as it stands.
///
/// A build that read the whole galaxy writes `checkpoint` beside it and says
/// the database derived it: the served payloads carry a downcast magnitude
/// and a bucketed temperature, so the full-precision inputs are here or
/// nowhere. A narrowed build writes none.
///
/// A cold build is regional: [`build_cells`] pushes every row into [`Build`]
/// under [`galos_index::region_budget`]. Nothing here holds a [`Tree`] — a
/// watch gets one by resuming from the resume point it leaves.
///
/// `stop` reaches every step: [`migrate`], which leaves what it has not
/// moved for a later open, and the build, which is asked per row and per
/// region — see [`Build`].
///
/// **A stopped build publishes nothing.** The invariant the directory is
/// read under is that its index file stands over the payloads beneath it
/// and its resume point over exactly the systems it serves, and a build
/// writes all three or none: up to the first payload the directory is left
/// as it was found, and past it there is no index file until a build
/// finishes. So the parts below are not written either — they would stand
/// for a galaxy this build did not finish reading — and the answer is
/// [`Reached::Stopped`], which is not a failure.
///
/// The metadata tables are the one step here a stop does not reach, and
/// they are written whatever the run has been asked. By the time they are
/// reached the cells, the names table and the resume point stand, and it is
/// that resume point which makes the next run resume rather than build — so
/// a directory left with published cells and tables short of them is one
/// nothing would ever repair. They are a query apiece and a body file a
/// system, so what a stop costs here is the tail of a build rather than the
/// whole of one.
pub async fn build_to_dir(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    parts: Parts,
    stop: &Stop<'_>,
    told: &Told<'_>,
) -> Result<Reached<BuildReport>> {
    migrate(dir, stop)?;
    let since = db.now().await?.naive_utc();
    let params = BuildParams::default();

    // The cells and the names come out of one read of every positioned
    // system, so asking for either reads it; asking for neither skips it.
    let cells = match parts.cells {
        false => None,
        true => {
            let budget = galos_index::region_budget();
            let built = build_cells(
                db, dir, checkpoint, params, budget, since, stop, told,
            )
            .await?;
            let report = match built {
                Built::Index(report) => report,
                Built::Stopped(abandoned) => {
                    return Ok(Reached::Stopped(abandoned));
                }
            };
            info!(
                regions = report.regions,
                budget,
                named = report.named,
                rows = report.named_rows,
                "cut the galaxy into regions and built each alone"
            );
            Some(report)
        }
    };
    // The names table alone, for a repair that wants it and not the tree:
    // one streamed read, and the rows sorted on disk rather than in memory.
    if parts.names && !parts.cells {
        write_names(db, dir, told).await?;
    }

    let meta = metadata::write_parts(db, dir, parts, told).await?;
    Ok(Reached::End(BuildReport { cells, meta }))
}

/// Write the names table and nothing else, streaming.
///
/// For `--only names`; a cold build writes it beside the cells out of the
/// same read. Answers how many systems the published table names. The read
/// is `ORDER BY address`, which is the order the table wants, so the
/// writer's external sort has nothing to do but merge one already ordered
/// run.
async fn write_names(
    db: &Database,
    dir: &Path,
    told: &Told<'_>,
) -> Result<usize> {
    let of = estimated(db, "systems").await;
    let mut names = galos_index::names::Writer::writing(dir)?;
    let query = format!(
        "{} WHERE position IS NOT NULL ORDER BY address",
        metadata::NAMES_SELECT
    );
    let mut rows = sqlx::query(&query).fetch(&db.pool);
    let mut read = 0u64;
    while let Some(row) = rows.next().await {
        names.push(metadata::name_from_row(&row?)?)?;
        read += 1;
        if read % TOLD_EVERY == 0 {
            told(Progress { step: step::NAMES, done: read, of });
        }
    }
    told(Progress { step: step::NAMES, done: read, of: Some(read) });
    Ok(names.finish()?)
}

/// The systems of `addresses` as build input.
///
/// Each is rebuilt whole from its current record through the fallback
/// [`Galaxy`] uses, so an incremental system lands where a full rebuild
/// would put it. An address with no positioned row is not in the result.
async fn inputs_for(db: &Database, addresses: &[i64]) -> Result<Vec<System>> {
    if addresses.is_empty() {
        return Ok(Vec::new());
    }
    let now = db.now().await?.naive_utc();
    let stars = stars_by_system(db, addresses).await?;
    let rows = sqlx::query(
        "SELECT address, \
                ST_X(position) AS x, ST_Y(position) AS y, ST_Z(position) AS z, \
                primary_star_class, updated_at \
         FROM systems WHERE address = ANY($1) AND position IS NOT NULL",
    )
    .bind(addresses)
    .fetch_all(&db.pool)
    .await?;
    rows.iter()
        .map(|row| {
            let address: i64 = row.try_get("address")?;
            let scanned = stars.get(&address).map_or(&[][..], Vec::as_slice);
            input_from_row(row, scanned, now)
        })
        .collect()
}

/// Bring `dir` level with the database and answer the clock it is level at.
///
/// Resumes from `checkpoint`, or builds the whole galaxy afresh where it
/// cannot, then runs delta passes until one has little enough left to hand
/// over.
///
/// The clock is read before each pass reads what changed, never after, so a
/// write racing a pass's read is asked for again by whoever follows the
/// cursor rather than missed by everyone. Each pass reads back a further
/// [`CURSOR_OVERLAP`]; applying a system twice is idempotent.
///
/// A pass ends the catch-up when what it found is smaller than one
/// [`CHANGED_CHUNK`]: the caller is usually the process writing to the
/// database, and the residue is already in that process's handoff buffer.
///
/// It leaves a whole resume point standing for exactly the systems the
/// directory serves; one short of what is served cannot be opened at all.
///
/// `parts` narrower than [`Parts::ALL`] is the repair case: a one-shot
/// [`build_to_dir`] of those parts, and the cursor answered is the clock
/// read before the build read anything. It is a *repair*, so it is refused
/// where there is nothing to repair — a directory serving nothing, written
/// one part at a time, is a galaxy of reaches with no tree over them, and
/// the run that wrote it read the whole database to say "the index is
/// level" over a directory `galos index status` cannot open.
///
/// A run asked to stop before the directory was level at all — which is a
/// cold build cut short, every other step leaving a directory that can be
/// followed — answers [`Reached::Stopped`] and no cursor, there being no
/// clock anything here is current as of.
pub async fn catch_up(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    parts: Parts,
    rebuild: bool,
    stop: &Stop<'_>,
    told: &Told<'_>,
) -> Result<Reached<chrono::NaiveDateTime>> {
    if parts != Parts::ALL {
        // Before the clock and before any read: what this costs otherwise
        // is the whole galaxy, and what it leaves cannot be opened.
        if serving(dir).is_none() {
            return Err(std::io::Error::other(format!(
                "{}: nothing is published here, and one part alone is a \
                 repair of a directory that is already built. Build it \
                 first, which writes every part:\n  galos ingest --from \
                 database -i {}",
                dir.display(),
                dir.display(),
            ))
            .into());
        }
        let since = db.now().await?.naive_utc();
        return Ok(build_to_dir(db, dir, checkpoint, parts, stop, told)
            .await?
            .map(|report| {
                info!(dir = %dir.display(), %report, "index parts derived");
                since
            }));
    }
    Ok(bring_level(db, dir, checkpoint, rebuild, stop, told)
        .await?
        .map(|it| it.cursor))
}

/// Whether whoever asked for this has stopped wanting it.
///
/// Asked per record while a cold build reads the galaxy, and otherwise
/// between chunks, between regions and between passes; a single query is
/// not interrupted, so the longest this takes to be obeyed is the longest
/// read a pass makes.
pub type Stop<'a> = dyn Fn() -> bool + Send + Sync + 'a;

/// What a step a stop can cut short came to: its answer, or how far the
/// build got before it was asked to stop.
///
/// A stop rides out as a value rather than an error. It is what the run
/// asked for, and a caller that read it as a failure would turn a Ctrl-C
/// into a failed run — see [`Abandoned`] for what such a run left behind.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Reached<T> {
    /// It ran to its end.
    End(T),
    /// It was asked to stop before there was one.
    Stopped(Abandoned),
}

impl<T> Reached<T> {
    /// The same answer, over whatever the caller wanted of it.
    pub fn map<U>(self, of: impl FnOnce(T) -> U) -> Reached<U> {
        match self {
            Reached::End(it) => Reached::End(of(it)),
            Reached::Stopped(abandoned) => Reached::Stopped(abandoned),
        }
    }

    /// What it came to, or [`None`] where it was stopped.
    pub fn end(self) -> Option<T> {
        match self {
            Reached::End(it) => Some(it),
            Reached::Stopped(_) => None,
        }
    }
}

/// Nothing ever asks this to stop, which is what a one-shot caller wants.
pub fn never() -> &'static Stop<'static> {
    &|| false
}

/// How far a step that takes minutes has got.
///
/// The steps here are reads of the whole galaxy: a cold build is every
/// positioned system and then everything ever scanned in one of them, and
/// on a database the size of the one this is written for that is an hour
/// in which nothing happens that a terminal can see. A run nobody can tell
/// from a hung one is the complaint; this is what answers it.
///
/// Told rather than drawn, for the same reason [`Stop`] is asked rather
/// than installed: what a bar looks like, and whether there is a terminal
/// to draw one on at all, is the binary's business. See `galos::bar`.
#[derive(Copy, Clone, Debug)]
pub struct Progress<'a> {
    /// What is being read or written, in the words an operator reads.
    pub step: &'a str,
    /// How much of it is behind this: systems, addresses or files.
    pub done: u64,
    /// How much there is altogether, where something cheap knows it.
    ///
    /// The planner's row estimate for a read of a whole table, the length
    /// of the changed set for a pass, and [`None`] for a merge of four
    /// cursors, whose count no query answers without reading it first.
    pub of: Option<u64>,
}

/// Where a long step says how far it has got.
pub type Told<'a> = dyn Fn(Progress<'_>) + Send + Sync + 'a;

/// Nobody is watching, which is what a test and a one-shot caller want.
pub fn untold() -> &'static Told<'static> {
    &|_| {}
}

/// How many records a read gets through between what it says.
///
/// Ten thousand is a few hundred lines over a galaxy of systems and a
/// tenth of a second at the rate a cold read holds, which is more often
/// than a terminal is redrawn and far less often than a row arrives.
const TOLD_EVERY: u64 = 10_000;

/// The steps a build and a pass are made of, as an operator reads them.
///
/// Named here rather than written at each call because the binary groups
/// what it draws by the step it is told, and two spellings of one step are
/// two bars for one read.
pub mod step {
    /// Every positioned system, which the cell tree and the names table
    /// are both built from.
    pub const SYSTEMS: &str = "reading every system";
    /// The same read, where only the names table is wanted.
    pub const NAMES: &str = "reading every system's name";
    /// The populated table, which is one query and one write.
    pub const POPULATED: &str = "reading the populated systems";
    /// Everything ever scanned: the stars, bodies and barycenters of every
    /// system with any of them, merged in address order.
    pub const SCANNED: &str = "reading everything scanned";
    /// The faction names, which is one query and one write.
    pub const FACTIONS: &str = "reading the faction names";
    /// The systems a pass found changed, applied a chunk at a time.
    pub const CHANGED: &str = "applying what changed";
}

/// The planner's estimate of how many rows `table` holds.
///
/// For a bar to draw against. `count(*)` over `systems` is a read of the
/// table this is about to read anyway, so the estimate `galos db status`
/// reports is what a total comes from here too: it is wrong by whatever
/// has arrived since the last `ANALYZE`, which moves a bar's last percent
/// and nothing else.
async fn estimated(db: &Database, table: &str) -> Option<u64> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT CASE WHEN c.reltuples < 0 \
                     THEN COALESCE(s.n_live_tup, 0) \
                     ELSE c.reltuples::bigint END \
           FROM pg_class c \
           JOIN pg_namespace n ON n.oid = c.relnamespace \
           LEFT JOIN pg_stat_user_tables s ON s.relid = c.oid \
          WHERE n.nspname = 'public' AND c.relkind = 'r' \
            AND c.relname = $1",
    )
    .bind(table)
    .fetch_optional(&db.pool)
    .await
    .ok()
    .flatten();
    row.map(|(rows,)| rows.max(0) as u64).filter(|rows| *rows > 0)
}

/// Bring `dir` level with the database, then keep it there as the feed
/// writes, publishing what each round of changes touched.
///
/// [`catch_up`] and then a poll every `interval`, which reads the systems
/// changed since the previous pass, moves each in the live [`Tree`] and
/// writes only the cells that changed. The metadata is patched the same pass
/// and only what moved is written.
///
/// The resume point rides outside `dir` and is never served. Every pass
/// records what it published, on a log that costs what moved; the base
/// behind it is rewritten only when that log has grown against it.
pub async fn watch(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    interval: Duration,
    rebuild: bool,
    stop: &Stop<'_>,
    told: &Told<'_>,
) -> Result<()> {
    let Reached::End(mut level) =
        bring_level(db, dir, checkpoint, rebuild, stop, told).await?
    else {
        info!(
            dir = %dir.display(),
            "asked to stop before there was an index to watch"
        );
        return Ok(());
    };
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
        pass(db, dir, checkpoint, &mut level, told).await?;
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

/// A directory level with the database: the tree and the tables as they
/// stand in the directory, and the clock the last pass covered.
///
/// Held together because a pass moves all of it at once; the tables are
/// derived once at the start and then only ever patched.
struct Level {
    tree: Tree,
    meta: Metadata,
    cursor: chrono::NaiveDateTime,
}

/// Resume `dir` or build it whole, then run delta passes until one finds
/// little enough left to hand over. The one copy of the build-or-resume that
/// both [`catch_up`] and [`watch`] start from.
///
/// `stop` is obeyed at every step: the layout migration leaves whatever it
/// has not moved for a later open, the build is asked per row and per
/// region, and the pass loop ends between passes. The resume between the
/// migration and the build is not interruptible, being one file read into
/// one tree.
///
/// A build is not started by a run that is already stopping. The migration
/// ahead of it may have been abandoned part way, and a cold build is the
/// whole galaxy: starting one to throw it away at its first check would be
/// the read set going before the stop was obeyed.
async fn bring_level(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    rebuild: bool,
    stop: &Stop<'_>,
    told: &Told<'_>,
) -> Result<Reached<Level>> {
    // Before the migration, which is the first thing here that touches the
    // directory. A refusal that arrives after a layout migration has moved
    // a galaxy's files is one that came too late — see [`refusal`].
    if !rebuild {
        if let Some(said) = refusal(dir, checkpoint) {
            return Err(std::io::Error::other(said).into());
        }
    }
    migrate(dir, stop)?;
    let params = BuildParams::default();
    let mut level = match resume(dir, checkpoint, &params) {
        Resume::Level(tree, meta, cursor) => {
            info!(
                systems = tree.len(),
                cursor = %cursor,
                checkpoint = %checkpoint.display(),
                "resumed from checkpoint"
            );
            Level { tree, meta, cursor }
        }
        // Everything a resume found wrong with a directory that is serving
        // systems. `--rebuild` is the operator saying it anyway, and then
        // it is said on the way past rather than swallowed.
        Resume::Refuse(said) if !rebuild => {
            return Err(std::io::Error::other(said).into());
        }
        other => {
            if let Resume::Refuse(said) = &other {
                warn!(dir = %dir.display(), "{said} Rebuilding as asked.");
            }
            // A build is not started by a run that is already stopping:
            // the migration ahead of it may have been abandoned part way,
            // and a cold build is the whole galaxy.
            if stop() {
                return Ok(Reached::Stopped(Abandoned::unstarted()));
            }
            match build_level(db, dir, checkpoint, stop, told).await? {
                Reached::End(level) => level,
                Reached::Stopped(abandoned) => {
                    return Ok(Reached::Stopped(abandoned));
                }
            }
        }
    };
    while !stop()
        && pass(db, dir, checkpoint, &mut level, told).await? >= CHANGED_CHUNK
    {
    }
    Ok(Reached::End(level))
}

/// Build the whole directory and open the tree a watch needs on what it
/// wrote.
///
/// Split out of [`bring_level`] because two arms reach it now: a directory
/// serving nothing, and one an operator asked to replace.
async fn build_level(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    stop: &Stop<'_>,
    told: &Told<'_>,
) -> Result<Reached<Level>> {
    info!(dir = %dir.display(), "building initial index (reading every system)");
    let start = Instant::now();
    // The cold build, which holds no tree and writes the whole
    // directory and the resume point. A watch needs a tree, so it
    // gets one the way a restart would: by resuming from what the
    // build just left.
    let report = match build_to_dir(db, dir, checkpoint, Parts::ALL, stop, told)
        .await?
    {
        Reached::End(report) => report,
        Reached::Stopped(abandoned) => {
            return Ok(Reached::Stopped(abandoned));
        }
    };
    info!(%report, elapsed = ?start.elapsed(), "initial index built");
    match resume(dir, checkpoint, &BuildParams::default()) {
        Resume::Level(tree, meta, cursor) => {
            Ok(Reached::End(Level { tree, meta, cursor }))
        }
        // An `Io` error because that is what it is: a directory on
        // disk that does not read as what was just written to it.
        _ => Err(std::io::Error::other(format!(
            "{}: the index this build just wrote will not resume, so \
             nothing can follow it",
            dir.display(),
        ))
        .into()),
    }
}

/// Fold everything a tree holds into a new base, current as of `cursor`.
///
/// The whole write, and so the expensive one: a pass appends what it moved
/// and asks for this only when the log has grown against the base — see
/// [`Pending::append`] — or when there is no base at all.
///
/// The base is written before the log is dropped, so a kill between the two
/// replays systems the base already holds rather than losing them.
fn record(
    checkpoint: &Path,
    cursor: chrono::NaiveDateTime,
    systems: impl IntoIterator<Item = System>,
) -> Result<()> {
    Checkpoint::compact(checkpoint, Some(cursor), By::Database, systems)?;
    Ok(())
}

/// One pass: read what has changed since the cursor, apply it a chunk at a
/// time, publish what that moved, and move the cursor on to the clock the
/// read covered. Answers how many addresses it found.
///
/// The clock comes first, before the read, so a write that commits while the
/// pass runs is asked for by the next pass rather than passed over by both.
///
/// The cells and the three tables written whole are written once, after the
/// last chunk; the body files and the names table's appends go as the
/// chunks do.
///
/// The resume point is written after the publish and never before it, which
/// is why the pass holds what it applied rather than logging each chunk: a
/// log frame stands for systems the directory already serves, and one ahead
/// of the publish leaves a killed run resuming a tree larger than the
/// directory beneath it, which [`resume`] must rebuild from.
async fn pass(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    level: &mut Level,
    told: &Told<'_>,
) -> Result<usize> {
    let now = db.now().await?.naive_utc();
    let touched = changed_addresses(db, level.cursor - CURSOR_OVERLAP).await?;
    if touched.is_empty() {
        debug!(since = %level.cursor, "polled, no changes");
        level.cursor = now;
        return Ok(0);
    }

    let start = Instant::now();
    let mut moved = Moved::default();
    // The tables written whole are or-ed across a pass's chunks; the body
    // files are one per system, so that one is summed.
    let mut body_files = 0;
    let mut applied: Vec<System> = Vec::new();
    // A catch-up's first passes are the week the directory was behind by,
    // which is chunks of ten thousand and minutes of them. Addresses asked
    // about rather than systems found: an address with no positioned row
    // still costs the chunk it was read in.
    let of = Some(touched.len() as u64);
    let mut asked = 0u64;
    for chunk in touched.chunks(CHANGED_CHUNK) {
        let inputs = inputs_for(db, chunk).await?;
        level.tree.apply(&inputs);
        let (chunk_moved, wrote) = level.meta.patch(db, dir, chunk).await?;
        moved.absorb(chunk_moved);
        body_files += wrote;
        applied.extend(inputs);
        asked += chunk.len() as u64;
        told(Progress { step: step::CHANGED, done: asked, of });
    }
    level.tree.publish(dir)?;
    let report = level.meta.publish_pass(db, dir, moved, body_files).await?;

    // What the directory now serves and the cursor it is current as of, in
    // one frame. The answer is whether the log has grown far enough against
    // the base to be worth folding in.
    let folding = Pending::append(checkpoint, Some(now), &applied)?;
    if folding {
        record(checkpoint, now, level.tree.inputs())?;
    }
    info!(
        changed = applied.len(),
        pages = touched.len().div_ceil(CHANGED_CHUNK),
        systems = level.tree.len(),
        rows = report.name_rows,
        bodies = report.body_files,
        compacted = folding,
        elapsed = ?start.elapsed(),
        "index updated"
    );
    level.cursor = now;
    Ok(touched.len())
}

/// What a resume point and the directory beside it allow.
///
/// Three answers rather than two. `None` used to mean both "there is
/// nothing here, build it" and "what is here cannot be resumed" — and
/// [`bring_level`] escalated either to a rebuild of the whole galaxy, over
/// whatever the directory was already serving. The cases are not the same
/// and only one of them is safe to take unasked.
enum Resume {
    /// Read back: the tree, the tables and the clock to follow from.
    Level(Tree, Metadata, chrono::NaiveDateTime),
    /// Nothing is served, so building the whole galaxy replaces nothing.
    Build,
    /// Systems are served and this run would replace them. Refused, with
    /// what to run if that is the intent.
    Refuse(String),
}

/// How many systems the directory publishes, which is what a rebuild would
/// replace.
///
/// An index file that is there but will not read — a foreign format
/// version — is not nothing: it is a published directory this build cannot
/// count, and replacing it unasked is the thing being prevented. So it
/// answers "some", loudly, rather than zero.
fn serving(dir: &Path) -> Option<u64> {
    if !dir.join(galos_index::store::INDEX_FILE).exists() {
        return None;
    }
    match Index::read(dir) {
        Ok(index) => index.root().map(|root| root.aggregate.count()),
        Err(_) => Some(u64::MAX),
    }
}

/// Say what a directory serves, for a refusal to quote.
fn serves(count: u64) -> String {
    match count {
        u64::MAX => "systems this build cannot count (its index file is of \
                     another format version)"
            .to_string(),
        n => format!("{n} systems"),
    }
}

/// Whether this run may write to `dir` at all, judged before anything has.
///
/// The cheap half of [`resume`]: what the directory serves and what the
/// resume point says wrote it, with no tree built and no table read. It is
/// separate so that it can be asked **before [`migrate`]**, which is the
/// first thing in [`bring_level`] to touch the directory — a refusal that
/// arrives after a layout migration has moved half a galaxy's files is a
/// refusal that came too late.
///
/// [`None`] is "carry on". [`Some`] is the whole refusal, in the words the
/// operator needs.
fn refusal(dir: &Path, path: &Path) -> Option<String> {
    let served = serving(dir)?;
    if served == 0 {
        return None;
    }
    let rebuild = format!(
        "\n  galos index status -i {dir}\n  \
         galos ingest --from database -i {dir} --rebuild",
        dir = dir.display(),
    );
    let checkpoint = match Checkpoint::read(path) {
        Ok(it) => it,
        Err(err) => {
            return Some(format!(
                "{} serves {} and its resume point {} will not read \
                 ({err}), so there is no way to edit what it holds and \
                 this run would replace all of it.{rebuild}",
                dir.display(),
                serves(served),
                path.display(),
            ));
        }
    };
    if checkpoint.by != By::Database {
        return Some(format!(
            "{} serves {}, derived from {:?}, and this run would replace \
             them with what the database holds. Nothing has been written.\
             {rebuild}",
            dir.display(),
            serves(served),
            checkpoint.by,
        ));
    }
    if checkpoint.cursor.is_none() {
        return Some(format!(
            "{} serves {} and its resume point carries no cursor to read \
             the changes since, so this run would replace all of it.\
             {rebuild}",
            dir.display(),
            serves(served),
        ));
    }
    None
}

/// Rebuild the live tree and the metadata tables from a resume point and the
/// directory they were published to, if the two still read as each other's.
///
/// The tree is the base built in one batch and the log applied over it,
/// which is the same two moves a pass makes.
///
/// Three gates, and which answer a failed one gives turns on whether the
/// directory is serving anything. The resume point must be the database's
/// own: an event-derived directory is internally consistent about the
/// little it holds, so it would pass both counting gates below. The
/// directory must be internally consistent, its cell tree and its names
/// table standing for exactly the same systems. And it must be at or ahead
/// of the resume point, a delta publish repairing only what the next
/// changes touch.
///
/// A directory serving nothing that fails any of them is [`Resume::Build`]:
/// there is nothing to lose. One that is serving systems is
/// [`Resume::Refuse`], which is the whole of the fix — the escalation from
/// "cannot resume" to "replace the galaxy" was silent, and is now a
/// sentence with `--rebuild` in it.
fn resume(dir: &Path, path: &Path, params: &BuildParams) -> Resume {
    let served = serving(dir).unwrap_or(0);
    let refuse = |why: String| match served {
        0 => Resume::Build,
        _ => Resume::Refuse(why),
    };
    if let Some(said) = refusal(dir, path) {
        return refuse(said);
    }
    let Ok(checkpoint) = Checkpoint::read(path) else {
        return Resume::Build;
    };
    if checkpoint.by != By::Database {
        info!(
            by = ?checkpoint.by,
            checkpoint = %path.display(),
            "checkpoint was written from the event feed and the directory \
             serves nothing; building afresh"
        );
        return Resume::Build;
    }
    let Some(cursor) = checkpoint.cursor else {
        debug!(
            checkpoint = %path.display(),
            "checkpoint carries no cursor to follow from; building afresh"
        );
        return Resume::Build;
    };
    let mut tree = Tree::build(checkpoint.base(), params);
    tree.apply(checkpoint.deltas());
    let (Ok(index), Ok(meta)) = (Index::read(dir), Metadata::resume(dir))
    else {
        return refuse(format!(
            "{} serves {} and its tables will not read back, so this run \
             would replace all of it.",
            dir.display(),
            serves(served),
        ));
    };
    let count = index.root().map_or(0, |root| root.aggregate.count());
    if meta.names() as u64 != count || count < tree.len() as u64 {
        return refuse(format!(
            "{} does not read as its own resume point — {} in the cell \
             tree, {} in the names table, {} in the resume point — so this \
             run would replace all of it.",
            dir.display(),
            count,
            meta.names(),
            tree.len(),
        ));
    }
    Resume::Level(tree, meta, cursor)
}

/// A summary of a build, for the binary to print and check.
///
/// Each part is what this build wrote rather than what stands in the
/// directory, so a part it was not asked for says so.
#[derive(Copy, Clone, Debug)]
pub struct BuildReport {
    /// The cell tree, where this build wrote one.
    pub cells: Option<ColdReport>,
    /// The metadata sidecars written beside the tree.
    pub meta: MetaReport,
}

impl BuildReport {
    /// Whether every system landed in exactly one cell: the partition holds.
    ///
    /// True where no tree was written.
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
    use crate::testing::Scratch;

    /// The address the write below writes
    const WATCHED: i64 = 900_001_000;

    /// The address the catch-up below writes
    const CAUGHT: i64 = 900_001_001;

    /// The address the resume point test below writes
    const RECORDED: i64 = 900_001_002;

    /// The address the resume point test writes before its first build
    const SETTLED: i64 = 900_001_003;

    /// A system at `position`, lit as the class `G` names rather than by a
    /// scan.
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
            kind: galos_index::StarKind::G,
        }
    }

    /// A report whose event timestamp is a year old still reads as newly
    /// arrived
    ///
    /// `updated_at` is the timestamp off the entry and may be years in the
    /// past, never greater than a cursor taken today. `received_at` is
    /// stamped by the upsert as the report is written, so the age of what a
    /// report describes has nothing to do with whether a watch sees it
    /// arrive.
    ///
    /// Needs a server to reach, named by `TEST_DATABASE_URL`, and stands
    /// down without one.
    #[async_std::test]
    async fn an_old_event_timestamp_still_reads_as_newly_arrived() {
        let Some(db) = Scratch::new().await else { return };

        // The cursor a watch would be holding: the database's clock, read
        // before anything is written.
        let before = db.now().await.expect("the clock should read").naive_utc();

        // What a journal import looks like: an event a year old.
        let happened = chrono::Utc::now() - chrono::TimeDelta::days(365);
        let system = elite_journal::system::System {
            pos: Some(elite_journal::system::Coordinate {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            }),
            ..elite_journal::system::System::new(WATCHED, "TEST WATCHED SYSTEM")
        };
        let mut conn = db.acquire().await.expect("a connection");
        crate::systems::System::from_journal(
            &mut conn, happened, "test", &system,
        )
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

        db.done().await;
    }

    /// A resume point the event feed wrote is refused, whatever it counts up
    /// to
    ///
    /// Both derivations write the same kind of resume point and only one
    /// stands for the galaxy. An event-derived directory is internally
    /// consistent about the little it holds, so it passes the counting gates
    /// a half-written directory fails. What wrote a checkpoint is therefore
    /// read before anything is counted.
    ///
    /// No database: the directory is two systems written by hand, and the
    /// only thing differing between the halves of the test is which
    /// derivation the checkpoint names.
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
        let mut names = galos_index::names::Writer::writing(&dir)
            .expect("the names writer should open");
        for system in &inputs {
            names
                .push(galos_index::meta::NameEntry {
                    address: system.id64 as i64,
                    name: format!("TEST {}", system.id64).into(),
                    position: [system.position[0] as f32, 0.0, 0.0],
                })
                .expect("a name should push");
        }
        names.finish().expect("the names should publish");
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
        Checkpoint::compact(
            &path,
            Some(cursor),
            By::Database,
            inputs.iter().copied(),
        )
        .expect("a resume point should write");
        assert!(
            matches!(resume(&dir, &path, &params), Resume::Level(..)),
            "a directory the database derived, matching its own resume point, \
             was refused",
        );
        assert_eq!(
            refusal(&dir, &path),
            None,
            "a directory the database derived was refused before it was read",
        );

        Checkpoint::compact(
            &path,
            Some(cursor),
            By::Events,
            inputs.iter().copied(),
        )
        .expect("a resume point should write");

        // The whole of the fix: an event-derived directory that is serving
        // systems is neither resumed nor silently rebuilt over. Before this
        // the answer here was "build the galaxy afresh", and what it
        // replaced was every system the directory published.
        let Resume::Refuse(said) = resume(&dir, &path, &params) else {
            panic!("a resume point the event feed wrote was rebuilt over")
        };
        assert!(
            said.contains("--rebuild"),
            "the refusal has to name the way past it: {}",
            said,
        );
        let before_writing = refusal(&dir, &path)
            .expect("the same refusal, before anything has been written");
        assert!(
            before_writing.contains("--rebuild"),
            "the pre-write gate has to name the way past it too: {}",
            before_writing,
        );

        // And a directory serving nothing is built without a word: there is
        // nothing there to lose, which is the case the old `None` was right
        // about.
        let empty = dir.with_extension("empty");
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).expect("a scratch directory");
        assert_eq!(refusal(&empty, &path), None);
        assert!(matches!(resume(&empty, &path, &params), Resume::Build));
        let _ = std::fs::remove_dir_all(&empty);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&path);
    }

    /// A poll an hour away is still a poll a run can be stopped between
    ///
    /// `watch` sleeps the interval between passes in ticks, so a run asked to
    /// stop is obeyed before the next pass rather than after the sleep.
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
    /// The cursor has to be at or past the moment the systems it read were
    /// written, whoever follows it asking only for what came after; and the
    /// directory has to be one a catch-up will adopt.
    ///
    /// `populated.bin` is the witness for the second half: it is written
    /// whole by a full build and by a resumed pass only where a political
    /// column moved, so a file left as the first call wrote it says the
    /// second resumed and published nothing.
    ///
    /// Needs a server to reach, named by `TEST_DATABASE_URL`, and stands
    /// down without one.
    #[async_std::test]
    async fn a_catch_up_levels_a_directory_and_leaves_one_to_resume() {
        let Some(db) = Scratch::new().await else { return };

        let dir = std::env::temp_dir()
            .join(format!("galos_db_catch_up_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let checkpoint = dir.with_extension("checkpoint");
        let _ = std::fs::remove_file(&checkpoint);

        // A system written before the catch-up looks, which is what the
        // catch-up owes the directory.
        let before = db.now().await.expect("the clock should read").naive_utc();
        let system = elite_journal::system::System {
            pos: Some(elite_journal::system::Coordinate {
                x: 4.0,
                y: 5.0,
                z: 6.0,
            }),
            ..elite_journal::system::System::new(CAUGHT, "TEST CAUGHT SYSTEM")
        };
        let mut conn = db.acquire().await.expect("a connection");
        crate::systems::System::from_journal(
            &mut conn,
            chrono::Utc::now(),
            "test",
            &system,
        )
        .await
        .expect("the system should write");

        let cursor = catch_up(
            &db,
            &dir,
            &checkpoint,
            Parts::ALL,
            false,
            never(),
            untold(),
        )
        .await
        .expect("the catch-up should run")
        .end()
        .expect("nothing asked it to stop");
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

        let again = catch_up(
            &db,
            &dir,
            &checkpoint,
            Parts::ALL,
            false,
            never(),
            untold(),
        )
        .await
        .expect("the second catch-up should run")
        .end()
        .expect("nothing asked it to stop");
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

        db.done().await;
    }

    /// One part alone is a repair, and a directory serving nothing has
    /// nothing to repair
    ///
    /// What this stops: a run asked for `--only reaches` against a
    /// directory nobody had built yet read the whole database, wrote a
    /// `reaches.bin` with no cell tree over it, and reported that the
    /// index was level. `galos index status` could not open what it left.
    ///
    /// The second half is the other side of the same rule: the same
    /// narrowed call against a directory that *is* built is the repair
    /// case, and it goes through.
    ///
    /// Needs a server to reach, named by `TEST_DATABASE_URL`, and stands
    /// down without one.
    #[async_std::test]
    async fn a_narrowed_derive_wants_a_directory_to_repair() {
        let Some(db) = Scratch::new().await else { return };

        let dir = std::env::temp_dir()
            .join(format!("galos_db_only_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let checkpoint = dir.with_extension("checkpoint");
        let _ = std::fs::remove_file(&checkpoint);

        let one = Parts { reaches: true, ..Parts::NONE };
        let refused =
            catch_up(&db, &dir, &checkpoint, one, false, never(), untold())
                .await;
        let Err(said) = refused else {
            panic!("a part alone was derived into a directory with no tree")
        };
        let said = format!("{said}");
        assert!(
            said.contains("galos ingest"),
            "a refusal should say how to build it: {}",
            said,
        );
        assert!(
            !galos_index::source::reaches_path(&dir).exists(),
            "the refusal came after the read it was there to save",
        );

        // Something to build a directory out of, so the repair below has
        // one to repair.
        let mut conn = db.acquire().await.expect("a connection");
        let system = elite_journal::system::System {
            pos: Some(elite_journal::system::Coordinate {
                x: 7.0,
                y: 8.0,
                z: 9.0,
            }),
            ..elite_journal::system::System::new(CAUGHT, "TEST CAUGHT SYSTEM")
        };
        crate::systems::System::from_journal(
            &mut conn,
            chrono::Utc::now(),
            "test",
            &system,
        )
        .await
        .expect("the system should write");
        catch_up(&db, &dir, &checkpoint, Parts::ALL, false, never(), untold())
            .await
            .expect("the build should run")
            .end()
            .expect("nothing asked it to stop");

        catch_up(&db, &dir, &checkpoint, one, false, never(), untold())
            .await
            .expect("a built directory is the repair case")
            .end()
            .expect("nothing asked it to stop");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&checkpoint);

        db.done().await;
    }

    /// A catch-up leaves a resume point standing for what the directory
    /// serves, however many frames the log holds for one system
    ///
    /// Nothing replays for the caller of a catch-up: it opens the directory
    /// for editing against the checkpoint beside it, and a checkpoint short
    /// of what is served cannot be opened at all.
    ///
    /// The pending log goes with it. A system reported again is published
    /// again and logged again — the look-back alone is enough for that, a
    /// pass reading back [`CURSOR_OVERLAP`] past its own cursor — so the
    /// log holds two frames naming one system, and what replaying them
    /// must land on is that one system and not two.
    ///
    /// **The frames are behind the publish and never ahead of it**, which
    /// is what [`pass`] writes in that order for: a frame stands for
    /// systems the directory already serves, and a resume point larger
    /// than the directory beneath it is one [`resume`] refuses rather than
    /// opens. So this appends nothing by hand; it reports the system twice
    /// and lets the passes log what they published.
    ///
    /// Needs a server to reach, named by `TEST_DATABASE_URL`, and stands
    /// down without one.
    #[async_std::test]
    async fn a_catch_up_leaves_a_checkpoint_for_what_it_serves() {
        let Some(db) = Scratch::new().await else { return };

        let dir = std::env::temp_dir()
            .join(format!("galos_db_recorded_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let checkpoint = dir.with_extension("checkpoint");
        let _ = std::fs::remove_file(&checkpoint);
        let _ = Pending::clear(&checkpoint);

        // Something for the first build to publish: a build over no systems
        // writes a directory nothing can resume from, and what this test is
        // about is the resume point a later pass leaves.
        let mut conn = db.acquire().await.expect("a connection");
        let settled = elite_journal::system::System {
            pos: Some(elite_journal::system::Coordinate {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            }),
            ..elite_journal::system::System::new(SETTLED, "TEST SETTLED SYSTEM")
        };
        crate::systems::System::from_journal(
            &mut conn,
            chrono::Utc::now(),
            "test",
            &settled,
        )
        .await
        .expect("the settled system should write");

        // A directory brought level, and then a system written after it
        // was: the pass that publishes this one is what has to leave the
        // resume point standing for it.
        catch_up(&db, &dir, &checkpoint, Parts::ALL, false, never(), untold())
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
            &mut conn,
            chrono::Utc::now(),
            "test",
            &system,
        )
        .await
        .expect("the system should write");

        catch_up(&db, &dir, &checkpoint, Parts::ALL, false, never(), untold())
            .await
            .expect("the second catch-up should run");

        // The same system reported again, which the pass below publishes
        // and logs a second frame for, behind the frame the pass above
        // left. Written rather than waited for: the look-back would carry
        // it anyway, and a test should not turn on how long it took to get
        // here.
        crate::systems::System::from_journal(
            &mut conn,
            chrono::Utc::now(),
            "test",
            &system,
        )
        .await
        .expect("the system should be reported again");

        catch_up(&db, &dir, &checkpoint, Parts::ALL, false, never(), untold())
            .await
            .expect("the third catch-up should run");

        let served = Index::read(&dir)
            .expect("the index should read")
            .root()
            .expect("the tree should have a root")
            .aggregate
            .count();
        let written = Checkpoint::read(&checkpoint)
            .expect("the resume point should read");
        // Two records over one system, which is what makes the replay
        // below worth asserting: `deltas` is every frame's records, in the
        // order the passes logged them.
        assert!(
            written.deltas().len() > 1,
            "the log holds {} record(s), so nothing here replays two over \
             one system",
            written.deltas().len(),
        );
        // The tree a restart would rebuild: the base in one batch and the
        // log over it, which is `resume`'s own two moves.
        let params = BuildParams::default();
        let mut rebuilt = Tree::build(written.base(), &params);
        rebuilt.apply(written.deltas());
        assert_eq!(
            rebuilt.len() as u64,
            served,
            "the directory serves {} systems and its resume point stands for \
             {}, so nothing can open it to edit what it holds",
            served,
            rebuilt.len(),
        );
        assert!(written.cursor.is_some(), "a resume point with no cursor");
        assert!(
            matches!(resume(&dir, &checkpoint, &params), Resume::Level(..)),
            "the resume point a catch-up left was refused by the resume it \
             was written for",
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&checkpoint);

        db.done().await;
    }

    /// A run that is already stopping does not start a cold build
    ///
    /// The case this exists for is the layout migration ahead of it being
    /// abandoned: `bring_level` finds nothing to resume, and a build
    /// started there would read the galaxy as far as its first check before
    /// it noticed the same flag the migration had already obeyed. What it
    /// must do instead is answer that it was stopped, leaving the directory
    /// with nothing in it for the next run to find.
    ///
    /// Needs a server to reach, named by `TEST_DATABASE_URL`, and stands
    /// down without one.
    #[async_std::test]
    async fn a_stopping_run_starts_no_build() {
        let Some(db) = Scratch::new().await else { return };

        let dir = std::env::temp_dir()
            .join(format!("galos_db_stopping_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let checkpoint = dir.with_extension("checkpoint");
        let _ = std::fs::remove_file(&checkpoint);

        let stopped = catch_up(
            &db,
            &dir,
            &checkpoint,
            Parts::ALL,
            false,
            &|| true,
            untold(),
        )
        .await;
        assert_eq!(
            stopped.expect("a stop is not a failure"),
            Reached::Stopped(Abandoned::unstarted()),
        );
        assert!(
            !dir.join(galos_index::store::INDEX_FILE).exists(),
            "a build ran anyway",
        );
        assert!(!checkpoint.exists(), "a resume point was written");
        assert_eq!(
            std::fs::read_dir(&dir).expect("the directory should read").count(),
            0,
            "a stopping run wrote to the directory",
        );

        let _ = std::fs::remove_dir_all(&dir);

        db.done().await;
    }
}
