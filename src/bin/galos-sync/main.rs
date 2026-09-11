//! Moving Elite's galaxy from wherever it is published into wherever it is
//! wanted.
//!
//! Several sources and two sinks, in one process. A source knows how to read
//! one publisher — journal files, the EDDN feed, an EDSM dump or API, a saved
//! EDDB dump — and says what it read through [`sink::Sink`]. A sink knows
//! what that means on its side: rows in Postgres, or a `galos_index`
//! directory a client draws from with no server at all.
//!
//! ```sh
//! galos-sync --from journal=~/Saved\ Games/…            # to the database
//! galos-sync --from journal=~/… --watch --db            # and keep following it
//! galos-sync --from journal=~/… --index .galos_journal  # to a directory instead
//! galos-sync --from eddn --index .galos_index           # a live map, no database
//! galos-sync --from eddn --db --index .galos_index      # one read, both of them
//! galos-sync --db --index .galos_index --watch 5        # the database into an index
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
//! The one thing worth knowing before reading any of it: **the sinks are not
//! interchangeable and are not meant to be.** A database keeps stations,
//! markets, signals and factions; an index keeps the sky and what is inside a
//! system. A source hands over everything it read and each sink takes what it
//! is for, saying in its own impl what it does with the rest and why. See
//! [`sink`].
//!
//! ## What `main` is
//!
//! A supervisor. It installs a SIGINT handler, holds the shutdown token both
//! halves read, and joins them before it answers — so a Ctrl-C ends in the
//! last publish, the whole-directory [`Sink::finish`](sink::Sink::finish) and
//! a resume point, rather than in a killed process and a directory nothing
//! can reopen. The status is `collect_ok && derive_ok`.

use clap::{Parser, ValueEnum};
use from::Source;
use galos_db::index::Parts;
use galos_db::Database;
use sink::index::INDEX_DIR;
use sink::relay::{Dropped, Live};
use sink::{Db, Fan, Index, Relay, Sink};
use std::io::{stderr, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

mod bar;
mod derive;
mod eddb;
mod eddn;
mod edsm;
mod from;
mod journal;
mod sink;

/// How often the index sink is asked to write what it has taken.
///
/// A database has written each message as it arrived and does nothing on a
/// beat. An index has been editing a tree in memory, and this is how often
/// that reaches the disk: a publish rewrites the index file whole, so doing
/// it per message at thirty a second would be thirty rewrites a second to
/// move one system. `--publish` says otherwise.
const PUBLISH_EVERY: u64 = 5;

/// Sync Elite's galaxy from its publishers into a database and an index.
#[derive(Parser)]
#[command(name = "galos-sync", version, about)]
struct Cli {
    /// Where to read from: `eddn`, `journal=PATH`, `edsm=PATH`,
    /// `edsm-api=NAME` or `eddb=PATH`. Repeatable; each is read once.
    #[arg(long = "from", value_name = "SOURCE")]
    from: Vec<Source>,

    /// Write to Postgres, which is what DATABASE_URL names.
    #[arg(long)]
    db: bool,

    /// Write an index directory, `.galos_index` where DIR is left off.
    #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = INDEX_DIR)]
    index: Option<PathBuf>,

    /// Keep following what was named, re-reading every SECS seconds rather
    /// than exiting. A second where SECS is left off.
    ///
    /// The journal poll and the database poll. EDDN is a subscription and
    /// follows either way.
    #[arg(long, value_name = "SECS", num_args = 0..=1, default_missing_value = "1")]
    watch: Option<u64>,

    /// Seconds between index publishes, one at least.
    #[arg(long, value_name = "SECS", default_value_t = PUBLISH_EVERY)]
    publish: u64,

    /// Rebuild only these parts, leaving the rest of the directory as it
    /// stands. Every part by default, and only for a one-shot derive.
    #[arg(long, value_name = "PART", value_delimiter = ',', num_args = 1..)]
    only: Vec<Part>,

    /// Resume file for the index, kept outside the served directory.
    /// `DIR.checkpoint` beside the index directory by default.
    #[arg(long, value_name = "FILE")]
    checkpoint: Option<PathBuf>,

    /// Whose journal this is, overriding what the files say.
    #[arg(short = 'u', long, value_name = "NAME")]
    user: Option<String>,

    /// EDDN's ZMQ address.
    #[arg(short = 'r', long, value_name = "URL", default_value = eddn::URL)]
    remote: String,

    /// Seconds of EDDN silence before the connection is replaced, or 0 to
    /// leave it alone.
    #[arg(long, value_name = "SECS")]
    stall: Option<u64>,

    /// EDSM's API: take everything in a cube this many light years across.
    #[arg(long, short, value_name = "LY", conflicts_with = "sphere")]
    cube: Option<u32>,

    /// EDSM's API: take everything within this many light years.
    #[arg(long, short, value_name = "LY")]
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

/// Whether the run has been asked to stop.
///
/// Read by every loop in the program and set by the SIGINT handler. A flag
/// rather than a channel because that is the whole of what it has to carry:
/// nothing is published through it, so a loop that reads it one pass late
/// does one more pass, which is the point — what must not happen is a loop
/// that never reads it, or a process that dies before its sink is closed.
#[derive(Clone)]
pub struct Shutdown(Arc<AtomicBool>);

impl Shutdown {
    fn new() -> Shutdown {
        Shutdown(Arc::new(AtomicBool::new(false)))
    }

    /// Ask the run to stop at the next place it can.
    ///
    /// The signal handler, and the index worker where it has failed: a
    /// half a run is not a run, and the sources would otherwise read a
    /// feed that never ends into a channel nobody is draining.
    pub fn ask(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether it has been asked.
    pub fn asked(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
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
                .unwrap_or_else(|_| "info".into()),
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
}

impl Options {
    fn of(cli: &Cli) -> Options {
        Options {
            user: cli.user.clone(),
            remote: cli.remote.clone(),
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
    let _lock = match &cli.index {
        Some(dir) => Some(
            galos_index::Lock::take(dir)
                .map_err(|err| format!("{}: {err}", dir.display()))?,
        ),
        None => None,
    };

    // Two pools, not one shared five connections. The collect side writes a
    // row per message and the derive side samples a clock and reads the
    // galaxy back; a `Sink` method cannot report failure, so an acquire that
    // stalls behind an hour-long build is a message silently dropped.
    let collecting = match cli.db {
        true => Some(open().await?),
        false => None,
    };
    let deriving = match cli.db && cli.index.is_some() {
        true => Some(open().await?),
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

    // Its own thread and its own `block_on`: a publish is a whole-galaxy
    // `fs::write` and everything else here is waiting on a socket.
    let deriving = match (&cli.index, &readings) {
        (Some(dir), Some((_, receiver))) => {
            let worker = derive::Derive {
                dir: dir.clone(),
                checkpoint: Index::checkpoint(dir, cli.checkpoint.as_deref()),
                db: deriving,
                publish: Duration::from_secs(cli.publish.max(1)),
                readings: receiver.clone(),
                live: live.clone(),
                dropped: dropped.clone(),
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

/// A pool onto whatever `DATABASE_URL` names.
async fn open() -> Result<Database, String> {
    Database::new().await.map_err(|err| format!("no database: {err}"))
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
            };
            journal.read(&mut fan, &shutdown).await
        }
        Source::Edsm(path) => {
            edsm::Dump { path }.read(&mut fan, &shutdown).await
        }
        Source::EdsmApi(name) => {
            let api =
                edsm::Api { name, cube: options.cube, sphere: options.sphere };
            api.read(&mut fan, &shutdown).await
        }
        Source::Eddb(path) => {
            eddb::Eddb { path }.read(&mut fan, &shutdown).await
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
            cli(&["--from", "journal=one", "--from", "journal=two", "--db"]);
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
}
