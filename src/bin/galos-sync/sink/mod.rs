//! Where a source puts what it read.
//!
//! Four sources feed this program and they had one destination between them:
//! Postgres. But the database is not the only thing that wants these
//! readings. `galos_index` is a file format a client draws from with no
//! server at all, and a commander running the map on their own machine has
//! every reason to want a directory of it kept current from their own journal
//! — or from EDDN — without standing a database up first.
//!
//! So the write path is a trait and the sources no longer name a
//! [`Database`](galos_db::Database). A source reads; a sink decides what that
//! means on the other side.
//!
//! ## What the trait is shaped by
//!
//! Not by what a database wants, and not by what an index wants: by what the
//! *sources* have to say. There are exactly two shapes of thing arriving
//! here, and the trait is those two and nothing else.
//!
//! - **An event.** [`Sink::entry`] — one `elite_journal` [`Entry<Event>`] and
//!   whoever wrote it. Journal files and EDDN carry the same events, which is
//!   the invariant `journal::record`'s header states, so this is the whole of
//!   both of those sources. Four EDDN schemas carry a payload with no `event`
//!   key and get a method apiece, since there is no `Event` to hand over.
//! - **A system row.** [`Sink::system`] — a name, a place and the political
//!   columns, which is what the EDSM and EDDB dumps hold and all they hold.
//!   Not an event and never was: nobody flew anywhere, a file was published.
//!
//! [`Sink::ensure_system`] is the one method that is about a sink's own
//! constraints rather than about a source. The database has a foreign key
//! onto `systems`, and the game writes events naming a system before the
//! arrival that would have created it, so an importer says "this address is
//! this place" ahead of anything pointing at it. An index has no keys and no
//! rows, so its answer is to do nothing — stated in the impl rather than
//! guessed at each call site.
//!
//! ## What it is not
//!
//! Not async-generic, not batched, and not a queue. One entry at a time, in
//! the order the source read them, because order is what the guarded writes
//! downstream turn on: a write is refused where something newer already
//! stands, so entries reaching a sink out of order land differently every
//! run. A sink that wants to batch does it behind this, where it can see what
//! it is batching.
//!
//! Nothing here returns an error. A refused write is one message that could
//! not be placed, said at `warn` by whoever refused it, and the next one is
//! read — since a feed that stopped at the first system it could not place
//! would stop for good. What *can* fail as a whole is [`Sink::flush`], which
//! is a sink being asked to make durable what it has been holding, and that
//! is a failure of the run rather than of a message.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use elite_journal::entry::market::{BlackMarket, Market, Outfitting, Shipyard};
use elite_journal::entry::{Entry, Event};
use elite_journal::prelude::{Allegiance, Economy, Government, Security};
use elite_journal::system::Coordinate;

pub mod db;
pub mod index;
pub mod tables;
pub mod to;

pub use db::Db;
pub use index::Index;
pub use to::To;

/// A system as a published dump gives it, which is not an event.
///
/// EDSM's nightly files and the saved EDDB dump carry this and nothing below
/// system level: no scans, no stations, no markets. It is the argument list
/// of `galos_db::systems::System::create` named as a struct, because five
/// optional columns in a row is a call nobody can read and two sinks now have
/// to take it.
#[derive(Clone, Debug)]
pub struct Row {
    pub address: i64,
    pub name: String,
    /// [`None`] where the dump has no coordinates for it, which is a system
    /// nobody has visited and neither sink can place.
    pub position: Option<Coordinate>,
    pub population: Option<u64>,
    pub security: Option<Security>,
    pub government: Option<Government>,
    pub allegiance: Option<Allegiance>,
    pub primary_economy: Option<Economy>,
    pub secondary_economy: Option<Economy>,
    /// When the dump says this reading was taken, which the guarded writes
    /// weigh against what already stands.
    pub updated_at: DateTime<Utc>,
    /// Which dump, named so a row can be traced back to the file it came out
    /// of.
    pub updated_by: String,
}

/// Where a source's readings go.
///
/// See the module header for what the shape is and is not. Implemented by
/// [`Db`] and [`Index`]; a source holds one behind `&mut dyn Sink` and never
/// asks which.
#[async_trait]
pub trait Sink: Send {
    /// One journal entry, and whoever wrote it.
    async fn entry(&mut self, entry: &Entry<Event>, user: &str);

    /// A system named and placed, ahead of anything that points at it.
    ///
    /// Answers whether the sink now knows the place. `false` is a system it
    /// could not record, and a caller that was about to write something
    /// keyed onto it may as well not bother.
    ///
    /// `why` names what asked, for the log: a source calls this from several
    /// places and "system named" and "scan" are different stories about the
    /// same failure.
    async fn ensure_system(
        &mut self,
        at: DateTime<Utc>,
        user: &str,
        address: i64,
        name: Option<&str>,
        position: Option<Coordinate>,
        why: &str,
    ) -> bool;

    /// A system as a dump gives it.
    async fn system(&mut self, row: &Row);

    /// A commodity market, which EDDN sends under its own schema.
    async fn market(&mut self, at: DateTime<Utc>, user: &str, it: &Market);

    /// A station's outfitting, likewise.
    async fn outfitting(
        &mut self,
        at: DateTime<Utc>,
        user: &str,
        it: &Outfitting,
    );

    /// A station's shipyard, likewise.
    async fn shipyard(&mut self, at: DateTime<Utc>, user: &str, it: &Shipyard);

    /// A fleet carrier's black market, likewise.
    async fn black_market(
        &mut self,
        at: DateTime<Utc>,
        user: &str,
        it: &BlackMarket,
    );

    /// Make durable what has been read, answering what went wrong.
    ///
    /// A database has written every message as it arrived and has nothing to
    /// do here. An index has been holding a tree and its tables in memory and
    /// this is where they reach the disk, so for that one it is the whole of
    /// the work. Called when a source finishes, and by a following source
    /// whenever it has caught up with what is being written — which is why it
    /// takes no argument and may be called as often as a source likes.
    async fn flush(&mut self) -> Result<(), String>;

    /// One line saying what this sink has taken, for the end of a run.
    fn said(&self) -> String;
}
