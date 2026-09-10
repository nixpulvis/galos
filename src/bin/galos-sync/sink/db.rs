//! Everything read, written to Postgres.
//!
//! The write path this program has always had, behind the trait the sources
//! now speak through. There is nothing here but the plumbing:
//! [`journal::record`](crate::journal::record) is where an event becomes rows
//! and it is unchanged, because it was already the one place that says how.
//!
//! No batching and no transaction spanning more than one message. Each write
//! is its own guarded upsert — newer wins, an older reading still fills a
//! blank — so a run interrupted halfway has written half the messages
//! correctly rather than none of them, and re-running costs nothing. That is
//! the property the whole import leans on, and holding messages back to write
//! them together would give it up for a throughput nobody has asked for.

use crate::journal::record;
use crate::sink::{Row, Sink};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use elite_journal::entry::market::{BlackMarket, Market, Outfitting, Shipyard};
use elite_journal::entry::{Entry, Event};
use elite_journal::system::Coordinate;
use galos_db::systems::{Economies, System};
use galos_db::Database;
use tracing::warn;

/// A [`Sink`] onto an open database.
///
/// Borrows rather than owns: the connection is opened once in `main` and the
/// pool behind it is what every write shares.
pub struct Db<'a> {
    db: &'a Database,
    /// Entries and rows written, for the line at the end of a run.
    wrote: u64,
}

impl<'a> Db<'a> {
    /// A sink onto `db`.
    pub fn new(db: &'a Database) -> Db<'a> {
        Db { db, wrote: 0 }
    }
}

#[async_trait]
impl Sink for Db<'_> {
    async fn entry(&mut self, entry: &Entry<Event>, user: &str) {
        record::entry(self.db, entry, user).await;
        self.wrote += 1;
    }

    async fn ensure_system(
        &mut self,
        at: DateTime<Utc>,
        user: &str,
        address: i64,
        name: Option<&str>,
        position: Option<Coordinate>,
        why: &str,
    ) -> bool {
        record::ensure_system(self.db, at, user, address, name, position, why)
            .await
    }

    async fn system(&mut self, row: &Row) {
        let wrote = System::create(
            self.db,
            row.address,
            &row.name,
            row.position,
            None,
            row.population,
            row.security,
            row.government,
            row.allegiance,
            Economies::new(row.primary_economy, row.secondary_economy),
            row.updated_at,
            &row.updated_by,
        )
        .await;
        match wrote {
            Ok(()) => self.wrote += 1,
            Err(err) => {
                warn!(system = %row.name, error = %err, "dumped system")
            }
        }
    }

    async fn market(&mut self, at: DateTime<Utc>, user: &str, it: &Market) {
        record::market(self.db, at, user, it).await;
        self.wrote += 1;
    }

    async fn outfitting(
        &mut self,
        at: DateTime<Utc>,
        user: &str,
        it: &Outfitting,
    ) {
        record::outfitting(self.db, at, user, it).await;
        self.wrote += 1;
    }

    async fn shipyard(&mut self, at: DateTime<Utc>, user: &str, it: &Shipyard) {
        record::shipyard(self.db, at, user, it).await;
        self.wrote += 1;
    }

    async fn black_market(
        &mut self,
        at: DateTime<Utc>,
        user: &str,
        it: &BlackMarket,
    ) {
        record::black_market(self.db, at, user, it).await;
        self.wrote += 1;
    }

    /// Nothing to do: every message was written as it arrived.
    async fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn said(&self) -> String {
        format!("{} messages written to the database", self.wrote)
    }
}
