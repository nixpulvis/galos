//! Everything done to the galaxy database: `galos db`.
//!
//! ```sh
//! galos db status                                 # what it is, how current
//! galos db migrate                                # bring the schema forward
//! galos db verify                                 # what is wrong in here
//! galos db catalog hygdata_v41.csv                # a survey from Earth
//! galos db stats                                  # what the galaxy holds
//! galos db backup --to latest.dump                # write it out
//! galos db merge --from postgresql://…/caught_up  # and make two one
//! ```
//!
//! Filling it is `galos ingest --db`, which is one verb for both stores
//! because it is one reading of one publisher — see `crate::ingest`. What
//! is here is everything else the database is asked or told.
//!
//! ## Backup, restore, and the merge that makes a failure survivable
//!
//! Stated by how a failure actually goes: a problem is noticed,
//! collection is pointed at a fresh database so the feed keeps landing
//! somewhere, the backup is restored beside it, and the two are then made
//! one. Without the third step a restore of a galaxy is hours during
//! which EDDN does not wait, and what it carried is lost.
//!
//! `merge` is that third step, and the rule in it already exists: every
//! write path in `galos_db` is a guarded upsert keyed by a natural key
//! and stamped, so folding one database into another is that same
//! statement with the values coming from a table instead of from a
//! message. See [`galos_db::merge`].
//!
//! `backup` and `restore` are `pg_dump` and `pg_restore`, and the module
//! doc on [`galos_db::dump`] says what that costs on a seeded galaxy and
//! what the cheap alternative is. They are verbs rather than a README
//! recipe because the flags that matter — the format `--jobs` forces,
//! `--no-owner`, and a `--clean` an operator must ask for — are the part
//! that is got wrong by hand.
//!
//! ## Why the stores are two groups, and the writing is not
//!
//! There are two stores and they are not two settings of one store. A
//! database keeps stations, markets, signals and factions — everything a
//! question is asked *about* — and an index keeps the sky: the cell tree a
//! map draws and the bodies inside a system. So what is asked *of* each is
//! its own group of verbs: `status`, `verify`, `migrate` and the rest mean
//! different work on each side and share no code, and a reader looking for
//! "what is wrong with my database" should not have to pick it out of a
//! list that also repairs body shards.
//!
//! **Filling them is the other way round.** One reading feeds both: a
//! source hands over what it read and each sink takes what it is for
//! ([`galos::read`], [`galos::sink`]). Two verbs for that would be two
//! subscriptions carrying the same galaxy, and an operator keeping both
//! current would run them side by side forever — so it is one `galos
//! ingest` and `--db`/`--index` name which sinks this run has. Naming both
//! reads each publisher once.
//!
//! The connection is read from `DATABASE_URL`, as everything else here
//! reads it, and [`status`] says out loud which database that came to.

use clap::Subcommand;
use galos_catalog::hyg;
// `HEARD` is what `RUST_LOG` falls back to, and it is this crate's `sqlx`
// that writes the one line it silences.
use galos_db::{Database, HEARD};
use std::path::{Path, PathBuf};

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
            // A merge is two `COPY`s and one `INSERT … SELECT` a table,
            // every one of them over as much of a galaxy as the other
            // database holds. They are slow because they are the verb,
            // and the alert would print each statement above the report.
            Command::Verify | Command::Merge { .. } => {
                format!("{HEARD},sqlx::query=error")
            }
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

    /// Write this database out to a file `pg_restore` can read back.
    ///
    /// A logical dump, which is the portable answer and not the cheap
    /// one: over a seeded galaxy it is hundreds of gigabytes and the
    /// restore rebuilds every index. What that is *for* is moving a
    /// database to a machine whose Postgres is a different major
    /// version. The cheap backup is a file-level copy of `PGDATA` with
    /// the postmaster stopped — seconds on a copy-on-write filesystem —
    /// and it is a runbook step rather than a verb because stopping the
    /// server is not something this should do behind an operator's back.
    Backup {
        /// Where the dump goes. Refused where something already stands
        /// there: overwriting a backup is the mistake with no recovery.
        #[arg(long, value_name = "PATH")]
        to: PathBuf,
        /// Dump on this many connections at once. More than one needs —
        /// and so selects — the directory format, a custom-format dump
        /// being one stream.
        #[arg(long, default_value_t = 1, value_name = "N")]
        jobs: u32,
    },

    /// Read a dump back into the database `DATABASE_URL` names.
    Restore {
        /// The dump to read, in either format [`Backup`] writes.
        #[arg(long, value_name = "PATH")]
        from: PathBuf,
        /// Restore on this many connections at once, which the directory
        /// format allows and a custom-format dump does not.
        #[arg(long, default_value_t = 1, value_name = "N")]
        jobs: u32,
        /// Drop each object before recreating it.
        ///
        /// Off by default. A restore over a database that already holds
        /// a galaxy is the mistake here with the worst blast radius, and
        /// this is what makes an operator say it out loud.
        #[arg(long)]
        clean: bool,
    },

    /// Fold another database into this one, newest record winning.
    ///
    /// What a failure needs and a re-import is not: collection is
    /// pointed at a fresh database so the feed keeps landing somewhere,
    /// the backup is restored beside it, and the two are then made one.
    /// The answer is what one database that had seen both streams would
    /// hold, which is the property the merge is tested against.
    ///
    /// The rule is the one every write path already holds — a guarded
    /// upsert keyed by a natural key, stamped, where the newer report
    /// wins a column and a blank never contradicts one. Nothing is
    /// deleted from this database by a merge.
    Merge {
        /// The database to fold in, as a connection URL. Read, never
        /// written.
        #[arg(long, value_name = "URL")]
        from: String,
        /// Carry only rows changed at or after this, as
        /// `2026-09-14T12:00:00`. The whole of the other database
        /// otherwise.
        #[arg(long, value_name = "WHEN")]
        since: Option<chrono::NaiveDateTime>,
        /// Weigh it and report what would cross; write nothing.
        ///
        /// The whole merge runs in one transaction either way, so this
        /// is that transaction rolled back rather than a second code
        /// path guessing at what the first would do.
        #[arg(long)]
        dry_run: bool,
    },
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
        Command::Migrate => migrate().await,
        Command::Verify => verify().await,
        Command::Catalog { file } => catalog(&file).await,
        Command::Stats => stats().await,
        Command::Backup { to, jobs } => backup(&to, jobs).await,
        Command::Restore { from, jobs, clean } => {
            restore(&from, jobs, clean).await
        }
        Command::Merge { from, since, dry_run } => {
            merge(&from, since, dry_run).await
        }
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

/// Write the database out, saying what it cost.
///
/// The pool is opened and dropped before `pg_dump` is spawned, for one
/// reason: it is what reads `.env`, and it is what says *now* rather than
/// three minutes in that nothing answers on `DATABASE_URL`. Nothing here
/// holds a connection while the child runs — a dump of a galaxy is hours
/// and five idle connections held across it are five an operator cannot
/// use.
async fn backup(to: &Path, jobs: u32) -> Result<bool, String> {
    let url = reachable().await?;
    println!("dumping {} to {}", talking_to(), to.display());
    let done = galos_db::dump::backup(&url, to, jobs).map_err(said)?;
    println!("{done}");
    Ok(true)
}

/// Read a dump back, saying what it cost.
///
/// The database has to exist and be reachable: this restores *into* one
/// rather than creating one, because `DATABASE_URL` is what says which,
/// and a verb that quietly created the database named by a typo is a
/// galaxy restored where nobody will look for it.
async fn restore(
    from: &Path,
    jobs: u32,
    clean: bool,
) -> Result<bool, String> {
    let url = reachable().await?;
    println!("restoring {} into {}", from.display(), talking_to());
    let done = galos_db::dump::restore(&url, from, jobs, clean)
        .map_err(said)?;
    // `Restored`'s own line already names `galos db verify` and says what
    // the count means, so nothing is added here.
    println!("{done}");
    Ok(true)
}

/// Fold another database into this one, saying what crossed.
///
/// Two pools at once, which nothing else in this program opens: the one
/// `DATABASE_URL` names is written and the one `--from` names is read.
/// They are told apart in every line this prints, because a merge run the
/// wrong way round is not a thing an operator finds out about later.
async fn merge(
    from: &str,
    since: Option<chrono::NaiveDateTime>,
    dry_run: bool,
) -> Result<bool, String> {
    let into = open(false).await?;
    let source = galos_db::Database::from_url(from)
        .await
        .map_err(|err| format!("no database at {}: {err}", named(from)))?;

    println!("folding {} into {}", named(from), talking_to());
    if let Some(since) = since {
        println!("  rows changed at or after {since}");
    }
    let mut said_table = |table: &galos_db::merge::Table| {
        println!("  {table}");
    };
    let done =
        galos_db::merge::merge(&into, &source, since, dry_run, &mut said_table)
            .await
            .map_err(said)?;
    print!("{done}");
    if dry_run {
        println!("nothing was written: this was --dry-run");
    }
    Ok(true)
}

/// The connection string the child tools are given, having made sure
/// something answers on it.
///
/// Opening the pool is what loads `.env`, so the URL cannot be read
/// before it and be sure of the answer — see [`Database::new`]. It is
/// dropped straight away: what wants the string is a child process, not
/// this one.
async fn reachable() -> Result<String, String> {
    drop(open(false).await?);
    std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL names no database".to_string())
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
