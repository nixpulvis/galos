//! The deterministic tick simulation.
//!
//! Hard rules, enforced structurally:
//! - No floats in sim state: quantities are integers, fractional rates use
//!   milli-unit accumulators.
//! - All randomness flows through the single seeded [`SimRng`].
//! - Nothing mutates sim state directly; UI and hosts send
//!   [`PlayerCommand`] events, drained at tick start and validated against
//!   the issuing actor's ownership.
//! - The fixed clock never changes; [`SimSpeed`] runs the whole tick
//!   schedule 0..N times per `FixedUpdate` step.
//!
//! Only genuinely global facts live in resources. Money, ledgers, and
//! market/political context are per-entity — see [`components`].

pub mod commands;
pub mod components;
pub mod control;
pub mod craft;
pub mod extract;
pub mod market;
pub mod power;
pub mod shipping;
pub mod stats;
pub mod upkeep;

pub use commands::{Action, PlayerCommand};
pub use components::*;

use crate::data::{ItemId, UPKEEP_PERIOD};
use bevy::prelude::*;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::collections::VecDeque;

/// Ordered stages of one tick. Named so hosts (`galos_game`, tests) can
/// schedule their own systems against sim phases.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimSet {
    /// Drain player commands.
    Commands,
    /// Re-derive who controls each system.
    Control,
    /// Per-station supply vs demand.
    Power,
    /// Maintenance and life support.
    Upkeep,
    /// Extraction and crafting.
    Production,
    /// Ships and contracts.
    Logistics,
    /// Market curves, interest.
    Market,
    /// Derived status, notices, bookkeeping.
    Stats,
}

#[derive(Resource, Default, Debug)]
pub struct SimClock {
    pub tick: u64,
}

/// Whole sim ticks per `FixedUpdate` step (the fixed clock stays at 10 Hz,
/// so speed never changes outcomes).
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

/// Rolling event feed for the UI ticker and headless log. Bounded — a long
/// game must not grow this forever.
#[derive(Resource, Debug)]
pub struct Notices {
    pub entries: VecDeque<(u64, Notice)>,
    pub cap: usize,
}

impl Default for Notices {
    fn default() -> Self {
        Notices { entries: VecDeque::new(), cap: 512 }
    }
}

impl Notices {
    pub fn push(&mut self, tick: u64, notice: Notice) {
        if self.entries.len() == self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back((tick, notice));
    }

    pub fn recent(&self, n: usize) -> impl Iterator<Item = &(u64, Notice)> {
        self.entries.iter().rev().take(n).rev()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Built { station: String, kind: crate::data::BuildingKind },
    CommandRejected { reason: String },
    Brownout { station: String },
    LifeSupportShort { station: String },
    MaintenanceShort { station: String, kind: crate::data::BuildingKind },
    NoFuel { station: String },
    PiracyLoss { item: ItemId, qty: u32 },
    Sold { station: String, item: ItemId, qty: u32, credits: i64 },
    Bought { station: String, item: ItemId, qty: u32, credits: i64 },
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

/// Standing costs are charged on a slower cadence than production.
pub fn on_upkeep_tick(clock: Res<SimClock>) -> bool {
    clock.tick > 0 && clock.tick % UPKEEP_PERIOD == 0
}

/// Sampling cadence for notices that would otherwise fire every tick.
pub fn on_report_tick(clock: Res<SimClock>) -> bool {
    clock.tick > 0 && clock.tick % 100 == 0
}
