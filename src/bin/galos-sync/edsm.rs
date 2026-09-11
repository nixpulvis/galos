//! EDSM's nightly dumps and its web API
//!
//! Both hand over the same shape: a system, where it is, and the political
//! columns somebody read off it. Nothing below system level, so nothing here
//! ever produces a scan, a station or a body — which is why it goes through
//! [`Sink::system`](crate::sink::Sink::system) and not through the event path
//! the journal and EDDN share.
//!
//! Stamped `Utc::now()` rather than with a time from the dump, which is what
//! this has always done: EDSM's files say when a system was last *updated in
//! EDSM* and not when the reading was taken, and the two are far enough apart
//! that using it would push year-old readings over fresh ones. The cost is
//! that an EDSM import lands in the newest Recency bucket. Worth knowing when
//! reading the map right after one.

use crate::bar;
use crate::sink::{Row, Sink};
use crate::Shutdown;
use chrono::offset::Utc;
use std::path::{Path, PathBuf};

/// A nightly dump already on disk: `--from edsm=PATH`.
pub struct Dump {
    pub path: PathBuf,
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

        let by = format!("EDSM file: {}", self.path.display());
        place(sink, shutdown, systems, &by).await;
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

        place(sink, shutdown, systems, "EDSM API").await;
        true
    }
}

/// Write what was read, saying which reading it was.
///
/// The same loop either way in, which is the whole reason the two ways in
/// are two structs and one reader: a dump and an API answer differ in how
/// they are asked and not at all in what comes back.
async fn place(
    sink: &mut dyn Sink,
    shutdown: &Shutdown,
    systems: Vec<edsm::System>,
    by: &str,
) {
    let bar = bar::progress(systems.len() as u64);
    for system in bar.wrap_iter(systems.into_iter()) {
        // A dump is millions of rows and the run may have been asked to
        // stop an hour into one. What has been written stands: every write
        // is its own guarded upsert and the next run re-reads the file.
        if shutdown.asked() {
            bar.abandon_with_message("stopped");
            return;
        }
        // No id is nothing to key by; no coordinates is nothing to place.
        let (Some(id), Some(coords)) = (system.id, system.coords) else {
            continue;
        };
        bar.set_message(format!("[EDSM] {}", system.name));
        sink.system(&Row {
            address: id as i64,
            name: system.name,
            position: Some(coords),
            population: system.information.population,
            security: system.information.security,
            government: system.information.government,
            allegiance: system.information.allegiance,
            primary_economy: system.information.economy,
            secondary_economy: system.information.second_economy,
            updated_at: Utc::now(),
            updated_by: by.to_string(),
        })
        .await;
    }
    bar.finish();
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
