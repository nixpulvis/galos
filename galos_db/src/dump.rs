//! A database written out to a file, and read back from one.
//!
//! `galos db backup --to PATH` and `galos db restore --from PATH`: a
//! logical dump written by `pg_dump` and read back by `pg_restore`, with
//! the flags that let the file land on a machine other than the one it
//! came off. It stands in for the two lines of recipe in the README,
//! which somebody was otherwise expected to type correctly at the worst
//! moment they will ever type them.
//!
//! # Two backups, and the cheap one is not this one
//!
//! A logical dump of a seeded galaxy is hundreds of gigabytes, and the
//! restore costs more than the dump did: `pg_restore` replays the rows
//! and then builds every index over them, which at the 200 M-system
//! target is most of a day. A backup that takes a day to come back is
//! not what anybody reaches for while the live database is wrong.
//!
//! The cheap one on this machine is the filesystem's. APFS clones: stop
//! the postmaster, `cp -c` the data directory — copy-on-write, seconds,
//! no bytes moved — and start it again. Restoring is pointing `PGDATA`
//! at the clone, which turns hours of loss to merge back afterwards into
//! a window measured in seconds. `pg_basebackup` against a running
//! instance is the portable version of the same idea, and costs the
//! bytes the clone does not.
//!
//! **Neither of those is a verb here.** Both are done to a server rather
//! than to a database: the clone wants the postmaster stopped, and a
//! process cannot report on a dump it took by killing the server it was
//! connected to. They belong in the runbook, and this paragraph is where
//! an operator who came to this verb first finds out about them.
//!
//! **`pg_dump -Fc` keeps its place for one job**: moving a database to a
//! machine whose Postgres is a different major version. A file clone of
//! `PGDATA` is unreadable by a server of another major version and a
//! base backup no better — both are the on-disk format, which is not
//! promised across majors. A custom-format dump is the only one of the
//! three that crosses. Reach for this verb for that, for a database
//! small enough that the day does not matter, and for a copy that has to
//! leave the machine as one file.
//!
//! # Measured, on this machine
//!
//! The 14 GB `galos_postimport_backup` database, dumped serially to an
//! APFS volume: **1.4 GB of dump in 1 min 24 s**, and **2 min 51 s** to
//! read it back into an empty database, which comes back 10 GB. The
//! restore is already twice the dump at this size and the gap is index
//! building, which is superlinear in the rows — that is where the day
//! at 200 M systems comes from, not from the bytes.
//!
//! **That restore exited 1, and it was right to.** Five errors, every
//! one of them a foreign key on `faction_id` that Postgres would not
//! put back because the rows arriving already violated it: the source
//! holds 76,791 `system_factions` naming a faction that is not there.
//! The source lists all five constraints; the restored copy has none
//! of them. So the round trip is where a database that was quietly
//! wrong stops being quiet, the restored copy answers every query and
//! is missing five constraints, and the only thing that says so is
//! `system_factions_without_faction` — see
//! [`crate::report::Verified::is_sound`], which named
//! `pg_restore --disable-triggers` as how those rows got there in the
//! first place. It is why [`Restored`] ends by naming `galos db
//! verify` instead of reporting a success and stopping.
//!
//! # Why a child process
//!
//! There is no other [`Command`] in this workspace, and this is the
//! exception rather than the start of a habit. Writing the dump here
//! would be a second implementation of `pg_dump`'s archive format, and
//! the format is the entire point of a backup: a file only `galos` can
//! read is worth nothing on the day `galos` is what is broken. What
//! makes these files a backup is that `pg_restore` on any machine with a
//! Postgres client reads them, so `pg_dump` is what writes them.
//!
//! # What is passed, and what is not
//!
//! `--no-owner --no-privileges`, always. A dump carrying ownership and
//! grants restores cleanly only onto a server with the same role names,
//! and the machine a backup is restored onto in a hurry is routinely a
//! laptop where the role is a person's login rather than `galos`.
//! Without them the restore is a page of "role does not exist" and a
//! database that is half there. What is given up is the grants, which
//! this schema does not use: everything is owned by whoever connects.
//!
//! `--verbose`, always, and the child's `stderr` is inherited rather
//! than captured — [`Command::status`] passes both pipes through
//! untouched. That pair is the difference between watching a four-hour
//! restore name the index it is building and staring at a silent
//! terminal for four hours wondering whether it has hung. Nothing is
//! parsed back out of it: a progress line is for a person, and the
//! counts this module reports are measured here instead. Errors arrive
//! by the same route, which is what makes [`crate::Error::Dump`] and
//! [`crate::Error::Restore`] saying "what it objected to is above"
//! honest rather than a shrug.
//!
//! The connection URL goes to the child and nowhere else. It is never
//! logged and never put in an error, for the reason `bin/galos/db`'s
//! `named` strips it out of a status line: the first thing done with one
//! of these lines is paste it into a bug report, and a password pasted
//! there is a credential to rotate rather than a question answered.
//!
//! # The failure this verb has
//!
//! Version skew, every time. `pg_dump` refuses a server newer than
//! itself outright, and the pair on a machine serving the galaxy is
//! routinely older than the server beside it, because the client
//! package and the server package are installed separately and upgraded
//! separately. Nothing here probes for it: asking `pg_dump --version`
//! and `SELECT version()` and comparing them would be a second, worse
//! copy of the check `pg_dump` already does and states precisely. The
//! tool's own refusal is passed through instead, and the answer to it is
//! a newer Postgres *client* package — never a flag, and never a
//! different format.
//!
//! The same package is why [`crate::Error::Uninstalled`] exists: a
//! machine can serve this database perfectly well with no `pg_dump` on
//! it at all, and what the operating system says about that is "No such
//! file or directory", naming neither the file nor where to get it.

use crate::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tracing::info;

/// The program that writes a dump.
const DUMP: &str = "pg_dump";

/// The program that reads one back.
const RESTORE: &str = "pg_restore";

/// What a backup wrote, and what it cost.
pub struct Dumped {
    /// Where the dump is, exactly as it was asked for.
    ///
    /// A file for a serial dump and a directory for a parallel one, and
    /// [`restore`] takes either without being told which.
    pub path: PathBuf,

    /// Every byte under [`Self::path`].
    ///
    /// The whole of the directory where the dump is one, not the entry
    /// itself: the number to compare against the disk this was going to
    /// be kept on is the number the disk will lose.
    pub bytes: u64,

    /// Wall clock across the child, connection and all.
    pub took: Duration,
}

/// What a restore read, and what it cost.
pub struct Restored {
    /// The dump that was read, exactly as it was named.
    pub path: PathBuf,

    /// Wall clock across the child.
    ///
    /// Most of it is index building rather than rows arriving, which is
    /// why a restore outlasts the dump it came from by a wide margin.
    pub took: Duration,
}

/// Write the database `url` names to `to`.
///
/// `jobs` is how many connections dump at once, and it also chooses the
/// format, because `pg_dump` parallelises into a directory and nothing
/// else: a custom-format dump is one stream written by one process, so
/// `--jobs` beside `--format=custom` is an error rather than a slower
/// dump. Asking for the format separately would be offering a
/// combination that does not exist. One job writes a single file;
/// more than one writes a directory of that name, and [`Dumped::bytes`]
/// is then the whole directory.
///
/// A parallel dump opens `jobs` connections beside its own, so the
/// server's connection limit has to have room for them. They share one
/// synchronised snapshot rather than taking one each, which is what
/// makes a dump spread over four connections still one moment.
///
/// **Refused where `to` already exists**, before the connection is
/// opened. Overwriting a backup is the one mistake in this module with
/// no recovery from it: the file that would have answered the question
/// is gone, and what replaced it is a dump of the database that has just
/// gone wrong.
pub fn backup(url: &str, to: &Path, jobs: u32) -> Result<Dumped, Error> {
    // `symlink_metadata` rather than `Path::exists`, which answers false
    // for a dangling symlink — something is at that path, `pg_dump`
    // will not write through it, and finding out now is better than
    // finding out after the connection is open.
    if fs::symlink_metadata(to).is_ok() {
        return Err(Error::Occupied(to.to_path_buf()));
    }

    let form = match jobs > 1 {
        true => "directory",
        false => "custom",
    };

    let mut pg_dump = Command::new(DUMP);
    pg_dump.arg(format!("--format={form}"));
    if jobs > 1 {
        pg_dump.arg(format!("--jobs={jobs}"));
    }
    pg_dump.args(["--no-owner", "--no-privileges", "--verbose"]);
    // `--file <path>` as two arguments rather than `--file=<path>`, so
    // the path stays an `OsStr`: a destination that is not UTF-8 is
    // still a destination.
    pg_dump.arg("--file").arg(to).arg(url);

    info!(to = %to.display(), jobs, form, "pg_dump");

    let began = Instant::now();
    let status = pg_dump.status().map_err(|e| uninstalled(DUMP, e))?;
    let took = began.elapsed();
    if !status.success() {
        return Err(Error::Dump(status));
    }

    Ok(Dumped { path: to.to_path_buf(), bytes: weigh(to)?, took })
}

/// Read the dump at `from` into the database `url` names.
///
/// `jobs` restores several objects at once, which is worth having for
/// the same reason the dump is not: the index builds are the hours, and
/// they are independent of each other. It works from either format,
/// unlike the dump side.
///
/// `clean` drops each object before recreating it, and it is **off
/// unless the caller says otherwise**. A restore into a database that
/// already holds a galaxy is the mistake here with the worst blast
/// radius — a week of collection dropped table by table by a command
/// meant to be cheap — and off by default is what makes an operator say
/// it out loud before it happens. With it off, a restore into a
/// database that already holds the schema fails object by object and
/// leaves what was there standing — measured at 108 errors ignored
/// over the migrated test template, and an exit status to match.
///
/// `--clean` is always sent with `--if-exists`, because on its own it
/// emits a bare `DROP` for every object in the dump: the first one the
/// target has not got is an error, so a `--clean` restore into an empty
/// database would fail for its being clean already.
///
/// **A non-zero exit is not proof that nothing was restored.**
/// `pg_restore` returns non-zero for any complaint at all, including
/// ones that leave a working database, so [`crate::Error::Restore`]
/// carries the status and says to read what the tool wrote rather than
/// deciding on the operator's behalf.
pub fn restore(
    url: &str,
    from: &Path,
    jobs: u32,
    clean: bool,
) -> Result<Restored, Error> {
    let mut pg_restore = Command::new(RESTORE);
    pg_restore.args(["--no-owner", "--no-privileges", "--verbose"]);
    if jobs > 1 {
        pg_restore.arg(format!("--jobs={jobs}"));
    }
    if clean {
        pg_restore.args(["--clean", "--if-exists"]);
    }
    pg_restore.arg("--dbname").arg(url).arg(from);

    info!(from = %from.display(), jobs, clean, "pg_restore");

    let began = Instant::now();
    let status = pg_restore.status().map_err(|e| uninstalled(RESTORE, e))?;
    let took = began.elapsed();
    if !status.success() {
        return Err(Error::Restore(status));
    }

    Ok(Restored { path: from.to_path_buf(), took })
}

/// The one spawn failure worth a sentence of its own.
///
/// `pg_dump` and `pg_restore` come from the Postgres *client* package,
/// and a machine serving this database is routinely installed with the
/// server alone. Every other spawn failure is the filesystem's and is
/// said as it was.
fn uninstalled(tool: &'static str, e: io::Error) -> Error {
    match e.kind() {
        io::ErrorKind::NotFound => Error::Uninstalled(tool),
        _ => Error::Io(e),
    }
}

/// Every byte under a path, whether it is a file or a directory.
///
/// A parallel dump is a directory of one file per table plus a table of
/// contents, so the answer to "how much disk did that cost" is the sum
/// and never the entry: a directory's own inode weighs under a hundred
/// bytes, and reporting that instead would say a galaxy fits in a cache
/// line. Recursion is two deep at most — `pg_dump` writes one level,
/// plus `blobs/` where there are large objects, which this schema has
/// none of.
fn weigh(path: &Path) -> io::Result<u64> {
    let entry = fs::symlink_metadata(path)?;
    if !entry.is_dir() {
        return Ok(entry.len());
    }

    let mut bytes = 0;
    for child in fs::read_dir(path)? {
        bytes += weigh(&child?.path())?;
    }
    Ok(bytes)
}

/// Bytes in the unit somebody would have said them in.
///
/// Powers of a thousand, not of 1024: what a dump is held against is a
/// disk, and a disk is sold in these. One decimal place, because the
/// second one has never changed anybody's next move.
fn weighed(bytes: u64) -> String {
    match bytes {
        n if n < 1_000 => format!("{n} bytes"),
        n if n < 1_000_000 => format!("{:.1} kB", n as f64 / 1e3),
        n if n < 1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        n if n < 1_000_000_000_000 => format!("{:.1} GB", n as f64 / 1e9),
        n => format!("{:.1} TB", n as f64 / 1e12),
    }
}

/// Elapsed time in the unit somebody would have said it in.
///
/// A dump of a development database is seconds and a restore of a seeded
/// one is hours, and `12841.7 s` is a number nobody converts in their
/// head at the end of a long night.
fn lasted(took: Duration) -> String {
    match took.as_secs() {
        s if s < 60 => format!("{:.1} s", took.as_secs_f64()),
        s if s < 3_600 => format!("{} min {} s", s / 60, s % 60),
        s => format!("{} h {} min", s / 3_600, (s % 3_600) / 60),
    }
}

impl fmt::Display for Dumped {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} written to {} in {}",
            weighed(self.bytes),
            self.path.display(),
            lasted(self.took),
        )
    }
}

impl fmt::Display for Restored {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // The pointer to `verify` is the whole of what this line is for
        // beyond the timing. A restore that dropped the foreign keys to
        // get the rows in leaves a database that answers every query and
        // is wrong, and `system_factions_without_faction` is the count
        // that says so — see `crate::report::Verified::is_sound`, which
        // names `pg_restore --disable-triggers` as how it happens.
        write!(
            f,
            "{} restored in {}; run galos db verify, where \
             system_factions_without_faction is the count that says \
             whether the constraints came back with the rows",
            self.path.display(),
            lasted(self.took),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, TEMPLATE};
    use sqlx::postgres::PgConnection;
    use sqlx::Connection;

    /// How far the schema got, asked of either side of a round trip.
    ///
    /// The migration table is rows and schema at once, so a dump that
    /// carried the tables and lost what was in them fails on it just as
    /// one that carried nothing does.
    const APPLIED: &str = "SELECT count(*)::bigint FROM _sqlx_migrations";

    /// A backup will not write over what is already there
    ///
    /// The refusal comes before anything is spawned or connected, which
    /// is why the url below names nothing and why this holds on a
    /// machine with no server to reach and no `pg_dump` installed.
    #[test]
    fn a_backup_will_not_write_over_what_is_already_there() {
        let to = std::env::temp_dir()
            .join(format!("galos_dump_held_{}.dump", std::process::id()));
        fs::write(&to, b"last week").expect("a file in the way");

        match backup("postgresql://localhost/nothing", &to, 1) {
            Err(Error::Occupied(named)) => assert_eq!(named, to),
            Err(e) => panic!("refused, but for the wrong reason: {}", e),
            Ok(_) => panic!("wrote over {}", to.display()),
        }

        assert_eq!(
            fs::read(&to).expect("the file to still be there"),
            b"last week",
            "the refusal is worth nothing if the file was touched anyway",
        );

        fs::remove_file(&to).ok();
    }

    /// A serial dump restores into a database that never held it
    ///
    /// The round trip is the only thing that proves an argument vector:
    /// a dump `pg_restore` will not read is not a backup, and no
    /// assertion about the flags can tell the difference.
    ///
    /// Needs a server to reach, named by `TEST_DATABASE_URL`, and
    /// stands down without one.
    #[async_std::test]
    async fn a_serial_dump_restores_into_a_database_that_never_held_it() {
        let Some(round) = Round::new(1, "serial").await else { return };

        println!("{}", round.dumped);
        let restored = round.read_back(false).expect("the dump restores");
        println!("{}", restored);

        assert!(
            !round.was_a_directory,
            "one job is one stream, which is one file",
        );
        assert!(round.dumped.bytes > 0, "an empty dump is not a dump");

        let (dumped, restored) = round.applied().await;
        assert_eq!(
            restored, dumped,
            "the restored database should be as far along as the one \
             that was dumped",
        );

        round.done().await;
    }

    /// A parallel dump is a directory, and weighs as one
    ///
    /// Two contracts in one round trip. `pg_dump` refuses `--jobs`
    /// beside a custom-format dump outright, so a [`backup`] that did
    /// not switch the format with the job count fails here at the dump
    /// rather than subtly later; and the bytes reported have to be what
    /// is in the directory rather than the directory, whose own entry
    /// on this filesystem is under a hundred bytes.
    ///
    /// Needs a server to reach, named by `TEST_DATABASE_URL`, and
    /// stands down without one.
    #[async_std::test]
    async fn a_parallel_dump_is_a_directory_and_weighs_as_one() {
        let Some(round) = Round::new(4, "parallel").await else { return };

        println!("{}", round.dumped);
        let restored = round.read_back(false).expect("the dump restores");
        println!("{}", restored);

        assert!(
            round.was_a_directory,
            "more than one job is a directory-format dump",
        );
        assert!(
            round.dumped.bytes > 4_096,
            "{} bytes is the directory, not what is in it",
            round.dumped.bytes,
        );

        let (dumped, restored) = round.applied().await;
        assert_eq!(
            restored, dumped,
            "the restored database should be as far along as the one \
             that was dumped",
        );

        round.done().await;
    }

    /// Restoring over a database that already holds it wants `--clean`
    /// said
    ///
    /// Which is the whole of why the flag defaults to off. Without it
    /// the second restore is a page of "already exists" and what was
    /// there is left standing — measured at 108 errors ignored over the
    /// migrated template; with it, each object is dropped and rebuilt
    /// and the restore succeeds. An operator who has pointed a restore
    /// at a database holding a galaxy gets the refusal by default and
    /// the destruction only by asking.
    ///
    /// The pairing with `--if-exists` is in here too: `--clean` alone
    /// emits a bare `DROP` for every object in the dump, so the first
    /// one the target has not got is an error.
    ///
    /// Needs a server to reach, named by `TEST_DATABASE_URL`, and
    /// stands down without one.
    #[async_std::test]
    async fn restoring_over_a_database_that_holds_it_wants_clean_said() {
        let Some(round) = Round::new(1, "clean").await else { return };

        round.read_back(false).expect("the first restore");

        match round.read_back(false) {
            Err(Error::Restore(status)) => assert!(!status.success()),
            Err(e) => panic!("refused, but for the wrong reason: {}", e),
            Ok(_) => panic!("a second restore went in without --clean"),
        }

        round.read_back(true).expect("a clean restore over the top");

        let (dumped, restored) = round.applied().await;
        assert_eq!(restored, dumped, "what --clean dropped it put back");

        round.done().await;
    }

    /// A dump of the test template, and an empty database for it.
    ///
    /// The template is the small one — a migrated schema with nothing
    /// in it — so a round trip is a fraction of a second rather than a
    /// galaxy. [`Scratch`] is asked for one and handed straight back,
    /// that being what makes the template in the first place: without
    /// it a first run against a fresh server would dump a database that
    /// is not there yet.
    ///
    /// [`Self::done`] takes the database and the dump away again, and a
    /// test that panics before reaching it leaves both — the same
    /// bargain [`Scratch::done`] makes, and both are named for this
    /// process.
    struct Round {
        /// What the dump cost, which is half of what is measured here.
        dumped: Dumped,

        /// What the dump is on disk, read while it is still there.
        ///
        /// Recorded rather than asked of [`Dumped::path`] in the test,
        /// which may run after [`Self::done`] has taken it away.
        was_a_directory: bool,

        server: String,
        name: String,
        to: PathBuf,
        jobs: u32,
    }

    impl Round {
        async fn new(jobs: u32, called: &str) -> Option<Round> {
            let server = server()?;
            Scratch::new().await?.done().await;

            let name =
                format!("galos_dump_{}_{}", called, std::process::id());
            let to = std::env::temp_dir().join(&name);
            // Whatever a run that was killed left behind.
            fs::remove_file(&to).or_else(|_| fs::remove_dir_all(&to)).ok();

            let dumped = backup(&on(&server, TEMPLATE), &to, jobs)
                .expect("the template should dump");
            let was_a_directory = to.is_dir();

            let mut conn = connect(&server).await;
            for statement in [
                format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", name),
                format!("CREATE DATABASE {}", name),
            ] {
                sqlx::query(&statement)
                    .execute(&mut conn)
                    .await
                    .expect("an empty database to restore into");
            }

            Some(Round { dumped, was_a_directory, server, name, to, jobs })
        }

        /// Read the dump back into this round's own database.
        fn read_back(&self, clean: bool) -> Result<Restored, Error> {
            let into = on(&self.server, &self.name);
            restore(&into, &self.to, self.jobs, clean)
        }

        /// How far each side says its schema got: dumped, then restored.
        async fn applied(&self) -> (i64, i64) {
            (
                counted(&on(&self.server, TEMPLATE)).await,
                counted(&on(&self.server, &self.name)).await,
            )
        }

        /// Drop the database and the dump.
        async fn done(self) {
            let mut conn = connect(&self.server).await;
            sqlx::query(&format!(
                "DROP DATABASE {} WITH (FORCE)",
                self.name
            ))
            .execute(&mut conn)
            .await
            .expect("this test's database to be dropped");
            fs::remove_file(&self.to)
                .or_else(|_| fs::remove_dir_all(&self.to))
                .ok();
        }
    }

    /// A connection, or the test dies saying which url it was.
    async fn connect(url: &str) -> PgConnection {
        PgConnection::connect(url)
            .await
            .unwrap_or_else(|e| panic!("{} should connect: {}", url, e))
    }

    /// How many migrations a database says it has run.
    async fn counted(url: &str) -> i64 {
        sqlx::query_scalar(APPLIED)
            .fetch_one(&mut connect(url).await)
            .await
            .expect("a migrated database should answer")
    }

    /// The server these tests run against, or nothing and they stand
    /// down.
    ///
    /// `TEST_DATABASE_URL` and never `DATABASE_URL`, exactly as
    /// [`crate::testing`] has it: the server being filled from EDDN is
    /// one `cargo test` cannot be pointed at by accident, and these
    /// tests make and drop databases on whatever they are given.
    fn server() -> Option<String> {
        dotenv::dotenv().ok();
        std::env::var("TEST_DATABASE_URL").ok()
    }

    /// The same server, a named database on it.
    ///
    /// String surgery rather than a parse, these tools taking a url
    /// rather than `sqlx`'s options. Anything after the database name
    /// goes with it, which is as much as a local round trip needs.
    fn on(server: &str, database: &str) -> String {
        let (scheme, rest) =
            server.split_once("://").unwrap_or(("postgresql", server));
        let authority = rest.split('/').next().unwrap_or(rest);
        format!("{scheme}://{authority}/{database}")
    }
}
