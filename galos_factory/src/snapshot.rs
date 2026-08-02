//! Snapshot input structs: the sim's entire view of the outside world.
//! Pure serde data mirroring galos_db columns — filled from RON fixtures
//! (standalone runner, tests), sqlx (galos_factory_db), or HTTP (future).
//! This crate never touches a database.
//!
//! Field types are `elite_journal`'s own enums wherever it models the
//! concept, so a snapshot loaded from Postgres needs no translation: the
//! DB columns are these same types.

use elite_journal::faction::State;
use elite_journal::station::StationType;
use elite_journal::system::{Economy, Security};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemSnapshot {
    pub address: i64,
    pub name: String,
    pub security: Security,
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
    /// A plain string upstream in `elite_journal::Star` too.
    pub class: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BodySnapshot {
    pub id: i64,
    pub name: String,
    /// `bodies.planet_class` (e.g. "Metal rich body", "Icy body"). Still a
    /// string in `elite_journal::Body`, which has a TODO for an enum.
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
    pub ty: StationType,
    /// Body the station sits on (surface) or orbits; None = star orbit.
    #[serde(default)]
    pub body: Option<String>,
    pub surface: bool,
    pub dist_ls: u32,
    /// From `stations.economies`, most significant first.
    #[serde(default)]
    pub economies: Vec<Economy>,
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
    pub state: State,
    /// Happiness band 1 (elated) ..= 5 (despondent); 3 = discontented.
    /// `elite_journal::Happiness` renames its variants to the raw
    /// `$Faction_HappinessBand2;` tokens, which RON cannot spell, so
    /// snapshots carry the band number and [`happiness_band`] converts.
    pub happiness_band: u8,
}

/// Band number for an `elite_journal::Happiness`, for callers loading
/// snapshots straight from the database.
pub fn happiness_band(happiness: elite_journal::faction::Happiness) -> u8 {
    use elite_journal::faction::Happiness::*;
    match happiness {
        Elated => 1,
        Happy => 2,
        Discontented => 3,
        Unhappy => 4,
        Despondent => 5,
        None => 3,
    }
}
