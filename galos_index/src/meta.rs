//! The serving records: the metadata the client reads beside the cells.
//!
//! The cells carry what the map *draws* — a system's position and photometry,
//! quantized and fixed-width. These carry what a *click* wants: a populated
//! system's political columns, the bodies inside a system, a faction's name,
//! and the name and place of every system for the search box and the router.
//!
//! They mirror the `galos_db` structs field for field and reuse the
//! `elite_journal` enums, so the client renders them through the same code it
//! rendered database rows through, changing only the type it names. They are
//! serde records rather than hand-rolled `FixedCodec`, since they are variable,
//! nested and read one system at a time rather than a million points a frame,
//! so the tedium a fixed layout would trade for is not worth its speed here.

use chrono::{DateTime, Utc};
use elite_journal::body::{
    AtmosphereType, BodyType, Composition, Discovery, Material, Orbit, Spin,
};
use std::fmt;
use elite_journal::prelude::{Allegiance, Economy, Government, Security};
use serde::{Deserialize, Serialize};

/// A system that changes: its political columns, its name and where it sits.
///
/// The dynamic set the map colours and navigates by, about 96,000 systems
/// against 129 million. Held resident, since a filter reads it over every drawn
/// system every frame and a colour cannot wait on a fetch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PopulatedSystem {
    pub address: i64,
    pub name: String,
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
    pub reach: Option<f32>,
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
    pub name: String,
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

/// One ancestor of a body, as the scan named it, nearest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parent {
    pub ty: Option<String>,
    pub id: i16,
}

/// What a body with a surface has, and a gas giant has none of.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub atmosphere_type: AtmosphereType,
    pub pressure: f32,
    pub composition: Option<Composition>,
    pub landable: bool,
    pub atmosphere: Option<String>,
    pub volcanism: Option<String>,
    pub terraform_state: Option<String>,
    pub materials: Vec<Material>,
}

/// A star within a system, with the fields a scan and its photometry carry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Star {
    pub system_address: i64,
    pub id: i16,
    pub name: String,
    /// Every ancestor the scan named, nearest first, empty for the primary.
    pub parents: Vec<Parent>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
    pub absolute_magnitude: f32,
    pub age_my: i32,
    pub distance_from_arrival_ls: f32,
    pub luminosity: String,
    pub star_class: String,
    pub stellar_mass: f32,
    pub subclass: i16,
    /// [`None`] for the primary, which goes round nothing.
    pub orbit: Option<Orbit>,
    pub spin: Spin,
    pub radius: f32,
    pub temperature: f32,
    pub discovery: Discovery,
}

/// A body within a system.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub system_address: i64,
    pub id: i16,
    /// Every ancestor the scan named, nearest first.
    pub parents: Vec<Parent>,
    pub name: String,
    pub body_type: Option<BodyType>,
    /// How far from the arrival star, in light seconds.
    pub distance_from_arrival: Option<f32>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
    pub planet_class: String,
    pub tidal_lock: bool,
    pub mass: f32,
    pub radius: f32,
    /// Measured at the cloud tops where there is no surface, so not in
    /// [`Surface`].
    pub gravity: f32,
    pub temperature: Option<f32>,
    /// [`None`] for a gas giant, which has no surface to record.
    pub surface: Option<Surface>,
    pub orbit: Orbit,
    pub spin: Spin,
    pub discovery: Discovery,
}

/// The center of mass a close pair of bodies goes round.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Barycenter {
    pub system_address: i64,
    pub id: i16,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
    /// [`None`] for the barycenter at the root of a multi-star system.
    pub orbit: Option<Orbit>,
}

/// Everything a click into a system pulls: its stars, bodies and barycenters.
///
/// One file per system, keyed by address, so the map fetches exactly the
/// system a click opened and nothing else. Empty where a system has no scan on
/// record, which reads the same as a system whose file was never written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemBodies {
    pub stars: Vec<Star>,
    pub bodies: Vec<Body>,
    pub barycenters: Vec<Barycenter>,
}

impl Star {
    /// The nearest ancestor, which is what the star's orbit is measured about.
    pub fn parent_id(&self) -> Option<i16> {
        self.parents.first().map(|parent| parent.id)
    }
}

impl Body {
    /// The nearest ancestor, which is what the body's orbit is measured about.
    pub fn parent_id(&self) -> Option<i16> {
        self.parents.first().map(|parent| parent.id)
    }
}
