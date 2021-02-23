use structopt::StructOpt;
use galos_db::{Error, Database};

mod eddn;
mod edsm;
mod eddb;

#[derive(StructOpt, Debug)]
enum Cli {
    Eddn(eddn::Cli),
    Edsm(edsm::Cli),
    Eddb(eddb::Cli),
}

trait SyncDb {
    // TODO: Return a result.
    fn sync_db(&self, db: &Database);
}

impl SyncDb for Cli {
    fn sync_db(&self, db: &Database) {
        match self {
            Cli::Eddn(cli) => cli.sync_db(db),
            Cli::Edsm(cli) => cli.sync_db(db),
            Cli::Eddb(cli) => cli.sync_db(db),
        }
    }
}

#[async_std::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::from_args();
    println!("{:?}", cli);
    let db = Database::new().await?;
    cli.sync_db(&db);

    Ok(())
}
