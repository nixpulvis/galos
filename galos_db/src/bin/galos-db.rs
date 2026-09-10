//! The galaxy database tool.
//!
//! ```sh
//! cargo run --release --bin galos-db -- catalog hygdata_v41.csv
//! ```
//!
//! Building the index moved to `galos-sync`, which is where every other way
//! of getting one already was: `galos-sync db --to index=.galos_index
//! --watch 5`. The database is one publisher among five, and reading it into
//! an index is the same sentence as reading a journal directory into one —
//! so it is said in the same program, with the same flags, rather than in a
//! tool a reader has to know to look in.
//!
//! The connection is read from `DATABASE_URL` like every other tool.

use clap::{Parser, Subcommand};
use galos_catalog::hyg;
use galos_db::{catalog, Database};
use std::io::{stderr, IsTerminal};
use std::path::PathBuf;

/// Work with the galaxy database.
#[derive(Parser)]
#[command(name = "galos-db", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
    }
}
