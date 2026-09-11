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
use crate::sink::{Row, Sink, To};
use chrono::offset::Utc;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// Sync from EDSM.
#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    from: From,
}

#[derive(Subcommand)]
enum From {
    /// Read a nightly dump already on disk.
    File(FileCli),
    /// Ask the web API about one system and its neighbourhood.
    Api(ApiCli),
}

/// Where what is read goes, shared by both ways in.
#[derive(Args, Clone)]
pub struct Into {
    /// Where to write what is read: `db`, or `index=DIR`. Repeatable,
    /// and `db` where it is not said at all.
    #[arg(long = "to", value_name = "SINK")]
    pub to: Vec<To>,

    /// Resume file for an index sink, kept outside the served directory.
    /// `DIR.checkpoint` beside the index directory by default.
    #[arg(long, value_name = "FILE")]
    pub checkpoint: Option<PathBuf>,
}

#[derive(Args)]
struct FileCli {
    /// The dump JSON to read.
    #[arg(name = "PATH")]
    path: String,
    #[command(flatten)]
    into: Into,
}

#[derive(Args)]
struct ApiCli {
    /// The system to ask about.
    #[arg(name = "NAME")]
    name: String,

    /// Take everything in a cube this many light years across.
    #[arg(long, short, conflicts_with = "sphere")]
    cube: Option<u32>,
    /// Take everything within this many light years.
    #[arg(long, short)]
    sphere: Option<u32>,
    #[command(flatten)]
    into: Into,
}

impl Cli {
    /// Which sinks were named, whichever way in was used.
    pub fn to(&self) -> &[To] {
        match &self.from {
            From::File(cli) => &cli.into.to,
            From::Api(cli) => &cli.into.to,
        }
    }

    /// Where the resume point goes, likewise.
    pub fn checkpoint(&self) -> Option<&std::path::Path> {
        match &self.from {
            From::File(cli) => cli.into.checkpoint.as_deref(),
            From::Api(cli) => cli.into.checkpoint.as_deref(),
        }
    }

    /// Read what was asked for, answering whether it could be read.
    pub async fn read(&self, sink: &mut dyn Sink) -> bool {
        let (systems, by) = match &self.from {
            From::File(cli) => {
                // `edsm::json` unwraps both of these. A path typed wrong is
                // not a thing to take the program down over: every other
                // source here says what it could not read and answers that
                // it read nothing.
                match dump(&cli.path) {
                    Ok(systems) => {
                        (systems, format!("EDSM file: {}", cli.path))
                    }
                    Err(err) => {
                        tracing::warn!(
                            file = %cli.path,
                            error = %err,
                            "unreadable dump",
                        );
                        return false;
                    }
                }
            }
            From::Api(cli) => {
                let asked = if let Some(n) = cli.sphere {
                    edsm::api::systems_sphere(&cli.name, Some(n as f64), None)
                } else if let Some(n) = cli.cube {
                    edsm::api::systems_cube(&cli.name, Some(n as f64))
                } else {
                    edsm::api::systems(&cli.name)
                };
                match asked {
                    Ok(systems) => (systems, "EDSM API".to_string()),
                    Err(err) => {
                        tracing::warn!(
                            system = %cli.name,
                            error = %err,
                            "the API would not answer",
                        );
                        return false;
                    }
                }
            }
        };

        let bar = bar::progress(systems.len() as u64);
        let drawing = bar::under(&bar);
        for system in bar.wrap_iter(systems.into_iter()) {
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
                updated_by: by.clone(),
            })
            .await;
        }
        bar.finish();
        drop(drawing);
        true
    }
}

/// A nightly dump read off the disk, saying what stopped it.
///
/// The reading `edsm::json` does, with the two failures it unwraps answered
/// instead: a path that is not there and a file that is not one of these.
/// Both are things a command line gets wrong, and neither is worth a
/// backtrace.
fn dump(path: &str) -> Result<Vec<edsm::System>, String> {
    let file = std::fs::File::open(path).map_err(|err| err.to_string())?;
    serde_json::from_reader(std::io::BufReader::new(file))
        .map_err(|err| err.to_string())
}
