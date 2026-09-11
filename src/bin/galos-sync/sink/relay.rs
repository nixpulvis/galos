//! What a collect-side sink hands across to the index worker.
//!
//! The derive side does not share an executor with the collect side: a
//! publish is a whole-galaxy `fs::write` and the index worker runs on its own
//! thread with its own `block_on`, so what a source read has to cross a
//! channel to reach it. [`Relay`] is the [`Sink`] that puts it there, and
//! [`Reading`] is what fits through.
//!
//! ## Why only two shapes cross
//!
//! Because only two mean anything on the other side. A market, an outfitting
//! list, a shipyard and a black market are a station's stock and the index
//! has no station in it; `ensure_system` exists for Postgres' foreign key and
//! an index has no keys. All five are dropped here rather than sent and
//! ignored there, which would be five messages a second crossing a channel to
//! be thrown away by a thread that is busy writing the galaxy.
//!
//! ## What a full channel means
//!
//! Two different things, which is why [`Relay`] asks which phase it is in.
//!
//! While the worker is catching up it is not reading the channel at all: it
//! is inside `galos_db::index::catch_up`, which for a cold directory is an
//! hour. The feed runs at 31 messages a second, so the buffer fills and the
//! reading in hand is dropped — counted, not sent. Nothing is lost by that:
//! the database sink beside this one wrote the entry before this saw it, and
//! the worker answers a nonzero count by discarding what it buffered and
//! running another catch-up round, which reads it back. See
//! [`crate::derive::buffered`].
//!
//! Once the worker is live there is no next round to recover a drop, so a
//! full channel is waited on instead. That is backpressure onto the source:
//! a slow index publish in front of the feed stops the collect task reading,
//! which stops the EDDN thread's envelope channel draining, and *that* is
//! where messages are dropped — at the edge, counted and said, rather than
//! silently inside ZMQ's own high-water mark. Slowing the database sink down
//! is the price, and it is the right one: a message dropped there is gone
//! from both sinks, and a message dropped at the edge is gone from a feed
//! that will report the same system again.

use crate::sink::{Row, Sink};
use async_channel::{Receiver, Sender, TrySendError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use elite_journal::entry::market::{BlackMarket, Market, Outfitting, Shipyard};
use elite_journal::entry::{Entry, Event};
use elite_journal::system::Coordinate;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tracing::debug;

/// How many readings may wait for the worker to come back from a catch-up.
///
/// Fifty thousand is about half an hour of a quiet feed and twenty-five
/// minutes of a busy one, against a full build that takes an hour — so a cold
/// start overflows and a warm one does not, which is exactly the split the
/// two policies are for. What it costs is a pointer and an enum tag apiece;
/// the events themselves are shared with the database sink rather than
/// copied.
pub const BUFFERED: usize = 50_000;

/// Whoever wrote an entry, as far as the index is concerned.
///
/// Nobody. [`Index::entry`](crate::sink::Index::entry) files nothing under a
/// commander and says why — an EDDN uploader id is an anonymised sender
/// rather than a person, and the galaxy tracks a journal's commander from the
/// journal's own events. So the name a collect-side sink was handed is not
/// carried across, and a reading is one pointer rather than a pointer and a
/// string allocated per message.
const NOBODY: &str = "";

/// One thing an index derives from, owned so it can cross a thread.
#[derive(Clone, Debug)]
pub enum Reading {
    /// An event, shared with whatever other sink was handed the same one.
    Entry(Arc<Entry<Event>>),
    /// A row out of a dump, which is the one shape that is not an event.
    System(Row),
}

impl Reading {
    /// Give this to the sink on the other side.
    pub async fn apply(self, sink: &mut dyn Sink) {
        match self {
            Reading::Entry(entry) => sink.entry(entry, NOBODY).await,
            Reading::System(row) => sink.system(&row).await,
        }
    }
}

/// Whether the worker is taking readings as they arrive.
///
/// Shared rather than passed, because the two halves are on different
/// threads: the worker sets it once, when its sink is open and it is about to
/// drain, and every relay reads it per message. `Relaxed` on both sides is
/// enough — nothing is being published through this flag, and a relay that
/// reads it one message late does one more counted drop or one more wait.
#[derive(Clone)]
pub struct Live(Arc<AtomicBool>);

impl Live {
    pub fn new() -> Live {
        Live(Arc::new(AtomicBool::new(false)))
    }

    /// The worker is draining now, so nothing more may be dropped.
    pub fn now(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn yet(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// How many readings were dropped for a full buffer, so far this round.
///
/// Counted rather than flagged: a round that dropped two is a channel that
/// was briefly full, and one that dropped four million is a build that was
/// never going to keep up. Both answer the same way, and the number is the
/// only thing that says which happened.
#[derive(Clone)]
pub struct Dropped(Arc<AtomicU64>);

impl Dropped {
    pub fn new() -> Dropped {
        Dropped(Arc::new(AtomicU64::new(0)))
    }

    /// What has been dropped since the count was last cleared.
    pub fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    /// Start a round's count over, answering what the last one came to.
    pub fn clear(&self) -> u64 {
        self.0.swap(0, Ordering::Relaxed)
    }
}

/// A [`Sink`] that puts what it is given on the index worker's channel.
pub struct Relay {
    readings: Sender<Reading>,
    live: Live,
    dropped: Dropped,
    /// Readings that reached the channel, for the line at the end of a run.
    sent: u64,
}

impl Relay {
    /// The channel and the two sides of it.
    ///
    /// Bounded at [`BUFFERED`]. The sender is cloned once per source — every
    /// source feeds the one worker — and the receiver is the worker's.
    pub fn channel() -> (Sender<Reading>, Receiver<Reading>) {
        async_channel::bounded(BUFFERED)
    }

    pub fn new(
        readings: Sender<Reading>,
        live: Live,
        dropped: Dropped,
    ) -> Relay {
        Relay { readings, live, dropped, sent: 0 }
    }

    /// Put a reading on the channel, or account for why it did not go.
    ///
    /// The module header is the whole of the reasoning: dropped while the
    /// worker is catching up, waited on once it is live.
    async fn place(&mut self, reading: Reading) {
        if self.live.yet() {
            // A closed channel is a worker that has finished or failed.
            // Nothing to say per message about it: `finish` says it once.
            if self.readings.send(reading).await.is_ok() {
                self.sent += 1;
            }
            return;
        }

        match self.readings.try_send(reading) {
            Ok(()) => self.sent += 1,
            Err(TrySendError::Full(_)) => {
                self.dropped.0.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Closed(_)) => {}
        }
    }
}

#[async_trait]
impl Sink for Relay {
    async fn entry(&mut self, entry: Arc<Entry<Event>>, _user: &str) {
        self.place(Reading::Entry(entry)).await;
    }

    /// Nothing. An index has no keys and nothing to point at one, so there is
    /// nothing here worth a place in the buffer.
    async fn ensure_system(
        &mut self,
        _at: DateTime<Utc>,
        _user: &str,
        _address: i64,
        _name: Option<&str>,
        _position: Option<Coordinate>,
        _why: &str,
    ) -> bool {
        true
    }

    async fn system(&mut self, row: &Row) {
        self.place(Reading::System(row.clone())).await;
    }

    /// The four station schemas, which an index has nowhere to put. Dropped
    /// here rather than sent and ignored on the other side.
    async fn market(&mut self, _at: DateTime<Utc>, _user: &str, _: &Market) {}
    async fn outfitting(
        &mut self,
        _at: DateTime<Utc>,
        _user: &str,
        _: &Outfitting,
    ) {
    }
    async fn shipyard(
        &mut self,
        _at: DateTime<Utc>,
        _user: &str,
        _: &Shipyard,
    ) {
    }
    async fn black_market(
        &mut self,
        _at: DateTime<Utc>,
        _user: &str,
        _: &BlackMarket,
    ) {
    }

    /// Nothing is held here: a reading is on the channel or it is not.
    ///
    /// What a publish means is the worker's, and it is on its own beat
    /// rather than on whatever beat a source happens to call this on.
    async fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// Nothing, and deliberately not a close.
    ///
    /// The channel is shared: every source has a relay of its own onto it,
    /// so a close here would end the run for the others the moment the
    /// first dump finished. What tells the worker there is no more coming
    /// is the last sender being dropped, which is the last source task
    /// ending.
    async fn finish(&mut self) -> Result<(), String> {
        debug!(sent = self.sent, "this source has no more for the index");
        Ok(())
    }

    fn said(&self) -> String {
        format!("{} readings handed to the index worker", self.sent)
    }
}
