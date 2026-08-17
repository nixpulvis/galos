//! Systems represent star systems in the Milky Way galaxy
use chrono::{DateTime, Utc};
use elite_journal::prelude::*;
use std::fmt;

/// A region already asked about, and the moment its answer is current as of
///
/// What a caller holding a sky holds is not a region but a region and a time:
/// the systems inside `range` of `center` that have not changed since `at`.
/// Anything else it has to ask for, and those two halves are one fact rather
/// than two, which is why they are one type.
///
/// Handed back to [`System::fetch_in_range_of_point`] so that a region already
/// covered is not read again. Several of them compose: a caller that has flown
/// about holds the union of everywhere it has been, and a system is left out of
/// the answer where any one of them vouches for it.
///
/// `at` is the database's clock, from [`crate::Database::now`], and read before
/// the question it stamps. A caller stamping these itself is a caller trusting
/// its clock against the one that writes `updated_at`.
#[derive(Debug, Clone)]
pub struct Survey {
    /// The middle of it, in light years from the galactic center
    pub center: [f64; 3],
    /// How far it reaches from there, in light years
    pub range: f64,
    /// The moment the systems inside it are current as of
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct System {
    pub address: i64,
    // TODO: We need to support multiple names
    pub name: String,
    pub position: Option<Coordinate>,
    pub population: u64,
    pub security: Option<Security>,
    pub government: Option<Government>,
    pub allegiance: Option<Allegiance>,
    pub economies: Option<Economies>,

    /// The factions present in the system, by id
    ///
    /// Ids rather than rows, since this is what a system is asked about in
    /// bulk: which of them a filter admits, over every system drawn, every
    /// frame. What a faction is called is its own row and is looked up once,
    /// by whoever is naming it.
    pub factions: Vec<i32>,

    /// How many bodies the system holds, as against how many are on record
    ///
    /// [`None`] until something reports it: the honk, the all-found tally or
    /// a nav beacon. Against `bodies` it says whether what is known about a
    /// system is all of it or a corner of it.
    pub body_count: Option<i32>,
    /// The belts and rings, which no body table will ever hold
    ///
    /// Only the honk counts them, so this stays [`None`] where the count came
    /// from either of the other two.
    pub non_body_count: Option<i32>,

    /// How far the system reaches from the star it arrives at, in metres
    ///
    /// The furthest of everything on record, measured to the far side of what
    /// is drawn for it. Systems run from a light second across to a fifth of a
    /// light year, so anything drawing a system as a whole has to be told
    /// rather than assume.
    ///
    /// [`None`] where nothing is on record, which is the database unable to
    /// say how far the system reaches rather than a system that reaches
    /// nowhere.
    pub reach: Option<f32>,

    // TODO: Find an elegent way to represent this.
    // & = foreign key = belongs_to
    // pub controlling_faction: &Faction,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

/// What a system trades in: the most of it, and the next most
///
/// The two travel together everywhere a system does. A secondary on its own
/// says nothing, so what is optional is the pair: a system either has an
/// economy on record or it has none at all. Within one, the primary is what
/// makes it worth having, and the secondary is what a system may or may not
/// carry besides.
#[derive(Debug, Clone, Copy)]
pub struct Economies {
    pub primary: Economy,
    pub secondary: Option<Economy>,
}

impl Economies {
    /// What two columns say about a system, if they say anything
    ///
    /// The database keeps the halves apart and nothing there holds them to
    /// each other, so a secondary standing on its own is a row that can be
    /// written even though it means nothing. It is read as silence.
    pub fn new(
        primary: Option<Economy>,
        secondary: Option<Economy>,
    ) -> Option<Self> {
        primary.map(|primary| Economies { primary, secondary })
    }
}

/// The primary, and the secondary after it where there is one
impl fmt::Display for Economies {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.secondary {
            Some(secondary) => write!(f, "{}/{}", self.primary, secondary),
            None => write!(f, "{}", self.primary),
        }
    }
}

mod create;
mod fetch;
pub mod nav;

impl Eq for System {}
impl PartialEq for System {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
    }
}

use std::hash::{Hash, Hasher};
impl Hash for System {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.address.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A primary is an economy, whether or not a secondary comes with it
    #[test]
    fn a_primary_is_an_economy() {
        let alone = Economies::new(Some(Economy::Agriculture), None).unwrap();
        assert_eq!(alone.primary, Economy::Agriculture);
        assert!(alone.secondary.is_none());

        let both =
            Economies::new(Some(Economy::Agriculture), Some(Economy::Tourism))
                .unwrap();
        assert_eq!(both.primary, Economy::Agriculture);
        assert_eq!(both.secondary, Some(Economy::Tourism));
    }

    /// A secondary with no primary is nothing at all
    ///
    /// Two columns can hold that pair even though no system is it, and
    /// reading it as an economy would put a system's second trade forward as
    /// its first.
    #[test]
    fn a_secondary_alone_is_no_economy() {
        assert!(Economies::new(None, Some(Economy::Tourism)).is_none());
        assert!(Economies::new(None, None).is_none());
    }

    /// An economy reads as one name, or as two divided by a stroke
    #[test]
    fn an_economy_writes_out_what_it_has() {
        let alone = Economies::new(Some(Economy::Agriculture), None).unwrap();
        assert_eq!(alone.to_string(), "Agriculture");

        let both =
            Economies::new(Some(Economy::Agriculture), Some(Economy::Tourism))
                .unwrap();
        assert_eq!(both.to_string(), "Agriculture/Tourism");
    }
}
