use std::path::PathBuf;
use std::process::ExitStatus;
use std::{env, error, fmt};

pub type Result<T> = std::result::Result<T, Error>;

pub enum Error {
    /// The variable itself, unset or not UTF-8
    DatabaseUrl(env::VarError),

    /// The file that would have set it, there and unreadable
    Dotenv(dotenv::Error),

    Sqlx(sqlx::Error),

    /// A build wrote to disk and the write failed.
    Io(std::io::Error),

    /// A system with no name on record whose address spells none either
    ///
    /// A null `systems.name` means the address spells it, which is 97.3 %
    /// of a galaxy. One the arithmetic cannot answer is a row written
    /// wrongly rather than a system without a name, so it is said rather
    /// than answered with an empty string.
    Nameless(i64),

    /// A backup was pointed at a path that already holds something
    ///
    /// The one mistake in `dump` with no recovery from it: the file that
    /// would have answered the question is gone, and what replaced it is
    /// a dump of the database that has just gone wrong.
    Occupied(PathBuf),

    /// `pg_dump` or `pg_restore` is not on this machine
    ///
    /// They are the Postgres *client* package, which a server routinely
    /// has none of. What the operating system says about it names
    /// neither the file nor where it comes from.
    Uninstalled(&'static str),

    /// `pg_dump` ran and exited non-zero
    Dump(ExitStatus),

    /// `pg_restore` ran and exited non-zero
    ///
    /// Carried rather than turned into a verdict: `pg_restore` exits
    /// non-zero for any complaint at all, including ones that leave a
    /// database worth keeping, so what it means is the operator's to
    /// read off what it wrote.
    Restore(ExitStatus),

    /// Two databases a merge was asked to fold together are at
    /// different migration versions
    ///
    /// Refused before a row is read. The two agree about most of their
    /// columns, which is what makes this easy to do by accident, and
    /// merging across the difference is how a column silently stops
    /// being written. [`None`] is a database with nothing migrated at
    /// all.
    Divergent(Option<i64>, Option<i64>),

    /// The foreign keys form a cycle, so no order writes them
    ///
    /// Carries what was left when the sort could go no further, which is
    /// the cycle and whatever hangs off it.
    Cyclic(Vec<String>),

    /// A table a merge has no rule for
    ///
    /// It carries no stamp, so there is nothing to weigh two readings of
    /// a row against, and `merge::rule` does not name it. Said rather
    /// than guessed at: a table added by a migration has to say what a
    /// merge should do with it.
    Unruled(String),

    /// A table with no unique key to merge on
    ///
    /// Without one there is no way to tell a row the target already
    /// holds from one it does not, and every merge would be an append.
    Keyless(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::DatabaseUrl(env::VarError::NotUnicode(_)) => {
                write!(f, "DATABASE_URL is set to something that is not UTF-8")
            }
            Error::DatabaseUrl(_) => write!(
                f,
                "DATABASE_URL is not set, and no .env file set it either. \
                 Put it in the environment, or in a .env file in the \
                 working directory or one above it, e.g. \
                 DATABASE_URL=postgresql://postgres@localhost/galos_development"
            ),
            Error::Dotenv(e) => write!(f, ".env could not be read: {}", e),
            Error::Sqlx(e) => write!(f, "{}", e),
            Error::Io(e) => write!(f, "{}", e),
            Error::Nameless(address) => write!(
                f,
                "system {} has no name on record and its address spells none",
                address
            ),
            Error::Occupied(path) => write!(
                f,
                "{} is already there, and a backup will not write over \
                 it. Name somewhere else, or move it out of the way.",
                path.display()
            ),
            Error::Uninstalled(tool) => write!(
                f,
                "{} is not installed. It comes with the Postgres client \
                 tools, which are packaged apart from the server: \
                 postgresql-client on Debian and its relatives, \
                 libpq or the full postgresql formula on Homebrew.",
                tool
            ),
            Error::Dump(status) => write!(
                f,
                "pg_dump gave up ({}); what it objected to is in what it \
                 wrote above. A server newer than the pg_dump reading it \
                 is the usual cause, and the fix for that is a newer \
                 Postgres client, not a different flag.",
                status
            ),
            Error::Restore(status) => write!(
                f,
                "pg_restore reported errors ({}); they are in what it \
                 wrote above. It exits non-zero for any complaint, \
                 including ones that leave a usable database, so read \
                 them before deciding the restore failed.",
                status
            ),
            Error::Divergent(ours, theirs) => write!(
                f,
                "these two databases are at different migration \
                 versions ({} and {}), and merging across a schema \
                 difference is how a column silently stops being \
                 written. Migrate them both first.",
                said(*ours),
                said(*theirs),
            ),
            Error::Cyclic(tables) => write!(
                f,
                "the foreign keys between {} form a cycle, so there is \
                 no order in which they can be written",
                tables.join(", ")
            ),
            Error::Unruled(table) => write!(
                f,
                "{} carries no stamp, so a merge has nothing to weigh \
                 two readings of a row against, and merge.rs names no \
                 rule for it. Say what a merge should do with it there.",
                table
            ),
            Error::Keyless(table) => write!(
                f,
                "{} has no unique key, so a merge cannot tell a row \
                 already on record from a new one",
                table
            ),
        }
    }
}

/// What `Display` says, rather than the derived form
///
/// A `main` returning `Result` prints its error with `Debug`, and the derived
/// one names neither the file nor the variable it was wanted for.
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Error::DatabaseUrl(e) => Some(e),
            Error::Dotenv(e) => Some(e),
            Error::Io(e) => Some(e),
            Error::Nameless(_) => None,
            Error::Occupied(_) => None,
            Error::Uninstalled(_) => None,
            Error::Dump(_) => None,
            Error::Restore(_) => None,
            Error::Divergent(_, _) => None,
            Error::Cyclic(_) => None,
            Error::Unruled(_) => None,
            Error::Keyless(_) => None,
            Error::Sqlx(e) => Some(e),
        }
    }
}

/// A migration version, or that the database has run none
///
/// A database with no `_sqlx_migrations` and one with an empty table both
/// read as nothing here, and both mean the same to somebody who has just
/// been told two databases disagree: this one was never set up.
fn said(version: Option<i64>) -> String {
    match version {
        Some(version) => version.to_string(),
        None => "nothing migrated".to_owned(),
    }
}

impl From<dotenv::Error> for Error {
    fn from(err: dotenv::Error) -> Error {
        Error::Dotenv(err)
    }
}

impl From<env::VarError> for Error {
    fn from(err: env::VarError) -> Error {
        Error::DatabaseUrl(err)
    }
}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Error {
        Error::Sqlx(err)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::Io(err)
    }
}
