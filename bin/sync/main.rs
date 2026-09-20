//! Moving Elite's galaxy from wherever it is published into wherever it is
//! wanted.
//!
//! Several sources and two sinks, in one process. A source knows how to read
//! one publisher — journal files, the EDDN feed, an EDSM dump or API, a saved
//! EDDB dump, a Spansh galaxy dump — and says what it read through
//! [`Sink`]. A sink knows what that means on its side: rows in Postgres,
//! or a `galos_index` directory a client draws from with no server at all.
//!
//! ```sh
//! galos-sync --from journal=~/Saved\ Games/…            # to the database
//! galos-sync --from journal=~/… --watch --db            # and keep following it
//! galos-sync --from journal=~/… --index .galos_index    # to a directory instead
//! galos-sync --from eddn --index .galos_index           # a live map, no database
//! galos-sync --from eddn --db --index .galos_index      # one read, both of them
//! galos-sync --db --index .galos_index --watch 5        # the database into an index
//! galos-sync --from spansh=galaxy.json --db             # every body in the dump
//! galos-sync --from spansh=galaxy.json --db --bulk --shard 0/8  # one of 8
//! ```
//!
//! ## The model
//!
//! Events are the live input to *both* sinks. The database is what an index
//! is rebuilt **from**, not what it is maintained from, so a running process
//! never reads the database to keep the index current:
//!
//! ```text
//! --from eddn ────────┐
//! --from journal=PATH ┼─> events ─┬─> --db     (Postgres, per message)
//! --from edsm=PATH ───┘           └─> --index  (cell tree + tables, on a beat)
//! ```
//!
//! `--from` repeats and every source reads into both sinks, which over EDDN
//! is the difference between one subscription and two carrying the same
//! galaxy. The sources run at once, each with a sink of its own onto the
//! same pool and the same channel.
//!
//! ## The two modes
//!
//! A run either **follows** something or **imports** something, and every
//! per-source flag belongs to one of the two — `--help` groups them that
//! way. A run follows where a source has no end: the feed, a journal under
//! `--watch`, or `--db --index --watch`, which follows rows rather than a
//! source. It is the following run that publishes the index on `--publish`'s
//! beat, because something is reading the directory while it is written to.
//!
//! Everything else is an import: a dump, a saved dump, an API answer, a
//! journal read once. It has an end, nothing is waiting on a half-written
//! directory, and the whole of it is written at [`Sink::finish`] when the
//! last record is read. See [`from::Source::follows`] and [`following`].
//!
//! The one thing worth knowing before reading any of it: **the sinks are not
//! interchangeable and are not meant to be.** A database keeps stations,
//! markets, signals and factions; an index keeps the sky and what is inside a
//! system. A source hands over everything it read and each sink takes what it
//! is for, saying in its own impl what it does with the rest and why. See
//! [`galos::sink`].
//!
//! ## The cold route
//!
//! One import does not go through the sinks at all. A run that names a
//! finite source, `--index DIR` and no `--db` has nothing to fan out to, and
//! the index sink's price for holding one reading open — a live tree and the
//! whole names table, a kilobyte a system — is what stops a two hundred
//! million system dump from being importable at all. Such a run goes through
//! [`cold`] instead, which cuts the galaxy into regions and builds them one
//! at a time, holding one region rather than the sky. See [`cold_route`].
//!
//! [`region_budget`] is the memory dial of that route, and what it bounds
//! is how many systems are held at once: a region's systems are read back
//! off its spill, built, written and dropped. A region can be dropped
//! because a cut is disjoint — every system in it is owned by a cell inside
//! it — so once it is built there is nothing left to ask it.
//!
//! ## What `main` is
//!
//! A supervisor. It installs a SIGINT handler, holds the shutdown token both
//! halves read, and joins them before it answers — so a Ctrl-C ends in the
//! last publish, the whole-directory [`Sink::finish`] and a resume point,
//! rather than in a killed process and a directory nothing can reopen. The
//! status is `collect_ok && derive_ok`.

use chrono::Utc;
use clap::{Parser, ValueEnum};
use from::Source;
use galos::sink::index::INDEX_DIR;
use galos::sink::relay::{Dropped, Live};
use galos::sink::{Db, Fan, Index, Relay, Sink};
use galos::{bar, Shard, Shutdown};
use galos_db::index::Parts;
// `HEARD` is what `RUST_LOG` falls back to, and it is this crate's `sqlx`
// that writes the one line it silences.
use galos_db::{Database, HEARD};
use galos_index::{
    region_budget, Build, BuildParams, Built, By, Ending, Rows, Start,
};
use std::io::{stderr, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};
use tracing::{info, warn};

mod derive;
mod eddb;
mod eddn;
mod edsm;
mod from;
mod journal;
mod spansh;

/// How often the index sink is asked to write what it has taken, where the
/// run is following something.
///
/// A database has written each message as it arrived and does nothing on a
/// beat. An index has been editing a tree in memory, and this is how often
/// that reaches the disk: a publish rewrites the index file whole, so doing
/// it per message at thirty a second would be thirty rewrites a second to
/// move one system. `--publish` says otherwise.
///
/// A run whose sources all end has no beat at all: there is nothing to see
/// a half-written directory and it is written whole when the run finishes.
const PUBLISH_EVERY: u64 = 5;

/// The heading the flags of a run that never ends are listed under.
const FOLLOWING: &str = "Following what is published";

/// The heading the flags of a run that reads something finite are listed
/// under.
const IMPORTING: &str = "Importing what is already there";

/// Sync Elite's galaxy from its publishers into a database and an index.
#[derive(Parser)]
#[command(name = "galos-sync", version, about)]
struct Cli {
    /// Where to read from: `eddn`, `journal=PATH`, `edsm=PATH`,
    /// `edsm-api=NAME`, `eddb=PATH` or `spansh=PATH`. Repeatable; each is
    /// read once.
    #[arg(long = "from", value_name = "SOURCE")]
    from: Vec<Source>,

    /// Write to Postgres, which is what DATABASE_URL names.
    #[arg(long)]
    db: bool,

    /// Write an index directory, `.galos_index` where DIR is left off.
    #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = INDEX_DIR)]
    index: Option<PathBuf>,

    /// Resume file for the index, kept outside the served directory.
    /// `DIR.checkpoint` beside the index directory by default.
    #[arg(long, value_name = "FILE")]
    checkpoint: Option<PathBuf>,

    /// Take the index directory even if a lock file is already there.
    ///
    /// For a lock left behind by a killed run: `SIGKILL`, a lost host or a
    /// container killed for its memory all leave the file without the
    /// process that made it. Check the pid the refusal names first — this
    /// clears the lock whether or not anything is still writing, and two
    /// writers over one directory interleave two galaxies into it.
    #[arg(long = "force-lock")]
    force_lock: bool,

    /// Replace a directory that already serves systems, where this run
    /// cannot resume what is there.
    ///
    /// A derive from the database rebuilds the whole directory when its
    /// resume point is not one of the database's — a directory built from
    /// a dump, or from the feed, whose systems this has no other way to
    /// carry forward. That replaces every system it publishes, so it is
    /// refused unless it is asked for here. `galos-index status DIR` says
    /// what wrote the one you have.
    #[arg(long)]
    rebuild: bool,

    /// Whose journal this is, overriding what the files say. Only with
    /// `--from journal=PATH`.
    #[arg(short = 'u', long, value_name = "NAME")]
    user: Option<String>,

    /// Rebuild only these parts, leaving the rest of the directory as it
    /// stands. Every part by default, and only for a one-shot derive.
    #[arg(long, value_name = "PART", value_delimiter = ',', num_args = 1..)]
    only: Vec<Part>,

    /// Keep following what was named rather than exiting, SECS apart. A
    /// second where SECS is left off.
    ///
    /// The database poll, and the longest a journal directory nothing is
    /// writing to is left alone: the filesystem says when a log was written
    /// to, so a journal is read as soon as the game writes rather than SECS
    /// later. EDDN is a subscription and follows either way.
    ///
    /// Only where there is something to follow: a journal directory, or
    /// `--db --index` with no `--from`. A dump has an end.
    #[arg(
        long,
        value_name = "SECS",
        num_args = 0..=1,
        default_missing_value = "1",
        help_heading = FOLLOWING,
    )]
    watch: Option<u64>,

    /// Seconds between index publishes, one at least, five by default. Only
    /// for a run that follows something; a run with an end publishes once,
    /// when it ends.
    #[arg(long, value_name = "SECS", help_heading = FOLLOWING)]
    publish: Option<u64>,

    /// EDDN's ZMQ address, the feed's own by default. Only with `--from
    /// eddn`.
    #[arg(short = 'r', long, value_name = "URL", help_heading = FOLLOWING)]
    remote: Option<String>,

    /// Seconds of EDDN silence before the connection is replaced, or 0 to
    /// leave it alone. Only with `--from eddn`.
    #[arg(long, value_name = "SECS", help_heading = FOLLOWING)]
    stall: Option<u64>,

    /// Open the database for a bulk import: commits left unflushed and a
    /// higher connection ceiling. For an import that can be re-run from its
    /// source, not for a live feed.
    #[arg(long, help_heading = IMPORTING)]
    bulk: bool,

    /// Read one share of a source: every Nth record of a dump, or every
    /// Nth file of a journal directory, offset I, so N processes cover it
    /// exactly once between them. A journal shards by file because a file
    /// is what names the commander who flew it. Every write is a guarded
    /// upsert keyed by an address, so shards that overlap cost only time.
    #[arg(
        long,
        value_name = "I/N",
        value_parser = from::shard,
        help_heading = IMPORTING,
    )]
    shard: Option<Shard>,

    /// EDSM's API: take everything in a cube this many light years across.
    /// Only with `--from edsm-api=NAME`.
    #[arg(
        long,
        short,
        value_name = "LY",
        conflicts_with = "sphere",
        help_heading = IMPORTING,
    )]
    cube: Option<u32>,

    /// EDSM's API: take everything within this many light years. Only with
    /// `--from edsm-api=NAME`.
    #[arg(long, short, value_name = "LY", help_heading = IMPORTING)]
    sphere: Option<u32>,
}

/// One part of what a built index directory holds
///
/// Named on `--only` to rebuild that part alone. What each is derived from
/// differs: `cells` and `names` come out of one read of every positioned
/// system, `reaches` and `bodies` out of one read of every scanned thing, and
/// `populated` and `factions` out of a query apiece. Asking for one reads only
/// what that one needs.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Part {
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
fn parts_of(named: &[Part]) -> Parts {
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

#[async_std::main]
async fn main() -> ExitCode {
    // Nothing a crate traces goes anywhere until something is listening for
    // it, dependencies included. `RUST_LOG` picks what to hear, and info
    // upwards from everything without it.
    tracing_subscriber::fmt()
        // Above whatever bars are drawing, so they keep the bottom lines
        // and the log does not land on top of them.
        .with_writer(bar::Log)
        // Color is for a terminal. Redirected, it would be escape codes
        // around every line of the log.
        .with_ansi(stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| HEARD.into()),
        )
        .init();

    match run(Cli::parse()).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(said) => {
            eprintln!("{said}");
            ExitCode::FAILURE
        }
    }
}

/// Whether this run is following something rather than reading it out.
///
/// The distinction the beat and half the per-source flags turn on. A run
/// follows where any source it names follows — see [`Source::follows`] —
/// or where it is the database into an index under `--watch`, which
/// follows rows rather than a source.
fn following(cli: &Cli) -> bool {
    if cli.from.is_empty() {
        return cli.watch.is_some() && cli.db && cli.index.is_some();
    }
    let watching = cli.watch.is_some();
    cli.from.iter().any(|source| source.follows(watching))
}

/// The source a cold build would read, where this run is one.
///
/// The builder takes records pushed at it and writes the directory whole
/// with no tree in memory in between, so a run takes the cold route where
/// the reading ends and the directory is all it writes: no source follows —
/// [`following`], which is [`Source::follows`] over every `--from` —
/// `--index` is named, and `--db` is not, since the sinks are what fan one
/// reading to two places.
///
/// One finite source, and the one with a reader that pushes. `edsm=PATH`
/// and `eddb=PATH` are finite files that want exactly this treatment; what
/// they lack is a read of their own into a [`Build`], and until one lands
/// they keep the sink path and still work.
///
/// One source and not several. Two finite sources into one directory is two
/// cold builds over the same directory, the second publishing its own
/// galaxy over the first, so a run naming more than one keeps the fan-out
/// that was written to carry them.
fn cold_route(cli: &Cli) -> Option<&PathBuf> {
    if cli.db || cli.index.is_none() || following(cli) {
        return None;
    }
    match cli.from.as_slice() {
        [Source::Spansh(path)] => Some(path),
        _ => None,
    }
}

/// Refuse the combinations of flags that cannot mean anything.
///
/// Each of these is a run that would otherwise start, do something other
/// than what was asked, and say nothing about it — which is worse than not
/// starting, since what these write is a database and a directory a map is
/// served from.
fn refused(cli: &Cli) -> Result<(), String> {
    if !cli.db && cli.index.is_none() {
        return Err(
            "nothing to write to: name --db, --index DIR, or both".to_string()
        );
    }

    for (at, source) in cli.from.iter().enumerate() {
        if cli.from[..at].contains(source) {
            return Err(format!(
                "--from {source} was named twice, and one run cannot read \
                 it as two sources"
            ));
        }
    }

    if cli.from.is_empty() && cli.index.is_none() {
        return Err("--db with nothing to read from and no index to derive \
                    has nothing to do"
            .to_string());
    }

    if cli.from.is_empty() && !cli.db {
        return Err("--index with nothing to read from and no --db to derive \
                    it from has nothing to do"
            .to_string());
    }

    // The repair case and nothing else. Narrowing a run that takes events is
    // a directory published half current: the feed reports a scan, the
    // reaches table is told about it and the cell tree is not, and the two
    // halves stand for different systems from there on.
    let repairing = cli.db
        && cli.index.is_some()
        && cli.from.is_empty()
        && cli.watch.is_none();
    if !cli.only.is_empty() && !repairing {
        return Err("--only rebuilds part of an index from the database, so \
                    it belongs to a one-shot derive: --db --index DIR with \
                    no --from and no --watch"
            .to_string());
    }

    if cli.checkpoint.is_some() && cli.index.is_none() {
        return Err("--checkpoint names where the index resumes from and \
                    this run writes no index"
            .to_string());
    }

    if cli.bulk && !cli.db {
        return Err("--bulk is how a database is opened and this run writes \
                    to none"
            .to_string());
    }

    if cli.shard.is_some() {
        if cli.from.is_empty() {
            return Err("--shard divides what a --from reads and this run \
                        reads nothing"
                .to_string());
        }
        if cli.watch.is_some() {
            return Err("--shard divides a file that is all there; --watch \
                        is a feed with no end to divide"
                .to_string());
        }
        for source in &cli.from {
            if let Source::Eddn | Source::EdsmApi(_) = source {
                return Err(format!(
                    "--shard divides a file between processes and \
                     `{source}` is not a file"
                ));
            }
        }
        // A cold build writes the whole of `dir`: the cut it counted, every
        // cell payload under it, the names table and a resume point saying
        // that is the galaxy. N processes each counting their own share
        // would each write a whole index over the others, and the directory
        // would end up holding whichever finished last. A share of a file is
        // not a share of a directory.
        if cold_route(cli).is_some() {
            return Err("--shard divides a file between processes and a run \
                        that builds an index from a dump alone writes the \
                        whole directory, not a share of it: each process \
                        would publish its own galaxy over the others. Name \
                        --db as well to take the shared read, or read the \
                        file whole"
                .to_string());
        }
    }

    // The flags that belong to one mode, or to one source, and would be
    // read by nothing where the run is the other. Each of these is written
    // by somebody who meant it — an import told to publish every two
    // seconds, a feed's address given to a run that reads a dump — and a
    // run that took them silently would do the opposite of what it was
    // asked and say so nowhere.
    let follows = following(cli);
    let journal = cli.from.iter().any(|it| matches!(it, Source::Journal(_)));
    let api = cli.from.iter().any(|it| matches!(it, Source::EdsmApi(_)));

    if cli.publish.is_some() {
        if !follows {
            return Err("--publish is the beat an index is written out on \
                        and the run publishes when it ends; the beat is \
                        for --from eddn, a journal under --watch, or --db \
                        --index --watch"
                .to_string());
        }
        if cli.from.is_empty() {
            return Err("--publish is the beat events are written out on \
                        and this run reads no source; a --db --index \
                        derive republishes on --watch's own beat"
                .to_string());
        }
    }

    if cli.watch.is_some() && !follows {
        return Err("--watch follows what is still being written and \
                    nothing here is: a journal directory, or --db --index \
                    with no --from. A dump is read out and the run ends"
            .to_string());
    }

    if cli.user.is_some() && !journal {
        return Err("--user is whose journal is being read and this run \
                    reads none: --from journal=PATH"
            .to_string());
    }

    if cli.remote.is_some() && !cli.from.contains(&Source::Eddn) {
        return Err("--remote is where the feed is subscribed to and this \
                    run does not read it: --from eddn"
            .to_string());
    }

    if cli.stall.is_some() && !cli.from.contains(&Source::Eddn) {
        return Err("--stall is how long the feed may carry nothing before \
                    its connection is replaced and this run does not read \
                    it: --from eddn"
            .to_string());
    }

    if cli.cube.is_some() && !api {
        return Err("--cube is how much of a neighbourhood EDSM's API is \
                    asked for and this run does not ask it: --from \
                    edsm-api=NAME"
            .to_string());
    }

    if cli.sphere.is_some() && !api {
        return Err("--sphere is how much of a neighbourhood EDSM's API is \
                    asked for and this run does not ask it: --from \
                    edsm-api=NAME"
            .to_string());
    }

    // A file named on the command line and not there is a typo, and a run
    // that carried on would publish a directory derived from nothing over
    // one that had something in it. Checked before a sink is opened, so a
    // refused run writes nothing at all.
    for source in &cli.from {
        let named = match source {
            Source::Journal(path)
            | Source::Edsm(path)
            | Source::Eddb(path)
            | Source::Spansh(path) => path,
            Source::Eddn | Source::EdsmApi(_) => continue,
        };
        if !named.exists() {
            return Err(format!(
                "--from {source} is not there. A leading `~` is expanded \
                 here, so a path that still cannot be found is the path"
            ));
        }
    }

    Ok(())
}

/// Everything the per-source flags say, carried to whichever source wants
/// them.
///
/// One struct rather than five arguments: the sources run concurrently, so
/// each task needs its own copy of what it was told, and a `Cli` is not
/// something to clone per source.
#[derive(Clone)]
struct Options {
    user: Option<String>,
    remote: String,
    stall: Option<Duration>,
    watch: Option<Duration>,
    cube: Option<u32>,
    sphere: Option<u32>,
    /// One process's share of a file, from `--shard`.
    shard: Option<Shard>,
}

impl Options {
    fn of(cli: &Cli) -> Options {
        Options {
            user: cli.user.clone(),
            // The feed's own address, where the run did not name one.
            remote: match &cli.remote {
                Some(url) => url.clone(),
                None => eddn::URL.to_string(),
            },
            // Nothing said is the default window; nought said is a
            // connection this leaves alone however quiet it goes.
            stall: match cli.stall {
                None => Some(eddn::STALL),
                Some(0) => None,
                Some(secs) => Some(Duration::from_secs(secs)),
            },
            watch: cli.watch.map(Duration::from_secs),
            cube: cli.cube,
            sphere: cli.sphere,
            shard: cli.shard,
        }
    }
}

/// Open what was named, read into it, and close it out.
///
/// The database is opened only where a database is one of the things being
/// written to. That is the point of the whole arrangement: `--index DIR`
/// runs on a machine with no `DATABASE_URL` and no Postgres installed, and a
/// connection opened unconditionally here would have made it need one.
async fn run(cli: Cli) -> Result<bool, String> {
    refused(&cli)?;

    let shutdown = Shutdown::new();
    listen(shutdown.clone());

    // One writer per directory for the length of the run. Two would each
    // hold their own tree of it and publish over one another, which nothing
    // downstream can notice and no resume point can repair.
    //
    // `--force-lock` clears one a killed run left behind, which is the only
    // way out of it: the file names a pid that is gone and nothing else
    // removes it. The judgement stays with whoever passed the flag, which
    // is why it is a flag and not a retry.
    let _lock = match &cli.index {
        Some(dir) => Some(
            if cli.force_lock {
                galos_index::Lock::force(dir)
            } else {
                galos_index::Lock::take(dir)
            }
            .map_err(|err| format!("{}: {err}", dir.display()))?,
        ),
        None => None,
    };

    // A dump straight into a directory, with no tree in memory in between.
    // See [`cold_route`] for which runs this is and why the rest are not.
    if let Some(path) = cold_route(&cli) {
        let dir = cli.index.as_deref().expect("an index, or this is None");
        let checkpoint = Index::checkpoint(dir, cli.checkpoint.as_deref());
        let mut source = spansh::Galaxy {
            path: path.to_owned(),
            dir: dir.to_owned(),
            now: Utc::now(),
            shutdown: shutdown.clone(),
        };
        return cold(&mut source, dir, &checkpoint);
    }

    // Two pools, not one shared five connections. The collect side writes a
    // row per message and the derive side samples a clock and reads the
    // galaxy back; a `Sink` method cannot report failure, so an acquire that
    // stalls behind an hour-long build is a message silently dropped.
    let collecting = match cli.db {
        true => Some(open(cli.bulk).await?),
        false => None,
    };
    let deriving = match cli.db && cli.index.is_some() {
        true => Some(open(false).await?),
        false => None,
    };

    // No sources: the index is derived from the database and nothing else.
    // This is what `galos-sync db` was.
    if cli.from.is_empty() {
        let (db, dir) = (
            deriving.expect("a database, or this run was refused"),
            cli.index.expect("an index, or this run was refused"),
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
    // sender is cloned once per source and dropped by main straight away, so
    // the channel closes when the last source finishes.
    let (live, dropped) = (Live::new(), Dropped::new());
    let readings = cli.index.as_ref().map(|_| Relay::channel());

    // The beat, where there is anything to publish between the start and
    // the end of the run. An import ends, and what it ends with is the
    // whole directory `finish` writes; publishing it half-read is a rewrite
    // of every changed cell and every table for a galaxy nobody can read
    // yet, since a dump's directory carries no cursor to resume from.
    let publish = following(&cli).then(|| {
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
                db: deriving,
                publish,
                readings: receiver.clone(),
                live: live.clone(),
                dropped: dropped.clone(),
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

    let options = Options::of(&cli);
    let mut reading = Vec::with_capacity(cli.from.len());
    for source in cli.from {
        let mut sinks: Vec<Box<dyn Sink>> = Vec::with_capacity(2);
        if let Some(db) = &collecting {
            sinks.push(Box::new(Db::new(db.clone())));
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
            collect(source, options, Fan::of(sinks), shutdown).await
        }));
    }
    // Main's own copy of the sender, which would otherwise hold the channel
    // open after every source had finished and leave the worker waiting for
    // a reading nobody is going to send.
    drop(readings);

    let mut collect_ok = true;
    for task in reading {
        collect_ok &= task.await;
    }

    // And only now the exit code. The worker is still publishing what the
    // sources handed it, and it has a whole directory and a resume point to
    // write after that.
    let derive_ok = match deriving {
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

    Ok(collect_ok && derive_ok)
}

/// Build `dir` from a finite source and leave a resume point beside it.
///
/// The regional build: every system is spilled to its bucket's file as the
/// source pushes it, the buckets are formed into regions of
/// [`region_budget`] systems, and the regions are built one at a time. What
/// is held at once is one region's systems and, at the end, every cell in
/// the galaxy — never a live tree over the sky, which is the whole reason
/// this path is not the sink one.
///
/// The resume point says [`By::Events`] and carries no cursor. A cursor is a
/// database clock — what a delta pass reads `received_at` against — and a
/// file published last Tuesday has none to offer; a dump's own newest
/// `updateTime` is a time out in the galaxy and not a time this program's
/// database wrote a row, so recording it as one would have the next
/// catch-up read back from a clock nothing here ever kept. `By::Events`
/// says the directory was derived from records rather than from the
/// database, which is what makes `galos_db::index` rebuild rather than
/// resume from it.
///
/// Ctrl-C during the read is a clean exit that keeps its place: the builder
/// publishes nothing — no cells, no names table, no resume point — but the
/// spills of every system it has read stay where they are, and the next run
/// over the same file takes them up and reads on from the line it stopped
/// at. The body files it wrote as it went stand too, each whole and each
/// filed under its own address. A run over a different dump, or over one
/// that has changed length, reads from the start and says so.
///
/// The metadata tables go in after [`Build::finish`] has answered, which
/// is after the index file. A stop then leaves no table at all: up to the
/// point of no return the directory is as the build found it, and past it
/// the directory serves nothing until a build finishes, so a table written
/// early would either break the first or stand for a galaxy nothing
/// published. And the resume point `finish` leaves is what makes the next
/// run resume rather than build, so tables short of the cells beside them
/// would be a directory nothing ever repairs. `galos_db::index` writes its
/// own parts at the same place, for the same reason.
fn cold(
    source: &mut spansh::Galaxy,
    dir: &Path,
    checkpoint: &Path,
) -> Result<bool, String> {
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    let start = Instant::now();
    let budget = region_budget();
    let failed = |err| format!("the index could not be built: {err}");

    // What the directory already stands for, where it stands for part of
    // this same file. The clock comes back with it: a build ages every
    // system against one moment, and a run carrying on with its own would
    // bin half the galaxy's Recency against another.
    let (taking_up, place) = match galos_index::left_off(checkpoint) {
        Some(left) => match spansh::Place::of(&left, &source.path) {
            Some(place) => {
                info!(
                    systems = left.systems(),
                    at = place.at(),
                    dir = %dir.display(),
                    "carrying on with the read this directory was published \
                     from",
                );
                source.now = place.now;
                (Start::Resuming(left), Some(place))
            }
            None => (Start::Fresh, None),
        },
        None => (Start::Fresh, None),
    };
    let carrying_on = place.is_some();

    let stop = || source.shutdown.asked();
    let mut build = Build::begin(
        dir,
        checkpoint,
        BuildParams::default(),
        budget,
        taking_up,
        &stop,
    )
    .map_err(failed)?;
    // Beside the build's own scratch rather than in it: `Build::finish`
    // clears that when it publishes, and these have to stand until the
    // tables have been written off them. A run carrying on starts from the
    // tables the directory publishes, those rows being the only copy of
    // what the read before it derived.
    let spill = rows_dir(checkpoint);
    let mut rows = match carrying_on {
        true => Rows::onto(&spill, dir),
        false => Rows::writing(&spill),
    }
    .map_err(failed)?;
    source.read(&mut build, &mut rows, place).map_err(failed)?;

    // A read cut short publishes what it read: a dump is read in file
    // order, so what has been read is a galaxy in itself, and a map can
    // open it while the rest of the file is still to come. The mark the
    // publish leaves is what the next run carries on from.
    let report = match build
        .finish(By::Events, None, Ending::Publish)
        .map_err(failed)?
    {
        Built::Index(report) => report,
        Built::Stopped(abandoned) => {
            info!(
                %abandoned,
                elapsed = ?start.elapsed(),
                dir = %dir.display(),
                "asked to stop before anything was read",
            );
            return Ok(true);
        }
    };

    // Every table the dump can fill, read back off the rows the read
    // spilled and written in address order. Not the factions: the dump's
    // own faction lists are passed over by `spansh::System`, and
    // nothing reading records could number a faction anyway — an empty
    // table would say the galaxy has none where an absent one says this
    // index cannot tell.
    let counts = rows.finish(dir).map_err(failed)?;

    // One line, in the shape the sink's own publish prints, so a directory
    // written by either builder says what happened the same way.
    info!(
        wrote = "whole",
        moved = report.systems,
        systems = report.systems,
        cells = report.cells,
        leaves = report.leaves,
        deepest = report.deepest_level,
        regions = report.regions,
        budget,
        named = report.named,
        rows = report.named_rows,
        populated = counts.populated,
        reaches = counts.reaches,
        boosts = counts.boosts,
        elapsed = ?start.elapsed(),
        dir = %dir.display(),
        "index published",
    );
    if !report.is_consistent() {
        warn!(
            systems = report.systems,
            placed = report.points,
            "the cut did not partition the galaxy",
        );
    }
    Ok(true)
}

/// Where a cold build's table rows are spilled: beside the resume point,
/// as the build's own scratch is.
///
/// A run starting over opens them at zero bytes, which is the clearing,
/// and [`Rows::finish`] removes them once the tables have been written.
fn rows_dir(checkpoint: &Path) -> PathBuf {
    let mut name = checkpoint.as_os_str().to_owned();
    name.push(".rows");
    PathBuf::from(name)
}

/// How many connections a bulk import may hold open
///
/// Above the five an ordinary run takes. A run reads every `--from` at once
/// and each holds a connection for the length of a message's transaction, so
/// the ceiling is what bounds how many sources can be writing at a time.
const BULK_CONNECTIONS: u32 = 16;

/// A pool onto whatever `DATABASE_URL` names.
///
/// `bulk` opens it for an import that can be re-run from its source:
/// `synchronous_commit` off, and [`BULK_CONNECTIONS`] of them.
async fn open(bulk: bool) -> Result<Database, String> {
    let opened = match bulk {
        true => Database::bulk(BULK_CONNECTIONS).await,
        false => Database::new().await,
    };
    opened.map_err(|err| format!("no database: {err}"))
}

/// Ask the run to stop on Ctrl-C, and kill it on the second one.
///
/// The whole reason there is a handler at all: the default action for SIGINT
/// is to die where it stands, and where it stands is usually mid-publish,
/// with a directory serving systems no resume point knows about. Asking
/// instead costs whatever is left of the current publish. A second Ctrl-C is
/// somebody who has decided that is too long, and it is theirs to have — the
/// directory is what it was before the run started publishing over it.
fn listen(shutdown: Shutdown) {
    let installed = ctrlc::set_handler(move || {
        if shutdown.asked() {
            eprintln!("stopping now; the index directory may be half written");
            std::process::exit(130);
        }
        eprintln!(
            "stopping: the last publish and the resume point still have to \
             be written, so this takes a moment. Ctrl-C again to stop now."
        );
        shutdown.ask();
    });
    if let Err(err) = installed {
        warn!(
            error = %err,
            "no signal handler; Ctrl-C will kill this run mid-publish",
        );
    }
}

/// Read one source into its own sinks, and close them out.
///
/// One of these per `--from`, run at once. Each has a [`Db`] and a [`Relay`]
/// of its own — the pool and the channel behind them are shared — so a dump
/// being read does not wait on the feed, and the feed does not wait on it.
async fn collect(
    source: Source,
    options: Options,
    mut fan: Fan,
    shutdown: Shutdown,
) -> bool {
    let read = match source {
        Source::Eddn => {
            let eddn = eddn::Eddn { url: options.remote, stall: options.stall };
            eddn.read(&mut fan, &shutdown).await
        }
        Source::Journal(path) => {
            let journal = journal::Journal {
                path,
                user: options.user,
                watch: options.watch,
                shard: options.shard,
            };
            journal.read(&mut fan, &shutdown).await
        }
        Source::Edsm(path) => {
            edsm::Dump { path, shard: options.shard }
                .read(&mut fan, &shutdown)
                .await
        }
        Source::EdsmApi(name) => {
            let api =
                edsm::Api { name, cube: options.cube, sphere: options.sphere };
            api.read(&mut fan, &shutdown).await
        }
        Source::Eddb(path) => {
            eddb::Eddb { path, shard: options.shard }
                .read(&mut fan, &shutdown)
                .await
        }
        Source::Spansh(path) => {
            spansh::Dump { path, shard: options.shard }
                .read(&mut fan, &shutdown)
                .await
        }
    };

    // Each sink decides what finishing means for it: nothing for a database,
    // which wrote every message as it arrived, and a last word for a relay,
    // which has handed everything on to a worker that closes the directory
    // out itself.
    let closed = match fan.finish().await {
        Ok(()) => true,
        Err(said) => {
            warn!(error = %said, "could not close the sinks out");
            false
        }
    };
    for said in fan.said_each() {
        info!("{said}");
    }
    read && closed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flags of a run, as the command line would have parsed them
    fn cli(said: &[&str]) -> Cli {
        let mut argv = vec!["galos-sync"];
        argv.extend_from_slice(said);
        Cli::try_parse_from(argv).expect("these flags should parse")
    }

    /// Naming nothing builds the whole index
    ///
    /// The flag is what a build is narrowed by, so its absence has to leave
    /// the build exactly as wide as it always was.
    #[test]
    fn a_build_naming_no_part_builds_them_all() {
        assert_eq!(parts_of(&[]), Parts::ALL);
    }

    /// And naming one builds that one and no other
    ///
    /// The point of the flag: a reach table stale by a change to how a reach
    /// is worked out is worth rewriting on its own, and the hundred megabytes
    /// of names beside it are not.
    #[test]
    fn a_build_naming_a_part_builds_only_that_part() {
        assert_eq!(
            parts_of(&[Part::Reaches]),
            Parts { reaches: true, ..Parts::NONE }
        );
        assert_eq!(
            parts_of(&[Part::Reaches, Part::Bodies]),
            Parts { reaches: true, bodies: true, ..Parts::NONE }
        );
    }

    /// A run that names no sink is refused
    ///
    /// Everything else here is about what a run writes, and this is the one
    /// that writes nowhere: a feed read at thirty messages a second into
    /// nothing at all.
    #[test]
    fn a_run_must_name_somewhere_to_write() {
        let Err(said) = refused(&cli(&["--from", "eddn"])) else {
            panic!("a run with no sink was accepted")
        };
        assert!(said.contains("--db"), "should say what to name: {}", said);
        assert!(refused(&cli(&["--from", "eddn", "--db"])).is_ok());
        assert!(refused(&cli(&["--from", "eddn", "--index", "d"])).is_ok());
    }

    /// A file that is not there is refused before anything is written
    ///
    /// The run that found this wrote an empty index over a directory that
    /// had a galaxy in it: the dump could not be opened, the source read
    /// nothing, and the worker published the nothing it was handed. So the
    /// check is here, where a refusal costs nothing, and not in the source.
    #[test]
    fn a_source_that_is_not_there_is_refused() {
        let missing =
            cli(&["--from", "spansh=/tmp/no/such/galaxy.json", "--index", "d"]);
        let Err(said) = refused(&missing) else {
            panic!("a dump that does not exist was accepted")
        };
        assert!(
            said.contains("galaxy.json") && said.contains("not there"),
            "should name the path and what is wrong: {}",
            said,
        );

        // The feed and the API name no file, so neither is checked for one.
        assert!(refused(&cli(&["--from", "eddn", "--db"])).is_ok());
        assert!(refused(&cli(&["--from", "edsm-api=Sol", "--db"])).is_ok());

        // A directory that is there is a source, journals being directories.
        let here = cli(&["--from", "journal=/tmp", "--db"]);
        assert!(refused(&here).is_ok(), "a directory should be readable");
    }

    /// The same source twice is one publisher read twice over
    ///
    /// Over EDDN that is two subscriptions carrying the same galaxy and
    /// every message written twice; over a dump it is the same file read
    /// into the same rows. Two *different* journals are the point of the
    /// flag repeating and are let through.
    #[test]
    fn the_same_source_cannot_be_named_twice() {
        let twice = cli(&["--from", "eddn", "--from", "eddn", "--db"]);
        let Err(said) = refused(&twice) else {
            panic!("one feed named twice was accepted")
        };
        assert!(said.contains("twice"), "should say why: {}", said);

        let two =
            cli(&["--from", "journal=src", "--from", "journal=bin", "--db"]);
        assert!(
            refused(&two).is_ok(),
            "two journals are two directories, not one read twice",
        );
    }

    /// A narrowed build is the repair case and nothing else
    ///
    /// `--only` names parts to derive from the database. A run taking
    /// events cannot honour it: the feed reports a scan, the reaches table
    /// is told and the cell tree is not, and the two halves of the
    /// directory stand for different systems from there on.
    #[test]
    fn only_belongs_to_a_one_shot_derive() {
        assert!(
            refused(&cli(&["--db", "--index", "d", "--only", "boosts"]))
                .is_ok(),
            "the repair case is what the flag is for",
        );

        let watched =
            cli(&["--db", "--index", "d", "--watch", "--only", "boosts"]);
        let Err(said) = refused(&watched) else {
            panic!("a narrowed watch was accepted")
        };
        assert!(said.contains("--only"), "should say why: {}", said);

        let fed = cli(&[
            "--from", "eddn", "--db", "--index", "d", "--only", "boosts",
        ]);
        assert!(refused(&fed).is_err(), "a narrowed feed was accepted");
    }

    /// A run with nothing to read and nothing to derive is refused
    ///
    /// `--db` on its own used to be a source. It is not one now: what reads
    /// the database is a derive, and a derive needs somewhere to write.
    #[test]
    fn a_run_must_have_something_to_read_or_derive() {
        assert!(refused(&cli(&["--db"])).is_err());
        assert!(
            refused(&cli(&["--db", "--index", "d"])).is_ok(),
            "the database into an index is the derive-only mode",
        );
        assert!(
            refused(&cli(&["--index", "d"])).is_err(),
            "an index with nothing to write into it",
        );
    }

    /// A resume point is the index's, so a run with no index cannot name one
    #[test]
    fn a_resume_point_needs_an_index_to_resume() {
        assert!(refused(&cli(&[
            "--from",
            "eddn",
            "--db",
            "--checkpoint",
            "somewhere",
        ]))
        .is_err());
    }

    /// `--index` with no directory is the default one
    #[test]
    fn an_index_defaults_to_its_usual_directory() {
        let cli = cli(&["--from", "eddn", "--index"]);
        assert_eq!(cli.index, Some(PathBuf::from(INDEX_DIR)));
    }

    /// A share is a share of a file, so only the sources that read one
    ///
    /// `eddn` is a subscription and `edsm-api` is an answer about one
    /// system's neighbourhood. Neither divides, and a run that took the flag
    /// anyway would be N processes each writing the whole of the same feed.
    #[test]
    fn a_share_belongs_to_a_source_that_reads_a_file() {
        // Paths that are there, `refused` checking for a file it was
        // pointed at; the flag under test is `--shard`.
        for said in [
            &["--from", "spansh=Cargo.toml", "--db", "--shard", "0/8"][..],
            &["--from", "journal=bin", "--db", "--shard", "3/4"][..],
            &["--from", "eddb=README.md", "--db", "--shard", "0/1"][..],
            &["--from", "edsm=Cargo.lock", "--db", "--shard", "1/2"][..],
        ] {
            assert!(refused(&cli(said)).is_ok(), "{:?} reads a file", said);
        }

        let feed = cli(&["--from", "eddn", "--db", "--shard", "0/8"]);
        let Err(said) = refused(&feed) else {
            panic!("a sharded feed was accepted")
        };
        assert!(said.contains("--shard"), "should say why: {}", said);

        let api = cli(&["--from", "edsm-api=Sol", "--db", "--shard", "0/8"]);
        assert!(refused(&api).is_err(), "a sharded API answer was accepted");

        let followed = cli(&[
            "--from",
            "journal=logs",
            "--db",
            "--shard",
            "0/8",
            "--watch",
        ]);
        assert!(refused(&followed).is_err(), "a sharded watch was accepted");
    }

    /// `--bulk` is how a database is opened, so a run must be writing to one
    #[test]
    fn bulk_belongs_to_a_run_that_writes_a_database() {
        assert!(refused(&cli(&["--from", "eddn", "--db", "--bulk"])).is_ok());
        assert!(refused(&cli(&["--from", "eddn", "--index", "d", "--bulk",]))
            .is_err());
    }

    /// The beat belongs to a run something is reading while it writes
    ///
    /// An import holds its tree in memory and writes the directory whole
    /// when it ends, so a publish halfway through is every changed cell and
    /// every table rewritten for a galaxy that is still being read in — and
    /// for a run with no database under it, one nothing can resume from
    /// either.
    #[test]
    fn a_beat_belongs_to_a_run_that_follows_something() {
        let import = cli(&[
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
        assert!(said.contains("--publish"), "should say why: {}", said);

        // The feed and a followed journal are what the beat is for.
        assert!(refused(&cli(&[
            "--from",
            "eddn",
            "--index",
            "d",
            "--publish",
            "2",
        ]))
        .is_ok());
        assert!(refused(&cli(&[
            "--from",
            "journal=bin",
            "--index",
            "d",
            "--watch",
            "--publish",
            "2",
        ]))
        .is_ok());

        // The same journal read once is an import like any other.
        assert!(
            refused(&cli(&[
                "--from",
                "journal=bin",
                "--index",
                "d",
                "--publish",
                "2",
            ]))
            .is_err(),
            "a journal read once has an end",
        );

        // And the database into an index republishes on `--watch`'s beat,
        // never on this one.
        let derive =
            cli(&["--db", "--index", "d", "--watch", "5", "--publish", "2"]);
        assert!(refused(&derive).is_err(), "a derive took a second beat");
    }

    /// `--watch` follows what is still being written, so there must be one
    ///
    /// A dump is all there when the run starts: a run told to watch one
    /// would read it out and exit anyway, which is the flag doing nothing.
    #[test]
    fn a_watch_belongs_to_something_still_being_written() {
        let dump = cli(&["--from", "spansh=Cargo.toml", "--db", "--watch"]);
        let Err(said) = refused(&dump) else {
            panic!("a watched dump was accepted")
        };
        assert!(said.contains("--watch"), "should say why: {}", said);

        assert!(refused(&cli(&["--from", "journal=bin", "--db", "--watch"]))
            .is_ok());
        assert!(
            refused(&cli(&["--db", "--index", "d", "--watch"])).is_ok(),
            "the database into an index follows rows",
        );
        assert!(
            refused(&cli(&["--from", "eddn", "--db", "--watch"])).is_ok(),
            "a subscription follows either way",
        );
    }

    /// The feed's flags belong to a run that reads the feed
    #[test]
    fn the_feeds_flags_need_the_feed() {
        let remote = cli(&[
            "--from",
            "spansh=Cargo.toml",
            "--db",
            "--remote",
            "tcp://x:1",
        ]);
        let Err(said) = refused(&remote) else {
            panic!("an address for a feed nothing reads was accepted")
        };
        assert!(said.contains("--remote"), "should say why: {}", said);

        let stall = cli(&["--from", "journal=bin", "--db", "--stall", "30"]);
        let Err(said) = refused(&stall) else {
            panic!("a stall window for a feed nothing reads was accepted")
        };
        assert!(said.contains("--stall"), "should say why: {}", said);

        assert!(refused(&cli(&[
            "--from",
            "eddn",
            "--db",
            "--remote",
            "tcp://x:1",
            "--stall",
            "30",
        ]))
        .is_ok());
    }

    /// A neighbourhood is what EDSM's API is asked for, so a run must ask it
    #[test]
    fn a_neighbourhood_needs_the_api_to_ask() {
        let cube = cli(&["--from", "edsm=Cargo.lock", "--db", "--cube", "50"]);
        let Err(said) = refused(&cube) else {
            panic!("a cube of a dump was accepted")
        };
        assert!(said.contains("--cube"), "should say why: {}", said);

        let sphere = cli(&["--from", "eddn", "--db", "--sphere", "50"]);
        assert!(refused(&sphere).is_err(), "a sphere of the feed");

        assert!(refused(&cli(&[
            "--from",
            "edsm-api=Sol",
            "--db",
            "--cube",
            "50",
        ]))
        .is_ok());
    }

    /// A commander is whose journal is read, so a run must read one
    #[test]
    fn a_commander_needs_a_journal_to_be_read() {
        let named = cli(&["--from", "eddn", "--db", "--user", "HRC-2"]);
        let Err(said) = refused(&named) else {
            panic!("a commander named over the feed was accepted")
        };
        assert!(said.contains("--user"), "should say why: {}", said);

        assert!(refused(&cli(&[
            "--from",
            "journal=bin",
            "--db",
            "--user",
            "HRC-2",
        ]))
        .is_ok());
    }

    /// The cold route is a finite read into a directory and nothing else
    ///
    /// The four conditions, one at a time. `--db` is the sinks fanning one
    /// reading to two places, which is what they are for; a second `--from`
    /// is two cold builds over one directory, the second publishing over the
    /// first; a feed never ends, so nothing could be published as the whole
    /// galaxy; and a source with no read of its own into a `Build` keeps the
    /// path it had.
    #[test]
    fn a_dump_alone_into_a_directory_takes_the_cold_route() {
        let alone = cli(&["--from", "spansh=Cargo.toml", "--index", "d"]);
        assert_eq!(
            cold_route(&alone),
            Some(&PathBuf::from("Cargo.toml")),
            "a dump with only a directory to write to",
        );

        for said in [
            &["--from", "spansh=Cargo.toml", "--index", "d", "--db"][..],
            &["--from", "spansh=Cargo.toml", "--db"][..],
            &["--from", "spansh=Cargo.toml", "--from", "eddn", "--index", "d"]
                [..],
            &[
                "--from",
                "spansh=Cargo.toml",
                "--from",
                "eddb=README.md",
                "--index",
                "d",
            ][..],
            &["--from", "eddb=README.md", "--index", "d"][..],
            &["--from", "eddn", "--index", "d"][..],
        ] {
            assert_eq!(
                cold_route(&cli(said)),
                None,
                "{:?} is the sinks' run",
                said,
            );
        }
    }

    /// A share of a file is not a share of a directory
    ///
    /// Every process of a sharded run writes the whole index it built from
    /// its own share, so N of them leave the directory holding whichever
    /// finished last rather than the galaxy. With `--db` the shares meet in
    /// Postgres and the flag means what it always did.
    #[test]
    fn a_share_of_a_dump_cannot_build_an_index_alone() {
        let shared = cli(&[
            "--from",
            "spansh=Cargo.toml",
            "--index",
            "d",
            "--shard",
            "0/8",
        ]);
        let Err(said) = refused(&shared) else {
            panic!("a sharded cold build was accepted")
        };
        assert!(said.contains("--shard"), "should say why: {}", said);

        assert!(refused(&cli(&[
            "--from",
            "spansh=Cargo.toml",
            "--index",
            "d",
            "--db",
            "--shard",
            "0/8",
        ]))
        .is_ok());
    }
}
