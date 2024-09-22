//! # Architecture
//!
//! - [`elite_journal`] - Elite: Dangerous journal file parser
//! - [`elite_dat`] - Elite: Dangerous visited star `.dat` parser
//! - [`eddn`] - A [EDDN](https://eddn.edcd.io) subscriber
//! - [`eddb`] - A [EDDB](https://eddb.io) data file parser (discontinued)
//! - [`edsm`] - A [EDSM](https://edsm.net) API adapter and data file parser
//! - [`galos_db`] - PostgreSQL database and ORM
//! - [`galos_map`] - A 3D galaxy map
//! - [`galos_server`] - An HTTP server for [`galos_db`]
//! - [`galos_gui`] - WIP
//! - [`galos`](#galos) - Shared code and the user CLI, `galos`
//!
//! In order to run the this tool, [`galos-sync`], [`galos-map`],
//! [`galos-server`], [`galos-gui`], a PostGIS database must be running and up
//! to date. The [`galos_db`] crate provides tools to manage this.
//!
//! # `galos`
//!
//! To launch the interactive terminal application, simply run `galos`.
//!```notrust
//! -------------------------------------------
//! | Current Location: Ngalinn, Fall Station |
//! -------------------------------------------
//! | Filter: +good:"Food and Water" -*M      |
//! -------------------------------------------
//! | [ ] Mannani                             |
//! | [x] Aitvas                              |
//! | [x] Sol                                 |
//! -------------------------------------------
//! | Totals: Hyperspace 453Ly,               |
//! -------------------------------------------
//!```
//!
//! ### `galos search [OPTIONS] <query>`
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
//! ### `galos route <system> <op> <system> [<op> <system>]...`
//!
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
//! ### `galos-sync <provider>`
//!
//! Syncs the DB with EDDN, EDSM and/or EDDB.
//!
//! Syncing from the `eddn` provider will subscribe to its ZMQ service and
//! continue to process events until the process is killed.

use galos_db::Database;

pub trait Run {
    // TODO: Reture Error
    fn run(&self, db: &Database);
}
