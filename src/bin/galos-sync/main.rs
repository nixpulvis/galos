//! Moving Elite's galaxy from wherever it is published into wherever it is
//! wanted.
//!
//! Five sources and two sinks. A source knows how to read one publisher —
//! journal files, the EDDN feed, an EDSM dump or API, a saved EDDB dump, or
//! this project's own database — and says what it read through
//! [`sink::Sink`]. A sink knows what that means on its side: rows in Postgres,
//! or a `galos_index` directory a client draws from with no server at all.
//!
//! ```sh
//! galos-sync journal ~/Saved\ Games/…/Elite\ Dangerous          # to the database
//! galos-sync journal ~/Saved\ Games/…/Elite\ Dangerous --watch  # and keep following it
//! galos-sync journal ~/… --to index=.galos_journal_index        # to a directory instead
//! galos-sync eddn --to index=.galos_index                       # a live map, no database
//! galos-sync eddn --to db --to index=.galos_index               # one read, both of them
//! galos-sync db --watch 5                                       # the database into an index
//! ```
//!
//! `--to` repeats, and a source reads once into everything it names. Over
//! EDDN that is the difference between one subscription and two carrying
//! the same galaxy.
//!
//! The one thing worth knowing before reading any of it: **the sinks are not
//! interchangeable and are not meant to be.** A database keeps stations,
//! markets, signals and factions; an index keeps the sky and what is inside a
//! system. A source hands over everything it read and each sink takes what it
//! is for, saying in its own impl what it does with the rest and why. See
//! [`sink`].
//!
//! `db` as a *source* is the one that is not like the others: it reads rows
//! rather than events, and its only sink is an index. It is here because it
//! is the same sentence as the rest — move what is in one place into another
//! — and because a reader looking for "how do I get an index" should find
//! every answer in one program. What it runs is `galos_db::index`, unchanged.

use clap::{Parser, Subcommand, ValueEnum};
use galos_db::index::Parts;
use galos_db::{index, Database};
use sink::{Db, Fan, Index, Sink, To};
use std::io::{stderr, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tracing::info;

mod bar;
mod eddb;
mod eddn;
mod edsm;
mod journal;
mod sink;

/// Sync Elite's galaxy from a publisher into a database or an index.
#[derive(Parser)]
#[command(name = "galos-sync", version, about)]
struct Cli {
    #[command(subcommand)]
    source: Source,
}

#[derive(Subcommand)]
enum Source {
    /// Import local journal files, and optionally keep following them.
    Journal(journal::Cli),
    /// Subscribe to EDDN and sync from incoming events until killed.
    Eddn(eddn::Cli),
    /// Sync from EDSM's nightly dumps or its web API.
    Edsm(edsm::Cli),
    /// Sync from a saved EDDB dump; EDDB itself is gone.
    Eddb(eddb::Cli),
    /// Build the galaxy index from this project's own database.
    ///
    /// The one source that reads rows rather than events, and the one whose
    /// only sink is an index. It is what `galos-db index` was, under a name that says
    /// which direction it runs in.
    Db(DbSource),
}

/// The database read into an index directory.
#[derive(Parser)]
pub struct DbSource {
    /// Directory to write the index into.
    #[arg(long = "to", value_name = "SINK", default_value = "index")]
    to: To,
    /// Resume file for --watch, kept outside the served index directory.
    /// `DIR.checkpoint` beside the index directory by default.
    #[arg(long, value_name = "FILE")]
    checkpoint: Option<PathBuf>,
    /// Follow the feed, republishing every SECS seconds rather than exiting.
    #[arg(long, value_name = "SECS", num_args = 0..=1, default_missing_value = "5")]
    watch: Option<u64>,
    /// Rebuild only these parts, leaving the rest of the directory as it
    /// stands. Every part by default.
    #[arg(
        long,
        value_name = "PART",
        value_delimiter = ',',
        num_args = 1..,
        conflicts_with = "watch"
    )]
    only: Vec<Part>,
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

impl Source {
    /// Which sinks this source was told to write to.
    ///
    /// Nothing named is the database, which is what every invocation of
    /// this program older than `--to` meant and still means.
    fn to(&self) -> Vec<To> {
        let named: &[To] = match self {
            Source::Journal(cli) => &cli.to,
            Source::Eddn(cli) => &cli.to,
            Source::Edsm(cli) => cli.to(),
            Source::Eddb(cli) => &cli.to,
            Source::Db(cli) => std::slice::from_ref(&cli.to),
        };
        if named.is_empty() {
            vec![To::Db]
        } else {
            named.to_vec()
        }
    }

    /// What `--checkpoint` named, where anything did.
    ///
    /// Where nothing did, the resume point is derived from the directory
    /// being written: see [`To::checkpoint`].
    fn checkpoint(&self) -> Option<&std::path::Path> {
        match self {
            Source::Journal(cli) => cli.checkpoint.as_deref(),
            Source::Eddn(cli) => cli.checkpoint.as_deref(),
            Source::Edsm(cli) => cli.checkpoint(),
            Source::Eddb(cli) => cli.checkpoint.as_deref(),
            Source::Db(cli) => cli.checkpoint.as_deref(),
        }
    }

    /// Read everything, saying whether all of it could be read.
    ///
    /// The status is the whole of what cron reads, so a run that lost a
    /// journal to the filesystem must not look like one with nothing left to
    /// do. What could be read is written either way.
    async fn read(&self, sink: &mut dyn Sink) -> bool {
        match self {
            Source::Journal(cli) => cli.read(sink).await,
            Source::Eddn(cli) => cli.read(sink).await,
            Source::Edsm(cli) => cli.read(sink).await,
            Source::Eddb(cli) => cli.read(sink).await,
            // Handled before a sink is opened: it is not a `Sink` client.
            Source::Db(_) => true,
        }
    }
}

#[async_std::main]
async fn main() -> ExitCode {
    // Nothing a crate traces goes anywhere until something is listening for
    // it, dependencies included. `RUST_LOG` picks what to hear, and info
    // upwards from everything without it.
    tracing_subscriber::fmt()
        // Above whatever bar is drawing, so the bar keeps the bottom line
        // and the log does not land on top of it.
        .with_writer(bar::Log)
        // Color is for a terminal. Redirected, it would be escape codes
        // around every line of the log.
        .with_ansi(stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    match run(Cli::parse().source).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(said) => {
            eprintln!("{said}");
            ExitCode::FAILURE
        }
    }
}

/// Open the sinks the source names, read into them, and close them out.
///
/// The database is opened only where a database is one of the things being
/// written to. That is the point of the whole arrangement: `--to index`
/// runs on a machine with no `DATABASE_URL` and no Postgres installed, and
/// a connection opened unconditionally here would have made it need one.
///
/// `--to db --to index=DIR` opens both and reads once into the pair, which
/// is the reason `--to` repeats: over EDDN the alternative is two
/// subscriptions carrying the same galaxy twice.
async fn run(source: Source) -> Result<bool, String> {
    // The database as a source is its own path: it reads rows rather than
    // events, so there is no `Sink` in it. `galos_db::index` is what it runs,
    // and this is only where the arguments are unpacked.
    if let Source::Db(cli) = &source {
        return index_from_database(cli).await.map(|()| true);
    }

    let named = source.to();
    each_its_own(&named, source.checkpoint())?;

    // Declared before the sinks so that it outlives them: `Db` borrows the
    // pool, and locals drop in reverse.
    let db = match named.contains(&To::Db) {
        true => Some(
            Database::new()
                .await
                .map_err(|err| format!("no database: {err}"))?,
        ),
        false => None,
    };

    let mut sinks: Vec<Box<dyn Sink + '_>> = Vec::with_capacity(named.len());
    for to in &named {
        sinks.push(match to {
            To::Db => Box::new(Db::new(db.as_ref().expect("a database"))),
            To::Index(dir) => {
                let checkpoint = To::checkpoint(dir, source.checkpoint());
                Box::new(Index::open(dir, &checkpoint)?)
            }
        });
    }

    let mut fan = Fan::of(sinks);
    let read = source.read(&mut fan).await;
    // Each sink decides what finishing means for it: nothing for a
    // database, which wrote every message as it arrived, and every part of
    // the directory written whole for an index, since a run that only ever
    // published deltas leaves behind the tables it never happened to move.
    // What reaches this is a one-shot run -- an import, a dump, a build --
    // a follower never returning to be closed out.
    fan.finish().await?;
    for said in fan.said_each() {
        info!("{said}");
    }
    Ok(read)
}

/// Refuse the ways two sinks would write over each other.
///
/// Two of the same sink is the whole of it. Two `Db` write every message
/// twice into one pool; two of the same directory each hold their own tree
/// of it and publish over one another, which nothing downstream can notice
/// and no resume point can repair. `--checkpoint` is the same hazard said
/// differently: it names one file, and two index sinks sharing a resume
/// point is a directory rebuilt from another directory's systems.
fn each_its_own(
    named: &[To],
    checkpoint: Option<&std::path::Path>,
) -> Result<(), String> {
    for (at, to) in named.iter().enumerate() {
        if named[..at].contains(to) {
            return Err(format!(
                "{to} was named twice, and one run cannot \
                                write it as two sinks"
            ));
        }
    }

    let indexes = named.iter().filter(|to| matches!(to, To::Index(_))).count();
    if indexes > 1 && checkpoint.is_some() {
        return Err("--checkpoint names one resume point and this run \
                    writes several index directories, which cannot share \
                    one: leave it off and each resumes from its own \
                    DIR.checkpoint"
            .to_string());
    }
    Ok(())
}

/// What `galos-db index` was, under the name that says which way it runs.
async fn index_from_database(cli: &DbSource) -> Result<(), String> {
    let To::Index(dir) = &cli.to else {
        return Err("the database cannot be its own sink; \
                    `galos-sync db` writes an index"
            .to_string());
    };
    let db =
        Database::new().await.map_err(|err| format!("no database: {err}"))?;

    match cli.watch {
        Some(secs) => {
            let checkpoint = To::checkpoint(dir, cli.checkpoint.as_deref());
            index::watch(&db, dir, &checkpoint, Duration::from_secs(secs))
                .await
                .map_err(|err| format!("{err}"))
        }
        None => {
            let report = index::build_to_dir(&db, dir, parts_of(&cli.only))
                .await
                .map_err(|err| format!("{err}"))?;
            println!("{report}");
            if !report.is_consistent() {
                eprintln!("warning: system count and placed points differ");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A watch cannot be asked for part of the index
    ///
    /// It keeps every part current by patching what the feed reports, and a
    /// pass that published some of that and not the rest would leave the
    /// directory disagreeing with itself.
    #[test]
    fn a_watch_cannot_be_narrowed_to_a_part() {
        let said = Cli::try_parse_from([
            "galos-sync",
            "db",
            "--watch",
            "5",
            "--only",
            "reaches",
        ]);
        assert!(said.is_err(), "a narrowed watch was accepted");
    }

    /// Every source but `db` defaults to the database
    ///
    /// What this program has always done, and what every invocation of it
    /// that predates the flag expects.
    #[test]
    fn a_source_writes_to_the_database_unless_told_otherwise() {
        let cli = Cli::try_parse_from(["galos-sync", "journal", "."])
            .expect("a journal import should parse");
        assert_eq!(cli.source.to(), vec![To::Db]);
    }

    /// And `db` defaults to an index, being unable to default to itself
    #[test]
    fn the_database_source_writes_an_index() {
        let cli = Cli::try_parse_from(["galos-sync", "db"])
            .expect("a database build should parse");
        assert_eq!(
            cli.source.to(),
            vec![To::Index(PathBuf::from(sink::to::INDEX_DIR))],
        );
    }

    /// `--to` repeats, and the order it was said in is the order written
    #[test]
    fn a_source_writes_to_every_sink_it_was_given() {
        let cli = Cli::try_parse_from([
            "galos-sync",
            "eddn",
            "--to",
            "db",
            "--to",
            "index=live",
        ])
        .expect("two sinks should parse");
        assert_eq!(
            cli.source.to(),
            vec![To::Db, To::Index(PathBuf::from("live"))],
        );
    }

    /// The same sink twice is two writers over one thing
    ///
    /// Two databases write every message twice; two of one directory each
    /// hold their own tree of it and publish over each other, which
    /// nothing downstream can notice.
    #[test]
    fn the_same_sink_cannot_be_named_twice() {
        let twice = [
            To::Index(PathBuf::from("live")),
            To::Index(PathBuf::from("live")),
        ];
        let Err(said) = each_its_own(&twice, None) else {
            panic!("one directory named twice was accepted")
        };
        assert!(said.contains("twice"), "should say why: {}", said);
        assert!(
            each_its_own(&[To::Db, To::Index(PathBuf::from("live"))], None)
                .is_ok(),
            "two different sinks are the point of the flag",
        );
    }

    /// One named resume point cannot answer for two directories
    #[test]
    fn two_indexes_cannot_share_a_resume_point() {
        let both = [
            To::Index(PathBuf::from("live")),
            To::Index(PathBuf::from("mine")),
        ];
        assert!(
            each_its_own(&both, None).is_ok(),
            "each derives its own where none is named",
        );
        assert!(
            each_its_own(&both, Some(std::path::Path::new("one.checkpoint")))
                .is_err(),
            "two directories were let share one resume point",
        );
    }
}
