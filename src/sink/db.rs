//! Everything read, written to Postgres.
//!
//! The write path this program has always had, behind the trait the sources
//! now speak through. There is nothing here but the plumbing:
//! [`galos_db::record`] is where an event becomes rows and it is unchanged,
//! because it was already the one place that says how.
//!
//! One transaction per message and none spanning more than one. Each write
//! inside it is a guarded upsert — newer wins, an older reading still fills
//! a blank — so a run interrupted halfway has written whole messages
//! correctly rather than none of them, and re-running costs nothing. That is
//! the property the whole import leans on.

use crate::sink::{Reporter, Sink, SystemReport};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use elite_journal::entry::market::{BlackMarket, Market, Outfitting, Shipyard};
use elite_journal::entry::{Entry, Event};
use galos_db::{record, Database};
use std::sync::Arc;

/// A [`Sink`] onto an open database.
///
/// Owns its handle rather than borrowing one. [`Database`] is a clone over
/// an `Arc<PgPool>`, so what this holds is a refcount onto the pool `main`
/// opened and every write still shares the same five connections — and a
/// sink that owns what it writes through is `'static`, which is what lets a
/// source reading into it be spawned. Several sources run at once now, each
/// with a sink of its own.
pub struct Db {
    db: Database,
    /// Entries and rows written, for the line at the end of a run.
    wrote: u64,
}

impl Db {
    /// A sink onto `db`.
    pub fn new(db: Database) -> Db {
        Db { db, wrote: 0 }
    }
}

#[async_trait]
impl Sink for Db {
    /// `by.named()`: whoever the source named, commander or uploader id
    /// alike. `updated_by` is provenance here and an anonymised sender still
    /// traces a row back to where it came from, which is what the column is
    /// for.
    async fn entry(&mut self, entry: Arc<Entry<Event>>, by: Reporter<'_>) {
        record::entry(&self.db, &entry, by.named()).await;
        self.wrote += 1;
    }

    async fn system(&mut self, report: &SystemReport, user: &str) {
        if record::system(&self.db, report, user).await {
            self.wrote += 1;
        }
    }

    async fn market(&mut self, at: DateTime<Utc>, user: &str, it: &Market) {
        record::market(&self.db, at, user, it).await;
        self.wrote += 1;
    }

    async fn outfitting(
        &mut self,
        at: DateTime<Utc>,
        user: &str,
        it: &Outfitting,
    ) {
        record::outfitting(&self.db, at, user, it).await;
        self.wrote += 1;
    }

    async fn shipyard(&mut self, at: DateTime<Utc>, user: &str, it: &Shipyard) {
        record::shipyard(&self.db, at, user, it).await;
        self.wrote += 1;
    }

    async fn black_market(
        &mut self,
        at: DateTime<Utc>,
        user: &str,
        it: &BlackMarket,
    ) {
        record::black_market(&self.db, at, user, it).await;
        self.wrote += 1;
    }

    /// Nothing to do: every message was written as it arrived.
    async fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// Likewise nothing. There is no held state to close out.
    async fn finish(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn said(&self) -> String {
        format!("{} messages written to the database", self.wrote)
    }
}
