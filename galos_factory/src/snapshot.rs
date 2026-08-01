//! Snapshot input structs: the sim's entire view of the outside world.
//! Pure serde data mirroring galos_db columns — filled from RON fixtures
//! (standalone runner, tests), sqlx (galos_factory_db), or HTTP (future).
//! This crate never touches a database.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemSnapshot {
    pub address: i64,
    pub name: String,
    /// `systems.security`, lowercase ("high"/"medium"/"low"/"anarchy").
    pub security: String,
    pub population: u64,
    pub stars: Vec<StarSnapshot>,
    pub bodies: Vec<BodySnapshot>,
    pub stations: Vec<StationSnapshot>,
    pub factions: Vec<FactionSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StarSnapshot {
    pub name: String,
    /// `stars.star_class` / `systems.primary_star_class` (e.g. "G", "M").
    pub class: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BodySnapshot {
    pub id: i64,
    pub name: String,
    /// `bodies.planet_class` (e.g. "Metal rich body", "Icy body").
    pub planet_class: String,
    pub landable: bool,
    /// `bodies.volcanism`, non-empty when present.
    #[serde(default)]
    pub volcanism: Option<String>,
    #[serde(default)]
    pub atmosphere: Option<String>,
    /// Derived from semi-major axis / arrival distance, light-seconds.
    pub dist_ls: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StationSnapshot {
    pub name: String,
    /// `stations.ty` (e.g. "Coriolis", "Outpost", "CraterOutpost").
    pub ty: String,
    /// Body the station sits on (surface) or orbits; None = star orbit.
    #[serde(default)]
    pub body: Option<String>,
    pub surface: bool,
    pub dist_ls: u32,
    pub shipyard: bool,
    pub listings: Vec<ListingSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListingSnapshot {
    /// EDDN commodity name — joins `items.ron` ids with `ed: true`.
    pub item: String,
    pub mean_price: u32,
    pub stock: u32,
    pub demand: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactionSnapshot {
    pub name: String,
    /// 0..=100 as delivered by the journal.
    pub influence: f32,
    /// BGS state name ("Boom", "Bust", "War", "None", ...).
    pub state: String,
    /// Happiness band 1 (elated) ..= 5 (despondent); 3 = content.
    pub happiness_band: u8,
}
