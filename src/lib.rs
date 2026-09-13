//! # Architecture
//!
//! The library behind two binaries: `galos`, the query CLI (`bin/galos/`),
//! and `galos-sync`, the ingest tool (`bin/sync/`). What they share lives
//! here: the [`sink`]s they write through, the [`bar`] they report
//! progress on, and the [`Shard`] and [`Shutdown`] that divide and end a
//! run.
//!
//! The formats and the stores are in the crates it depends on:
//!
//! - [`elite_journal`] - Elite: Dangerous journal file parser
//! - [`eddn`] - A [EDDN](https://eddn.edcd.io) subscriber
//! - [`eddb`] - A [EDDB](https://eddb.io) data file parser (discontinued)
//! - [`edsm`] - A [EDSM](https://edsm.net) API adapter and data file parser
//! - [`spansh`] - A [Spansh](https://spansh.co.uk) galaxy dump reader
//! - [`galos_db`] - PostgreSQL database and ORM
//! - [`galos_index`] - The index format, and the accumulator that fills it
//!
//! `galos`, and any run of `galos-sync` given `--db`, need a PostGIS
//! database migrated up to date. The [`galos_db`] crate provides the tools
//! to manage it.
//!
//! # Commands
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
//! ### `galos-sync --from SOURCE... [--db] [--index DIR]`
//!
//! Syncs the database and an index directory from EDDN, EDSM, EDDB and the
//! game's own journal files, in one process.
//!
//! `--from` repeats, and `--from eddn` subscribes to its ZMQ service and
//! processes events until the run is asked to stop.
//!
//! Its write path is here rather than in the binary: [`sink`] is the seam
//! the sources write through, and it is what an integration test has to be
//! able to reach. An event becomes rows in [`galos_db::record`]. What lives
//! in `bin/sync` is the command line, the sources — the journal reader
//! among them — and the supervisor that joins them.

use galos_db::Database;

pub mod bar;
pub mod shard;
pub mod shutdown;
pub mod sink;

pub use shard::Shard;
pub use shutdown::Shutdown;

pub trait Run {
    // TODO: Reture Error
    fn run(&self, db: &Database);
}
