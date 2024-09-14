//! Systems represent star systems in the Milky Way galaxy
use serde::Deserialize;
use chrono::{DateTime, Utc};
use elite_journal::prelude::*;

// {
//     address: 8863905784394,
//     name: "EOL PROU KW-L C8-32",
//     position: {
//         type: "Point",
//         coordinates: [-9581.6875,-932.21875,19788.59375]
//     },
//     population: null,
//     security: null,
//     government: null,
//     allegiance: null,
//     primary_economy: null,
//     secondary_economy: null,
//     updated_at: "2024-09-14T18:21:49",
//     updated_by: "ff7005fa8ac4baf8712fcd7e5a4bc57f5bb92170"
// }

#[derive(Deserialize, Debug, Clone)]
pub struct System {
    pub address: i64,
    // TODO: We need to support multiple names
    pub name: String,
    pub position: Option<Coordinate>,
    pub population: Option<i64>,
    pub security: Option<Security>,
    pub government: Option<Government>,
    pub allegiance: Option<Allegiance>,
    pub primary_economy: Option<Economy>,
    pub secondary_economy: Option<Economy>,

    // TODO: Find an elegent way to represent this.
    // & = foreign key = belongs_to
    // pub controlling_faction: &Faction,
    // pub factions: Vec<Faction>
    pub updated_at: String,
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
