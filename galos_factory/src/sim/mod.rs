//! The deterministic tick simulation.
//!
//! Hard rules, enforced structurally:
//! - No floats in sim state: quantities are integers, fractional rates use
//!   milli-unit accumulators.
//! - All randomness flows through the single seeded [`SimRng`].
//! - UI and hosts never mutate sim state directly; they push
//!   [`PlayerCommand`]s into the [`CommandQueue`], drained at tick start.
//! - The fixed clock never changes; [`SimSpeed`] runs the whole tick
//!   schedule 0..N times per `FixedUpdate` step.

pub mod commands;
pub mod components;
pub mod craft;
pub mod extract;
pub mod market;
pub mod power;
pub mod shipping;
pub mod stats;
pub mod upkeep;

pub use commands::PlayerCommand;
pub use components::*;

use crate::data::ItemId;
use bevy::prelude::*;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::collections::HashMap;

#[derive(Resource, Default, Debug)]
pub struct SimClock {
    pub tick: u64,
}

/// Whole sim ticks per `FixedUpdate` step (the fixed clock stays at 10 Hz).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SimSpeed {
    Paused,
    #[default]
    X1,
    X10,
    X60,
}

impl SimSpeed {
    pub fn ticks_per_step(self) -> u32 {
        match self {
            SimSpeed::Paused => 0,
            SimSpeed::X1 => 1,
            SimSpeed::X10 => 10,
            SimSpeed::X60 => 60,
        }
    }
}

#[derive(Resource, Debug)]
pub struct SimRng(pub ChaCha8Rng);

impl SimRng {
    pub fn from_seed(seed: u64) -> Self {
        SimRng(ChaCha8Rng::seed_from_u64(seed))
    }
}

#[derive(Resource, Default, Debug)]
pub struct Credits(pub i64);

#[derive(Resource, Debug)]
pub struct Debt {
    pub interest_milli: u32,
    pub ceiling: i64,
}

impl Default for Debt {
    fn default() -> Self {
        // ~0.01% interest per tick on negative balances; run ends past the
        // ceiling (enforced in a later milestone).
        Debt { interest_milli: 0, ceiling: -1_000_000 }
    }
}

/// System-wide modifiers derived from BGS data at seed/refresh time.
/// All multipliers are milli-scaled (1000 = neutral).
#[derive(Resource, Debug, Clone)]
pub struct SystemModifiers {
    pub productivity_milli: u32,
    pub tax_milli: u32,
    pub piracy_milli: u32,
    pub solar_milli: u32,
    pub scoopable_star: bool,
}

impl Default for SystemModifiers {
    fn default() -> Self {
        SystemModifiers {
            productivity_milli: 1000,
            tax_milli: 50,
            piracy_milli: 0,
            solar_milli: 1000,
            scoopable_star: true,
        }
    }
}

/// Player commands queued by UI/hosts, drained at tick boundary.
#[derive(Resource, Default, Debug)]
pub struct CommandQueue(pub Vec<PlayerCommand>);

/// Tick-stamped notices for the UI ticker and headless log.
#[derive(Resource, Default, Debug)]
pub struct Notices(pub Vec<(u64, Notice)>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Built { station: String, kind: crate::data::BuildingKind },
    BuildFailed { station: String, reason: String },
    Brownout { station: String },
    LifeSupportShort { station: String },
    MaintenanceShort { station: String, kind: crate::data::BuildingKind },
    NoFuel { station: String },
    PiracyLoss { item: ItemId, qty: u32 },
    Sold { station: String, item: ItemId, qty: u32, credits: i64 },
    Bought { station: String, item: ItemId, qty: u32, credits: i64 },
}

/// Cumulative production accounting (rates derived by callers).
#[derive(Resource, Default, Debug)]
pub struct Stats {
    pub produced: HashMap<ItemId, u64>,
    pub consumed: HashMap<ItemId, u64>,
    pub sold: HashMap<ItemId, u64>,
    pub revenue: i64,
    pub expenses: i64,
}

/// Runs the tick schedule [`SimSpeed`] times per fixed step.
pub fn drive_ticks(world: &mut World) {
    let n = world.resource::<SimSpeed>().ticks_per_step();
    for _ in 0..n {
        world.run_schedule(crate::SimTick);
    }
}

pub fn advance_clock(mut clock: ResMut<SimClock>) {
    clock.tick += 1;
}
