//! A dump from EDDB, which is gone
//!
//! The site closed and stopped publishing, so nothing new arrives here and
//! nothing this reads is newer than the day it shut. What it takes is a dump
//! already on disk, and that is all it will ever take. The systems in one are
//! stamped with their own `updated_at`, so the guards downstream keep a newer
//! reading from EDDN or a journal from being written over — and so an index
//! files them in the Recency bucket they belong in rather than making a dead
//! site look like news.

use elite_journal::system::Coordinate;
use galos::bar;
use galos::sink::{Sink, SystemReport};
use galos::{Shard, Shutdown};
use std::path::PathBuf;
use tracing::warn;

/// A saved dump on disk: `--from eddb=PATH`.
pub struct Eddb {
    pub path: PathBuf,

    /// One row in `n`, where the run was told to take a share of the file.
    pub shard: Option<Shard>,
}

impl Eddb {
    /// Read the dump, answering whether it could be opened at all.
    pub async fn read(&self, sink: &mut dyn Sink, shutdown: &Shutdown) -> bool {
        let mut dump = match eddb::Dump::csv(&self.path) {
            Ok(dump) => dump,
            Err(err) => {
                warn!(
                    file = %self.path.display(),
                    error = %err,
                    "unreadable dump",
                );
                return false;
            }
        };

        // The shard's own rows, not the file's: a bar counting up to the whole
        // file would stop at a fraction of itself and say nothing about why.
        //
        // Records, not bytes: the CSV is counted before it is walked, so
        // the count is known and a byte position is not. The line reads the
        // same either way; see `bar::imported`.
        let rows = dump.len();
        let by = match self.shard {
            Some(shard) => format!("EDDB {shard}"),
            None => "EDDB".to_string(),
        };
        let mut bar = bar::imported(
            &by,
            bar::Extent::Records(match self.shard {
                Some(shard) => shard.share(rows),
                None => rows,
            }),
        );
        for (at, result) in dump.into_iter().enumerate() {
            // Stopped part way through is a dump half written, which is
            // exactly what an interrupted run of this always was: every
            // write is its own guarded upsert and the next run re-reads
            // the file from the top.
            if shutdown.asked() {
                bar.abandoned("stopped");
                return true;
            }
            // Another process's row. Counted by position in the file, so the
            // shards agree about whose it is without talking to each other.
            if let Some(shard) = self.shard {
                if !shard.mine(at as u64) {
                    continue;
                }
            }
            bar.through(1);
            let Ok(system) = result else {
                bar.missed();
                continue;
            };
            // A row with no address is one nothing can be keyed by, here or
            // in a tree.
            let Some(address) = system.ed_system_address else {
                bar.missed();
                continue;
            };

            let landed = sink
                .system(
                    &SystemReport {
                        name: Some(system.name),
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
                        ..SystemReport::new(address as i64, system.updated_at)
                    },
                    "EDDB dump",
                )
                .await;
            bar.took(landed);
        }
        bar.done();
        true
    }
}
