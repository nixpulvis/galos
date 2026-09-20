//! Everything done to the galaxy database: `galos db`.
//!
//! ```sh
//! galos db status                                 # what it is, how current
//! galos db migrate                                # bring the schema forward
//! galos db ingest --from journal=~/Saved\ Games/… # a commander's own logs
//! galos db ingest --from eddn --watch             # follow the feed
//! galos db ingest --from spansh=galaxy.json --bulk --shard 0/8
//! galos db verify                                 # what is wrong in here
//! galos db catalog hygdata_v41.csv                # a survey from Earth
//! galos db stats                                  # what the galaxy holds
//! ```
//!
//! ## Why this is a verb group and not a pair of flags
//!
//! There are two stores and they are not two settings of one store. A
//! database keeps stations, markets, signals and factions — everything a
//! question is asked *about* — and an index keeps the sky: the cell tree a
//! map draws and the bodies inside a system. One reading feeds both, which
//! is why the reading is shared rather than copied per sink
//! ([`galos::read`]); each sink then takes what it is for and says in its
//! own impl what it does with the rest. A pair of flags choosing between
//! them was never choosing between two outputs of one job, it was two jobs
//! sharing a command line, and most of what either one was given had to be
//! refused for the other. So they are two groups of verbs under one
//! command, and what each verb writes is in its name rather than in a flag
//! beside it. The machine settles the rest: an index is served out of a
//! directory with no server at all, so a machine that builds one has no
//! Postgres on it, no `DATABASE_URL`, and — the `db` feature being off
//! there — no client compiled in and none of this group to ask for.
//!
//! The connection is read from `DATABASE_URL`, as everything else here
//! reads it, and [`status`] says out loud which database that came to.

use clap::Subcommand;
use galos_catalog::hyg;
// `HEARD` is what `RUST_LOG` falls back to, and it is this crate's `sqlx`
// that writes the one line it silences.
use galos_db::{Database, HEARD};
use std::path::{Path, PathBuf};

mod ingest;

/// Work with the galaxy database.
#[derive(clap::Args)]
pub struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}

impl Cli {
    /// What `RUST_LOG` falls back to for the verb named here.
    ///
    /// [`HEARD`] for all but one. `verify`'s ancestry check is a hash
    /// anti-join over every `parent_ids` element — 1.35 s on a 3.4 M-system
    /// database, measured at the server's default 4 MB `work_mem` — which
    /// trips `sqlx`'s one-second slow-statement alert and prints the whole
    /// statement above the report. The query is slow *on purpose*: it is
    /// what the verb is. An alert about the one thing the operator asked
    /// for is noise, and noise directly above the answer. `RUST_LOG` still
    /// overrides this, which is how to see it.
    pub fn heard(&self) -> String {
        match self.command {
            Command::Verify => format!("{HEARD},sqlx::query=error"),
            _ => HEARD.to_string(),
        }
    }
}

#[derive(Subcommand)]
pub(super) enum Command {
    /// Say which database this is, how current it is and what it holds.
    ///
    /// Row counts come from the planner's statistics rather than from
    /// `count(*)`, a galaxy being too many rows to count for a status
    /// line, and the report says so where it does it.
    Status,

    /// Read what a publisher publishes into the database.
    ///
    /// `--from` repeats and every source named is read at once, each with
    /// a sink of its own onto the same pool: a dump and the feed are one
    /// run, and neither waits on the other.
    ///
    /// A run either **follows** something or **imports** something, and
    /// every per-source flag belongs to one of the two — the flags below
    /// are grouped that way. It follows where a source has no end: the
    /// feed, a spool, or a journal under `--watch`. Everything else is an
    /// import — a dump, a saved dump, an API answer, a journal read once —
    /// and has an end the run exits at.
    Ingest(ingest::Ingest),

    /// Run the migrations this build carries, and say where that left the
    /// database.
    ///
    /// Every query in `galos_db` names its columns by hand, so a database
    /// a migration behind fails at the first statement touching the column
    /// it is missing. The migrations are compiled in, so a copy of this
    /// tool on a server migrates that server with no source tree beside
    /// it.
    Migrate,

    /// Count what is wrong with the database, and exit 1 where it is
    /// unsound.
    ///
    /// Only what the schema cannot already guarantee: the joins no foreign
    /// key can express, and the columns made nullable on purpose. Nothing
    /// here asks Postgres whether Postgres works.
    Verify,

    /// Compare a star catalog's positions against this database's.
    ///
    /// Matches by name — the only key the two share — and reports where they
    /// disagree about how far away a star is, which is the measurement that
    /// gets revised. The frame between them is fitted from the matched stars
    /// rather than assumed, so a wrong guess about axes cannot masquerade as
    /// every star being in the wrong place.
    Catalog {
        /// The HYG catalog CSV to compare against.
        file: PathBuf,
    },

    /// What the galaxy in here is made of, and what it costs to keep.
    Stats,
}

/// Answer one `galos db` verb.
///
/// Three outcomes rather than two. `Err` is a run that could not be made —
/// a refused command line, a pool that would not open — and is printed by
/// the caller; `Ok(false)` is a run that was made, said everything it had
/// to say, and did not like the answer, which is [`verify`]'s unsound
/// database and an ingest whose source failed under it. Both leave 1, and
/// the difference between them is whether anything was reported before the
/// exit code.
pub async fn run(cli: Cli) -> Result<bool, String> {
    match cli.command {
        Command::Status => status().await,
        Command::Ingest(it) => ingest::run(it).await,
        Command::Migrate => migrate().await,
        Command::Verify => verify().await,
        Command::Catalog { file } => catalog(&file).await,
        Command::Stats => stats().await,
    }
}

/// Say which database this is, and what state it is in.
///
/// The name first, because every number under it is about whichever
/// database the environment happened to name: a status read against the
/// wrong galaxy is worse than no status, and the one thing an operator
/// cannot check from the output is the thing the output is of.
///
/// It is read *after* the pool is open, which is the one subtlety here:
/// `DATABASE_URL` is as likely to come out of a `.env` as out of the
/// environment, and opening the pool is what reads that file.
async fn status() -> Result<bool, String> {
    let db = open(false).await?;
    println!("{}\n", talking_to());
    print!("{}", galos_db::report::status(&db).await.map_err(said)?);
    Ok(true)
}

/// Run the migrations, and say what that came to.
///
/// This is the verb that replaces running `sqlx-cli` against the server by
/// hand, so what it prints has to answer what would otherwise be asked of
/// `_sqlx_migrations` straight afterwards: did anything run, and where does
/// the database stand now. A run that applied nothing says so and still
/// names the version, because "already current" and "pointed at the wrong
/// database" are the same silence otherwise.
///
/// Which database, too, and here rather than only in [`status`]: this is
/// the one verb that changes a schema, and "pointed at the wrong database"
/// is the mistake it is worth spending a line on.
async fn migrate() -> Result<bool, String> {
    let db = open(false).await?;
    println!("{}", talking_to());
    let done = galos_db::migrate::migrate(&db).await.map_err(said)?;
    match done.applied {
        0 => println!("nothing to apply"),
        1 => println!("1 migration applied"),
        applied => println!("{applied} migrations applied"),
    }
    match (done.version, &done.described) {
        (Some(version), Some(description)) => {
            println!("at migration {version} ({description})")
        }
        _ => println!(
            "no migrations on record, which is a database nothing has set up"
        ),
    }
    Ok(true)
}

/// Count what is wrong with the database, and leave by the answer.
///
/// 1 where it is unsound and 0 where it is not, so a cron line is
/// `galos db verify || …`. What counts as unsound is
/// [`galos_db::report::Verified::is_sound`] and is decided there rather
/// than here: telling damage from a backlog is a judgement about the
/// schema — a system nobody has sent coordinates for yet is a galaxy that
/// is incomplete, and every galaxy is incomplete — and this end only reads
/// the verdict.
///
/// The whole report prints either way. An exit code is a thing to alert
/// off, not a thing to read a database off.
async fn verify() -> Result<bool, String> {
    let db = open(false).await?;
    let verified = galos_db::report::verify(&db).await.map_err(said)?;
    print!("{verified}");
    Ok(verified.is_sound())
}

/// Compare a survey taken from Earth against the galaxy in here.
///
/// The file is read before the pool is opened, in that order on purpose: a
/// mistyped path is by far the likeliest way this ends, and it costs
/// nothing to find out before a connection is made. What is read is counted
/// on stderr, so the report on stdout is a thing to redirect whole.
async fn catalog(file: &Path) -> Result<bool, String> {
    let handle = std::fs::File::open(file)
        .map_err(|err| format!("{}: {err}", file.display()))?;
    let read = hyg::read(handle)
        .map_err(|err| format!("{}: {err}", file.display()))?;
    eprintln!(
        "{} catalog stars, {} named, {} without a distance",
        read.stars.len(),
        read.stars.iter().filter(|s| s.name.is_some()).count(),
        read.unplaced.len(),
    );
    let db = open(false).await?;
    let comparison = galos_db::catalog::compare_to_catalog(&db, &read.stars)
        .await
        .map_err(said)?;
    print!("{}", galos_db::catalog::report(&comparison));
    Ok(true)
}

/// What the galaxy in here is made of, and what it costs to keep.
async fn stats() -> Result<bool, String> {
    let db = open(false).await?;
    print!("{}", galos_db::report::stats(&db).await.map_err(said)?);
    Ok(true)
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

/// Which database this is, out of `DATABASE_URL`.
///
/// Read here rather than kept on the pool because the pool does not keep
/// it: `Database` holds connections, and what they were opened off is the
/// environment's to say.
fn talking_to() -> String {
    match std::env::var("DATABASE_URL") {
        Ok(url) => named(&url),
        // Unreachable through the verbs, the pool being open by the time
        // any of them asks — the variable is what opened it.
        Err(_) => "a database DATABASE_URL does not name".to_string(),
    }
}

/// The host and the name out of a connection URL, and nothing else out of
/// it.
///
/// Those two are the whole of what identifies a server to somebody who has
/// three of them. Not the password: a status line is the first thing
/// pasted into a bug report, and `postgres://galos:hunter2@…` pasted there
/// is a credential that has to be rotated rather than a question that has
/// been answered.
fn named(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    // Last `@` rather than first: a password may hold one, a host may not.
    let host = authority.rsplit_once('@').map_or(authority, |(_, it)| it);
    // `?sslmode=…` and the like are how it is reached, not what it is.
    let database = path.split(['?', '#']).next().unwrap_or_default();
    match (database.is_empty(), host.is_empty()) {
        (true, true) => "a database DATABASE_URL does not name".to_string(),
        (true, false) => format!("the default database on {host}"),
        // `postgresql:///galos` is the local socket, which is the
        // arrangement on the machine the server is on.
        (false, true) => format!("{database} over a local socket"),
        (false, false) => format!("{database} on {host}"),
    }
}

/// What a database error reads as on the way out.
///
/// The verbs answer `String`, a refusal from the command line being one
/// already, and [`main`] prints whichever came back. Nothing here adds a
/// prefix: `galos_db::Error` says what it was and where, and "verify:"
/// in front of it would only say what the command line already says.
fn said(err: galos_db::Error) -> String {
    err.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A URL is named by its host and its database and by nothing else
    ///
    /// The password especially, which is the reason the naming is a
    /// function rather than a `println!` of the variable.
    #[test]
    fn a_database_is_named_without_its_password() {
        assert_eq!(
            named("postgresql://galos:hunter2@db.example.com:5432/galaxy"),
            "galaxy on db.example.com:5432",
        );
        assert_eq!(named("postgresql://localhost/galos_development"), {
            "galos_development on localhost"
        });
        assert_eq!(
            named("postgresql:///galos_development"),
            "galos_development over a local socket",
        );
        assert_eq!(
            named("postgresql://localhost/galaxy?sslmode=require"),
            "galaxy on localhost",
        );
        assert_eq!(
            named("postgresql://galos:pass@word@localhost/galaxy"),
            "galaxy on localhost",
            "a password may hold an `@`, so the split is on the last one",
        );
    }
}
