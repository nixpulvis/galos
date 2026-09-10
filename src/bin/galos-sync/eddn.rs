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

use crate::sink::{Sink, To};
use clap::Parser;
use eddn::{subscribe, Message, URL};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// How long EDDN may carry nothing before its connection is replaced
///
/// A busy hour of it runs at 31 messages a second and does not go a second
/// without one, so two minutes of quiet is not EDDN being quiet.
const STALL: Duration = Duration::from_secs(120);

/// How often a sink holding its writes is asked to make them durable.
///
/// A database has written each message as it arrived and does nothing here.
/// An index has been editing a tree in memory, and this is the beat it
/// publishes on — the same order as `galos-sync db --watch`, and for the same
/// reason: a publish rewrites the index file whole, so doing it per message
/// at thirty a second would be thirty rewrites a second to move one system.
const PUBLISH_EVERY: Duration = Duration::from_secs(5);

/// Subscribe to EDDN and sync until killed.
#[derive(Parser)]
pub struct Cli {
    // TODO: Take an `Endpoint`, which parses, so a bad address is a
    // complaint about the argument rather than a panic on connecting.
    /// ZMQ remote address.
    #[arg(short = 'r', long = "remote", default_value = URL)]
    pub url: String,

    /// Seconds of silence before the connection is replaced, or 0 to leave
    /// it alone.
    #[arg(long = "stall", value_name = "SECS")]
    pub stall: Option<u64>,

    /// Where to write what arrives: `db`, or `index=DIR`.
    #[arg(long = "to", value_name = "SINK", default_value = "db")]
    pub to: To,

    /// Resume file for an index sink, kept outside the served directory.
    #[arg(long, value_name = "FILE", default_value = crate::sink::to::CHECKPOINT)]
    pub checkpoint: PathBuf,
    // TODO: Filters?
}

impl Cli {
    /// Follow the feed. Never returns short of the subscription ending.
    pub async fn read(&self, sink: &mut dyn Sink) -> bool {
        let stall = match self.stall {
            None => Some(STALL),
            Some(0) => None,
            Some(secs) => Some(Duration::from_secs(secs)),
        };

        let mut published = Instant::now();
        for result in subscribe(&self.url, stall) {
            match result {
                Ok(envelop) => {
                    place(
                        sink,
                        envelop.message,
                        &envelop.schema_ref,
                        &envelop.header.uploader_id,
                    )
                    .await
                }
                Err(err) => warn!(error = %err, "unreadable message"),
            }

            // On a beat rather than per message, and asked rather than
            // decided here: a database sink has nothing to do and says so by
            // doing nothing.
            if published.elapsed() >= PUBLISH_EVERY {
                if let Err(said) = sink.flush().await {
                    warn!(error = %said, "could not publish");
                }
                published = Instant::now();
            }
        }
        true
    }
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
        Message::Journal(entry) => sink.entry(&entry, user).await,
        Message::Commodity(e) => sink.market(e.timestamp, user, &e.event).await,

        // The three schemas whose payload carries no `event` key, and so
        // could not be reached at all until messages were placed by their
        // `$schemaRef`.
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
