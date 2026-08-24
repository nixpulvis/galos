//!  Database adapter and functions for `galos`
//!
//! Set `DATABASE_URL` for configuring the connection. E.g:
//! - `postgresql://localhost/galos_development`
//! - `postgresql://postgres:"pw"@10.0.1.2/galos_production`
//!
//! Upon calling [`Database::new`] a `.env` file will also be loaded to set
//! that variable. Having no such file is not an error.
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::env;

pub mod error;
pub use self::error::{Error, Result};

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

/// What the user typed, as a `LIKE` pattern matching those letters
///
/// `%` and `_` mean something to `LIKE` and nothing to whoever typed them, so
/// they are held out at the pattern's own escape character. The escape itself
/// goes first, or escaping the other two would go on to be read as an escape
/// in its own right.
///
/// Here rather than beside either of the searches that reads a name this way,
/// there being two of them: a system is searched for by name and so is a
/// faction, and what `LIKE` reads is the same in both. The ordering below is
/// the whole of what there is to get wrong, and it is worth getting wrong in
/// one place at most.
///
/// Said out loud rather than kept in, since the searches that read a name this
/// way say in their own docs that they do, and a caller reading that is owed
/// the thing it names.
pub fn escaped(query: &str) -> String {
    query.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name with nothing special in it is left as it is
    #[test]
    fn a_plain_name_is_left_alone() {
        assert_eq!(escaped("Col 285 Sector"), "Col 285 Sector");
    }

    /// The two characters `LIKE` reads are held out
    ///
    /// A user typing either means the character. Left as they are, `%` would
    /// match the rest of every name on record and `_` any character at all,
    /// so a search for a literal one would answer with systems that have
    /// nothing to do with it.
    #[test]
    fn the_wildcards_are_held_out() {
        assert_eq!(escaped("100%"), r"100\%");
        assert_eq!(escaped("a_b"), r"a\_b");
    }

    /// The escape character is held out first
    ///
    /// Or the backslash put in front of a `%` would itself be escaped
    /// afterwards, leaving `\\%`: a literal backslash followed by a wildcard,
    /// which is the wildcard the escaping was there to take away.
    #[test]
    fn the_escape_is_held_out_before_what_it_escapes() {
        assert_eq!(escaped(r"a\%b"), r"a\\\%b");
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
pub mod markets;
mod orbit;
pub mod outfitting;
pub mod rings;
pub mod shipyard;
pub mod stars;
pub mod stations;
pub mod system_signals;
pub mod systems;
pub mod index;
