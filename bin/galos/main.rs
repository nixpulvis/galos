//! Elite's galaxy: the two stores, and the questions asked of them.
//!
//! ```sh
//! galos index ingest --from eddn --dir .galos_index   # fill a directory
//! galos db ingest --from eddn                         # fill the database
//! galos index status .index/full                      # what a directory holds
//! galos db verify                                     # what is wrong in there
//! galos search -s 'Sol*'                              # ask about it
//! ```
//!
//! ## One command, three groups of verbs
//!
//! [`index`] is everything done to an index directory and [`db`] is
//! everything done to the database; [`search`] and [`route`] are the
//! questions, which only the database can answer. The groups are separate
//! because the two stores are not two settings of one store — a database
//! keeps stations, markets, signals and factions, an index keeps the sky —
//! and they are one *program* because filling either one is the same
//! reading of the same publishers, through [`galos::read`], and because a
//! reader should not have to know which of two binaries a verb lives in.
//!
//! What was two flags is now the verb's own name. `--db` and `--index DIR`
//! took a dozen refusals between them to say which combinations meant
//! anything; `galos index ingest` and `galos db ingest` need none, and
//! filling both stores is the two commands run side by side, each with its
//! own subscription.
//!
//! ## The `db` feature
//!
//! On by default, and off is the point. Built `--no-default-features`, this
//! binary is the [`index`] group alone: no `sqlx`, no `dotenv`, no
//! `DATABASE_URL`, and no client compiled in. That is what a machine
//! serving the map from a directory wants, since it has no Postgres for a
//! client to open. `galos index build --from database` and
//! `galos index ingest --catch-up` are the two verbs inside that group
//! which need the feature, and they are the two that are about the other
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

    /// Clear a lock left behind by a builder that was killed, and take it.
    ///
    /// The refusal names the pid holding the directory. Check it first: a
    /// lock cleared while its builder is merely slow to answer is two
    /// writers over one directory, which is what the lock is for.
    ///
    /// Global rather than per-verb: it is a judgement about the directory,
    /// and every verb that writes one can meet the same refusal.
    #[arg(long, global = true)]
    force_lock: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Fill, inspect and repair an index directory.
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

    let forced = cli.force_lock;
    match cli.command {
        Command::Index(it) => index::run(it, forced).await,
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

/// The filter this run listens with, which one verb has an opinion about.
fn heard(command: &Command) -> String {
    match command {
        #[cfg(feature = "db")]
        Command::Db(it) => it.heard(),
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
