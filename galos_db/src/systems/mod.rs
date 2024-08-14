//! Systems represent star systems in the Milky Way galaxy
use chrono::{DateTime, Utc};
use elite_journal::prelude::*;

/// ### Schema
///
/// ```
///                              Table "public.systems"
///      Column       |            Type             | Collation | Nullable | Default
/// -------------------+-----------------------------+-----------+----------+---------
/// address           | bigint                      |           | not null |
/// name              | character varying           |           | not null |
/// position          | geometry(PointZ)            |           |          |
/// population        | bigint                      |           |          |
/// security          | security                    |           |          |
/// government        | government                  |           |          |
/// allegiance        | allegiance                  |           |          |
/// primary_economy   | economy                     |           |          |
/// secondary_economy | economy                     |           |          |
/// updated_at        | timestamp without time zone |           | not null |
/// updated_by        | character varying           |           | not null |
/// Indexes:
///     "systems_pkey" PRIMARY KEY, btree (address)
///     "systems_position_idx" gist ("position" gist_geometry_ops_nd)
///     "systems_position_key" UNIQUE CONSTRAINT, btree ("position")
///     "systems_upper_idx" btree (upper(name::text))
/// Referenced by:
///     TABLE "bodies" CONSTRAINT "bodies_system_address_fkey" FOREIGN KEY (system_address) REFERENCES systems(address)
///     TABLE "conflicts" CONSTRAINT "conflicts_system_address_fkey" FOREIGN KEY (system_address) REFERENCES systems(address)
///     TABLE "stations" CONSTRAINT "stations_system_address_fkey" FOREIGN KEY (system_address) REFERENCES systems(address)
///     TABLE "stations" CONSTRAINT "stations_system_address_fkey1" FOREIGN KEY (system_address) REFERENCES systems(address)
///     TABLE "system_faction_influences" CONSTRAINT "system_faction_influences_system_address_fkey" FOREIGN KEY (system_address) REFERENCES systems(address)
///     TABLE "system_faction_states" CONSTRAINT "system_faction_states_system_address_fkey" FOREIGN KEY (system_address) REFERENCES systems(address)
///     TABLE "system_factions" CONSTRAINT "system_factions_system_address_fkey" FOREIGN KEY (system_address) REFERENCES systems(address)
/// ```
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
