//! End-of-tick derived state: factory status for buildings that are down
//! for maintenance, and sampled notices that would otherwise spam every
//! tick. Rate dashboards derive from each actor's [`Ledger`].

use super::*;
use bevy::prelude::*;

/// Factories the producing systems filter out by archetype — down for
/// maintenance, or with no recipe assigned — would otherwise keep whatever
/// status they last had.
pub fn mark_offline(mut factories: Query<&mut Status, With<MaintenanceDue>>) {
    for mut status in factories.iter_mut() {
        status.0 = FactoryStatus::Offline;
    }
}

pub fn mark_idle(
    mut factories: Query<
        &mut Status,
        (Without<ActiveRecipe>, Without<MaintenanceDue>),
    >,
) {
    for mut status in factories.iter_mut() {
        status.0 = FactoryStatus::Idle;
    }
}

pub fn sample_notices(
    clock: Res<SimClock>,
    mut notices: ResMut<Notices>,
    stations: Query<(&Station, &PowerGrid)>,
) {
    for (station, grid) in stations.iter() {
        if grid.demand_mw > 0 && grid.satisfaction_milli < 1000 {
            notices.push(
                clock.tick,
                Notice::Brownout { station: station.name.clone() },
            );
        }
    }
}
