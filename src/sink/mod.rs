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
//! - **An event.** [`Sink::entry`] — one `elite_journal` [`Entry<Event>`] and
//!   whoever wrote it. Journal files and EDDN carry the same events, which is
//!   the invariant `galos_db::record`'s header states, so this is the whole
//!   of both of those sources. Four EDDN schemas carry a payload with no
//!   `event` key and get a method apiece, since there is no `Event` to hand
//!   over. Behind an [`Arc`] rather than a reference, because one reading is
//!   now written to Postgres *and* handed across a thread to the index
//!   worker, and `Entry` is not `Clone` — the whole galaxy of the event tree
//!   would have to derive it. A refcount is what two sinks share instead.
//! - **A system report.** [`Sink::system`] — [`SystemReport`], which is a
//!   name, a place, the political columns and the body counts: what the EDSM
//!   and EDDB dumps hold and all they hold, and what every event naming a
//!   system reduces to. A dump is not an event and never was — nobody flew
//!   anywhere, a file was published — but what it states about a system is
//!   the same shape, so it takes the same road.
//!
//!   This is also how a sink is told a system exists before anything keyed
//!   onto it arrives: Postgres has a foreign key onto `systems` and the game
//!   writes events naming a system before the arrival that would have
//!   created it. There used to be a second method for that alone. An index
//!   has no keys and no rows and answered it by doing nothing, which is a
//!   sign the question was the database's own and not a source's.
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
use std::sync::Arc;

pub mod db;
pub mod index;
pub mod relay;
pub mod tables;

pub use db::Db;
pub use index::Index;
pub use relay::Relay;

/// What a sink did with a reading of a system.
///
/// Sources count these as they read, to show how many systems a run has
/// taken in and how many were new.
pub use galos_db::systems::Landed;

/// The whole of what a source says about a system, which is
/// [`SystemReport`].
///
/// It used to be a struct here called `Row`, named after the Postgres row
/// the EDSM and EDDB dumps were written to call, and the accumulator on the
/// other side of the program held a struct of its own with the same columns
/// spelled differently. Both are gone: one shape, in `galos_index`, where
/// the rule for merging two of them lives as well.
pub use galos_index::SystemReport;

/// A system's name, in the one spelling everything here uses.
///
/// Upper case by construction, so a source folds case once as it builds a
/// report and nothing downstream folds it again. See
/// [`galos_index::name`] for what that saved.
pub use galos_index::SystemName;

/// Who a source says wrote what it is handing over.
///
/// A string was not enough, and the way it was not enough is a bug that ran
/// for a while. Both kinds of source name somebody — a journal names the
/// commander flying, EDDN names the sender that forwarded the message, a
/// dump names the file it was read out of — and the two are not the same
/// claim: an uploader id is anonymised and is not a person. Both are
/// provenance, which is what `updated_by` is, so both sinks keep whatever
/// the source said.
///
/// With one index worker serving every source, a `&str` meant the galaxy
/// could not tell them apart: `--from eddn --from journal=DIR` filed
/// everybody else's scans under whoever was flying locally, because the
/// accumulator tracked the commander from `Commander` events and EDDN
/// carries none to say otherwise.
pub enum Reporter<'a> {
    /// A commander, as their own journal named them.
    Commander(&'a str),
    /// An EDDN sender's uploader id, or the file a dump was read out of.
    Uploader(&'a str),
    /// Nobody this reading can be filed under, nothing having named one.
    Nobody,
}

impl Reporter<'_> {
    /// Whoever the source named, for a column that wants provenance.
    pub fn named(&self) -> &str {
        match self {
            Reporter::Commander(who) | Reporter::Uploader(who) => who,
            Reporter::Nobody => "",
        }
    }

    /// The same claim again, for handing to a second sink.
    ///
    /// A [`Fan`] gives every sink the same reading, and this is not [`Copy`]:
    /// a `Copy` enum of borrowed strings would be one a caller could keep
    /// past the borrow it names.
    pub fn same(&self) -> Reporter<'_> {
        match self {
            Reporter::Commander(who) => Reporter::Commander(who),
            Reporter::Uploader(who) => Reporter::Uploader(who),
            Reporter::Nobody => Reporter::Nobody,
        }
    }
}

/// Where a source's readings go.
///
/// See the module header for what the shape is and is not. Implemented by
/// [`Db`] and [`Index`]; a source holds one behind `&mut dyn Sink` and never
/// asks which.
#[async_trait]
pub trait Sink: Send {
    /// One journal entry, and whoever wrote it.
    ///
    /// Returns what happened to the system the entry names, or [`None`]
    /// for the entries naming none — most of them, a market or a docking
    /// saying nothing about a system's own columns.
    async fn entry(
        &mut self,
        entry: Arc<Entry<Event>>,
        by: Reporter<'_>,
    ) -> Option<Landed>;

    /// A system as a source reported it: named, placed, and with whatever
    /// columns the report carried.
    ///
    /// EDSM's and EDDB's dumps are nothing but these, and so is the pre-pass
    /// a journal directory makes over its own files to name the systems it
    /// is about to talk about. `user` is who to file the reading under,
    /// which is a column on the database's row and nothing the index
    /// publishes; see [`SystemReport`]'s header.
    ///
    /// There is no `ensure_system` beside this any more. It existed for
    /// Postgres' foreign key — the game writes events naming a system
    /// before the arrival that would have created it — and that is what
    /// this method does, so the two were one thing said twice.
    ///
    /// Returns what happened to the reading, or [`None`] where the sink
    /// wrote no row for it: a report no `systems` row can be made from, a
    /// refused write, or a sink that only passes the reading along.
    async fn system(
        &mut self,
        report: &SystemReport,
        user: &str,
    ) -> Option<Landed>;

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

    /// Close the run out, answering what went wrong.
    ///
    /// What a source calls once, when there is no more to read. A database
    /// has nothing left to do; an index writes every part of its directory
    /// whole, because a run that only ever published deltas would leave
    /// behind the tables nothing in this run happened to change. The
    /// difference used to be in `main`, which meant knowing which sink it
    /// held; a sink knows what finishing means for it.
    async fn finish(&mut self) -> Result<(), String>;

    /// One line saying what this sink has taken, for the end of a run.
    fn said(&self) -> String;
}

/// Several sinks, written to as one.
/// What `--db --index=DIR` is: one read of a publisher filling both, rather
/// than two runs of this program over the same feed, which for EDDN means
/// two subscriptions and twice the messages for the same galaxy.
///
/// Nothing here borrows: [`Db`] owns its `Database` and [`Relay`] owns a
/// channel sender, so a fan is `'static` and a source reading into one can
/// be spawned. That is what lets several sources run at once, each with a
/// fan of its own over the same pool and the same channel.
///
/// In order and one after another, never concurrently. The order a source
/// reads in is what the guarded writes downstream turn on, and a sink that
/// is slow is slow for the run rather than for its neighbour.
pub struct Fan {
    sinks: Vec<Box<dyn Sink>>,
}

impl Fan {
    /// A sink over all of these.
    pub fn of(sinks: Vec<Box<dyn Sink>>) -> Fan {
        Fan { sinks }
    }

    /// What each sink has to say, one line apiece.
    ///
    /// Not one joined sentence: a database counts every message it was
    /// handed and an index counts the systems it took and the tree it
    /// holds, and the two numbers answer different questions.
    pub fn said_each(&self) -> Vec<String> {
        self.sinks.iter().map(|sink| sink.said()).collect()
    }

    /// Every failure, rather than the first.
    ///
    /// A durability call must reach every sink: stopping at the first
    /// failure would leave the ones after it holding what they had while
    /// the run carried on reading into them.
    fn all(failed: Vec<String>) -> Result<(), String> {
        if failed.is_empty() {
            Ok(())
        } else {
            Err(failed.join("; "))
        }
    }
}

#[async_trait]
impl Sink for Fan {
    /// The same reading to each, which is one refcount apiece rather than
    /// one copy of the event apiece.
    ///
    /// Answered by [`Landed::widest`]; see [`Sink::system`] below.
    async fn entry(
        &mut self,
        entry: Arc<Entry<Event>>,
        by: Reporter<'_>,
    ) -> Option<Landed> {
        let mut landed = None;
        for sink in &mut self.sinks {
            let said = sink.entry(Arc::clone(&entry), by.same()).await;
            landed = Landed::widest(landed, said);
        }
        landed
    }

    /// Answers with the strongest landing any sink reported.
    ///
    /// The sinks are stores filled at different times, so they disagree:
    /// the database calls a system it has held since the last dump an
    /// update, while an index directory written from nothing this morning
    /// calls it new. Both are right about themselves, so the fan reports
    /// [`Landed::New`] — a reading that created a system somewhere created
    /// one. Per-sink numbers are in [`Fan::said_each`], one line each at
    /// the end of a run.
    ///
    /// A [`Relay`] reports nothing, since it passes readings to the index
    /// worker instead of storing them: on a `--db --index` run the source's
    /// bar shows the database's answer and the index worker counts its own.
    async fn system(
        &mut self,
        report: &SystemReport,
        user: &str,
    ) -> Option<Landed> {
        let mut landed = None;
        for sink in &mut self.sinks {
            landed = Landed::widest(landed, sink.system(report, user).await);
        }
        landed
    }

    async fn market(&mut self, at: DateTime<Utc>, user: &str, it: &Market) {
        for sink in &mut self.sinks {
            sink.market(at, user, it).await;
        }
    }

    async fn outfitting(
        &mut self,
        at: DateTime<Utc>,
        user: &str,
        it: &Outfitting,
    ) {
        for sink in &mut self.sinks {
            sink.outfitting(at, user, it).await;
        }
    }

    async fn shipyard(&mut self, at: DateTime<Utc>, user: &str, it: &Shipyard) {
        for sink in &mut self.sinks {
            sink.shipyard(at, user, it).await;
        }
    }

    async fn black_market(
        &mut self,
        at: DateTime<Utc>,
        user: &str,
        it: &BlackMarket,
    ) {
        for sink in &mut self.sinks {
            sink.black_market(at, user, it).await;
        }
    }

    async fn flush(&mut self) -> Result<(), String> {
        let mut failed = Vec::new();
        for sink in &mut self.sinks {
            if let Err(said) = sink.flush().await {
                failed.push(said);
            }
        }
        Fan::all(failed)
    }

    async fn finish(&mut self) -> Result<(), String> {
        let mut failed = Vec::new();
        for sink in &mut self.sinks {
            if let Err(said) = sink.finish().await {
                failed.push(said);
            }
        }
        Fan::all(failed)
    }

    /// Every sink's line, joined — for a caller that wants one string.
    ///
    /// `main` asks [`Fan::said_each`] instead and logs a line apiece.
    fn said(&self) -> String {
        self.said_each().join("; ")
    }
}
