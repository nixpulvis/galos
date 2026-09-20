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
//! the rows the catch-up reads back hold the entry already — the database
//! was written before this saw it — and the worker answers a nonzero count
//! by discarding what it buffered and running another round, which reads it
//! back. See [`crate::read::derive::buffered`], and note that only
//! `galos-index ingest --catch-up` has rounds to recover a drop with.
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

use crate::sink::{Landed, Reporter, Sink, SystemReport};
use async_channel::{Receiver, Sender, TrySendError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use elite_journal::entry::market::{BlackMarket, Market, Outfitting, Shipyard};
use elite_journal::entry::{Entry, Event};
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

/// Who a system report is said to be by on the other side.
///
/// Nobody. [`Index::system`](crate::sink::Index::system) records no
/// provenance above body level — an index has no `updated_by` for a
/// system — so the name a collect-side sink was handed is not carried for
/// one.
const NOBODY: &str = "";

/// Whoever a source named, owned so it can cross a thread.
///
/// A [`Reporter`] borrows its name from the source that read it and a
/// reading outlives that borrow by a channel's worth of time, so the name
/// is held behind an [`Arc`] and shared with the last reading that carried
/// the same one.
///
/// Which of the two it is crosses as well as the name. `updated_by` is
/// provenance and both kinds are some, but only one of them is a claim
/// about a person, and a boundary that forgot which would be the one place
/// in the program where an anonymised sender becomes a commander.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Named {
    /// A commander, as their own journal named them.
    Commander(Arc<str>),
    /// An EDDN sender's uploader id, or the file a dump was read out of.
    Uploader(Arc<str>),
    /// Nothing named anybody: the id was dropped before this.
    Nobody,
}

impl Named {
    /// This name, for the sink on the other side.
    fn same(&self) -> Reporter<'_> {
        match self {
            Named::Commander(who) => Reporter::Commander(who),
            Named::Uploader(who) => Reporter::Uploader(who),
            Named::Nobody => Reporter::Nobody,
        }
    }

    /// Whether `by` is the name this already is, allocation and all.
    fn is(&self, by: &Reporter<'_>) -> bool {
        match (self, by) {
            (Named::Commander(held), Reporter::Commander(who))
            | (Named::Uploader(held), Reporter::Uploader(who)) => {
                held.as_ref() == *who
            }
            (Named::Nobody, Reporter::Nobody) => true,
            _ => false,
        }
    }

    /// Whoever `by` named, owned.
    fn of(by: Reporter<'_>) -> Named {
        match by {
            Reporter::Commander(who) => Named::Commander(Arc::from(who)),
            Reporter::Uploader(who) => Named::Uploader(Arc::from(who)),
            Reporter::Nobody => Named::Nobody,
        }
    }
}

/// One thing an index derives from, owned so it can cross a thread.
#[derive(Clone, Debug)]
pub enum Reading {
    /// An event, shared with whatever other sink was handed the same one,
    /// and whoever the source that read it named.
    Entry(Arc<Entry<Event>>, Named),
    /// A system as a source reported it, which is the one shape that is not
    /// an event.
    System(SystemReport),
}

impl Reading {
    /// Give this to the sink on the other side.
    ///
    /// Drops what that sink made of it: this is the far end of a channel
    /// and nobody here is waiting to hear. The worker's sink keeps its own
    /// counts and says them at the end of the run.
    pub async fn apply(self, sink: &mut dyn Sink) {
        match self {
            Reading::Entry(entry, named) => {
                sink.entry(entry, named.same()).await;
            }
            Reading::System(report) => {
                sink.system(&report, NOBODY).await;
            }
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
#[derive(Clone, Default)]
pub struct Live(Arc<AtomicBool>);

impl Live {
    pub fn new() -> Live {
        Live::default()
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
#[derive(Clone, Default)]
pub struct Dropped(Arc<AtomicU64>);

impl Dropped {
    pub fn new() -> Dropped {
        Dropped::default()
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
    /// The name last handed over, so that a source naming the same one
    /// every message pays a comparison and a refcount rather than a string.
    ///
    /// It cannot be sent once per batch instead. Every source clones this
    /// sender into the one channel, so two of them interleave arbitrarily
    /// and a "who is reporting now" message would apply to whatever
    /// happened to arrive next — which is the bug this exists to fix, one
    /// indirection further out.
    named: Named,
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
        Relay { readings, live, dropped, sent: 0, named: Named::Nobody }
    }

    /// The name to send alongside an entry, shared with the last one where
    /// it is the same name.
    fn named(&mut self, by: Reporter<'_>) -> Named {
        if !self.named.is(&by) {
            self.named = Named::of(by);
        }
        self.named.clone()
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
    /// Whoever the source named crosses with the entry, both kinds of name
    /// alike: `updated_by` is provenance, and a body filed under a dump's
    /// own file is the only provenance a dump has.
    ///
    /// Returns [`None`]: nothing has been stored when this returns — the
    /// reading is on a channel, and the worker's own
    /// [`Index`](crate::sink::Index) is what stores and counts it. So a
    /// source's running total comes from the database where there is one,
    /// and the index reports its own at the end of the run.
    async fn entry(
        &mut self,
        entry: Arc<Entry<Event>>,
        by: Reporter<'_>,
    ) -> Option<Landed> {
        let named = self.named(by);
        self.place(Reading::Entry(entry, named)).await;
        None
    }

    async fn system(
        &mut self,
        report: &SystemReport,
        _user: &str,
    ) -> Option<Landed> {
        self.place(Reading::System(report.clone())).await;
        None
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One scan, as a journal or a dump states one.
    fn scan() -> Arc<Entry<Event>> {
        Arc::new(
            serde_json::from_str(
                r#"{"timestamp":"2026-08-08T12:00:00Z","event":"Scan",
                    "ScanType":"Detailed","StarSystem":"Sol",
                    "SystemAddress":10477373803,"BodyName":"Sol",
                    "BodyID":1,"StarType":"G","Subclass":2,
                    "StellarMass":1.0,"Radius":696000000.0,
                    "AbsoluteMagnitude":4.83,"Age_MY":4600,
                    "SurfaceTemperature":5778.0,"Luminosity":"Va",
                    "RotationPeriod":0.0,"AxialTilt":0.0,
                    "WasDiscovered":true,"WasMapped":false,
                    "DistanceFromArrivalLS":0.0}"#,
            )
            .expect("the scan should parse"),
        )
    }

    /// Whoever a source named crosses the channel, uploader and all
    ///
    /// Every source of a `--from` run reaches the index through here, so a
    /// name dropped at this boundary is a name no index ever sees. A dump's
    /// is the file it was read out of, which is all the provenance a
    /// published file has.
    #[test]
    fn the_name_a_source_gave_crosses_the_channel() {
        let (sender, readings) = Relay::channel();
        let mut relay = Relay::new(sender, Live::new(), Dropped::new());
        let live = Named::Commander("cmdr".into());
        let dumped = Named::Uploader("Spansh galaxy_7days.json".into());

        pollster::block_on(async {
            relay.entry(scan(), Reporter::Commander("cmdr")).await;
            relay
                .entry(scan(), Reporter::Uploader("Spansh galaxy_7days.json"))
                .await;
            relay.entry(scan(), Reporter::Nobody).await;
        });

        let crossed = |said: &str| match readings.try_recv() {
            Ok(Reading::Entry(_, named)) => named,
            other => panic!("{}: {:?}", said, other),
        };
        assert_eq!(crossed("the commander did not cross"), live);
        assert_eq!(crossed("the published file did not cross"), dumped);
        assert_eq!(crossed("nobody did not cross"), Named::Nobody);
    }
}
