use galos::Run;
use galos_db::{Database, Error};
use std::io::{stderr, IsTerminal};
use structopt::StructOpt;

mod bar;
mod eddb;
mod eddn;
mod edsm;
mod journal;

#[derive(StructOpt, Debug)]
pub enum Cli {
    #[structopt(about = "Import local journal files")]
    Journal(journal::Cli),
    #[structopt(
        about = "Subscribes to EDDN to continuously sync from incoming events"
    )]
    Eddn(eddn::Cli),
    #[structopt(about = "Sync from EDSM's nightly dumps")]
    Edsm(edsm::Cli),
    #[structopt(about = "Sync from a saved EDDB dump; EDDB itself is gone")]
    Eddb(eddb::Cli),
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        match self {
            Cli::Journal(cli) => cli.run(db),
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
        // Above whatever bar is drawing, so the bar keeps the bottom line
        // and the log does not land on top of it.
        .with_writer(bar::Log)
        // Color is for a terminal. Redirected, it would be escape codes
        // around every line of the log.
        .with_ansi(stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::from_args();
    let db = Database::new().await?;
    cli.run(&db);

    Ok(())
}
