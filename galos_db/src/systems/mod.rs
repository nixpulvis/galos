//! Systems represent star systems in the Milky Way galaxy
use chrono::{DateTime, Utc};
use elite_journal::prelude::*;

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
    pub primary_economy: Option<Economy>,
    pub secondary_economy: Option<Economy>,

    /// The factions present in the system, by id
    ///
    /// Ids rather than rows, since this is what a system is asked about in
    /// bulk: which of them a filter admits, over every system drawn, every
    /// frame. What a faction is called is its own row and is looked up once,
    /// by whoever is naming it.
    pub factions: Vec<i32>,

    // TODO: Find an elegent way to represent this.
    // & = foreign key = belongs_to
    // pub controlling_faction: &Faction,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
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
