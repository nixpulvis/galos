//! Elite's galaxy: filling the two stores, and asking them.
//!
//! ```sh
//! galos ingest --from eddn --db --index      # one read, both stores
//! galos ingest --from spansh=galaxy.json --index   # a galaxy, region by region
//! galos ingest --from database --index --watch 5   # the rows into a directory
//! galos index status -i .index/full       # what a directory holds
//! galos index merge -i .index/full --from .index/caught_up
//! galos db verify                            # what is wrong in there
//! galos db merge --from postgresql://host/caught_up
//! galos search -s 'Sol*'                     # ask about it
//! ```
//!
//! ## One verb writes; two groups are asked
//!
//! [`ingest`] is the only thing here that *fills* anything, and `--db` and
//! `--index` name which stores this run writes. That is one verb because
//! it is one job: a source reads a publisher and hands over what it read,
//! and each sink takes what it is for ([`galos::read`],
//! [`galos::sink`]). Naming both reads each publisher once — over EDDN the
//! difference between one subscription and two carrying the same galaxy,
//! and the run an operator keeping both stores current actually wants.
//!
//! [`index`] and [`db`] are what is asked *of* each store once it is
//! filled, and those are two groups because the stores are not two
//! settings of one store: a database keeps stations, markets, signals and
//! factions, an index keeps the sky, and `status`, `verify` and `migrate`
//! mean different work on each side with no code in common.
//! [`search`] and [`route`] are the questions, which only the database can
//! answer.
//!
//! ## The `db` feature
//!
//! On by default, and off is the point. Built `--no-default-features`,
//! this binary is [`ingest`] `--index` and the [`index`] group: no `sqlx`,
//! no `dotenv`, no `DATABASE_URL`, and no client compiled in. That is what
//! a machine serving the map from a directory wants, since it has no
//! Postgres for a client to open. `--db` and `--from database` are what
//! the feature adds to the writing, and they are what is about the other
//! store.

// `row!`/`table!` for the query verbs' output, which is the only thing
// here that prints a table — and which is not in the build at all without
// a database to draw one from.
#[cfg(feature = "db")]
#[macro_use]
extern crate prettytable;

use clap::{Parser, Subcommand};
use std::io::{stderr, IsTerminal};
use std::process::ExitCode;

mod index;
mod ingest;

#[cfg(feature = "db")]
mod db;
#[cfg(feature = "db")]
mod route;
#[cfg(feature = "db")]
mod search;

/// Elite's galaxy: an index directory, a database, and what they are asked.
#[derive(Parser)]
#[command(name = "galos", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read a publisher into the database, an index directory, or both.
    ///
    /// `--from` names a publisher and repeats; `--db` and `-i/--index`
    /// name where what is read goes, and naming both reads each publisher
    /// once into the pair.
    ///
    /// **Every other flag belongs to one source or one sink, and the
    /// heading it is listed under says which.** A flag the run has no use
    /// for is refused rather than ignored — `--user` over the feed, a
    /// `--shard` of a subscription, a `--publish` beat for a run with an
    /// end — because a run that quietly did something other than what it
    /// was asked is the failure that is hardest to notice.
    Ingest(ingest::Cli),

    /// Inspect and repair an index directory.
    Index(index::Cli),

    /// Work with the galaxy database.
    #[cfg(feature = "db")]
    Db(db::Cli),

    /// Search for systems, bodies, stations and factions.
    #[cfg(feature = "db")]
    Search(search::Cli),

    /// Plot routes between systems.
    #[cfg(feature = "db")]
    Route(route::Cli),
}

/// What `RUST_LOG` falls back to where the verb does not say.
///
/// `galos_db::HEARD` where there is a database in the build — info and
/// above, less the one `~/.pgpass` line every pool opens with — and plain
/// `info` where there is not, there being no chatty dependency to silence
/// once `sqlx` is out of the tree.
#[cfg(feature = "db")]
const HEARD: &str = galos_db::HEARD;
#[cfg(not(feature = "db"))]
const HEARD: &str = "info";

#[async_std::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Nothing a crate traces goes anywhere until something is listening
    // for it, dependencies included. The reporting verbs print; the
    // filling ones trace, and so does everything they call.
    tracing_subscriber::fmt()
        // Above whatever bars are drawing, so they keep the bottom lines
        // and the log does not land on top of them. An ingest of a dump
        // draws one per source.
        .with_writer(galos::bar::Log)
        // Color is for a terminal. Redirected, it would be escape codes
        // around every line of the log.
        .with_ansi(stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| heard(&cli.command).into()),
        )
        .init();

    match cli.command {
        Command::Ingest(it) => match ingest::run(it).await {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(said) => {
                eprintln!("{said}");
                ExitCode::FAILURE
            }
        },
        Command::Index(it) => index::run(it),
        #[cfg(feature = "db")]
        Command::Db(it) => match db::run(it).await {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(said) => {
                eprintln!("{said}");
                ExitCode::FAILURE
            }
        },
        // The questions, which open a pool and print a table. Neither has
        // an exit code of its own to report yet.
        #[cfg(feature = "db")]
        Command::Search(it) => match asked().await {
            Ok(db) => {
                it.run(&db);
                ExitCode::SUCCESS
            }
            Err(said) => {
                eprintln!("{said}");
                ExitCode::FAILURE
            }
        },
        #[cfg(feature = "db")]
        Command::Route(it) => match asked().await {
            Ok(db) => {
                it.run(&db);
                ExitCode::SUCCESS
            }
            Err(said) => {
                eprintln!("{said}");
                ExitCode::FAILURE
            }
        },
    }
}

/// The filter this run listens with, which two verbs have an opinion about.
fn heard(command: &Command) -> String {
    match command {
        #[cfg(feature = "db")]
        Command::Db(it) => it.heard(),
        Command::Ingest(it) => it.heard(),
        _ => HEARD.to_string(),
    }
}

/// A pool for a question, on whatever `DATABASE_URL` names.
///
/// The querying half opens five connections and reads; the writing half
/// has [`db::open`], which takes `--bulk` into account.
#[cfg(feature = "db")]
async fn asked() -> Result<galos_db::Database, String> {
    galos_db::Database::new()
        .await
        .map_err(|err| format!("no database to ask: {err}"))
}
