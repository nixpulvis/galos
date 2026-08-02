//! The world model as ECS components.
//!
//! Five layers, mirroring DESIGN.md and the galos_db schema:
//!
//! - **Star system** — the weather. Carries environment (security-driven
//!   piracy, star class) and the derived [`Control`] of whoever holds the
//!   most influence right now.
//! - **Faction** — a real, synced BGS faction, modelled as a corporation:
//!   it holds an account, owns NPC stations and ships, and issues contracts.
//!   Its per-system standing lives in a [`Presence`] entity, one per
//!   (faction, system) pair — the same shape as `system_factions`.
//! - **Commander** — a player. Holds a personal account and may be a
//!   [`MemberOf`] a faction. (How members interact with faction money is
//!   deliberately unmodelled for now.)
//! - **Body** — the geology a station inherits.
//! - **Station** — the container: power grid, shared storage, slots — with
//!   **factories as its ECS children**.
//!
//! Assets (stations, ships, contracts) point at their owning actor with
//! [`OwnedBy`], which may be a commander or a faction.

use crate::data::{BuildingKind, ItemId, RecipeId};
use bevy::prelude::*;
use elite_journal::faction::{Happiness, State};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ----------------------------------------------------------------- actors

/// An economic actor's balance. Present on commanders and factions alike.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Credits(pub i64);

/// Interest accrues on a negative balance; sustained insolvency past the
/// ceiling is bankruptcy.
#[derive(Component, Clone, Copy, Debug)]
pub struct Debt {
    pub interest_milli: u32,
    pub ceiling: i64,
}

impl Default for Debt {
    fn default() -> Self {
        Debt { interest_milli: 0, ceiling: -1_000_000 }
    }
}

/// A player.
#[derive(Component, Clone, Debug)]
pub struct Commander {
    pub name: String,
}

/// A real BGS faction, acting as a corporation for the NPC economy.
#[derive(Component, Clone, Debug)]
pub struct Faction {
    pub name: String,
}

/// Which faction an actor flies for. Carries no economic meaning yet.
#[derive(Component, Clone, Copy, Debug)]
pub struct MemberOf(pub Entity);

/// Standing with factions the actor does *not* belong to.
#[derive(Component, Clone, Debug, Default)]
pub struct Reputation(pub HashMap<Entity, i32>);

/// The actor that owns this station, ship, or contract.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnedBy(pub Entity);

/// Per-actor production and trade accounting.
#[derive(Component, Clone, Debug, Default)]
pub struct Ledger {
    pub produced: HashMap<ItemId, u64>,
    pub consumed: HashMap<ItemId, u64>,
    pub sold: HashMap<ItemId, u64>,
    pub revenue: i64,
    pub expenses: i64,
}

#[derive(Bundle)]
pub struct CommanderBundle {
    pub commander: Commander,
    pub credits: Credits,
    pub debt: Debt,
    pub ledger: Ledger,
    pub reputation: Reputation,
}

impl CommanderBundle {
    pub fn new(name: impl Into<String>, credits: i64) -> Self {
        CommanderBundle {
            commander: Commander { name: name.into() },
            credits: Credits(credits),
            debt: Debt::default(),
            ledger: Ledger::default(),
            reputation: Reputation::default(),
        }
    }
}

#[derive(Bundle)]
pub struct FactionBundle {
    pub faction: Faction,
    pub credits: Credits,
    pub ledger: Ledger,
}

impl FactionBundle {
    pub fn new(name: impl Into<String>, treasury: i64) -> Self {
        FactionBundle {
            faction: Faction { name: name.into() },
            credits: Credits(treasury),
            ledger: Ledger::default(),
        }
    }
}

// ----------------------------------------------------------- star systems

#[derive(Component, Clone, Debug)]
pub struct StarSystem {
    pub address: i64,
    pub name: String,
}

/// Environment that belongs to the system itself rather than to whoever
/// happens to control it. Milli-scaled (1000 = neutral).
#[derive(Component, Clone, Copy, Debug)]
pub struct SystemEnv {
    /// Chance per ship arrival of losing the ship, from security.
    pub piracy_milli: u32,
    pub solar_milli: u32,
    pub scoopable_star: bool,
}

impl Default for SystemEnv {
    fn default() -> Self {
        SystemEnv { piracy_milli: 0, solar_milli: 1000, scoopable_star: true }
    }
}

/// A faction's standing in one system — one entity per (faction, system),
/// mirroring the `system_factions` table.
#[derive(Component, Clone, Debug)]
pub struct Presence {
    pub faction: Entity,
    pub system: Entity,
    /// 0..=100 as delivered by the journal.
    pub influence: f32,
    pub state: State,
    pub happiness: Happiness,
}

/// Derived each tick from the [`Presence`] entities: who runs this system
/// and what that costs everyone operating here.
#[derive(Component, Clone, Copy, Debug)]
pub struct Control {
    pub faction: Option<Entity>,
    /// Cut the controlling faction takes on market sales.
    pub tax_milli: u32,
    /// Workforce productivity from the controlling faction's happiness.
    pub productivity_milli: u32,
    pub boom: bool,
}

impl Default for Control {
    fn default() -> Self {
        Control {
            faction: None,
            tax_milli: 100,
            productivity_milli: 1000,
            boom: false,
        }
    }
}

#[derive(Bundle)]
pub struct StarSystemBundle {
    pub system: StarSystem,
    pub env: SystemEnv,
    pub control: Control,
}

// ----------------------------------------------------------------- bodies

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
    /// Extra power demand from hostile temp/atmosphere (1000 = none).
    pub overhead_milli: u32,
}

#[derive(Bundle)]
pub struct BodyBundle {
    pub body: Body,
    pub in_system: InSystem,
    pub deposits: Deposits,
    pub env: BodyEnv,
}

// --------------------------------------------------------------- stations

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
    /// Light-seconds from the arrival star; drives travel times.
    pub dist_ls: u32,
}

/// The star system a station or body sits in.
///
/// The ECS hierarchy (`Parent`/`Children`) is reserved for the hot
/// station → factory relation that the production systems walk every tick;
/// the colder system link is an explicit reference so there is never any
/// ambiguity about what a `Parent` means at a given level.
#[derive(Component, Clone, Copy, Debug)]
pub struct InSystem(pub Entity);

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

/// Marks stations offering the E:D `Shipyard` service — where ships are
/// bought.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Shipyard;

/// Set while station life support went unsupplied.
#[derive(Component, Clone, Copy, Debug)]
pub struct LifeSupport {
    pub ok: bool,
}

impl Default for LifeSupport {
    fn default() -> Self {
        LifeSupport { ok: true }
    }
}

#[derive(Bundle)]
pub struct StationBundle {
    pub station: Station,
    pub in_system: InSystem,
    pub owner: OwnedBy,
    pub slots: Slots,
    pub storage: Storage,
    pub power: PowerGrid,
    pub life_support: LifeSupport,
}

// -------------------------------------------------------------- factories

/// A production facility. Its station is its ECS [`Parent`].
#[derive(Component, Clone, Copy, Debug)]
pub struct Factory {
    pub kind: BuildingKind,
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

/// Present while a building's maintenance went unpaid. Offline buildings
/// neither work nor draw power; the marker is removed on the next
/// successful charge, so queries filter by archetype.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct MaintenanceDue;

#[derive(Bundle)]
pub struct FactoryBundle {
    pub factory: Factory,
    pub recipe: ActiveRecipe,
    pub cap: OutputCap,
    pub progress: CraftProgress,
    pub status: Status,
}

impl FactoryBundle {
    pub fn new(kind: BuildingKind) -> Self {
        FactoryBundle {
            factory: Factory { kind },
            recipe: ActiveRecipe(None),
            cap: OutputCap(None),
            progress: CraftProgress::default(),
            status: Status::default(),
        }
    }
}

// -------------------------------------------------------------- logistics

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

/// A standing agreement to move `item` between two stations. A player
/// "supply route" is a self-issued contract with assigned ships; the issuer
/// is the contract's [`OwnedBy`].
#[derive(Component, Clone, Debug)]
pub struct Contract {
    pub from: Entity,
    pub to: Entity,
    pub item: ItemId,
    /// Credits per delivered unit, paid by the issuer to the carrier.
    /// Zero for self-contracts.
    pub pay_per_unit: u32,
    /// Stop hauling while the destination holds at least this much
    /// (request threshold). `None` = haul everything available.
    pub target: Option<u32>,
    /// Never draw the origin below this floor (surplus-export threshold).
    pub reserve: u32,
}

#[derive(Component, Clone, Debug)]
pub struct Ship {
    pub class: ShipClass,
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

// ---------------------------------------------------------------- markets

/// An NPC market, on a faction-owned station. Prices derive from the
/// supply/demand curve each tick:
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
