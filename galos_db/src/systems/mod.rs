//! Systems represent star systems in the Milky Way galaxy
use crate::Error;
use chrono::{DateTime, Utc};
use elite_journal::prelude::*;
use galos_index::core::procedural;
use galos_index::SystemName;
use std::fmt;

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

    // TODO: Find an elegent way to represent this.
    // & = foreign key = belongs_to
    // pub controlling_faction: &Faction,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

impl System {
    /// The name a row holds, or the one its address spells.
    ///
    /// A null `name` is the whole of what that column saves: 97.3 % of a
    /// galaxy's names are what [`procedural::name_of`] spells out of the
    /// address, so what is written down is the exceptions -- the names
    /// people gave, and Frontier's hand-authored regions. Every read of the
    /// column comes through here, there being one rule about what a null
    /// means and no reason to spell it out at each of the reads.
    ///
    /// A null the arithmetic cannot answer is [`Error::Nameless`]: not a
    /// system without a name, but a row written against the rule, and said
    /// so rather than handed back as an empty name.
    pub fn name_of(
        address: i64,
        stored: Option<String>,
    ) -> Result<SystemName, Error> {
        match stored {
            Some(name) => Ok(SystemName::new(name)),
            None => {
                procedural::name_of(address).ok_or(Error::Nameless(address))
            }
        }
    }
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

/// What a system write did to the row it was for
///
/// Read out of the upsert itself rather than queried afterwards: the
/// statement returns whether it inserted the row and whether the row's
/// stamp ended up at this reading's, which is all three cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landed {
    /// No row existed; this write created it.
    New,
    /// A row existed and this reading is what it now says.
    Updated,
    /// A row existed with a newer reading, so this one did not win.
    ///
    /// Not a refusal: an older reading still fills columns the row has
    /// never held, per the merge rule in [`System::create`]. It means the
    /// stamp did not move, so counting it as an update would count a
    /// reading that was thrown away.
    Stale,
}

impl Landed {
    /// Read the two flags the upsert returns.
    ///
    /// `inserted` is `xmax = 0`, exact for a statement writing one row:
    /// the conflict path locks the row it updates and the new version
    /// carries that lock. `took` is `updated_at = $stamp`, which given
    /// `updated_at = GREATEST(old, $stamp)` is the same test the merge
    /// arms use.
    fn of(inserted: bool, took: bool) -> Landed {
        match (inserted, took) {
            (true, _) => Landed::New,
            (false, true) => Landed::Updated,
            (false, false) => Landed::Stale,
        }
    }

    /// The stronger of two landings, for a write with more than one part
    ///
    /// `New` beats `Updated` beats `Stale` beats nothing: a reading that
    /// created a system in either part created one. Per-store numbers are
    /// in each store's own end-of-run line.
    pub fn widest(a: Option<Landed>, b: Option<Landed>) -> Option<Landed> {
        fn rank(it: Option<Landed>) -> u8 {
            match it {
                Some(Landed::New) => 3,
                Some(Landed::Updated) => 2,
                Some(Landed::Stale) => 1,
                None => 0,
            }
        }
        if rank(a) >= rank(b) {
            a
        } else {
            b
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
