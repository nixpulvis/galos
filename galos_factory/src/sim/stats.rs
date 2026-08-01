//! End-of-tick bookkeeping: brownout notices (sampled to avoid spam).
//! Rate dashboards derive from the cumulative [`Stats`] counters.

use super::*;
use bevy::prelude::*;

pub fn stats(
    clock: Res<SimClock>,
    mut notices: ResMut<Notices>,
    stations: Query<(&Station, &PowerGrid)>,
) {
    if clock.tick % 100 != 0 {
        return;
    }
    for (station, grid) in stations.iter() {
        if grid.demand_mw > 0 && grid.satisfaction_milli < 1000 {
            notices.0.push((clock.tick, Notice::Brownout { station: station.name.clone() }));
        }
    }
}
