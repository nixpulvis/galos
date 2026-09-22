//! Reading a publisher into whatever is being kept: `galos ingest`.
//!
//! ```sh
//! galos ingest --from eddn --db                       # rows
//! galos ingest --from eddn --index                    # a directory
//! galos ingest --from eddn --db --index               # both, one read
//! galos ingest --from journal=~/Saved\ Games/… --db --watch
//! galos ingest --from spansh=galaxy.json --index      # a galaxy, region by region
//! galos ingest --from spansh=galaxy.json --db --bulk --shard 0/8
//! galos ingest --from database --index --watch 5      # the rows into a directory
//! ```
//!
//! **One verb, because it is one job.** A source knows how to read one
//! publisher; a sink knows what a reading means on its side. `--db` and
//! `--index` name which sinks this run has, and naming both reads each
//! publisher *once* into the pair — which over EDDN is the difference
//! between one subscription and two carrying the same galaxy, and is the
//! run an operator keeping both stores current actually wants.
//!
//! ```text
//! --from eddn ────────┐
//! --from journal=PATH ┼─> readings ─┬─> --db     (Postgres, per message)
//! --from edsm=PATH ───┘             └─> --index  (cell tree + tables, on a beat)
//! ```
//!
//! ## What is inferred, and what is said
//!
//! The index has two write routes and they are ~200 GB of resident memory
//! apart: the sink, which holds a live tree and publishes on a beat so a
//! map can read the directory as it is written, and the regional build,
//! which spills every system to its bucket and holds one region at a time.
//! Which one a run takes is **not** a flag and not a verb — it follows from
//! what the run is:
//!
//! - A finite dump, an index and no database has nothing to fan out to and
//!   no reader waiting on a half-written directory, so it takes the
//!   regional build. That is the only route a two hundred million system
//!   dump fits in.
//! - Everything else holds the tree: a feed has no end to build from, and a
//!   run writing rows as well has a second sink the builder cannot feed.
//!
//! **The route is announced when the run starts**, because it is the
//! difference between minutes and hours and between 7 GB and 200 GB, and a
//! run that silently chose the other one is a run nobody can account for.
//! That was the actual complaint against inferring it — not the inference.

use chrono::Utc;
use clap::Args;
use galos::read::from::{self, Source};
use galos::read::{cold, derive, Qualifiers, PUBLISH_EVERY};
use galos::sink::index::INDEX_DIR;
use galos::sink::relay::{Dropped, Live};
use galos::sink::{Fan, Index, Relay, Sink};
use galos::{Shard, Shutdown};
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;

/// What this run calls itself in a spool's `cursors/` directory.
///
/// One name per consumer, so a recorded feed read into rows and the same
/// spool read into a directory keep their own places in it and neither
/// moves the other's. One name for both *sinks* of one run, though: they
/// are one reading, and it is the reading that has a position.
const CONSUMER: &str = "galos";

// **Each heading is the condition its flags need**, because that is the
// question a reader has: not "is this a following flag or an importing
// flag" — which was the old grouping, and told nobody anything — but "does
// this flag do something in the run I am about to make".
//
// A flag under a heading it does not match is **refused, not ignored**.
// `--user` over EDDN is somebody who thinks they are filing a galaxy under
// their own name; a run that took the flag and did nothing with it would
// be wrong in exactly the way that is hardest to notice. See
// [`refused`] and `galos::read::Qualifiers::refused`.

/// Where the reading is written. Every run names at least one.
const SINKS: &str = "Where it goes (at least one)";

/// The two flags of a run with no end.
///
/// Two spellings, because a heading that named `database` in a build with
/// no database in it would be pointing at a source that answers "rebuild
/// with the `db` feature".
#[cfg(feature = "db")]
const FOLLOWING: &str = "Only where the source keeps going (eddn, spool, \
                         journal, database)";
#[cfg(not(feature = "db"))]
const FOLLOWING: &str =
    "Only where the source keeps going (eddn, spool, journal)";

/// `--shard`, which divides a file.
const SHARING: &str = "Only where the source is a file (journal, edsm, \
                       eddb, spansh)";

/// EDDN's own two.
const FEED: &str = "Only with --from eddn";

/// The journal's one.
const JOURNAL: &str = "Only with --from journal=PATH";

/// EDSM's API's two.
const API: &str = "Only with --from edsm-api=NAME";

/// The rows-into-a-directory build's two. Behind the feature, with the
/// flags it heads: without a database there is no `--from database` to
/// qualify.
#[cfg(feature = "db")]
const ROWS: &str = "Only with --from database";

/// The one flag about how the pool is opened, likewise.
#[cfg(feature = "db")]
const BULK: &str = "Only with --db";

/// Read what a publisher publishes into the database, a directory, or both.
#[derive(Args)]
pub struct Cli {
    /// Where to read from: `eddn`, `spool=DIR`, `journal=PATH`,
    /// `edsm=PATH`, `edsm-api=NAME`, `eddb=PATH`, `spansh=PATH`, or
    /// `database`. Repeatable; each is read once.
    ///
    /// A spool is a recorded feed — see `eddn record`. It is followed from
    /// this run's own cursor unless it is told otherwise:
    /// `spool=DIR,from=earliest` replays what is held, and
    /// `spool=DIR,since=2026-09-19T12:00:00Z` starts at an hour.
    ///
    /// `database` is the rows read back out into a directory, which is how
    /// an index is rebuilt rather than maintained.
    #[arg(long = "from", value_name = "SOURCE", required = true)]
    from: Vec<Source>,

    /// Write to Postgres, which is what `DATABASE_URL` names.
    #[cfg(feature = "db")]
    #[arg(long, help_heading = SINKS)]
    db: bool,

    /// Write an index directory, `.galos_index` where DIR is left off.
    ///
    /// **`-i/--index` names the directory everywhere it appears — but
    /// here it also chooses a sink**, which is why it takes no
    /// `GALOS_INDEX` default the way `galos index`'s verbs do: those act
    /// on a directory that exists and there is one obvious one to act on,
    /// while an environment variable that silently made every ingest
    /// write a galaxy of cells is not a default, it is a surprise.
    #[arg(
        short = 'i',
        long,
        value_name = "DIR",
        num_args = 0..=1,
        default_missing_value = INDEX_DIR,
        help_heading = SINKS,
    )]
    index: Option<PathBuf>,

    /// Resume file for the index, kept outside the served directory.
    /// `DIR.checkpoint` beside the index directory by default.
    ///
    /// Refused without `-i`: it is where an index resumes from, and a run
    /// writing none has nothing to resume.
    #[arg(long, value_name = "FILE", help_heading = SINKS)]
    checkpoint: Option<PathBuf>,

    /// Whose journal this is, overriding what the files say.
    ///
    /// For a directory of logs copied off another machine, where the file
    /// names no commander this one would recognise.
    #[arg(short = 'u', long, value_name = "NAME", help_heading = JOURNAL)]
    user: Option<String>,

    /// Replace a directory that already serves systems, where this run
    /// cannot resume what is there.
    ///
    /// Reading `--from database` into a directory rebuilds the whole of it
    /// when the resume point beside it is not one of the database's — a
    /// directory built from a dump, or from the feed, whose systems the
    /// rows have no other way to carry forward. That replaces every system
    /// it publishes, so it is refused unless it is asked for here. `galos
    /// index status -i DIR` says what wrote the one you have.
    #[cfg(feature = "db")]
    #[arg(long, help_heading = ROWS)]
    rebuild: bool,

    /// Rebuild only these parts of the directory, leaving the rest as it
    /// stands. Every part by default.
    ///
    /// The repair case: a change to how one part is derived leaves every
    /// published copy of that part stale and everything beside it fine,
    /// and rebuilding the lot to fix one is a hundred megabytes of
    /// rewriting to say nothing new.
    ///
    /// Refused with `--watch`, which would repair one part forever while
    /// the rest of the directory aged behind it, and refused where the
    /// directory is not built: one part written into an empty directory is
    /// a table with no tree over it, which nothing can open.
    #[cfg(feature = "db")]
    #[arg(
        long,
        value_name = "PART",
        value_delimiter = ',',
        num_args = 1..,
        help_heading = ROWS,
    )]
    only: Vec<Part>,

    /// Keep following what was named rather than exiting, SECS apart. A
    /// second where SECS is left off.
    ///
    /// SECS is the database poll, and the longest a journal directory
    /// nothing is writing to is left alone — the filesystem says when a
    /// log was written, so a journal is read as the game writes rather
    /// than SECS later.
    ///
    /// **EDDN and a spool follow with or without it**; it is a dump that
    /// this cannot mean anything for, and naming one is refused rather
    /// than waited on.
    #[arg(
        long,
        value_name = "SECS",
        num_args = 0..=1,
        default_missing_value = "1",
        help_heading = FOLLOWING,
    )]
    watch: Option<u64>,

    /// Seconds between index publishes, one at least, five by default.
    ///
    /// The beat exists because something may be reading the directory
    /// while it is written. A run with an end has no beat — it writes the
    /// whole directory once, when it finishes — so naming this for one is
    /// refused, as is naming it for a run that writes no index at all.
    #[arg(long, value_name = "SECS", help_heading = FOLLOWING)]
    publish: Option<u64>,

    /// The feed's ZMQ address, its published one by default.
    #[arg(short = 'r', long, value_name = "URL", help_heading = FEED)]
    remote: Option<String>,

    /// Seconds of silence before the connection is replaced, or 0 to
    /// leave it alone however quiet it goes.
    ///
    /// A ZMQ subscription that has died answers no error and delivers
    /// nothing, so a quiet socket is reopened rather than trusted.
    #[arg(long, value_name = "SECS", help_heading = FEED)]
    stall: Option<u64>,

    /// Open the database for a bulk import: commits left unflushed and a
    /// higher connection ceiling. For an import that can be re-run from
    /// its source, not for a live feed.
    #[cfg(feature = "db")]
    #[arg(long, help_heading = BULK)]
    bulk: bool,

    /// Read one share of the file: every Nth record, offset I, so N
    /// processes cover it exactly once between them.
    ///
    /// A journal directory shards by *file*, because a file is what names
    /// the commander who flew it. Every write is a guarded upsert keyed by
    /// an address, so shares that overlap cost time and nothing else.
    ///
    /// Refused with `--watch`: a share is of what is there when the run
    /// starts, and what arrives after it is every share's.
    #[arg(
        long,
        value_name = "I/N",
        value_parser = from::shard,
        help_heading = SHARING,
    )]
    shard: Option<Shard>,

    /// Take everything in a cube this many light years across, around the
    /// system named.
    #[arg(
        long,
        short,
        value_name = "LY",
        conflicts_with = "sphere",
        help_heading = API,
    )]
    cube: Option<u32>,

    /// Take everything within this many light years of it instead.
    #[arg(long, short, value_name = "LY", help_heading = API)]
    sphere: Option<u32>,

    /// Clear a lock left behind by a builder that was killed, and take it.
    ///
    /// The refusal names the pid holding the directory. Check it first: a
    /// lock cleared while its builder is merely slow to answer is two
    /// writers over one directory, which is what the lock is for.
    ///
    /// Here and on the `index` verbs, which are the two that write a
    /// directory and so the two that can meet the refusal.
    #[arg(long, help_heading = SINKS)]
    force_lock: bool,
}

/// One part of what a built index directory holds
///
/// Named on `--only` to rebuild that part alone. What each is derived from
/// differs: `cells` and `names` come out of one read of every positioned
/// system, `reaches` and `bodies` out of one read of every scanned thing,
/// and `populated` and `factions` out of a query apiece. Asking for one
/// reads only what that one needs.
#[cfg(feature = "db")]
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Part {
    /// The cell tree and its payloads, which the map draws the galaxy from.
    Cells,
    /// Every system's name and place: the search index and the routing graph.
    Names,
    /// The populated systems the map colors and filters by.
    Populated,
    /// How far each scanned system reaches, which every shell is sized by.
    Reaches,
    /// Which systems can supercharge a drive, which the router plots by.
    Boosts,
    /// The faction id-to-name table.
    Factions,
    /// One file per system of the stars, bodies and barycenters in it.
    Bodies,
}

/// The parts `named` comes to, which is every part where nothing was named
#[cfg(feature = "db")]
fn parts_of(named: &[Part]) -> galos_db::index::Parts {
    use galos_db::index::Parts;
    if named.is_empty() {
        return Parts::ALL;
    }

    let mut parts = Parts::NONE;
    for part in named {
        match part {
            Part::Cells => parts.cells = true,
            Part::Names => parts.names = true,
            Part::Populated => parts.populated = true,
            Part::Reaches => parts.reaches = true,
            Part::Boosts => parts.boosts = true,
            Part::Factions => parts.factions = true,
            Part::Bodies => parts.bodies = true,
        }
    }
    parts
}

impl Cli {
    /// The per-source flags, as [`Qualifiers`] reads them.
    ///
    /// The rules about them are written once, in `galos::read`: each is a
    /// flag that qualifies a reading rather than naming one, and each is
    /// refused where the run reads no source it could qualify.
    fn qualifiers(&self) -> Qualifiers<'_> {
        Qualifiers {
            sources: &self.from,
            user: self.user.as_deref(),
            remote: self.remote.as_deref(),
            stall: self.stall,
            watch: self.watch,
            cube: self.cube,
            sphere: self.sphere,
            shard: self.shard,
        }
    }

    /// Whether this run writes rows.
    fn to_db(&self) -> bool {
        #[cfg(feature = "db")]
        return self.db;
        #[cfg(not(feature = "db"))]
        false
    }

    /// The sources that are publishers rather than the other store.
    fn published(&self) -> Vec<Source> {
        self.from
            .iter()
            .filter(|source| !matches!(source, Source::Database))
            .cloned()
            .collect()
    }

    /// Whether the rows are one of the things being read.
    fn from_db(&self) -> bool {
        self.from.iter().any(|it| matches!(it, Source::Database))
    }

    /// The dump this run would build a directory from, where it is one.
    ///
    /// The regional build, and the four things that make a run it: one
    /// source, that source a dump with a read of its own into a `Build`, an
    /// index to write, and no database to fan out to. A run with a second
    /// sink cannot take it — the builder takes records and writes the
    /// directory whole, with nothing to hand a second sink — and a run with
    /// a second *source* would be two builds over one directory, the second
    /// publishing its own galaxy over the first.
    ///
    /// `edsm=PATH` and `eddb=PATH` are finite files that want exactly this
    /// treatment; what they lack is a read of their own into a `Build`, and
    /// until one lands they hold the tree and still work.
    fn whole(&self) -> Option<&PathBuf> {
        if self.to_db() || self.index.is_none() {
            return None;
        }
        match self.from.as_slice() {
            [Source::Spansh(path)] => Some(path),
            _ => None,
        }
    }
}

/// Read every named source into every named sink, and close them out.
///
/// A supervisor: it installs the signal handler, holds the shutdown token
/// both halves read, and joins them before it answers — so a Ctrl-C or a
/// SIGTERM ends in the last publish, the whole-directory [`Sink::finish`]
/// and a resume point, rather than in a killed process and a directory
/// nothing can reopen.
pub async fn run(cli: Cli) -> Result<bool, String> {
    refused(&cli)?;

    let shutdown = Shutdown::new();
    galos::shutdown::on_interrupt(shutdown.clone());

    // One writer per directory for the length of the run. Two would each
    // hold their own tree of it and publish over one another, which nothing
    // downstream can notice and no resume point can repair.
    let _lock = match &cli.index {
        Some(dir) => Some(
            match cli.force_lock {
                true => galos_index::Lock::force(dir),
                false => galos_index::Lock::take(dir),
            }
            .map_err(|err| format!("{}: {err}", dir.display()))?,
        ),
        None => None,
    };

    // A dump straight into a directory, with no tree in memory in between.
    // Announced, because it is the difference between holding one region
    // and holding the sky: see the module header.
    if let Some(path) = cli.whole() {
        let dir = cli.index.as_deref().expect("an index, or this is None");
        let checkpoint = Index::checkpoint(dir, cli.checkpoint.as_deref());
        info!(
            dump = %path.display(),
            dir = %dir.display(),
            "building the directory region by region: one finite source, one \
             directory, and nothing else to write, so the systems are \
             spilled and raised a region at a time rather than held in a \
             tree",
        );
        let mut source = galos::read::spansh::Galaxy {
            path: path.to_owned(),
            dir: dir.to_owned(),
            now: Utc::now(),
            shutdown: shutdown.clone(),
        };
        return cold::cold(&mut source, dir, &checkpoint);
    }

    // Two pools, not one shared five connections. The collect side writes a
    // row per message and the derive side samples a clock and reads the
    // galaxy back; a `Sink` method cannot report failure, so an acquire
    // that stalls behind an hour-long build is a message silently dropped.
    #[cfg(feature = "db")]
    let collecting = match cli.db {
        true => Some(open(cli.bulk).await?),
        false => None,
    };
    #[cfg(feature = "db")]
    let deriving = match cli.from_db() || (cli.db && cli.index.is_some()) {
        true => Some(open(false).await?),
        false => None,
    };

    // `--from database` with nothing else: the directory is derived from
    // the rows and from nothing else, which is a build rather than a
    // reading and has no channel and no sinks in it.
    #[cfg(feature = "db")]
    if cli.from_db() && cli.published().is_empty() {
        let (db, dir) = (
            deriving.expect("a database, or this run was refused"),
            cli.index.clone().expect("an index, or this run was refused"),
        );
        let checkpoint = Index::checkpoint(&dir, cli.checkpoint.as_deref());
        return derive::from_database(
            &db,
            &dir,
            &checkpoint,
            parts_of(&cli.only),
            cli.watch.map(Duration::from_secs),
            cli.rebuild,
            &shutdown,
        )
        .await
        .map(|()| true);
    }

    // The index worker's channel, where there is an index to feed. The
    // sender is cloned once per source and dropped here straight away, so
    // the channel closes when the last source finishes.
    let (live, dropped) = (Live::new(), Dropped::new());
    let readings = cli.index.as_ref().map(|_| Relay::channel());

    // The beat, where there is anything to publish between the start and
    // the end of the run. An import ends, and what it ends with is the
    // whole directory `finish` writes; publishing it half-read is a
    // rewrite of every changed cell and every table for a galaxy nobody
    // can read yet.
    let publish = cli.qualifiers().following().then(|| {
        let secs = cli.publish.unwrap_or(PUBLISH_EVERY).max(1);
        Duration::from_secs(secs)
    });

    // Its own thread and its own `block_on`: a publish is a whole-galaxy
    // `fs::write` and everything else here is waiting on a socket.
    let deriving = match (&cli.index, &readings) {
        (Some(dir), Some((_, receiver))) => {
            let worker = derive::Derive {
                dir: dir.clone(),
                checkpoint: Index::checkpoint(dir, cli.checkpoint.as_deref()),
                #[cfg(feature = "db")]
                db: deriving,
                publish,
                readings: receiver.clone(),
                live: live.clone(),
                dropped: dropped.clone(),
                #[cfg(feature = "db")]
                rebuild: cli.rebuild,
                shutdown: shutdown.clone(),
            };
            Some(
                std::thread::Builder::new()
                    .name("index".to_string())
                    .spawn(move || async_std::task::block_on(worker.run()))
                    .map_err(|err| format!("no thread for the index: {err}"))?,
            )
        }
        _ => None,
    };

    let options = cli.qualifiers().options(CONSUMER);
    let published = cli.published();
    let mut reading = Vec::with_capacity(published.len());
    for source in published {
        let mut sinks: Vec<Box<dyn Sink>> = Vec::with_capacity(2);
        #[cfg(feature = "db")]
        if let Some(db) = &collecting {
            sinks.push(Box::new(galos::sink::Db::new(db.clone())));
        }
        if let Some((sender, _)) = &readings {
            sinks.push(Box::new(Relay::new(
                sender.clone(),
                live.clone(),
                dropped.clone(),
            )));
        }

        let (options, shutdown) = (options.clone(), shutdown.clone());
        reading.push(async_std::task::spawn(async move {
            galos::read::collect(source, options, Fan::of(sinks), shutdown)
                .await
        }));
    }
    // This run's own copy of the sender, which would otherwise hold the
    // channel open after every source had finished and leave the worker
    // waiting for a reading nobody is going to send.
    drop(readings);

    let mut collected = true;
    for task in reading {
        collected &= task.await;
    }

    // And only now the exit code. The worker is still publishing what the
    // sources handed it, and it has a whole directory and a resume point to
    // write after that.
    let derived = match deriving {
        None => true,
        Some(thread) => match thread.join() {
            Ok(Ok(())) => true,
            Ok(Err(said)) => {
                eprintln!("{said}");
                false
            }
            Err(_) => {
                eprintln!("the index worker panicked");
                false
            }
        },
    };

    Ok(collected && derived)
}

/// How many connections a bulk import may hold open
///
/// Above the five an ordinary run takes. A run reads every `--from` at once
/// and each holds a connection for the length of a message's transaction,
/// so the ceiling is what bounds how many sources can be writing at a time.
#[cfg(feature = "db")]
const BULK_CONNECTIONS: u32 = 16;

/// A pool onto whatever `DATABASE_URL` names.
///
/// `bulk` opens it for an import that can be re-run from its source:
/// `synchronous_commit` off, and [`BULK_CONNECTIONS`] of them.
#[cfg(feature = "db")]
async fn open(bulk: bool) -> Result<galos_db::Database, String> {
    let opened = match bulk {
        true => galos_db::Database::bulk(BULK_CONNECTIONS).await,
        false => galos_db::Database::new().await,
    };
    opened.map_err(|err| format!("no database: {err}"))
}

/// Refuse the combinations of flags that cannot mean anything.
///
/// The per-source rules are [`Qualifiers::refused`], shared with nothing
/// else now but written where they are read. What is here is what only a
/// run with two possible sinks can get wrong.
fn refused(cli: &Cli) -> Result<(), String> {
    cli.qualifiers().refused()?;

    // Before the general refusal below, which would answer "name a sink"
    // to a run that named the one sink this source has.
    if cli.from_db() {
        if cli.to_db() {
            return Err(
                "--from database --db would read the rows into themselves"
                    .into(),
            );
        }
        if cli.index.is_none() {
            return Err(
                "--from database reads the rows into a directory; name \
                 -i/--index DIR"
                    .into(),
            );
        }
    }

    if !cli.to_db() && cli.index.is_none() {
        return Err(
            "nothing to write to: name --db, -i/--index DIR, or both".into()
        );
    }

    if cli.checkpoint.is_some() && cli.index.is_none() {
        return Err(
            "--checkpoint is where an index resumes from; this run writes \
             none"
                .into(),
        );
    }

    if cli.publish.is_some() {
        if cli.index.is_none() {
            return Err(
                "--publish is the beat an index is written on; this run \
                 writes none"
                    .into(),
            );
        }
        if !cli.qualifiers().following() {
            return Err(
                "--publish is the beat of a run that follows something; \
                 this run has an end, and publishes once when it reaches it"
                    .into(),
            );
        }
    }

    #[cfg(feature = "db")]
    if !cli.only.is_empty() {
        if !cli.from_db() {
            return Err(
                "--only names the parts to derive from the rows: --from \
                 database"
                    .into(),
            );
        }
        if cli.watch.is_some() {
            return Err(
                "--only is the repair case, and --watch keeps repairing one \
                 part while the rest of the directory ages behind it"
                    .into(),
            );
        }
        if !cli.published().is_empty() {
            return Err(
                "--only is a derive from the rows alone; this run also reads \
                 a publisher, which fills every part it touches"
                    .into(),
            );
        }
    }

    #[cfg(feature = "db")]
    if cli.rebuild && !cli.from_db() {
        return Err(
            "--rebuild replaces a directory the rows cannot resume: --from \
             database"
                .into(),
        );
    }

    #[cfg(feature = "db")]
    if cli.bulk && !cli.db {
        return Err(
            "--bulk is how a database is opened and this run writes to none"
                .into(),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The flags of an ingest, as the command line would have parsed them.
    ///
    /// Through the whole command line — `galos ingest …` — rather than
    /// through [`Cli`] alone, which cannot parse itself: it is a
    /// `clap::Args`, and what turns argv into one is the verb it hangs
    /// off. Parsing a wrapper struct instead would test the flags and not
    /// the wiring, which is the half that breaks silently.
    fn ingesting(said: &[&str]) -> Cli {
        let mut argv = vec!["galos", "ingest"];
        argv.extend_from_slice(said);
        match crate::Cli::try_parse_from(argv)
            .expect("these flags should parse")
            .command
        {
            crate::Command::Ingest(it) => it,
            _ => unreachable!("the verb is `ingest`"),
        }
    }

    /// A run must name somewhere to write
    ///
    /// The one thing this verb can be given that means nothing at all: a
    /// feed read at thirty messages a second into neither store.
    #[test]
    fn a_run_must_name_somewhere_to_write() {
        let Err(said) = refused(&ingesting(&["--from", "eddn"])) else {
            panic!("a run with no sink was accepted")
        };
        assert!(said.contains("--db"), "should say what to name: {said}");

        assert!(refused(&ingesting(&["--from", "eddn", "--db"])).is_ok());
        assert!(
            refused(&ingesting(&["--from", "eddn", "--index", "d"])).is_ok()
        );
        assert!(refused(&ingesting(&[
            "--from", "eddn", "--db", "--index", "d",
        ]))
        .is_ok());
    }

    /// The dump route is inferred, and this is what it is inferred from
    ///
    /// Four conditions, and each of the last three is a run that would
    /// have to hold a tree instead: a second sink the builder cannot feed,
    /// a second source that would publish its own galaxy over the first,
    /// and a source with no whole-directory read of its own.
    #[test]
    fn a_dump_alone_into_a_directory_is_built_region_by_region() {
        let alone = ingesting(&["--from", "spansh=Cargo.toml", "--index", "d"]);
        assert_eq!(alone.whole(), Some(&PathBuf::from("Cargo.toml")));

        for said in [
            // Rows as well: the builder has nothing to hand the other
            // sink.
            &["--from", "spansh=Cargo.toml", "--index", "d", "--db"][..],
            // A second source over the same directory.
            &[
                "--from",
                "spansh=Cargo.toml",
                "--from",
                "journal=bin",
                "--index",
                "d",
            ][..],
            // A feed has no end to build from.
            &["--from", "eddn", "--index", "d"][..],
            // A dump with no `Build` read of its own.
            &["--from", "edsm=Cargo.lock", "--index", "d"][..],
            // No directory at all.
            &["--from", "spansh=Cargo.toml", "--db"][..],
        ] {
            assert_eq!(
                ingesting(said).whole(),
                None,
                "{said:?} holds the tree",
            );
        }
    }

    /// The rows are read into a directory and never into themselves
    #[cfg(feature = "db")]
    #[test]
    fn the_rows_are_read_into_a_directory() {
        let into_itself =
            ingesting(&["--from", "database", "--db", "--index", "d"]);
        let Err(said) = refused(&into_itself) else {
            panic!("the rows read into themselves were accepted")
        };
        assert!(said.contains("themselves"), "should say why: {said}");

        let nowhere = ingesting(&["--from", "database", "--db"]);
        assert!(
            refused(&nowhere).is_err(),
            "the rows have one sink and it is not the rows",
        );

        let sound = ingesting(&["--from", "database", "--index", "d"]);
        assert!(refused(&sound).is_ok(), "{:?}", refused(&sound));
    }

    /// A narrowed derive is the repair case and nothing else
    ///
    /// `--only` names parts to derive from the rows. A run taking events
    /// cannot honour it — the feed reports a scan, the reaches table is
    /// told and the cell tree is not, and the two halves of the directory
    /// stand for different systems from there on — and a *follower* would
    /// repair one part forever while the rest aged behind it.
    #[cfg(feature = "db")]
    #[test]
    fn only_belongs_to_a_one_shot_derive() {
        let repair = ingesting(&[
            "--from", "database", "--index", "d", "--only", "boosts",
        ]);
        assert!(refused(&repair).is_ok(), "the repair case is the point");

        for said in [
            &[
                "--from", "database", "--index", "d", "--only", "boosts",
                "--watch",
            ][..],
            &["--from", "eddn", "--index", "d", "--only", "boosts"][..],
            &[
                "--from", "database", "--from", "eddn", "--index", "d",
                "--only", "boosts",
            ][..],
        ] {
            assert!(
                refused(&ingesting(said)).is_err(),
                "{said:?} cannot honour --only",
            );
        }
    }

    /// The beat belongs to a run something is reading while it writes
    #[test]
    fn a_beat_belongs_to_a_run_that_follows_something() {
        let import = ingesting(&[
            "--from",
            "spansh=Cargo.toml",
            "--index",
            "d",
            "--publish",
            "2",
        ]);
        let Err(said) = refused(&import) else {
            panic!("an import on a beat was accepted")
        };
        assert!(said.contains("--publish"), "should say why: {said}");

        let no_index = ingesting(&["--from", "eddn", "--db", "--publish", "2"]);
        assert!(
            refused(&no_index).is_err(),
            "a beat for a directory this run does not write",
        );

        let followed =
            ingesting(&["--from", "eddn", "--index", "d", "--publish", "2"]);
        assert!(refused(&followed).is_ok());
    }

    /// `--index` with no directory is the usual one
    #[test]
    fn an_index_defaults_to_its_usual_directory() {
        let it = ingesting(&["--from", "eddn", "--index"]);
        assert_eq!(it.index, Some(PathBuf::from(INDEX_DIR)));
    }

    /// The shared rules are reached from this verb's flags
    ///
    /// One test rather than one per rule: what each rule *means* is pinned
    /// in `galos::read`, where it is written, and what is this verb's
    /// business is that every qualifier on the command line arrives there.
    /// A flag left out of [`Cli::qualifiers`] would be a flag silently
    /// ignored.
    #[test]
    fn the_shared_rules_are_reached_from_here() {
        let named = ingesting(&["--from", "eddn", "--db", "--user", "HRC-2"]);
        let Err(said) = refused(&named) else {
            panic!("a commander named over the feed was accepted")
        };
        assert!(said.contains("--user"), "should say why: {said}");

        let sharded = ingesting(&["--from", "eddn", "--db", "--shard", "0/8"]);
        assert!(refused(&sharded).is_err(), "a sharded feed was accepted");

        let sound =
            ingesting(&["--from", "journal=bin", "--db", "--user", "HRC-2"]);
        assert!(refused(&sound).is_ok(), "{:?}", refused(&sound));
    }
}
