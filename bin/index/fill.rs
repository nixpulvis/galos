//! The two verbs that put a galaxy in the directory.
//!
//! Both answer "fill this directory", and which one to reach for is a
//! question about memory rather than taste.
//!
//! [`ingest`] holds a live tree and the whole names table — about a
//! kilobyte a system — and publishes on a beat, because something may be
//! reading the directory while it is written to. That is what a feed, a
//! journal and a small dump want.
//!
//! [`build`] holds one region at a time, spilling every system to its
//! bucket as the source pushes it, and publishes nothing until the whole
//! directory is written. It is the only way a two hundred million system
//! dump fits in memory at all, and it is what `--from database` runs as
//! well — a derive from rows is a whole-directory build with a cursor on
//! the end of it.
//!
//! The old tool chose between them by inspecting the flags: one finite
//! source, an index, no database, nothing following. That heuristic is
//! gone, and the operator names the route. What it cost to guess was a
//! galaxy-sized import silently taking the route that cannot hold one.

use clap::Args;
use galos::read::from::{self, Source};
use galos::read::{cold, derive, Qualifiers, PUBLISH_EVERY};
use galos::sink::index::INDEX_DIR;
use galos::sink::relay::{Dropped, Live};
use galos::sink::{Index, Relay, Sink};
use galos::{Shard, Shutdown};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;
use tracing::error;

/// What this tool calls itself in a spool's `cursors/` directory.
///
/// Its own name, so an index following a recorded feed and a database
/// following the same one keep separate places in it and neither can move
/// the other's.
const CONSUMER: &str = "galos-index";

/// The heading the flags of a run that never ends are listed under.
const FOLLOWING: &str = "Following what is published";

/// The heading the flags of a run that reads something finite are listed
/// under.
const IMPORTING: &str = "Importing what is already there";

/// Read a publisher into the directory, following it where it does not end.
#[derive(Args)]
pub struct Ingest {
    /// Where to read from: `eddn`, `spool=DIR`, `journal=PATH`,
    /// `edsm=PATH`, `edsm-api=NAME`, `eddb=PATH` or `spansh=PATH`.
    /// Repeatable; each is read once.
    ///
    /// A spool is a recorded feed — see `eddn record`. It is followed
    /// from this tool's own cursor unless it is told otherwise:
    /// `spool=DIR,from=earliest` replays what is held, and
    /// `spool=DIR,since=2026-09-19T12:00:00Z` starts at an hour.
    ///
    /// A galaxy-sized `spansh=PATH` belongs to `build`: this route holds a
    /// tree of every system it has read.
    #[arg(long = "from", value_name = "SOURCE", required = true)]
    from: Vec<Source>,

    /// The index directory to write.
    #[arg(long, short, value_name = "DIR", default_value = INDEX_DIR)]
    dir: PathBuf,

    /// Resume file, kept outside the served directory.
    /// `DIR.checkpoint` beside the index directory by default.
    #[arg(long, value_name = "FILE")]
    checkpoint: Option<PathBuf>,

    /// Whose journal this is, overriding what the files say. Only with
    /// `--from journal=PATH`.
    #[arg(short = 'u', long, value_name = "NAME")]
    user: Option<String>,

    /// Bring the directory level with the database before taking live
    /// readings, repeating until the feed is no longer outrunning it.
    ///
    /// **The one thing here that reads Postgres.** Without it an ingest
    /// into an empty directory serves only what the feed has mentioned
    /// since the run started; with it the directory starts as the whole
    /// galaxy the rows hold and the feed carries it forward, with no gap
    /// between the two — what arrives during the catch-up is buffered,
    /// and a buffer that overflowed is thrown away and read back from the
    /// rows rather than drained with a hole in it. See
    /// `galos::read::derive`.
    ///
    /// The alternative is `build --from database` and then an ingest,
    /// which loses whatever was written between the build's cursor and
    /// the first message the feed happens to mention.
    #[cfg(feature = "db")]
    #[arg(long)]
    catch_up: bool,

    /// Replace a directory that already serves systems, where the
    /// catch-up cannot resume what is there.
    ///
    /// A catch-up rebuilds the whole directory when its resume point is
    /// not one of the database's — a directory built from a dump, or from
    /// the feed, whose systems it has no other way to carry forward. That
    /// replaces every system it publishes, so it is refused unless it is
    /// asked for here. `galos-index status DIR` says what wrote the one
    /// you have.
    #[cfg(feature = "db")]
    #[arg(long, requires = "catch_up")]
    rebuild: bool,

    /// Keep following what was named rather than exiting, SECS apart. A
    /// second where SECS is left off.
    ///
    /// The longest a journal directory nothing is writing to is left
    /// alone: the filesystem says when a log was written to, so a journal
    /// is read as soon as the game writes rather than SECS later. EDDN is
    /// a subscription and follows either way.
    ///
    /// Only where there is something to follow. A dump has an end.
    #[arg(
        long,
        value_name = "SECS",
        num_args = 0..=1,
        default_missing_value = "1",
        help_heading = FOLLOWING,
    )]
    watch: Option<u64>,

    /// Seconds between publishes, one at least, five by default. Only for
    /// a run that follows something; a run with an end publishes once,
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

    /// Read one share of a source: every Nth record of a dump, or every
    /// Nth file of a journal directory, offset I, so N processes cover it
    /// exactly once between them. A journal shards by file because a file
    /// is what names the commander who flew it.
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

/// Derive the whole directory from one finite source.
#[derive(Args)]
pub struct Build {
    /// What to derive it from: `database`, or `spansh=PATH`.
    ///
    /// One source and not several: two of them into one directory is two
    /// builds over the same place, the second publishing its own galaxy
    /// over the first.
    #[arg(long = "from", value_name = "SOURCE")]
    from: Whence,

    /// The index directory to write.
    #[arg(long, short, value_name = "DIR", default_value = INDEX_DIR)]
    dir: PathBuf,

    /// Resume file, kept outside the served directory.
    /// `DIR.checkpoint` beside the index directory by default.
    #[arg(long, value_name = "FILE")]
    checkpoint: Option<PathBuf>,

    /// Rebuild only these parts, leaving the rest of the directory as it
    /// stands. Every part by default. Only `--from database`.
    #[cfg(feature = "db")]
    #[arg(long, value_name = "PART", value_delimiter = ',', num_args = 1..)]
    only: Vec<Part>,

    /// Replace a directory that already serves systems, where this run
    /// cannot resume what is there.
    ///
    /// A derive from the database rebuilds the whole directory when its
    /// resume point is not one of the database's — a directory built from
    /// a dump, or from the feed, whose systems this has no other way to
    /// carry forward. That replaces every system it publishes, so it is
    /// refused unless it is asked for here.
    #[cfg(feature = "db")]
    #[arg(long)]
    rebuild: bool,

    /// Keep following the rows rather than exiting, SECS apart. A second
    /// where SECS is left off. Only `--from database`.
    #[cfg(feature = "db")]
    #[arg(
        long,
        value_name = "SECS",
        num_args = 0..=1,
        default_missing_value = "1",
        help_heading = FOLLOWING,
    )]
    watch: Option<u64>,
}

/// Where a build derives a directory from.
///
/// The publishers a [`Source`] names are files and feeds; the database is
/// the other store, and deriving one from the other is a different
/// sentence — it reads rows, it carries a cursor, and it is the one thing
/// here that needs a database at all. So it is a word this verb
/// understands rather than a variant in the grammar both tools share, and
/// a build with the `db` feature off refuses it by name instead of
/// offering a source it cannot read.
#[derive(Clone, Debug)]
enum Whence {
    /// The rows, which is `galos-db`'s store read back out.
    #[cfg(feature = "db")]
    Database,
    /// A publisher, of which one has a build of its own: `spansh=PATH`.
    Published(Source),
}

impl std::str::FromStr for Whence {
    type Err = String;

    fn from_str(said: &str) -> Result<Whence, String> {
        if said.eq_ignore_ascii_case("database") {
            #[cfg(feature = "db")]
            return Ok(Whence::Database);
            #[cfg(not(feature = "db"))]
            return Err(
                "this build has no database in it; rebuild with the `db` \
                 feature, or name a dump"
                    .to_string(),
            );
        }
        // The source grammar's own refusal lists the publishers and knows
        // nothing of `database`, which is this verb's word. So the list a
        // mistyped `--from` is answered with is completed here rather
        // than a reader being told about six of the seven things they
        // could have meant.
        said.parse().map(Whence::Published).map_err(|said: String| {
            match cfg!(feature = "db") {
                true => format!("{said}, or `database`"),
                false => said,
            }
        })
    }
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

/// Read every named source into the directory, and close it out.
///
/// The sources run at once, each with a [`Relay`] of its own onto one
/// channel, and one worker thread drains that channel into the live tree.
/// Its own thread because a publish is a whole-galaxy `fs::write` and
/// everything else here is waiting on a socket.
pub async fn ingest(it: Ingest, forced: bool) -> ExitCode {
    // Refused before anything is installed, locked or opened: a run that
    // cannot mean anything should leave the directory and the terminal
    // exactly as it found them.
    if let Err(said) = refused(&it) {
        eprintln!("{said}");
        return ExitCode::FAILURE;
    }

    let shutdown = Shutdown::new();
    galos::shutdown::on_interrupt(shutdown.clone());

    // One writer per directory for the length of the run. Two would each
    // hold their own tree of it and publish over one another, which
    // nothing downstream can notice and no resume point can repair.
    let _lock = match held(&it.dir, forced) {
        Ok(lock) => lock,
        Err(said) => {
            eprintln!("{said}");
            return ExitCode::FAILURE;
        }
    };

    // The beat belongs to a run that follows something. An import ends,
    // and what it ends with is the whole directory `finish` writes;
    // publishing it half-read is a rewrite of every changed cell and
    // every table for a galaxy nobody can read yet.
    let publish = qualifying(&it).following().then(|| {
        let secs = it.publish.unwrap_or(PUBLISH_EVERY).max(1);
        Duration::from_secs(secs)
    });

    // The database, where the run asked to be brought level with it
    // first, and no connection at all otherwise — which is every other
    // run of this verb, and the reason the tool builds without a
    // database client in it.
    #[cfg(feature = "db")]
    let db = match it.catch_up {
        false => None,
        true => match galos_db::Database::new().await {
            Ok(db) => Some(db),
            Err(err) => {
                eprintln!("no database to catch up from: {err}");
                return ExitCode::FAILURE;
            }
        },
    };

    let (live, dropped) = (Live::new(), Dropped::new());
    let (sender, receiver) = Relay::channel();
    let worker = derive::Derive {
        dir: it.dir.clone(),
        checkpoint: Index::checkpoint(&it.dir, it.checkpoint.as_deref()),
        #[cfg(feature = "db")]
        db,
        publish,
        readings: receiver,
        live: live.clone(),
        dropped: dropped.clone(),
        #[cfg(feature = "db")]
        rebuild: it.rebuild,
        shutdown: shutdown.clone(),
    };
    let deriving = match std::thread::Builder::new()
        .name("index".to_string())
        .spawn(move || async_std::task::block_on(worker.run()))
    {
        Ok(thread) => thread,
        Err(err) => {
            eprintln!("no thread for the index: {err}");
            return ExitCode::FAILURE;
        }
    };

    let options = qualifying(&it).options(CONSUMER);

    let mut reading = Vec::with_capacity(it.from.len());
    for source in it.from {
        let relay = Relay::new(sender.clone(), live.clone(), dropped.clone());
        let sinks: Vec<Box<dyn Sink>> = vec![Box::new(relay)];
        let (options, shutdown) = (options.clone(), shutdown.clone());
        reading.push(async_std::task::spawn(async move {
            galos::read::collect(
                source,
                options,
                galos::sink::Fan::of(sinks),
                shutdown,
            )
            .await
        }));
    }
    // This run's own copy of the sender, which would otherwise hold the
    // channel open after every source had finished and leave the worker
    // waiting for a reading nobody is going to send.
    drop(sender);

    let mut collected = true;
    for task in reading {
        collected &= task.await;
    }

    // And only now the exit code. The worker is still publishing what the
    // sources handed it, and it has a whole directory and a resume point
    // to write after that.
    let derived = match deriving.join() {
        Ok(Ok(())) => true,
        Ok(Err(said)) => {
            eprintln!("{said}");
            false
        }
        Err(_) => {
            eprintln!("the index worker panicked");
            false
        }
    };

    match collected && derived {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    }
}

/// Derive the whole directory, publishing nothing until it is built.
pub async fn build(it: Build, forced: bool) -> ExitCode {
    let shutdown = Shutdown::new();
    galos::shutdown::on_interrupt(shutdown.clone());

    if let Err(said) = refused_build(&it) {
        eprintln!("{said}");
        return ExitCode::FAILURE;
    }

    let _lock = match held(&it.dir, forced) {
        Ok(lock) => lock,
        Err(said) => {
            eprintln!("{said}");
            return ExitCode::FAILURE;
        }
    };
    let checkpoint = Index::checkpoint(&it.dir, it.checkpoint.as_deref());

    let built = match it.from {
        Whence::Published(Source::Spansh(path)) => {
            let mut source = galos::read::spansh::Galaxy {
                path,
                dir: it.dir.clone(),
                now: chrono::Utc::now(),
                shutdown: shutdown.clone(),
            };
            cold::cold(&mut source, &it.dir, &checkpoint)
        }
        // `edsm=PATH` and `eddb=PATH` are finite files that want exactly
        // this treatment; what they lack is a read of their own into a
        // `Build`, and until one lands they are an ingest and still work.
        Whence::Published(other) => Err(format!(
            "there is no whole-directory build of {other}; read it with \
             `ingest --from` instead"
        )),
        #[cfg(feature = "db")]
        Whence::Database => {
            let db = match galos_db::Database::new().await {
                Ok(db) => db,
                Err(err) => {
                    eprintln!("no database to build from: {err}");
                    return ExitCode::FAILURE;
                }
            };
            derive::from_database(
                &db,
                &it.dir,
                &checkpoint,
                parts_of(&it.only),
                it.watch.map(Duration::from_secs),
                it.rebuild,
                &shutdown,
            )
            .await
            .map(|()| true)
        }
    };

    match built {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(said) => {
            error!("{said}");
            ExitCode::FAILURE
        }
    }
}

/// Take the directory for the length of the run, or say why not.
///
/// `--force-lock` clears one a killed run left behind, which is the only
/// way out of it: the file names a pid that is gone and nothing else
/// removes it. The judgement stays with whoever passed the flag, which is
/// why it is a flag and not a retry.
fn held(dir: &Path, forced: bool) -> Result<galos_index::Lock, String> {
    match forced {
        true => galos_index::Lock::force(dir),
        false => galos_index::Lock::take(dir),
    }
    .map_err(|err| format!("{}: {err}", dir.display()))
}

/// The per-source flags, as [`galos::read::Qualifiers`] reads them.
///
/// Both tools' `ingest` takes these same eight flags and the rules about
/// them are written once, over there. What is here is the one rule that is
/// this tool's own.
fn qualifying(it: &Ingest) -> Qualifiers<'_> {
    Qualifiers {
        sources: &it.from,
        user: it.user.as_deref(),
        remote: it.remote.as_deref(),
        stall: it.stall,
        watch: it.watch,
        cube: it.cube,
        sphere: it.sphere,
        shard: it.shard,
    }
}

/// Refuse what the shared rules do, and then the beat.
///
/// `--publish` is the index's alone: there is no beat on the other side,
/// a database having written each message as it arrived. An import holds
/// its tree in memory and writes the directory whole when it ends, so a
/// publish halfway through is every changed cell and every table
/// rewritten for a galaxy that is still being read in — and one nothing
/// can resume from either, a dump's directory carrying no cursor.
fn refused(it: &Ingest) -> Result<(), String> {
    let qualifiers = qualifying(it);
    qualifiers.refused()?;
    if it.publish.is_some() && !qualifiers.following() {
        return Err(
            "--publish is the beat of a run that follows something; this run \
             has an end, and publishes once when it reaches it"
                .into(),
        );
    }
    Ok(())
}

/// Refuse the builds that would write something other than what was
/// asked.
///
/// Two, both about `--from database`. A dump that is not there is the
/// third, and it matters most: the run that found it wrote an empty index
/// over a directory that had a galaxy in it — the dump could not be
/// opened, the source read nothing, and the build published the nothing
/// it was handed.
fn refused_build(it: &Build) -> Result<(), String> {
    // Through the shared rules, so a missing dump is refused in the one
    // wording both tools use — and, being a list of one, so is a second
    // `--from`, which clap has already refused by then.
    match &it.from {
        Whence::Published(source) => {
            let sources = std::slice::from_ref(source);
            Qualifiers {
                sources,
                user: None,
                remote: None,
                stall: None,
                watch: None,
                cube: None,
                sphere: None,
                shard: None,
            }
            .refused()?
        }
        #[cfg(feature = "db")]
        Whence::Database => {}
    }

    // `--only` names parts to derive from the rows, which is the repair
    // case: a reach table stale by a change to how a reach is worked out
    // is worth rewriting on its own, and the hundred megabytes of names
    // beside it are not. A *follower* cannot honour it — each pass would
    // move one table and leave the rest standing for an older galaxy, and
    // the resume point it wrote would claim otherwise.
    #[cfg(feature = "db")]
    if !it.only.is_empty() {
        if it.watch.is_some() {
            return Err(
                "--only is the repair case, and --watch keeps repairing one \
                 part while the rest of the directory ages behind it"
                    .into(),
            );
        }
        if !matches!(it.from, Whence::Database) {
            return Err(
                "--only names the parts to derive from the rows; a dump is \
                 read once and written whole"
                    .into(),
            );
        }
    }
    #[cfg(feature = "db")]
    if it.watch.is_some() && !matches!(it.from, Whence::Database) {
        return Err(
            "--watch follows the rows as they change; a dump has an end".into(),
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
    /// Through a wrapper rather than `Ingest::try_parse_from`: `Args` is
    /// not a parser on its own, and what is under test is the flags as the
    /// verb actually receives them.
    #[derive(Parser)]
    struct Ingesting {
        #[command(flatten)]
        it: Ingest,
    }

    fn ingesting(said: &[&str]) -> Ingest {
        let mut argv = vec!["galos-index"];
        argv.extend_from_slice(said);
        Ingesting::try_parse_from(argv).expect("these flags should parse").it
    }

    #[derive(Parser)]
    struct Building {
        #[command(flatten)]
        it: Build,
    }

    fn building(said: &[&str]) -> Build {
        let mut argv = vec!["galos-index"];
        argv.extend_from_slice(said);
        Building::try_parse_from(argv).expect("these flags should parse").it
    }

    /// Naming nothing builds the whole index
    ///
    /// The flag is what a build is narrowed by, so its absence has to
    /// leave the build exactly as wide as it always was.
    #[cfg(feature = "db")]
    #[test]
    fn a_build_naming_no_part_builds_them_all() {
        assert_eq!(parts_of(&[]), galos_db::index::Parts::ALL);
    }

    /// And naming one builds that one and no other
    ///
    /// The point of the flag: a reach table stale by a change to how a
    /// reach is worked out is worth rewriting on its own, and the hundred
    /// megabytes of names beside it are not.
    #[cfg(feature = "db")]
    #[test]
    fn a_build_naming_a_part_builds_only_that_part() {
        use galos_db::index::Parts;
        assert_eq!(
            parts_of(&[Part::Reaches]),
            Parts { reaches: true, ..Parts::NONE }
        );
        assert_eq!(
            parts_of(&[Part::Reaches, Part::Bodies]),
            Parts { reaches: true, bodies: true, ..Parts::NONE }
        );
    }

    /// A dump that is not there is refused before the directory is touched
    ///
    /// `build` has a path check of its own because it takes one source
    /// rather than a list, and it is where the check matters most: the run
    /// that found this wrote an empty index over a directory that had a
    /// galaxy in it — the dump could not be opened, the source read
    /// nothing, and the build published the nothing it was handed.
    #[test]
    fn a_dump_that_is_not_there_is_refused() {
        let Err(said) = refused_build(&building(&[
            "--from",
            "spansh=/tmp/no/such/galaxy.json",
        ])) else {
            panic!("a build of a dump that does not exist was accepted")
        };
        assert!(
            said.contains("galaxy.json") && said.contains("not there"),
            "should name the path and what is wrong: {said}",
        );
    }

    /// The shared rules are reached from this verb's flags
    ///
    /// One test rather than one per rule: what each rule *means* is
    /// pinned in `galos::read`, where it is written, and what is this
    /// tool's business is that every qualifier on the command line
    /// arrives there. A flag left out of [`qualifying`] would be a flag
    /// silently ignored, which is the failure this catches.
    #[test]
    fn the_shared_rules_are_reached_from_here() {
        let named = ingesting(&["--from", "eddn", "--user", "HRC-2"]);
        let Err(said) = refused(&named) else {
            panic!("a commander named over the feed was accepted")
        };
        assert!(said.contains("--user"), "should say why: {said}");

        let sharded = ingesting(&["--from", "eddn", "--shard", "0/8"]);
        assert!(refused(&sharded).is_err(), "a sharded feed was accepted");

        let sound = ingesting(&["--from", "journal=bin", "--user", "HRC-2"]);
        assert!(refused(&sound).is_ok(), "{:?}", refused(&sound));
    }

    /// A narrowed build is the repair case and nothing else
    ///
    /// Each pass of a follower would move one table and leave the rest
    /// standing for an older galaxy, with a resume point claiming
    /// otherwise.
    #[cfg(feature = "db")]
    #[test]
    fn only_belongs_to_a_one_shot_derive() {
        assert!(
            refused_build(&building(&[
                "--from", "database", "--only", "boosts",
            ]))
            .is_ok(),
            "the repair case is what the flag is for",
        );

        let watched =
            building(&["--from", "database", "--watch", "--only", "boosts"]);
        let Err(said) = refused_build(&watched) else {
            panic!("a narrowed watch was accepted")
        };
        assert!(said.contains("--only"), "should say why: {}", said);

        let dump =
            building(&["--from", "spansh=Cargo.toml", "--only", "boosts"]);
        assert!(
            refused_build(&dump).is_err(),
            "a dump is read once and written whole",
        );
    }

    /// The directory a run writes, where it did not say
    #[test]
    fn an_index_defaults_to_its_usual_directory() {
        assert_eq!(
            ingesting(&["--from", "eddn"]).dir,
            PathBuf::from(INDEX_DIR)
        );
        assert_eq!(
            building(&["--from", "spansh=Cargo.toml"]).dir,
            PathBuf::from(INDEX_DIR)
        );
    }

    /// The beat belongs to a run something is reading while it writes
    ///
    /// An import holds its tree in memory and writes the directory whole
    /// when it ends, so a publish halfway through is every changed cell
    /// and every table rewritten for a galaxy that is still being read in
    /// — and one nothing can resume from either, a dump's directory
    /// carrying no cursor.
    #[test]
    fn a_beat_belongs_to_a_run_that_follows_something() {
        let import =
            ingesting(&["--from", "spansh=Cargo.toml", "--publish", "2"]);
        let Err(said) = refused(&import) else {
            panic!("an import on a beat was accepted")
        };
        assert!(said.contains("--publish"), "should say why: {}", said);

        // The feed and a followed journal are what the beat is for.
        assert!(
            refused(&ingesting(&["--from", "eddn", "--publish", "2"])).is_ok()
        );
        assert!(refused(&ingesting(&[
            "--from",
            "journal=bin",
            "--watch",
            "--publish",
            "2",
        ]))
        .is_ok());

        // The same journal read once is an import like any other.
        assert!(
            refused(&ingesting(&["--from", "journal=bin", "--publish", "2",]))
                .is_err(),
            "a journal read once has an end",
        );
    }

    /// The database is a word this verb knows, and a dump is a source
    ///
    /// The two routes a build has, told apart by what was named rather
    /// than by inspecting four flags the way the old tool did.
    #[test]
    fn a_build_names_either_the_rows_or_a_dump() {
        assert!(matches!(
            building(&["--from", "spansh=Cargo.toml"]).from,
            Whence::Published(Source::Spansh(_)),
        ));
        #[cfg(feature = "db")]
        assert!(matches!(
            building(&["--from", "database"]).from,
            Whence::Database,
        ));
        #[cfg(not(feature = "db"))]
        assert!(
            Building::try_parse_from(["galos-index", "--from", "database"])
                .is_err(),
            "a build with no database in it should refuse the word",
        );
    }
}
