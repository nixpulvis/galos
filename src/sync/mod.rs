use structopt::StructOpt;
use galos_db::{Error, Database};
use crate::Run;

mod eddn;
mod edsm;
mod eddb;

#[derive(StructOpt, Debug)]
pub enum Cli {
    Eddn(eddn::Cli),
    Edsm(edsm::Cli),
    Eddb(eddb::Cli),
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        match self {
            Cli::Eddn(cli) => cli.run(db),
            Cli::Edsm(cli) => cli.run(db),
            Cli::Eddb(cli) => cli.run(db),
        }
    }
}

#[async_std::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::from_args();
    println!("{:?}", cli);
    let db = Database::new().await?;
    cli.run(&db);

    Ok(())
}
