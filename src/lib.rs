//! # Architecture
//!
//! The library behind three binaries: `galos-index` (`bin/index/`) and
//! `galos-db` (`bin/db/`), which are the two stores and every way of
//! filling one, and `galos`, the query CLI (`bin/galos/`). What they share
//! lives here: the [`read`] path the publishers come in through, the
//! [`sink`]s they are written out through, the [`bar`] progress is
//! reported on, and the [`Shard`] and [`Shutdown`] that divide and end a
//! run.
//!
//! The formats and the stores are in the crates it depends on:
//!
//! - [`elite_journal`] - Elite: Dangerous journal file parser
//! - [`eddn`] - A [EDDN](https://eddn.edcd.io) subscriber
//! - [`eddb`] - A [EDDB](https://eddb.io) data file parser (discontinued)
//! - [`edsm`] - A [EDSM](https://edsm.net) API adapter and data file parser
//! - [`spansh`] - A [Spansh](https://spansh.co.uk) galaxy dump reader
//! - `galos_db` - PostgreSQL database and ORM, behind the `db` feature
//! - [`galos_index`] - The index format, and the accumulator that fills it
//!
//! ## The `db` feature
//!
//! On by default, and off is the point: `galos-index` writes and serves a
//! directory, and with the feature off nothing in the build has `sqlx`,
//! `dotenv` or a `DATABASE_URL` in it. That is not a packaging detail —
//! it is what lets the index tool run on a machine with no Postgres
//! installed, which is the arrangement the whole index format exists for.
//!
//! Two things in the seam used to name a database and now do not:
//! [`sink::Landed`], which is what a write came to, and [`sink::Clock`],
//! which is what a resume point's cursor is read off. The database
//! implements the second and converts into the first, in
//! `sink::db` — the one module that has a database to do it with.
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
//! ### `galos-index <status|ingest|build|migrate|verify|sweep|pack|diff|sectors> …`
//!
//! Everything that is done to an index directory. `ingest --from SOURCE`
//! follows or reads a publisher into it; `build --from database` derives
//! one from rows; the rest report on, repair or take apart a directory
//! that is already there. `--from` repeats, and `--from eddn` subscribes
//! to its ZMQ service until the run is asked to stop.
//!
//! ### `galos-db <status|ingest|migrate|verify|catalog|stats> …`
//!
//! The same shape over Postgres, with `ingest --from SOURCE` reading the
//! same publishers through the same [`read`] path into the other sink.
//!
//! Both write paths are here rather than in a binary: [`sink`] is the seam
//! the sources write through and it is what an integration test has to be
//! able to reach, and [`read`] is the sources themselves, which the two
//! tools share entire. What lives in `bin/` is the command line and the
//! supervisor that joins the halves of a run.

pub mod bar;
pub mod read;
pub mod shard;
pub mod shutdown;
pub mod sink;

pub use shard::Shard;
pub use shutdown::Shutdown;

/// A `galos` query subcommand.
///
/// The query CLI's own seam, which has always taken a database because
/// querying is what it does. Behind the `db` feature with everything else
/// that names one.
#[cfg(feature = "db")]
pub trait Run {
    // TODO: Reture Error
    fn run(&self, db: &galos_db::Database);
}
