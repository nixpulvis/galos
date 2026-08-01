//! ECS components mirroring the world model: bodies carry geology, stations
//! are the buildable containers (power grid + shared storage + slots),
//! factories occupy slots, ships fulfill contracts between stations.

use crate::data::{BuildingKind, ItemId, RecipeId};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Owner {
    Player,
    Npc,
}

// ---------------------------------------------------------------- bodies

#[derive(Component, Clone, Debug)]
pub struct Body {
    pub name: String,
    /// Light-seconds from the system's arrival star; drives travel times.
    pub dist_ls: u32,
}

/// Mineable deposits with milli-scaled richness (1000 = nominal).
#[derive(Component, Clone, Debug, Default)]
pub struct Deposits(pub Vec<(ItemId, u32)>);

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct BodyEnv {
    pub volcanism: bool,
    /// Extra power demand multiplier from hostile temp/atmosphere (1000 = none).
    pub overhead_milli: u32,
}

// -------------------------------------------------------------- stations

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// On a landable body.
    Surface(Entity),
    /// Orbiting a body, or the star itself when `None`.
    Orbital(Option<Entity>),
}

#[derive(Component, Clone, Debug)]
pub struct Station {
    pub name: String,
    pub placement: Placement,
    pub owner: Owner,
    /// Light-seconds from the arrival star; drives travel times.
    pub dist_ls: u32,
}

#[derive(Component, Clone, Debug)]
pub struct Slots {
    pub total: u32,
}

/// The station's shared storage pool — the implicit belt network.
#[derive(Component, Clone, Debug)]
pub struct Storage {
    pub pool: HashMap<ItemId, u32>,
    pub cap: u32,
}

impl Storage {
    pub fn new(cap: u32) -> Self {
        Storage { pool: HashMap::new(), cap }
    }

    pub fn total(&self) -> u32 {
        self.pool.values().sum()
    }

    pub fn count(&self, item: ItemId) -> u32 {
        self.pool.get(&item).copied().unwrap_or(0)
    }

    pub fn free(&self) -> u32 {
        self.cap.saturating_sub(self.total())
    }

    /// Adds up to `qty`, clamped by capacity; returns the amount stored.
    pub fn add(&mut self, item: ItemId, qty: u32) -> u32 {
        let stored = qty.min(self.free());
        if stored > 0 {
            *self.pool.entry(item).or_insert(0) += stored;
        }
        stored
    }

    /// Removes up to `qty`; returns the amount actually taken.
    pub fn take(&mut self, item: ItemId, qty: u32) -> u32 {
        let have = self.count(item);
        let taken = qty.min(have);
        if taken > 0 {
            let slot = self.pool.get_mut(&item).unwrap();
            *slot -= taken;
            if *slot == 0 {
                self.pool.remove(&item);
            }
        }
        taken
    }

    /// True when every `(item, qty)` is available.
    pub fn has_all(&self, needs: &[(ItemId, u32)]) -> bool {
        needs.iter().all(|(item, qty)| self.count(*item) >= *qty)
    }

    /// Takes all `(item, qty)` atomically; false (and untouched) if short.
    pub fn take_all(&mut self, needs: &[(ItemId, u32)]) -> bool {
        if !self.has_all(needs) {
            return false;
        }
        for (item, qty) in needs {
            self.take(*item, *qty);
        }
        true
    }
}

/// Per-tick power balance, recomputed by the `power_balance` system.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PowerGrid {
    pub supply_mw: u32,
    pub demand_mw: u32,
    /// min(1000, supply/demand): every consumer runs at this fraction.
    pub satisfaction_milli: u32,
}

/// Marks stations offering the E:D `Shipyard` service — where ships are bought.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Shipyard;

/// Station condition from the upkeep system.
#[derive(Component, Clone, Copy, Debug)]
pub struct LifeSupport {
    pub life_support_ok: bool,
}

impl Default for LifeSupport {
    fn default() -> Self {
        LifeSupport { life_support_ok: true }
    }
}

// ------------------------------------------------------------- factories

#[derive(Component, Clone, Copy, Debug)]
pub struct Factory {
    pub kind: BuildingKind,
    pub station: Entity,
}

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ActiveRecipe(pub Option<RecipeId>);

/// Idle the factory while the station pool holds at least this much of the
/// recipe's primary output — the throttle that stops hoarding.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct OutputCap(pub Option<u32>);

/// Craft state: inputs are pulled from station storage when a cycle starts
/// (`holding` = true), outputs pushed when `progress_milli` completes.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct CraftProgress {
    pub progress_milli: u64,
    pub holding: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FactoryStatus {
    #[default]
    Idle,
    Running,
    Starved,
    OutputBlocked,
    Offline,
}

/// Display/bookkeeping status, refreshed each tick by the sim systems.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Status(pub FactoryStatus);

/// Set while a building's maintenance went unpaid; cleared on the next
/// successful charge. Offline buildings neither work nor draw power.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct MaintenanceDue(pub bool);

// -------------------------------------------------------------- shipping

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShipClass {
    Hauler,
    Type6,
    Type7,
    Type9,
}

impl ShipClass {
    pub fn cargo_cap(self) -> u32 {
        match self {
            ShipClass::Hauler => 16,
            ShipClass::Type6 => 100,
            ShipClass::Type7 => 220,
            ShipClass::Type9 => 500,
        }
    }

    pub fn fuel_per_leg(self) -> u32 {
        match self {
            ShipClass::Hauler => 1,
            ShipClass::Type6 => 2,
            ShipClass::Type7 => 4,
            ShipClass::Type9 => 8,
        }
    }

    pub fn price(self) -> i64 {
        match self {
            ShipClass::Hauler => 30_000,
            ShipClass::Type6 => 250_000,
            ShipClass::Type7 => 900_000,
            ShipClass::Type9 => 4_000_000,
        }
    }
}

/// A standing agreement to move `item` from one station to another.
/// A player "supply route" is a self-issued contract with assigned ships.
#[derive(Component, Clone, Debug)]
pub struct Contract {
    pub issuer: Owner,
    pub from: Entity,
    pub to: Entity,
    pub item: ItemId,
    /// Credits per delivered unit, paid by the issuer to the carrier.
    /// Zero for self-contracts.
    pub pay_per_unit: u32,
    /// Stop hauling while the destination holds at least this much of the
    /// item (request threshold). `None` = haul everything available.
    pub target: Option<u32>,
    /// Never draw the origin below this floor (surplus-export threshold).
    pub reserve: u32,
}

#[derive(Component, Clone, Debug)]
pub struct Ship {
    pub class: ShipClass,
    pub owner: Owner,
    pub contract: Option<Entity>,
    pub state: ShipState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShipState {
    /// Docked at a station with nothing to do.
    Idle {
        at: Entity,
    },
    /// At the contract's origin, waiting for cargo + fuel.
    Loading,
    Outbound {
        ticks_left: u32,
        cargo: u32,
    },
    Returning {
        ticks_left: u32,
    },
}

// --------------------------------------------------------------- markets

/// NPC market. Prices derive from the supply/demand curve each tick:
/// `price = base × curve(stock/demand_baseline) × modifiers`.
#[derive(Component, Clone, Debug, Default)]
pub struct Market {
    pub entries: HashMap<ItemId, MarketEntry>,
}

#[derive(Clone, Copy, Debug)]
pub struct MarketEntry {
    pub base_price: u32,
    pub stock: u32,
    pub demand_baseline: u32,
    /// NPC drain of stock, milli-units per tick.
    pub consumption_milli: u32,
    pub consum_accum_milli: u32,
}

/// E:D-style supply pressure: scarce → up to 1.6×, glutted → down to 0.4×.
pub fn price_milli(entry: &MarketEntry) -> u32 {
    let baseline = entry.demand_baseline.max(1) as u64;
    let ratio_milli = (entry.stock as u64 * 1000) / baseline;
    let factor = 1600i64 - (ratio_milli as i64 * 800 / 1000);
    factor.clamp(400, 1600) as u32
}

pub fn unit_price(entry: &MarketEntry) -> i64 {
    (entry.base_price as i64 * price_milli(entry) as i64) / 1000
}

/// Travel time in ticks between two stations, from real light-second
/// distances (5-tick docking overhead, 1 tick per 50 ls).
pub fn travel_ticks(a_dist_ls: u32, b_dist_ls: u32) -> u32 {
    5 + a_dist_ls.abs_diff(b_dist_ls) / 50
}
