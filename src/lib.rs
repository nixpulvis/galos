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
//!```notrust
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
//!```
//!
//! ```notrust
//! Usage: galos <command> ...
//! ```
//!
//!
//! ##### `galos search [OPTIONS] <query>`
//!
//! Search for systems, bodies, and stations in the database. This command shows a
//! selection of details for each object found.
//!
//! Examples (TODO):
//! ```notrust
//! $ galos search --count HD* sphere=500Ly
//! $ galos search Meliae cube=40Ly factions={influence<7.5%}
//! $ galos search --limit 50 --order factions.influence (HD*|HIP*) factions={influence<7.5%}
//! ```
//!
//!
//! ##### `galos route <system> <op> <system> [<op> <system>]...`
//! Plot routes between systems, bodies, and stations in the database.
//!
//! Where `op` is one of:
//! - `A -> B` specifies a direct path from A to B
//! - `A + B` specifies a path to both A and B, where the route could either visit
//!     A or B first
//! - `A | B` specifies a path to either A or B
//!
//! Examples:
//! ```notrust
//! $ galos route Sol -> Alpha Centauri
//!
//! $ galos route Wolf 397 -> Sol + Meliae -> Nagalinn + Sol
//! yields:       Wolf 397 -> Meliae -> Sol -> Nagalinn
//! ```
//!
//! TODO: Incorperate queries for both `+` and `|` nodes in the route.
//!
//! ##### `galos-sync <provider>`
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

#[cfg(unix)]
use galos_db::Database;

#[cfg(unix)]
pub trait Run {
    // TODO: Reture Error
    fn run(&self, db: &Database);
}
