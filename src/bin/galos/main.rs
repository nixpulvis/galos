#[cfg(unix)]
#[macro_use]
extern crate prettytable;

#[cfg(unix)]
use galos::Run;
#[cfg(unix)]
use galos_db::{Database, Error};
use structopt::StructOpt;

#[cfg(unix)]
#[derive(StructOpt, Debug)]
struct Cli {
    #[structopt(
        short = "d",
        long = "database",
        help = "override default (.env) database URL"
    )]
    database_url: Option<String>,
    #[structopt(subcommand)]
    subcommand: Subcommand,
}

#[cfg(unix)]
#[derive(StructOpt, Debug)]
enum Subcommand {
    #[structopt(about = "Search for systems, bodies, stations, factions, etc")]
    Search(search::Cli),
    #[structopt(about = "Plot routes between to and from many systems")]
    Route(route::Cli),
}

#[cfg(unix)]
impl Run for Subcommand {
    fn run(&self, db: &Database) {
        match self {
            Subcommand::Search(cli) => cli.run(db),
            Subcommand::Route(cli) => cli.run(db),
        }
    }
}

#[cfg(unix)]
#[async_std::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::from_args();
    let db = if let Some(url) = cli.database_url {
        Database::from_url(&url).await?
    } else {
        Database::new().await?
    };

    cli.subcommand.run(&db);
    Ok(())
}

#[cfg(unix)]
mod route;
#[cfg(unix)]
mod search;

#[cfg(not(unix))]
fn main() {}
