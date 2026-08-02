//! The production sim at the heart of the galos factory game.
//!
//! This crate is deliberately independent of the 3D map: the sim is a set of
//! Bevy ECS systems over plain entities (factions, commanders, systems,
//! bodies, stations, factories, ships, contracts), stepped deterministically
//! one tick at a time. The `ui` feature adds egui panels shared by the
//! standalone runner and the full game.

pub mod data;
pub mod seed;
pub mod sim;
pub mod snapshot;
#[cfg(feature = "ui")]
pub mod ui;

use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;
use sim::SimSet;

/// The schedule containing one whole sim tick, run 0..N times per
/// `FixedUpdate` depending on [`sim::SimSpeed`]. Driving this schedule
/// directly (as the headless runner and tests do) bypasses wall-clock time
/// entirely, keeping ticks exactly reproducible.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SimTick;

/// Core sim plugin: static data, global resources, and the tick schedule.
/// Contains no rendering and no I/O.
pub fn sim_plugin(app: &mut App) {
    let statics = data::StaticData::load().expect("invalid embedded game data");
    app.insert_resource(statics);
    app.init_resource::<sim::SimClock>();
    app.init_resource::<sim::SimSpeed>();
    app.init_resource::<sim::Notices>();
    app.insert_resource(sim::SimRng::from_seed(0));
    app.add_event::<sim::PlayerCommand>();

    app.init_schedule(SimTick);
    app.configure_sets(
        SimTick,
        (
            SimSet::Commands,
            SimSet::Control,
            SimSet::Power,
            SimSet::Upkeep,
            SimSet::Production,
            SimSet::Logistics,
            SimSet::Market,
            SimSet::Stats,
        )
            .chain(),
    );

    app.add_systems(
        SimTick,
        (
            sim::commands::apply_commands.in_set(SimSet::Commands),
            sim::control::resolve_control.in_set(SimSet::Control),
            sim::power::power_balance.in_set(SimSet::Power),
            sim::upkeep::upkeep
                .in_set(SimSet::Upkeep)
                .run_if(sim::on_upkeep_tick),
            (sim::extract::extract, sim::craft::craft)
                .chain()
                .in_set(SimSet::Production),
            sim::shipping::shipping.in_set(SimSet::Logistics),
            (sim::market::market_tick, sim::market::accrue_interest)
                .in_set(SimSet::Market),
            (
                sim::stats::mark_offline,
                sim::stats::mark_idle,
                sim::stats::sample_notices.run_if(sim::on_report_tick),
                sim::advance_clock,
            )
                .chain()
                .in_set(SimSet::Stats),
        ),
    );

    app.add_systems(FixedUpdate, sim::drive_ticks);
    app.insert_resource(Time::<Fixed>::from_hz(10.0));
}
