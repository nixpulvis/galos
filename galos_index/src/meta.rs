//! The serving records: the metadata the client reads beside the cells.
//!
//! The cells carry what the map *draws* — a system's exact position and its
//! fixed-width photometry. These carry what a *click* wants: a populated
//! system's political columns, the bodies inside a system, a faction's name,
//! and the name and place of every system for the search box and the router.
//!
//! They mirror the `galos_db` structs field for field and reuse the
//! `elite_journal` enums, so the client renders them through the same code it
//! rendered database rows through, changing only the type it names. They are
//! serde records rather than hand-rolled `FixedCodec`, since they are variable,
//! nested and read one system at a time rather than a million points a frame,
//! so the tedium a fixed layout would trade for is not worth its speed here.

use crate::name::SystemName;
use chrono::{DateTime, Utc};
use elite_journal::body::{
    AtmosphereType, BodyType, Composition, Material, Orbit, Spin,
};
use elite_journal::prelude::{Allegiance, Economy, Government, Security};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

/// What a system's arrival star can supercharge a frame shift drive by
///
/// Flying the jet cone of a neutron star or a white dwarf in supercruise, with
/// a fuel scoop, charges the drive for one jump: four times the range off a
/// neutron star, half again off a white dwarf, and more of both off a drive
/// built for it. The charge is held until a jump spends it, so what it is
/// worth is a fact about the system a ship is standing in and not about how it
/// got there — which is what lets the router read it as a property of a place.
///
/// Which of the two, rather than the multiplier: what a boost is worth depends
/// on the drive fitted, and the table is about the sky. See
/// `galos_map::systems::route::graph::Drive`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Boost {
    /// A white dwarf: half again, and a much larger exclusion zone to be
    /// caught out by.
    WhiteDwarf,
    /// A neutron star: four times over, which is what a neutron highway is.
    Neutron,
}

impl Boost {
    /// What the star of class `primary_star_class` can supercharge, if it can
    ///
    /// The arrival star's class, which is the one that matters: a ship drops in
    /// at the main star and can reach its jet cone without crossing the system.
    /// A neutron star is class `N`; every white dwarf class begins with `D`
    /// (`DA`, `DB`, `DC` and their variants). Nothing else has a jet cone to
    /// fly — a black hole is class `H` and gives nothing, whatever it looks
    /// like it should.
    pub fn of(primary_star_class: &str) -> Option<Boost> {
        match primary_star_class {
            "N" => Some(Boost::Neutron),
            class if class.starts_with('D') => Some(Boost::WhiteDwarf),
            _ => None,
        }
    }

    /// What the star is called, for a reader rather than for a router
    ///
    /// The class is all this table keeps of a star — the index publishes
    /// what can supercharge and on what, not a spectrum — so it is the one
    /// thing a client can say about a star's kind without fetching the
    /// system's bodies.
    pub fn named(&self) -> &'static str {
        match self {
            Boost::Neutron => "neutron star",
            Boost::WhiteDwarf => "white dwarf",
        }
    }
}

/// Whether a ship can refuel at a star of this class
///
/// A fuel scoop takes hydrogen out of a star's corona, and a star either has
/// hydrogen to give or it does not: the main sequence classes **K, G, B, F,
/// O, A and M** do, and nothing else in the sky does. A brown dwarf is too
/// cool to have started fusing, a white dwarf and a neutron star are what is
/// left after the hydrogen went, a carbon star's envelope is the wrong
/// element, and a black hole gives nothing at all.
///
/// Which is why a route that cannot be refuelled is not a slower route but a
/// stranded ship, and the reason this is a fact worth publishing rather than
/// guessing. **Temperature cannot answer it**: the map's six log-spaced
/// buckets put M dwarfs, brown dwarfs and black holes in the same bucket and
/// O stars in with neutron stars, so the class itself is the only honest
/// source.
///
/// Matched on the whole class and not on its first letter, because the sky
/// has classes that begin with a scoopable letter and are not: `MS` and `S`
/// are S-type stars, which share no hydrogen envelope with an `M` dwarf, and
/// `AeBe` is a Herbig star rather than an `A`. The giants and supergiants
/// *are* the same star grown — `M_RedGiant`, `K_OrangeGiant`,
/// `A_BlueWhiteSuperGiant` — and the game scoops them, so a class is
/// scoopable when it is one of the seven letters exactly or that letter
/// followed by what kind of one it is.
pub fn scoopable(primary_star_class: &str) -> bool {
    StarKind::of(primary_star_class).scoops()
}

/// What kind of star a system arrives at, in one byte
///
/// **The fact a fuel-aware route is built on, sized to be carried rather
/// than looked up.** A router asks it of every system it expands, which
/// rules out anything with a hash in it: the classed systems are some
/// ninety-five million, and a resident map over them is gigabytes. A byte
/// is what a cell's payload can hold beside a position the router is
/// faulting anyway.
///
/// The kinds are the families the game distinguishes and not the spectrum:
/// what a route needs to know is whether a ship can refuel, whether it can
/// supercharge, and what to call the thing. A subclass and a luminosity say
/// nothing to either question.
///
/// [`Self::Unknown`] is a real answer and the commonest one. Most of the
/// galaxy has never been scanned, and "nothing has looked" has to be told
/// apart from "nothing to scoop" — a route across unexplored space would
/// otherwise read as a route that strands.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum StarKind {
    /// Nothing has said, which is most of the sky
    #[default]
    Unknown = 0,
    /// The main sequence, which is what a fuel scoop lives on
    O = 1,
    B = 2,
    A = 3,
    F = 4,
    G = 5,
    K = 6,
    M = 7,
    /// Too cool to have started fusing: L, T and Y
    BrownDwarf = 8,
    /// What is left when the hydrogen went, and half again on a jump
    WhiteDwarf = 9,
    /// The same, and four times over: class N
    Neutron = 10,
    /// Class H, and the supermassive one at the centre
    BlackHole = 11,
    /// Carbon and S-type: an envelope of the wrong element
    Carbon = 12,
    /// Wolf-Rayet, which is a core with its envelope blown off
    WolfRayet = 13,
    /// Still forming: T Tauri and Herbig Ae/Be
    Forming = 14,
    /// Something the galaxy holds that none of the above names
    Other = 15,
}

impl StarKind {
    /// What the class the journals and the dumps state comes to
    ///
    /// Matched on the whole class rather than its first letter, because the
    /// sky has classes that begin with a scoopable letter and hold nothing
    /// to scoop: `MS` and `S` are S-type stars and `AeBe` is a Herbig star,
    /// while `M_RedGiant` and `K_OrangeGiant` are the same stars grown.
    pub fn of(primary_star_class: &str) -> StarKind {
        let mut letters = primary_star_class.chars();
        let Some(first) = letters.next() else {
            return StarKind::Unknown;
        };
        let sort = letters.next();
        // The letter alone, or the letter and what kind of one it is.
        let plain = matches!(sort, None | Some('_'));
        match (first, plain) {
            ('O', true) => StarKind::O,
            ('B', true) => StarKind::B,
            ('A', true) => StarKind::A,
            ('F', true) => StarKind::F,
            ('G', true) => StarKind::G,
            ('K', true) => StarKind::K,
            ('M', true) => StarKind::M,
            ('L' | 'T' | 'Y', true) => StarKind::BrownDwarf,
            ('N', true) => StarKind::Neutron,
            ('H', _) => StarKind::BlackHole,
            ('D', _) => StarKind::WhiteDwarf,
            ('W', _) => StarKind::WolfRayet,
            ('C' | 'S', _) | ('M', false) => StarKind::Carbon,
            ('T', false) => StarKind::Forming,
            ('A', false) => StarKind::Forming,
            _ => StarKind::Other,
        }
    }

    /// Whether a ship can refuel here
    ///
    /// A fuel scoop takes hydrogen out of a star's corona, and a star either
    /// has hydrogen to give or it does not: the main sequence classes **K,
    /// G, B, F, O, A and M** do, and nothing else in the sky does. Which is
    /// why a route that cannot be refuelled is not a slower route but a
    /// stranded ship.
    ///
    /// [`Self::Unknown`] answers false and the caller has to tell the two
    /// apart itself — see the type's own note.
    pub fn scoops(&self) -> bool {
        matches!(
            self,
            StarKind::O
                | StarKind::B
                | StarKind::A
                | StarKind::F
                | StarKind::G
                | StarKind::K
                | StarKind::M
        )
    }

    /// What it can supercharge a drive on, where it can
    ///
    /// The same answer [`Boost::of`] reads off the class string, off the
    /// byte instead: a cone is a white dwarf's or a neutron star's.
    pub fn boost(&self) -> Option<Boost> {
        match self {
            StarKind::WhiteDwarf => Some(Boost::WhiteDwarf),
            StarKind::Neutron => Some(Boost::Neutron),
            _ => None,
        }
    }

    /// What it is called, for a reader rather than for a router
    ///
    /// [`None`] where nothing has said, so a row can leave the column empty
    /// rather than print a guess.
    pub fn named(&self) -> Option<&'static str> {
        Some(match self {
            StarKind::Unknown => return None,
            StarKind::O => "class O",
            StarKind::B => "class B",
            StarKind::A => "class A",
            StarKind::F => "class F",
            StarKind::G => "class G",
            StarKind::K => "class K",
            StarKind::M => "class M",
            StarKind::BrownDwarf => "brown dwarf",
            StarKind::WhiteDwarf => "white dwarf",
            StarKind::Neutron => "neutron star",
            StarKind::BlackHole => "black hole",
            StarKind::Carbon => "carbon star",
            StarKind::WolfRayet => "Wolf-Rayet",
            StarKind::Forming => "forming star",
            StarKind::Other => "unusual star",
        })
    }

    /// The byte a payload carries it as.
    pub fn code(&self) -> u8 {
        *self as u8
    }

    /// And back, anything unrecognised reading as nothing having been said:
    /// a byte from a build that knew kinds this one does not is a kind this
    /// one cannot claim anything about.
    pub fn from_code(code: u8) -> StarKind {
        match code {
            1 => StarKind::O,
            2 => StarKind::B,
            3 => StarKind::A,
            4 => StarKind::F,
            5 => StarKind::G,
            6 => StarKind::K,
            7 => StarKind::M,
            8 => StarKind::BrownDwarf,
            9 => StarKind::WhiteDwarf,
            10 => StarKind::Neutron,
            11 => StarKind::BlackHole,
            12 => StarKind::Carbon,
            13 => StarKind::WolfRayet,
            14 => StarKind::Forming,
            15 => StarKind::Other,
            _ => StarKind::Unknown,
        }
    }
}

impl crate::serialization::Encode for StarKind {
    fn encode(&self, out: &mut Vec<u8>) {
        crate::serialization::Encode::encode(&self.code(), out);
    }
}

impl crate::serialization::Decode for StarKind {
    fn decode(cur: &mut &[u8]) -> Option<StarKind> {
        Some(StarKind::from_code(<u8 as crate::serialization::Decode>::decode(
            cur,
        )?))
    }
}

impl crate::serialization::FixedCodec for StarKind {
    const LEN: usize = 1;
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

/// One ancestor of a body, as the scan named it, nearest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parent {
    pub ty: Option<String>,
    pub id: i16,
}

impl Parent {
    /// Whether this ancestor is a barycentre
    ///
    /// The journal names a barycentre parent `Null` and nothing else by that
    /// name, so the type alone answers it. Worth asking even where the
    /// barycentre has no row of its own: what kind of thing a chain names is
    /// known from the naming, and only its own orbit waits on its scan.
    pub fn is_barycenter(&self) -> bool {
        self.ty.as_deref() == Some("Null")
    }

    /// The ancestry a scan named, nearest first.
    ///
    /// A scan writes each ancestor as a one-entry map of kind to id, and the
    /// walk back to the star is what places the thing, so the order and the
    /// whole chain are kept. `galos_db::bodies::Parent::chain`'s rule, in
    /// the index's own vocabulary.
    pub fn chain(named: &[BTreeMap<String, i16>]) -> Vec<Parent> {
        named
            .iter()
            .filter_map(|parent| {
                let (ty, id) = parent.iter().next()?;
                Some(Parent { ty: Some(ty.clone()), id: *id })
            })
            .collect()
    }
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
    /// Whether anybody had mapped the star when it was scanned, which is a
    /// fact about the star rather than about the scan the way the discovery
    /// flag that used to sit beside this was.
    pub mapped: bool,
    /// When the star was found, where a scan on record says so.
    ///
    /// [`None`] where every scan on record found it already discovered, such
    /// a scan saying somebody had been there without saying when.
    pub discovered_at: Option<DateTime<Utc>>,
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
    /// Whether anybody had mapped the body when it was scanned, which is a
    /// fact about the body rather than about the scan the way the discovery
    /// flag that used to sit beside this was.
    pub mapped: bool,
    /// When the body was found, where a scan on record says so.
    ///
    /// [`None`] where every scan on record found it already discovered, such
    /// a scan saying somebody had been there without saying when.
    pub discovered_at: Option<DateTime<Utc>>,
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
