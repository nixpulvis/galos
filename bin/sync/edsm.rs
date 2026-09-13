//! EDSM's nightly dumps and its web API
//!
//! Both hand over the same shape: a system, where it is, and the political
//! columns somebody read off it. Nothing below system level, so nothing here
//! ever produces a scan, a station or a body — which is why it goes through
//! [`Sink::system`] and not through the event path the journal and EDDN
//! share.
//!
//! Stamped `Utc::now()` rather than with a time from the dump, which is what
//! this has always done: EDSM's files say when a system was last *updated in
//! EDSM* and not when the reading was taken, and the two are far enough apart
//! that using it would push year-old readings over fresh ones. The cost is
//! that an EDSM import lands in the newest Recency bucket. Worth knowing when
//! reading the map right after one.

use chrono::offset::Utc;
use galos::bar;
use galos::sink::{Sink, SystemName, SystemReport};
use galos::{Shard, Shutdown};
use std::path::{Path, PathBuf};

/// A nightly dump already on disk: `--from edsm=PATH`.
pub struct Dump {
    pub path: PathBuf,

    /// One system in `n`, where the run was told to take a share of the file.
    pub shard: Option<Shard>,
}

/// The web API, asked about one system: `--from edsm-api=NAME`.
pub struct Api {
    pub name: String,
    /// Everything in a cube this many light years across, from `--cube`.
    pub cube: Option<u32>,
    /// Everything within this many light years, from `--sphere`.
    pub sphere: Option<u32>,
}

impl Dump {
    /// Read the file, answering whether it could be read at all.
    pub async fn read(&self, sink: &mut dyn Sink, shutdown: &Shutdown) -> bool {
        // `edsm::json` unwraps both of these. A path typed wrong is not a
        // thing to take the program down over: every other source here says
        // what it could not read and answers that it read nothing.
        let systems = match dump(&self.path) {
            Ok(systems) => systems,
            Err(err) => {
                tracing::warn!(
                    file = %self.path.display(),
                    error = %err,
                    "unreadable dump",
                );
                return false;
            }
        };

        let by = crate::from::published("EDSM", &self.path);
        place(sink, shutdown, systems, &by, self.shard).await;
        true
    }
}

impl Api {
    /// Ask the API, answering whether it answered.
    pub async fn read(&self, sink: &mut dyn Sink, shutdown: &Shutdown) -> bool {
        let asked = if let Some(n) = self.sphere {
            edsm::api::systems_sphere(&self.name, Some(n as f64), None)
        } else if let Some(n) = self.cube {
            edsm::api::systems_cube(&self.name, Some(n as f64))
        } else {
            edsm::api::systems(&self.name)
        };
        let systems = match asked {
            Ok(systems) => systems,
            Err(err) => {
                tracing::warn!(
                    system = %self.name,
                    error = %err,
                    "the API would not answer",
                );
                return false;
            }
        };

        place(sink, shutdown, systems, "EDSM API", None).await;
        true
    }
}

/// Write what was read, saying which reading it was.
///
/// The same loop either way in, which is the whole reason the two ways in
/// are two structs and one reader: a dump and an API answer differ in how
/// they are asked and not at all in what comes back.
///
/// `shard` is a file's share and is always `None` for the API, an answer
/// about one system's neighbourhood being nothing to divide.
async fn place(
    sink: &mut dyn Sink,
    shutdown: &Shutdown,
    systems: Vec<edsm::System>,
    by: &str,
    shard: Option<Shard>,
) {
    // The shard's own systems, not the file's: a bar counting up to the whole
    // file would stop at a fraction of itself and say nothing about why.
    //
    // Records, not bytes: `edsm::json` parses the whole array before
    // anything walks it, so the count is known and a byte position is not.
    // The line reads the same either way; see `bar::imported`.
    let read = systems.len() as u64;
    let tag = match shard {
        Some(shard) => format!("EDSM {shard}"),
        None => "EDSM".to_string(),
    };
    let mut bar = bar::imported(
        &tag,
        bar::Extent::Records(match shard {
            Some(shard) => shard.share(read),
            None => read,
        }),
    );
    for (at, system) in systems.into_iter().enumerate() {
        // A dump is millions of rows and the run may have been asked to
        // stop an hour into one. What has been written stands: every write
        // is its own guarded upsert and the next run re-reads the file.
        if shutdown.asked() {
            bar.abandoned("stopped");
            return;
        }
        // Another process's system. Counted by position in the file, so the
        // shards agree about whose it is without talking to each other.
        if let Some(shard) = shard {
            if !shard.mine(at as u64) {
                continue;
            }
        }
        bar.through(1);
        // No id is nothing to key by; no coordinates is nothing to place.
        let (Some(id), Some(coords)) = (system.id, system.coords) else {
            bar.missed();
            continue;
        };
        let landed = sink
            .system(
                &SystemReport {
                    name: Some(SystemName::new(system.name)),
                    position: Some(coords),
                    population: system.information.population,
                    security: system.information.security,
                    government: system.information.government,
                    allegiance: system.information.allegiance,
                    primary_economy: system.information.economy,
                    secondary_economy: system.information.second_economy,
                    ..SystemReport::new(id as i64, Utc::now())
                },
                by,
            )
            .await;
        bar.took(landed);
    }
    bar.done();
}

/// A nightly dump read off the disk, saying what stopped it.
///
/// The reading `edsm::json` does, with the two failures it unwraps answered
/// instead: a path that is not there and a file that is not one of these.
/// Both are things a command line gets wrong, and neither is worth a
/// backtrace.
fn dump(path: &Path) -> Result<Vec<edsm::System>, String> {
    let file = std::fs::File::open(path).map_err(|err| err.to_string())?;
    serde_json::from_reader(std::io::BufReader::new(file))
        .map_err(|err| err.to_string())
}
