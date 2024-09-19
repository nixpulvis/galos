#[cfg(unix)]
use galos::Run;
use galos_db::{Database, Error};
use structopt::StructOpt;

mod eddb;
mod eddn;
mod edsm;
mod journal;

#[cfg(unix)]
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
    #[structopt(about = "Sync from EDDB's nightly dumps")]
    Eddb(eddb::Cli),
}

#[cfg(unix)]
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

#[cfg(unix)]
#[async_std::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::from_args();
    let db = Database::new().await?;
    cli.run(&db);

    Ok(())
}

#[cfg(not(unix))]
fn main() {}
