//! The live feed: everyone else's game, forwarded
//!
//! A ZMQ subscription that never returns. Everything EDDN carries is
//! something the game also writes to a journal, so nothing is written here:
//! what this does is place a message — the payload has already been read as
//! the schema above it said it should be — and hand it to the sink.
//!
//! Into an index it is the same feed with no database under it. What that
//! gets you is a `.galos_index` kept current by the galaxy at large on a
//! machine with no Postgres on it; what it gives up is everything the index
//! has no room for, which is stations, markets and signals. The sink says so
//! itself rather than this having to know.
//!
//! ## Why it is the one source with a thread
//!
//! `eddn::subscribe` hands back an iterator that parks the OS thread it is
//! called on: `recv_timeout` on a ZMQ socket parks it for the stall window,
//! and a subscription being replaced sleeps five seconds before it tries
//! again. Neither yields to an executor, so a `for` loop over it inside an
//! async task stops every other task on that thread — the journal follower,
//! the index worker's channel, the timers.
//!
//! So the loop gets a thread of its own and the envelopes cross to the async
//! side through a bounded channel. The sink stays where it was: on the async
//! side, owned by the task that reads the channel, never handed to the
//! thread.

use crate::sink::Sink;
use crate::Shutdown;
use async_channel::{Receiver, TrySendError};
use eddn::{subscribe, Envelope, Message};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Where the feed is, where `--remote` says nothing.
///
/// The `eddn` crate's own address, named here so the command line can
/// default to it without the module and the crate having to be told apart
/// at the call site.
pub const URL: &str = eddn::URL;

/// How long EDDN may carry nothing before its connection is replaced
///
/// A busy hour of it runs at 31 messages a second and does not go a second
/// without one, so two minutes of quiet is not EDDN being quiet.
pub const STALL: Duration = Duration::from_secs(120);

/// How many envelopes may wait for the async side to take them.
///
/// Ten thousand is five minutes of a busy feed, which is far longer than
/// anything on the async side has any business taking. What fills this is
/// the index worker's channel being full while the worker is live, which
/// stops the task below reading — see [`crate::sink::relay`]. This is where
/// that shows up as a number rather than as ZMQ quietly dropping at its own
/// high-water mark.
const WAITING: usize = 10_000;

/// How long a quiet channel is waited on before the run is asked whether it
/// is still wanted.
///
/// The feed does not go a second without a message, so this is only ever
/// paid on the way out: it is how long Ctrl-C takes to be noticed here.
const TICK: Duration = Duration::from_millis(250);

/// The live feed, and what it takes to reach it.
pub struct Eddn {
    /// ZMQ remote address, from `--remote`.
    pub url: String,
    /// Seconds of silence before the connection is replaced, from `--stall`,
    /// and [`None`] where it was told to leave the connection alone.
    pub stall: Option<Duration>,
}

impl Eddn {
    /// Follow the feed until it ends or the run is asked to stop.
    pub async fn read(&self, sink: &mut dyn Sink, shutdown: &Shutdown) -> bool {
        let envelopes = feed(self.url.clone(), self.stall, shutdown.clone());
        info!(url = %self.url, "subscribed to EDDN");

        loop {
            match async_std::future::timeout(TICK, envelopes.recv()).await {
                Ok(Ok(envelope)) => {
                    place(
                        sink,
                        envelope.message,
                        &envelope.schema_ref,
                        &envelope.header.uploader_id,
                    )
                    .await
                }
                // The thread has stopped and nothing more is coming.
                Ok(Err(_)) => break,
                Err(_) => {}
            }
            if shutdown.asked() {
                break;
            }
        }
        true
    }
}

/// Read the subscription on a thread of its own.
///
/// Answers the receiving end. What crosses is an [`Envelope`] and nothing
/// else: a message that could not be read is said here, where it happened,
/// rather than carried over to be said somewhere with less to say about it.
///
/// The thread is not joined. It is parked inside ZMQ for up to the stall
/// window and there is no way to interrupt that from outside, so a run
/// shutting down closes the channel and leaves it: the thread holds no
/// durable state, notices the closed channel on its next message, and the
/// process exits either way once the halves that *do* hold state have been
/// closed out.
fn feed(
    url: String,
    stall: Option<Duration>,
    shutdown: Shutdown,
) -> Receiver<Envelope> {
    let (sender, receiver) = async_channel::bounded(WAITING);
    std::thread::Builder::new()
        .name("eddn".to_string())
        .spawn(move || {
            let mut dropped: u64 = 0;
            for result in subscribe(&url, stall) {
                match result {
                    Ok(envelope) => match sender.try_send(envelope) {
                        Ok(()) => {}
                        // The async side is not keeping up, which means the
                        // index worker is not: see the module header. Said
                        // at one, two, four, eight, so a feed that is
                        // permanently behind does not write a line a
                        // message about it.
                        Err(TrySendError::Full(_)) => {
                            dropped += 1;
                            if dropped.is_power_of_two() {
                                warn!(
                                    dropped = dropped,
                                    "the sync is behind the feed; messages \
                                     are being dropped as they arrive",
                                );
                            }
                        }
                        Err(TrySendError::Closed(_)) => break,
                    },
                    Err(err) => warn!(error = %err, "unreadable message"),
                }
                if shutdown.asked() {
                    break;
                }
            }
            debug!(dropped = dropped, "the EDDN thread is done");
        })
        .expect("a thread for the feed");
    receiver
}

/// Hand a message to whatever writes what it holds
///
/// Everything EDDN carries is something the game also writes to a journal, so
/// nothing is written here. What this does is place a message: the payload has
/// already been read as the schema above it said it should be, and each of
/// those shapes has one thing that knows what to do with it.
async fn place(
    sink: &mut dyn Sink,
    message: Message,
    schema_ref: &str,
    user: &str,
) {
    match message {
        // Shared rather than copied: with `--db --index` this same entry is
        // written to Postgres and handed to the index worker.
        Message::Journal(entry) => sink.entry(Arc::new(entry), user).await,
        Message::Commodity(e) => sink.market(e.timestamp, user, &e.event).await,

        // These three and the market above: the four schemas whose payload
        // carries no `event` key, and so could not be reached at all until
        // messages were placed by their `$schemaRef`.
        Message::Outfitting(e) => {
            sink.outfitting(e.timestamp, user, &e.event).await
        }
        Message::Shipyard(e) => {
            sink.shipyard(e.timestamp, user, &e.event).await
        }
        Message::BlackMarket(e) => {
            sink.black_market(e.timestamp, user, &e.event).await
        }

        // A schema nothing here reads yet. Said at `debug` because it is
        // most of what EDDN carries, and saying it at all is the only way
        // to know what is going by.
        Message::Unmodeled(_) => {
            debug!(schema = %schema_ref, "unmodeled schema")
        }
    }
}
