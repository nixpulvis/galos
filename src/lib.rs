//! # Architecture
//!
//! The library behind one binary, `galos` (`bin/galos/`): `ingest`, which
//! is every way of filling either store, the `index` and `db` groups,
//! which are what is asked of each store once it holds something, and
//! `search` and `route`, the queries. What they share lives here:
//! the [`read`] path the publishers come in through, the [`sink`]s they
//! are written out through, the [`bar`] progress is reported on, and the
//! [`Shard`] and [`Shutdown`] that divide and end a run.
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
//! On by default, and off is the point: `galos ingest --index` writes a
//! directory and the `galos index` verbs serve and repair one, and with
//! the feature off nothing in the build has `sqlx`, `dotenv` or a
//! `DATABASE_URL` in it. That is not a packaging detail — it is what lets
//! those runs work on a machine with no Postgres installed, which is the
//! arrangement the whole index format exists for.
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
//! ### `galos ingest --from SOURCE… [--db] [--index [DIR]] …`
//!
//! The one verb that fills anything. `--from` names a publisher and
//! repeats — `eddn`, `spool=DIR`, `journal=PATH`, `edsm=PATH`,
//! `edsm-api=NAME`, `eddb=PATH`, `spansh=PATH`, `database` — and `--from
//! eddn` subscribes to its ZMQ service until the run is asked to stop.
//! `--db` and `--index` name the sinks, and naming both reads each
//! publisher once into the pair. `--from database --index DIR` is the
//! rows read back out into a directory, which is how one is rebuilt
//! rather than maintained.
//!
//! ### `galos index <status|diff|verify|backup|restore|merge|pack|sweep|migrate|sectors> …`
//!
//! Everything done *to* an index directory that already exists: report on
//! it, compare two of them, repair one, or take one apart. None of it
//! needs a database.
//!
//! Three of them are about a *pair* of directories. `backup` and
//! `restore` copy a directory and the resume point beside it, in an order
//! that makes a copy taken across a live publish safe; what comes out is
//! an index directory like any other. `merge` folds one directory into
//! another, newest record winning, which is what a production failure
//! needs and a re-import is not: collection is pointed at a fresh
//! directory while the original is seen to, and the two are then made
//! one.
//!
//! ### `galos db <status|migrate|verify|catalog|stats|backup|restore|merge> …`
//!
//! The same shape over Postgres: what it is, what is wrong with it, and
//! what it holds — and the same three about a pair of them. `backup` and
//! `restore` are `pg_dump` and `pg_restore` with the flags that matter
//! already right; `merge` folds another database into this one by the
//! rule every write path already holds, a guarded upsert keyed by a
//! natural key and stamped.
//!
//! The write path is here rather than in `bin/`: [`sink`] is the seam
//! the sources write through and it is what an integration test has to be
//! able to reach, and [`read`] is the sources themselves, which the two
//! sinks share entire. What lives in `bin/` is the command line and
//! the supervisor that joins the halves of a run.

pub mod bar;
pub mod read;
pub mod shard;
pub mod shutdown;
pub mod sink;

pub use shard::Shard;
pub use shutdown::Shutdown;
