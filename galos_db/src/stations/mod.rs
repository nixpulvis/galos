//! A station which can be docked at within a system
use chrono::{DateTime, Utc};
use elite_journal::station::{LandingPads, EconomyShare, Service, StationType};
use elite_journal::{Allegiance, Government};

/// ### Schema
///
/// ```
///                              Table "public.stations"
///       Column       |            Type             | Collation | Nullable | Default
///  name              | character varying           |           | not null |
///  ty                | stationtype                 |           | not null |
///  dist_from_star_ls | double precision            |           |          |
///  market_id         | bigint                      |           |          |
///  landing_pads      | landingpads                 |           |          |
///  faction           | character varying           |           |          |
///  government        | government                  |           |          |
///  allegiance        | allegiance                  |           |          |
///  services          | service[]                   |           |          |
///  economies         | economyshare[]              |           |          |
///  updated_at        | timestamp without time zone |           | not null |
///  updated_by        | character varying           |           | not null |
/// Indexes:
///     "stations_pkey" PRIMARY KEY, btree (system_address, name)
/// Foreign-key constraints:
///     "stations_system_address_fkey" FOREIGN KEY (system_address) REFERENCES systems(address)
///     "stations_system_address_fkey1" FOREIGN KEY (system_address) REFERENCES systems(address)
/// ```
#[derive(Debug, PartialEq)]
pub struct Station {
    pub system_address: i64,
    pub name: String,
    pub ty: Option<StationType>,
    pub dist_from_star_ls: Option<f64>,
    pub market_id: Option<i64>,
    pub landing_pads: Option<LandingPads>,
    pub faction: Option<String>,        // TODO: Faction type?
    pub government: Option<Government>, // TODO: Government type?
    pub allegiance: Option<Allegiance>,
    pub services: Option<Vec<Service>>,
    pub economies: Option<Vec<EconomyShare>>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

impl Eq for Station {}

mod create;
mod fetch;
