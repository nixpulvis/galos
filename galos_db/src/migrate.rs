//! Bringing a database up to what the rest of this crate's queries assume
//!
//! Every query in this crate names columns by hand, so a database one
//! migration behind fails at the first statement that touches the column it
//! is missing, with an error about that column and nothing about the cause.
//! This is the module that closes that gap, and [`applied`] is what lets
//! `status` say the version out loud before anything else is attempted.
//!
//! # Why the macro and not the directory
//!
//! [`sqlx::migrate!`] reads `./migrations` at compile time and bakes the
//! statements into the binary, so a `galos-db` copied to a server migrates
//! that server without the source tree beside it.
//! [`sqlx::migrate::Migrator::new`], which `testing` uses, reads the
//! directory at run time — right for a test that runs out of the
//! workspace, wrong for a tool that does not.
//!
//! The macro loses nothing by doing it early. Five of the migrations here
//! open with a `-- no-transaction` line, because they build indexes
//! `CONCURRENTLY` and Postgres refuses that inside a transaction block:
//!
//! - `20260816000000_read_a_bodys_reach_from_the_index`
//! - `20260816000010_read_a_stars_reach_from_the_index`
//! - `20260816000020_read_a_barycenters_reach_from_the_index`
//! - `20260911000000_read_the_arrival_star_where_the_reach_is_read`
//! - `20260911000010_drop_the_index_the_arrival_star_supersedes`
//!
//! The marker is a `sqlx` convention, not a SQL one, and the macro honours
//! it exactly as the directory reader does: `sqlx-macros` sets `no_tx` on
//! each embedded migration from the same leading-line test, and the Postgres
//! driver runs those statements on the bare connection instead of opening a
//! transaction around them. Hand-running the files with `psql -f` would not
//! know about the marker — nor about `_sqlx_migrations`, nor about the
//! advisory lock that stops two of these racing — which is why neither this
//! module nor the binary above it ever does that.

use crate::{Database, Result};

/// The migrations, embedded.
///
/// A `static` rather than a call, so the parse happens once at compile time
/// and [`migrate`] is only ever running statements.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// What a run of the migrations did, and where it left the database.
///
/// `applied` is what this run added; `version` and `described` are the
/// newest row on record afterwards, which is the same thing `status` shows
/// and is worth printing even when `applied` is zero — that is the "already
/// current" answer, and a version beside it is how an operator tells it from
/// "connected to the wrong database".
pub struct Migrated {
    pub applied: usize,
    pub version: Option<i64>,
    pub described: Option<String>,
}

/// Run every migration the database has not run, and say what that was.
///
/// The count is measured, not reported: `sqlx`'s
/// [`sqlx::migrate::Migrator::run`] answers `()` on success and there is no
/// hook to count from, so this reads `_sqlx_migrations` before and after
/// and subtracts. That is honest about what it can see and nothing more —
/// a row another process applied while this one was running counts here
/// too, which is the correct reading of "how far did the database move",
/// and the advisory lock `run` takes makes it a narrow window in any case.
///
/// Runs on a single pooled connection rather than the pool, because that
/// lock is held per connection: handing `run` the pool would let it acquire
/// one connection for the lock and another for a statement.
pub async fn migrate(db: &Database) -> Result<Migrated> {
    let before = recorded(db).await?;

    let mut conn = db.acquire().await?;
    MIGRATOR.run(&mut *conn).await.map_err(sqlx::Error::from)?;
    // Returned to the pool before the reads below, so a pool of one still
    // answers them.
    drop(conn);

    let after = recorded(db).await?;
    let newest = applied(db).await?;

    Ok(Migrated {
        // Saturating because the subtraction is across two reads of a table
        // somebody else may have touched, and a negative count of applied
        // migrations means nothing to a reader.
        applied: after.saturating_sub(before),
        version: newest.as_ref().map(|(version, _)| *version),
        described: newest.map(|(_, description)| description),
    })
}

/// The newest migration the database has on record, if it has any.
///
/// [`None`] covers two states that look alike from here and read alike to an
/// operator: a database with no `_sqlx_migrations` table at all, and one with
/// an empty table. Both mean "nothing has been migrated", which is a thing
/// `status` must be able to say rather than an error it dies of — reporting
/// the version is most of the value of `status` on a database that is not set
/// up yet.
///
/// A missing table is Postgres' `42P01`, `undefined_table`. Only that code is
/// swallowed; a permission error or a dead connection is still an error.
pub async fn applied(db: &Database) -> Result<Option<(i64, String)>> {
    let newest: std::result::Result<Option<(i64, String)>, sqlx::Error> =
        sqlx::query_as(
            "SELECT version, description FROM _sqlx_migrations \
             ORDER BY version DESC LIMIT 1",
        )
        .fetch_optional(&db.pool)
        .await;

    match newest {
        Ok(row) => Ok(row),
        Err(e) if undefined_table(&e) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// How many migrations the database says it has run.
///
/// The before-and-after measurement [`migrate`] subtracts. An unmigrated
/// database has run none, which is the `42P01` case again.
async fn recorded(db: &Database) -> Result<usize> {
    let count: std::result::Result<i64, sqlx::Error> =
        sqlx::query_scalar("SELECT count(*)::bigint FROM _sqlx_migrations")
            .fetch_one(&db.pool)
            .await;

    match count {
        Ok(n) => Ok(n.max(0) as usize),
        Err(e) if undefined_table(&e) => Ok(0),
        Err(e) => Err(e.into()),
    }
}

/// Whether the server turned a statement away for naming a table it has not
/// got.
fn undefined_table(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Database(e) => e.code().as_deref() == Some("42P01"),
        _ => false,
    }
}
