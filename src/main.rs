//! # Architecture
//!
//! - [`elite_journal`][elite_journal] - Elite: Dangerous journal file parser
//! - [`elite_dat`][elite_dat] - Elite: Dangerous visited star `.dat` parser
//! - [`eddn`][eddn] - A [EDDN](https://eddn.edcd.io) subscriber
//! - [`eddb`][eddb] - A [EDDB](https://eddb.io) data file parser
//! - [`edsm`][edsm] - A [EDSM](https://edsm.net) API adapter and data file parser
//! - [`galos_db`][galos_db] - PostgreSQL database and ORM
//! - [`galos`](#galos) - Shared code and the user CLI, `galos`
//! - [`galos-server`](#galos-server) - Web-server for the API and website
//! - [`galos-worker`](#galos-worker) - Background jobs to complement the server
//! - [`galos-gui`](#galos-gui) - Graphical application, primarily for mapping
//!
//! In order to run the server, worker, locally configured CLI and GUI, the PostgreSQL database
//! must be running and up to date. The [`galos_db`][galos_db] crate provides tools to manage this.
//! Both [`galos-server`](#galos-server) and [`galos-worker`](#galos-worker) can be run
//! independently by storing job requests and responses in the database.
//!
//! # `galos`
//!
//! To launch the interactive terminal application, simply run `galos`.
//!
//!     ----------------------------------------------------------------------------
//!     | Current Location: Ngalinn, Fall Station                                  |
//!     ----------------------------------------------------------------------------
//!     | Filter: +good:"Food and Water" -*M                                       |
//!     ----------------------------------------------------------------------------
//!     | [ ] Mannani                                                              |
//!     | [x] Aitvas                                                               |
//!     | [x] Sol                                                                  |
//!     |                                                                          |
//!     |                                                                          |
//!     |                                                                          |
//!     |                                                                          |
//!     |                                                                          |
//!     |                                                                          |
//!     |                                                                          |
//!     |                                                                          |
//!     |                                                                          |
//!     ----------------------------------------------------------------------------
//!     | Totals: Hyperspace 453Ly,                                                |
//!     ----------------------------------------------------------------------------
//!
//! ```
//! Usage: galos <command> ...
//! ```
//!
//!
//! ##### `galos search <name> [<filter>]`
//!
//! Search for systems, bodies, and stations in the database. This command shows a
//! summary view for each object found.
//!
//! Examples:
//! ```
//! $ galos search HD* within=10Ly
//! ```
//!
//!
//! ##### `galos info <name>`
//! Lookup systems, bodies, and stations in the database. This command shows a
//! detailed view for the found object.
//!
//! Examples:
//! ```
//! $ galos info Sol
//! ```
//!
//!
//! ##### `galos route <system> <path> <system> [<path> <system>]...`
//! Plot routes between systems, bodies, and stations in the database.
//!
//! - `A -> B` specifies a direct path from A to B
//! - `A + B` specifies a path to both A and B, where the route could either visit
//!     A or B first
//! - `A | B` specifies a path to either A or B
//!
//! Examples:
//! ```
//! $ galos route Sol -> Alpha Centauri
//!
//! $ galos route Wolf 397 -> Sol + Meliae -> Nagalinn + Sol
//! yields:       Wolf 397 -> Meliae -> Sol -> Nagalinn
//! ```
//!
//! ##### `galos sync <provider>`
//! Syncs the DB with EDDN, EDSM and/or EDDB.
//!
//! Syncing from the `eddn` provider will subscribe to its ZMQ service and continue to process
//! events until the process is killed.
//!
//! # `galos-server`
//! TODO
//!
//! # `galos-worker`
//! TODO
//!
//! # `galos-gui`
//! TODO

use structopt::StructOpt;
use galos_db::{Error, Database};

#[derive(StructOpt, Debug)]
enum Cli {
    Search(SearchCli),
    Info(InfoCli),
    Route(RouteCli),
    Sync(sync::Cli),
}

pub trait Run {
    fn run(&self, db: &Database);
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        match self {
            Cli::Search(cli) => cli.run(db),
            Cli::Info(cli)   => cli.run(db),
            Cli::Route(cli)  => cli.run(db),
            Cli::Sync(cli)   => cli.run(db),
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

#[derive(StructOpt, Debug)]
pub struct SearchCli {}

impl Run for SearchCli {
    fn run(&self, db: &Database) {
        unimplemented!();
    }
}

#[derive(StructOpt, Debug)]
pub struct InfoCli {}

impl Run for InfoCli {
    fn run(&self, db: &Database) {
        unimplemented!();
    }
}

#[derive(StructOpt, Debug)]
pub struct RouteCli {}

impl Run for RouteCli {
    fn run(&self, db: &Database) {
        unimplemented!();
    }
}

mod sync;
