#![cfg(unix)]
use crate::journal::record;
use crate::Run;
use async_std::task;
use eddn::{subscribe, Message, URL};
use galos_db::Database;
use std::time::Duration;
use structopt::StructOpt;
use tracing::{debug, warn};

/// How long EDDN may carry nothing before its connection is replaced
///
/// A busy hour of it runs at 31 messages a second and does not go a second
/// without one, so two minutes of quiet is not EDDN being quiet.
const STALL: Duration = Duration::from_secs(120);

#[derive(StructOpt, Debug)]
pub struct Cli {
    // Type as a URL? ZMQ doesn't bother :(
    #[structopt(short = "r", long = "remote", default_value = URL, help = "ZMQ remote address")]
    pub url: String,

    #[structopt(
        long = "stall",
        help = "Seconds of silence before the connection is replaced, or 0 to leave it alone"
    )]
    pub stall: Option<u64>,
    // TODO: Filters?
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let stall = match self.stall {
            None => Some(STALL),
            Some(0) => None,
            Some(secs) => Some(Duration::from_secs(secs)),
        };

        for result in subscribe(&self.url, stall) {
            if let Ok(envelop) = result {
                process_message(
                    db,
                    envelop.message,
                    &envelop.schema_ref,
                    &envelop.header.uploader_id,
                );
            } else if let Err(err) = result {
                warn!(error = %err, "unreadable message");
            }
        }
    }
}

/// Hand a message to whatever writes what it holds
///
/// Everything EDDN carries is something the game also writes to a journal, so
/// nothing is written here. What this does is place a message: the payload has
/// already been read as the schema above it said it should be, and each of
/// those shapes has one thing that knows what to do with it.
fn process_message(
    db: &Database,
    message: Message,
    schema_ref: &str,
    user: &str,
) {
    task::block_on(async {
        match message {
            Message::Journal(entry) => record::entry(db, &entry, user).await,
            Message::Commodity(e) => {
                record::market(db, e.timestamp, user, &e.event).await
            }

            // The three schemas whose payload carries no `event` key, and so
            // could not be reached at all until messages were placed by their
            // `$schemaRef`.
            Message::Outfitting(e) => {
                record::outfitting(db, e.timestamp, user, &e.event).await
            }

            Message::Shipyard(e) => {
                record::shipyard(db, e.timestamp, user, &e.event).await
            }

            Message::BlackMarket(e) => {
                record::black_market(db, e.timestamp, user, &e.event).await
            }

            // A schema nothing here reads yet. Said at `debug` because it is
            // most of what EDDN carries, and saying it at all is the only way
            // to know what is going by.
            Message::Unmodeled(_) => {
                debug!(schema = %schema_ref, "unmodeled schema")
            }
        }
    })
}
