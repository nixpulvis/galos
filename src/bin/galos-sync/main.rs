use clap::Parser;
use galos::Run;
use galos_db::{Database, Error};
use std::io::{stderr, IsTerminal};

mod eddb;
mod eddn;
mod edsm;
mod journal;

#[derive(Parser, Debug)]
#[command(name = "galos-sync", version, about = "Fill the database")]
struct Args {
    #[command(subcommand)]
    provider: Cli,
}

#[derive(clap::Subcommand, Debug)]
pub enum Cli {
    /// Import local journal files
    Journal(journal::Cli),
    /// Subscribes to EDDN to continuously sync from incoming events
    #[cfg(unix)]
    Eddn(eddn::Cli),
    /// Sync from EDSM's nightly dumps
    #[command(subcommand)]
    Edsm(edsm::Cli),
    /// Sync from EDDB's nightly dumps
    Eddb(eddb::Cli),
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        match self {
            Cli::Journal(cli) => cli.run(db),
            #[cfg(unix)]
            Cli::Eddn(cli) => cli.run(db),
            Cli::Edsm(cli) => cli.run(db),
            Cli::Eddb(cli) => cli.run(db),
        }
    }
}

#[async_std::main]
async fn main() -> Result<(), Error> {
    // Nothing a crate traces goes anywhere until something is listening for
    // it, dependencies included. `RUST_LOG` picks what to hear, and info
    // upwards from everything without it.
    tracing_subscriber::fmt()
        // Color is for a terminal. Redirected, it would be escape codes
        // around every line of the log.
        .with_ansi(stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let db = Database::new().await?;
    args.provider.run(&db);

    Ok(())
}
