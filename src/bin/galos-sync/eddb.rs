//! A dump from EDDB, which is gone
//!
//! The site closed and stopped publishing, so nothing new arrives here and
//! nothing this reads is newer than the day it shut. What it takes is a dump
//! already on disk, and that is all it will ever take. The systems in one are
//! stamped with their own `updated_at`, so the guards downstream keep a newer
//! reading from EDDN or a journal from being written over — and so an index
//! files them in the Recency bucket they belong in rather than making a dead
//! site look like news.

use crate::bar;
use crate::sink::{Row, Sink, To};
use clap::Parser;
use elite_journal::system::Coordinate;
use std::path::PathBuf;
use tracing::warn;

/// Sync from a saved EDDB dump.
#[derive(Parser)]
pub struct Cli {
    /// The dump CSV to read.
    #[arg(name = "PATH")]
    pub path: String,

    /// Where to write what is read: `db`, or `index=DIR`.
    #[arg(long = "to", value_name = "SINK", default_value = "db")]
    pub to: To,

    /// Resume file for an index sink, kept outside the served directory.
    /// `DIR.checkpoint` beside the index directory by default.
    #[arg(long, value_name = "FILE")]
    pub checkpoint: Option<PathBuf>,
}

impl Cli {
    /// Read the dump, answering whether it could be opened at all.
    pub async fn read(&self, sink: &mut dyn Sink) -> bool {
        let mut dump = match eddb::Dump::csv(&self.path) {
            Ok(dump) => dump,
            Err(err) => {
                warn!(file = %self.path, error = %err, "unreadable dump");
                return false;
            }
        };

        let bar = bar::progress(dump.len());
        let _drawing = bar::under(&bar);
        for result in bar.wrap_iter(dump.into_iter()) {
            let Ok(system) = result else { continue };
            // A row with no address is one nothing can be keyed by, here or
            // in a tree.
            let Some(address) = system.ed_system_address else { continue };

            bar.set_message(format!("[EDDB] {}", system.name));
            sink.system(&Row {
                address: address as i64,
                name: system.name,
                position: Some(Coordinate {
                    x: system.coords.x,
                    y: system.coords.y,
                    z: system.coords.z,
                }),
                population: system.population,
                security: system.security,
                government: system.government,
                allegiance: system.allegiance,
                primary_economy: system.primary_economy,
                secondary_economy: None,
                updated_at: system.updated_at,
                updated_by: "EDDB dump".to_string(),
            })
            .await;
        }
        bar.finish();
        true
    }
}
