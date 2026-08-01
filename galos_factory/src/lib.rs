//! The production sim at the heart of the galos factory game.
//!
//! This crate is deliberately independent of the 3D map: the sim is a set of
//! Bevy ECS systems over plain entities (bodies, stations, factories, ships,
//! contracts), stepped deterministically one tick at a time. The `ui` feature
//! adds egui panels shared by the standalone runner and the full game.

pub mod data;
pub mod seed;
pub mod sim;
pub mod snapshot;
#[cfg(feature = "ui")]
pub mod ui;

use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

/// The schedule containing one whole sim tick, run 0..N times per
/// `FixedUpdate` depending on [`sim::SimSpeed`]. Driving this schedule
/// directly (as the headless runner and tests do) bypasses wall-clock time
/// entirely, keeping ticks exactly reproducible.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SimTick;

/// Core sim plugin: static data, resources, and the tick schedule.
/// Contains no rendering and no I/O.
pub fn sim_plugin(app: &mut App) {
    let statics = data::StaticData::load().expect("invalid embedded game data");
    app.insert_resource(statics);
    app.init_resource::<sim::SimClock>();
    app.init_resource::<sim::SimSpeed>();
    app.init_resource::<sim::Credits>();
    app.init_resource::<sim::Debt>();
    app.init_resource::<sim::SystemModifiers>();
    app.init_resource::<sim::CommandQueue>();
    app.init_resource::<sim::Notices>();
    app.init_resource::<sim::Stats>();
    app.insert_resource(sim::SimRng::from_seed(0));

    app.init_schedule(SimTick);
    app.add_systems(
        SimTick,
        (
            sim::commands::apply_commands,
            sim::power::power_balance,
            sim::upkeep::upkeep,
            sim::extract::extract,
            sim::craft::craft,
            sim::shipping::shipping,
            sim::market::market_tick,
            sim::stats::stats,
            sim::advance_clock,
        )
            .chain(),
    );

    app.add_systems(FixedUpdate, sim::drive_ticks);
    app.insert_resource(Time::<Fixed>::from_hz(10.0));
}
