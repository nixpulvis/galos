//!  Database adapter and functions for `galos`
//!
//! Set `DATABASE_URL` for configuring the connection. E.g:
//! - `postgresql://localhost/galos_development`
//! - `postgresql://postgres:"pw"@10.0.1.2/galos_production`
//!
//! Upon calling [`Database::new`] a `.env` file will also be loaded to set
//! that variable. Having no such file is not an error.
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Executor;
use std::env;

pub mod catalog;
pub mod error;
pub use self::error::{Error, Result};

/// The `sqlx` this crate speaks, for callers naming its types
///
/// Every write below system level takes a [`sqlx::PgConnection`], and a
/// caller that has no `sqlx` of its own still has to name one.
pub use sqlx;

/// What a tool of this crate's listens for when `RUST_LOG` says nothing.
///
/// Info upwards from everything, less one line `sqlx` writes on every
/// connection: that it could not open `~/.pgpass`. Not having a password
/// file is the ordinary case — a `DATABASE_URL` carries what it needs, or
/// the socket trusts the user — and it is said at `warn` once per pool, so
/// a run that opens one for collecting and one for deriving greets a
/// commander with two warnings about a file they were never expected to
/// have. `RUST_LOG` overrides all of this, including the silence.
///
/// It lives here rather than in either binary because the line comes from
/// this crate's `sqlx`, so whoever opens one of these pools is who has to
/// silence it.
pub const HEARD: &str = "info,sqlx_postgres::options::pgpass=off";

#[derive(Clone)]
pub struct Database {
    pub(crate) pool: PgPool,
}

impl Database {
    pub async fn new() -> Result<Self> {
        // Only a missing file is passed over, `DATABASE_URL` having other
        // places to come from. One that is there and unreadable is an error.
        match dotenv::dotenv() {
            Ok(_) => {}
            Err(e) if e.not_found() => {}
            Err(e) => return Err(Error::Dotenv(e)),
        }
        let url = env::var("DATABASE_URL")?;

        let pool =
            PgPoolOptions::new().max_connections(5).connect(&url).await?;

        Ok(Database { pool })
    }

    pub async fn from_url(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new().max_connections(5).connect(url).await?;

        Ok(Database { pool })
    }

    /// A pool for bulk work, with `synchronous_commit` off
    ///
    /// A commit returns without waiting for the write-ahead log to reach the
    /// disk, so a crash loses the last of what was committed; the rows are
    /// re-derivable from the file being imported. `max_connections` is the
    /// ceiling, which a sharded import wants above the ordinary five.
    pub async fn bulk(max_connections: u32) -> Result<Self> {
        match dotenv::dotenv() {
            Ok(_) => {}
            Err(e) if e.not_found() => {}
            Err(e) => return Err(Error::Dotenv(e)),
        }
        let url = env::var("DATABASE_URL")?;

        Self::bulk_from_url(&url, max_connections).await
    }

    /// [`Self::bulk`] against a named database
    pub async fn bulk_from_url(
        url: &str,
        max_connections: u32,
    ) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    conn.execute("SET synchronous_commit = off").await?;
                    Ok(())
                })
            })
            .connect(url)
            .await?;

        Ok(Database { pool })
    }

    /// A connection for a run of writes that belong together
    ///
    /// Everything written on it lands or none of it does. Each write below
    /// system level takes the connection it is to run on, so what one
    /// transaction covers is the caller's to say.
    pub async fn begin(
        &self,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>> {
        Ok(self.pool.begin().await?)
    }

    /// A connection for a write that stands alone
    ///
    /// The same connection the writes below system level take, without a
    /// transaction around it: each statement stands on its own.
    pub async fn acquire(
        &self,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>> {
        Ok(self.pool.acquire().await?)
    }

    /// What the database's clock says
    ///
    /// For a caller keeping track of how current what it holds is. Rows carry
    /// the database's clock in `updated_at`, and a caller comparing those
    /// against its own is a caller trusting two clocks to agree: run a little
    /// fast and it stamps itself later than the writes it has not seen yet,
    /// and every one of those is missed for good.
    ///
    /// Read before the question it stamps rather than after, so that anything
    /// written while the question is being answered is asked for again next
    /// time. Asking twice costs a row; asking never loses one.
    pub async fn now(&self) -> Result<chrono::DateTime<chrono::Utc>> {
        let now: chrono::NaiveDateTime =
            sqlx::query_scalar("SELECT now() AT TIME ZONE 'utc'")
                .fetch_one(&self.pool)
                .await?;

        Ok(now.and_utc())
    }
}

/// Say that a write was turned away for being older than what is on record
///
/// Every guarded write says it the same way, so a run of the sync can be counted
/// rather than read. What that is worth: a refusal writes nothing and answers
/// no error, so a guard that fires at the right rate and one that never fires at
/// all leave a database looking exactly alike. Uploaders batch and reconnect, and
/// about one message in three hundred arrives older than one already seen for the
/// same thing, so that is roughly the rate to expect.
///
/// `what` names the kind of thing, and `sent` is when the game wrote the message
/// that lost. Nothing says what is on record instead: reading it back would be a
/// second query on a path taken thirty times a second to say something the row
/// itself already holds.
pub(crate) fn turned_away(what: &str, sent: chrono::DateTime<chrono::Utc>) {
    tracing::debug!(what, %sent, "older than what is on record");
}

pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

impl Page {
    pub fn by(limit: i64) -> Self {
        Page { limit, offset: 0 }
    }

    pub fn turn(&self, n: i64) -> Self {
        Page { limit: self.limit, offset: self.offset + n }
    }
}

pub mod articles;
pub mod barycenters;
pub mod black_market;
pub mod bodies;
pub mod body_signals;
pub mod clusters;
pub mod codex_entries;
pub mod factions;
pub mod index;
pub mod markets;
pub mod migrate;
mod orbit;
pub mod outfitting;
pub mod record;
pub mod report;
pub mod rings;
pub mod shipyard;
pub mod stars;
pub mod stations;
pub mod system_signals;
pub mod systems;
/// A database of a test's own, made and dropped by the test
///
/// Out of an ordinary build, this being a module that makes and drops
/// databases. `cfg(test)` alone would not do: the tests that want it are
/// mostly integration tests, which link this library exactly as any other
/// caller does and see only what a feature turns on. The feature alone would
/// do, this crate asking for it of itself as a dev-dependency, but then the
/// unit tests in `index` would be one resolver decision away from not
/// compiling.
#[cfg(any(test, feature = "testing"))]
pub mod testing;
