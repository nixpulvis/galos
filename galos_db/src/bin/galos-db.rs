//! The galaxy database tool.
//!
//! ```sh
//! # Build the index into a directory and exit.
//! cargo run --release --bin galos-db -- index .galos_index
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

use clap::{Parser, Subcommand};
use galos_db::{index, Database};
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
        /// Follow the feed, republishing every SECS seconds rather than exiting.
        #[arg(long, value_name = "SECS", num_args = 0..=1, default_missing_value = "5")]
        watch: Option<u64>,
    },
}

fn main() -> galos_db::Result<()> {
    // Without a subscriber nothing the tool or the crate traces is heard;
    // `--watch` in particular would run silently. Info and above by default,
    // `RUST_LOG` to change it, colour only when stderr is a terminal.
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
        Command::Index { dir, watch } => {
            let db = Database::new().await?;
            match watch {
                Some(secs) => {
                    index::watch(&db, &dir, Duration::from_secs(secs)).await
                }
                None => {
                    let report = index::build_to_dir(&db, &dir).await?;
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
