//! # Architecture
//!
//! - [`elite_journal`] - Elite: Dangerous journal file parser
//! - [`elite_dat`] - Elite: Dangerous visited star `.dat` parser
//! - [`eddn`] - A [EDDN](https://eddn.edcd.io) subscriber
//! - [`eddb`] - A [EDDB](https://eddb.io) data file parser (discontinued)
//! - [`edsm`] - A [EDSM](https://edsm.net) API adapter and data file parser
//! - [`galos_db`] - PostgreSQL database and ORM
//! - `galos_map` - A 3D galaxy map
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
//! One program with two faces. `galos <command>` answers a question and
//! exits; `galos` on its own takes over the terminal and lets you walk
//! around. They are not two programs that happen to share a database, and
//! this crate is the arrangement that keeps them from becoming two.
//!
//! ```notrust
//!                    argv ──┐
//!  a line typed at the UI ──┼──► Query ──ask(db)──► View ──┬──► cli::print
//!  a row's link, followed ──┘                              └──► tui::render
//! ```
//!
//! [`query::Query`] is what was asked, apart from who asked it. It derives
//! clap's grammar, so the shell and the UI's command bar parse the same
//! language with the same help and the same errors. Asking one answers with a
//! [`view::View`]: a title, some tables and fields, and a line summing it up.
//! Not printed output — [`cli`] prints it and [`tui`] draws it, and because
//! the columns were chosen once there is no pair of them to disagree.
//!
//! The last piece is what makes the UI worth having. Rows carry a
//! [`view::Link`], which is a `Query`, so every place the cursor can be taken
//! is a place the CLI can be told to go, and the UI shows the command line
//! for the row it is sitting on. Nothing is reachable interactively that
//! could not be typed, and adding a command to [`query::Query`] gives both
//! faces of the program the same new thing to do.
//!
//! ### `galos`
//!
//! With no command, the terminal UI. `?` for the keys, `:` to ask something.
//!
//! ```notrust
//!  galos › Systems matching Meliae › Meliae          ? keys   q quit
//!    address     3107241104
//!    position    -68, -8, 46
//!    population  9,832,821
//!    allegiance  Independent
//!
//!  Factions
//!  Faction                    Influence  State
//!  Meliae Blue Federal Party      42.1%  None
//!  Aegis Core                     18.0%  Election
//!
//!  Stations (8 of 14)
//!  ...
//!  2 factions                       galos search -f "Aegis Core"
//! ```
//!
//! ### `galos search [-s NAME] [-f NAME] [-r LY] [-l N] [-c]`
//!
//! Systems, by name, by who is present in them, or by what is within a radius
//! of one. The three narrow together.
//!
//! ```notrust
//! $ galos search -s Meliae
//! $ galos search -s Sol -r 50
//! $ galos search -f "Aegis Core" -c
//! ```
//!
//! ### `galos info <system>`
//!
//! Everything on record about one system, with the first few of its factions,
//! stations and bodies. A fragment naming several systems answers with the
//! several.
//!
//! ### `galos bodies <system>` and `galos stations <system>`
//!
//! All of them, rather than the handful `info` shows.
//!
//! ### `galos factions <name>`
//!
//! Factions by any part of their name. Where they are is `galos search -f`.
//!
//! ### `galos route <from> <to> [-r LY]`
//!
//! Plot a route between two systems, in jumps of `-r` light years.
//!
//! ```notrust
//! $ galos route Sol "Alpha Centauri"
//! $ galos route "Wolf 397" Meliae -r 32
//! ```
//!
//! TODO: `A -> B` for a direct path, `A + B` for a path through both in
//! either order, and `A | B` for a path through either.
//!
//! ### `galos-sync <provider>`
//!
//! Syncs the DB with EDDN, EDSM and/or EDDB.
//!
//! Syncing from the `eddn` provider will subscribe to its ZMQ service and
//! continue to process events until the process is killed.

pub mod cli;
pub mod error;
pub mod query;
pub mod tui;
pub mod view;

pub use self::error::{Error, Result};

use galos_db::Database;

/// Doing something, as against asking something
///
/// What `galos-sync` subcommands are: they read a dump or subscribe to a feed
/// and write what they find, and there is no answer to draw at the end of it.
/// [`query::Ask`] is the other half, and the two are apart because a query
/// that returns a [`view::View`] can be put on a screen and a sync that runs
/// until it is killed cannot.
pub trait Run {
    // TODO: Return Error
    fn run(&self, db: &Database);
}
