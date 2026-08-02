//! Snapshot input structs: the sim's entire view of the outside world.
//! Pure serde data mirroring galos_db columns — filled from RON fixtures
//! (standalone runner, tests), sqlx (galos_factory_db), or HTTP (future).
//! This crate never touches a database.
//!
//! Field types are `elite_journal`'s own enums wherever it models the
//! concept, so a snapshot loaded from Postgres needs no translation: the
//! DB columns are these same types.

use elite_journal::faction::{Happiness, State};
use elite_journal::station::StationType;
use elite_journal::system::{Economy, Security};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
    /// `stations.faction` — the faction that owns and runs this station.
    /// Falls back to the system's highest-influence faction when absent.
    #[serde(default)]
    pub controlling_faction: Option<String>,
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
    /// Serialized as its band number — see [`happiness_band`].
    #[serde(with = "happiness_band", rename = "happiness_band")]
    pub happiness: Happiness,
}

/// `Happiness` on the wire.
///
/// Every other BGS field here is its `elite_journal` type verbatim, but
/// that crate renames the `Happiness` variants to the raw journal tokens
/// (`$Faction_HappinessBand2;`), which RON cannot spell as identifiers.
/// Snapshots therefore carry the band number — 1 (elated) ..= 5
/// (despondent), 0 for unknown — and convert here, so the band
/// representation never escapes the parsing layer.
mod happiness_band {
    use super::*;

    pub fn serialize<S: Serializer>(
        happiness: &Happiness,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(match happiness {
            Happiness::Elated => 1,
            Happiness::Happy => 2,
            Happiness::Discontented => 3,
            Happiness::Unhappy => 4,
            Happiness::Despondent => 5,
            Happiness::None => 0,
        })
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Happiness, D::Error> {
        match u8::deserialize(deserializer)? {
            0 => Ok(Happiness::None),
            1 => Ok(Happiness::Elated),
            2 => Ok(Happiness::Happy),
            3 => Ok(Happiness::Discontented),
            4 => Ok(Happiness::Unhappy),
            5 => Ok(Happiness::Despondent),
            band => Err(serde::de::Error::custom(format!(
                "happiness band {band} is not in 0..=5"
            ))),
        }
    }
}
