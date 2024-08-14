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

    // TODO: Find an elegent way to represent this.
    // & = foreign key = belongs_to
    // pub controlling_faction: &Faction,
    // pub factions: Vec<Faction>
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
