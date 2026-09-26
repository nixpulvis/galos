//! The serving tables: the metadata the client reads beside the cells.
//!
//! The cells carry what the map *draws* — a system's exact position and its
//! fixed-width photometry. These carry what a *click* and a *route* want: a
//! populated system's political columns, how far a system reaches, what it
//! can supercharge, a faction's name, and the name and place of every system
//! for the search box and the router. What a system holds inside it is
//! [`super::bodies`].
//!
//! They mirror the `galos_db` structs field for field and reuse the
//! `elite_journal` enums, so the client renders them through the same code it
//! rendered database rows through, changing only the type it names. They are
//! serde records rather than hand-rolled `FixedCodec`, since they are variable,
//! nested and read one system at a time rather than a million points a frame,
//! so the tedium a fixed layout would trade for is not worth its speed here.

use crate::core::name::SystemName;
use crate::core::record::Boost;
use elite_journal::prelude::{Allegiance, Economy, Government, Security};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A system that changes: its political columns, its name and where it sits.
///
/// The dynamic set the map colors and navigates by, about 96,000 systems
/// against 129 million. Held resident, since a filter reads it over every drawn
/// system every frame and a color cannot wait on a fetch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PopulatedSystem {
    pub address: i64,
    pub name: SystemName,
    pub position: [f32; 3],
    pub population: u64,
    pub security: Option<Security>,
    pub government: Option<Government>,
    pub allegiance: Option<Allegiance>,
    pub primary_economy: Option<Economy>,
    pub secondary_economy: Option<Economy>,
    /// The factions present, by id; a name is [`Faction`], looked up once.
    pub factions: Vec<i32>,
    pub body_count: Option<i32>,
    pub non_body_count: Option<i32>,
}

/// How far a system reaches from its arrival star, in metres.
///
/// The far edge of the furthest thing on record, over its bodies, its stars
/// and the points a close pair goes round. Its own table rather than a column
/// on [`PopulatedSystem`], because the map sizes *every* system it draws by
/// this and only one in forty of them is populated — and its own table rather
/// than a column on [`NameEntry`], because a reach is the one thing here that
/// really changes with the feed: a scan arrives and the system it is about
/// grows. Names and positions change about never, which is what lets that
/// table be published in chunks.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SystemReach {
    pub address: i64,
    /// Metres from the arrival star to the far edge of what is on record
    pub reach: f32,
}

/// A system whose arrival star can supercharge a drive, where it is, and on
/// what.
///
/// Its own table, as the reaches are and for the same reason: it is about
/// every system rather than the populated few, it is a fact the feed reports
/// (a system's main star class arrives with its first scan), and the router
/// reads it over the whole galaxy without fetching anything. Two systems in a
/// hundred are in it — 3,846,802 of 200,071,629.
///
/// **The place is in it because the router's question is where the cones
/// are.** A table of addresses alone made the client join four million of
/// them against the names table's address column to find out — 4 GB of
/// mapping faulted and 7.9 s before a galactic route could begin planning,
/// paid once a session and visible as a click that hung. Twelve bytes a row
/// here is 46 MB over the table and nothing at all at read time, and it is
/// what takes the names table out of the routing path altogether.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SystemBoost {
    pub address: i64,
    pub boost: Boost,
    /// Where it sits, in light years, at the precision the names table
    /// publishes: a route resolves a waypoint by descending to the place,
    /// so a light year of rounding changes nothing.
    pub position: [f32; 3],
}

/// A name and where it is: the search index and the routing graph in one.
///
/// Every system, not just the populated ones, since a search reaches any name
/// and a route steps between any two positions. The positions here are the
/// graph the client runs A* over, so the router needs nothing loaded past this
/// one table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NameEntry {
    pub address: i64,
    pub name: SystemName,
    pub position: [f32; 3],
}

/// A faction's id and the name it is shown under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Faction {
    pub id: i32,
    pub name: String,
}

/// What a system trades in: the most of it, and the next most.
///
/// The pair travels together everywhere a system does: a secondary alone says
/// nothing, so what is optional is the pair. The primary is what makes it worth
/// having, the secondary what it may carry besides.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Economies {
    pub primary: Economy,
    pub secondary: Option<Economy>,
}

impl Economies {
    /// Two economy columns as a pair, if the primary says anything.
    pub fn new(
        primary: Option<Economy>,
        secondary: Option<Economy>,
    ) -> Option<Economies> {
        primary.map(|primary| Economies { primary, secondary })
    }
}

impl fmt::Display for Economies {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.secondary {
            Some(secondary) => write!(f, "{}/{}", self.primary, secondary),
            None => write!(f, "{}", self.primary),
        }
    }
}
