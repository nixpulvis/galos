//! The galaxy database tool.
//!
//! ```sh
//! # Build the index into a directory and exit.
//! cargo run --release --bin galos-db -- index .galos_index
//!
//! # Rebuild one part of it, leaving the rest of the directory alone.
//! cargo run --release --bin galos-db -- index --only reaches
//!
//! # Follow the feed: build once, then publish changes every few seconds.
//! cargo run --release --bin galos-db -- index .galos_index --watch 5
//! ```
//!
//! In `--watch` mode the index rides on top of `galos-sync`: the sync writes
//! systems to the database and this follows the rows those writes leave,
//! moving each changed system in place and rewriting only the cells it touched.
//! The directory defaults to `.galos_index` and the interval to five seconds.
//! The connection is read from `DATABASE_URL` like every other tool.

use clap::{Parser, Subcommand, ValueEnum};
use galos_catalog::hyg;
use galos_db::index::Parts;
use galos_db::{catalog, index, Database};
use std::io::{stderr, IsTerminal};
use std::path::PathBuf;
use std::time::Duration;

/// Work with the galaxy database.
#[derive(Parser)]
#[command(name = "galos-db", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build the galaxy index from the database, or follow the feed to keep it current.
    Index {
        /// Directory to write the index into.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
        /// Resume file for --watch, kept outside the served index directory.
        #[arg(long, value_name = "FILE", default_value = ".galos_checkpoint")]
        checkpoint: PathBuf,
        /// Follow the feed, republishing every SECS seconds rather than exiting.
        #[arg(long, value_name = "SECS", num_args = 0..=1, default_missing_value = "5")]
        watch: Option<u64>,
        /// Rebuild only these parts, leaving the rest of the directory as it
        /// stands. Every part by default.
        #[arg(
            long,
            value_name = "PART",
            value_delimiter = ',',
            num_args = 1..,
            conflicts_with = "watch"
        )]
        only: Vec<Part>,
    },
    /// Compare a star catalog's positions against this database's.
    ///
    /// Matches by name — the only key the two share — and reports where they
    /// disagree about how far away a star is, which is the measurement that
    /// gets revised. The frame between them is fitted from the matched stars
    /// rather than assumed, so a wrong guess about axes cannot masquerade as
    /// every star being in the wrong place.
    Catalog {
        /// The HYG catalog CSV to compare against.
        file: PathBuf,
    },
}

/// One part of what a built index directory holds
///
/// Named on `--only` to rebuild that part alone. What each is derived from
/// differs: `cells` and `names` come out of one read of every positioned
/// system, `reaches` and `bodies` out of one read of every scanned thing, and
/// `populated` and `factions` out of a query apiece. Asking for one reads only
/// what that one needs.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Part {
    /// The cell tree and its payloads, which the map draws the galaxy from.
    Cells,
    /// Every system's name and place: the search index and the routing graph.
    Names,
    /// The populated systems the map colors and filters by.
    Populated,
    /// How far each scanned system reaches, which every shell is sized by.
    Reaches,
    /// Which systems can supercharge a drive, which the router plots by.
    Boosts,
    /// The faction id-to-name table.
    Factions,
    /// One file per system of the stars, bodies and barycenters in it.
    Bodies,
}

/// The parts `named` comes to, which is every part where nothing was named
fn parts_of(named: &[Part]) -> Parts {
    if named.is_empty() {
        return Parts::ALL;
    }

    let mut parts = Parts::NONE;
    for part in named {
        match part {
            Part::Cells => parts.cells = true,
            Part::Names => parts.names = true,
            Part::Populated => parts.populated = true,
            Part::Reaches => parts.reaches = true,
            Part::Boosts => parts.boosts = true,
            Part::Factions => parts.factions = true,
            Part::Bodies => parts.bodies = true,
        }
    }
    parts
}

fn main() -> galos_db::Result<()> {
    // Without a subscriber nothing the tool or the crate traces is heard;
    // `--watch` in particular would run silently. Info and above by default,
    // `RUST_LOG` to change it, color only when stderr is a terminal.
    tracing_subscriber::fmt()
        .with_ansi(stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    async_std::task::block_on(run(Cli::parse().command))
}

async fn run(command: Command) -> galos_db::Result<()> {
    match command {
        Command::Catalog { file } => {
            let handle =
                std::fs::File::open(&file).map_err(galos_db::Error::from)?;
            let read = hyg::read(handle).map_err(|e| {
                galos_db::Error::from(std::io::Error::other(e.to_string()))
            })?;
            eprintln!(
                "{} catalog stars, {} named, {} without a distance",
                read.stars.len(),
                read.stars.iter().filter(|s| s.name.is_some()).count(),
                read.unplaced.len(),
            );
            let db = Database::new().await?;
            let comparison =
                catalog::compare_to_catalog(&db, &read.stars).await?;
            print!("{}", catalog::report(&comparison));
            Ok(())
        }
        Command::Index { dir, checkpoint, watch, only } => {
            let db = Database::new().await?;
            match watch {
                Some(secs) => {
                    index::watch(
                        &db,
                        &dir,
                        &checkpoint,
                        Duration::from_secs(secs),
                    )
                    .await
                }
                None => {
                    let report =
                        index::build_to_dir(&db, &dir, parts_of(&only)).await?;
                    println!("{report}");
                    if !report.is_consistent() {
                        eprintln!(
                            "warning: system count and placed points differ"
                        );
                    }
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Naming nothing builds the whole index
    ///
    /// The flag is what a build is narrowed by, so its absence has to leave
    /// the build exactly as wide as it always was.
    #[test]
    fn a_build_naming_no_part_builds_them_all() {
        assert_eq!(parts_of(&[]), Parts::ALL);
    }

    /// And naming one builds that one and no other
    ///
    /// The point of the flag: a reach table stale by a change to how a reach
    /// is worked out is worth rewriting on its own, and the hundred megabytes
    /// of names beside it are not.
    #[test]
    fn a_build_naming_a_part_builds_only_that_part() {
        assert_eq!(
            parts_of(&[Part::Reaches]),
            Parts { reaches: true, ..Parts::NONE }
        );
        assert_eq!(
            parts_of(&[Part::Reaches, Part::Bodies]),
            Parts { reaches: true, bodies: true, ..Parts::NONE }
        );
    }

    /// A watch cannot be asked for part of the index
    ///
    /// It keeps every part current by patching what the feed reports, and a
    /// pass that published some of that and not the rest would leave the
    /// directory disagreeing with itself: a body file for a system whose reach
    /// was not rewritten, or a name chunk for a system with no cell. The one
    /// thing to do about a stale part is to rebuild it, which is a build.
    #[test]
    fn a_watch_cannot_be_narrowed_to_a_part() {
        assert!(Cli::try_parse_from([
            "galos-db", "index", "--only", "reaches"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "galos-db", "index", "--watch", "5", "--only", "reaches"
        ])
        .is_err());
    }

    /// The parts are named the way the directory is laid out
    ///
    /// Every part the build knows about is offered on the flag, since a part
    /// that cannot be named is a part that can only be rebuilt by rebuilding
    /// everything.
    #[test]
    fn every_part_can_be_named() {
        let all = parts_of(&[
            Part::Cells,
            Part::Names,
            Part::Populated,
            Part::Reaches,
            Part::Boosts,
            Part::Factions,
            Part::Bodies,
        ]);

        assert_eq!(all, Parts::ALL);
    }
}
