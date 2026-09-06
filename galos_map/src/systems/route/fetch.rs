use crate::systems::fetch::{FetchIndex, FetchTasks, RawSystem};
use crate::systems::route::graph::{Jumps, Routing};
use crate::systems::spawn::build_system;
use crate::{Names, Populated};
use bevy::prelude::*;
use bevy::tasks::AsyncComputeTaskPool;

/// Walk the jump graph between two named systems and build the stops
///
/// No clock is written. [`LastFetchedAt`](crate::systems::fetch::LastFetchedAt)
/// is the spyglass region fetch's own, measuring the throttle and the poll from
/// the last region asked for; a route is not a region, and resetting it here
/// put off the next region read by the throttle for no better reason than that
/// the user had plotted something.
/// Ask for a route through `stops`, in order, at a ship's jump `range`
///
/// Flown leg by leg, from each stop to the next, and handed back as one run of
/// hops. A leg that cannot be flown ends the route at the stop it would have
/// set out from, and what comes back is the hops up to there: the collector
/// reads how far along the stops the hops got, and says which leg the route
/// stopped short at.
pub fn fetch_route(
    stops: Vec<String>,
    range: String,
    tasks: &mut ResMut<FetchTasks>,
    time: &Res<Time<Real>>,
    jumps: &Res<Jumps>,
    how: Routing,
    names: &Res<Names>,
    populated: &Res<Populated>,
) {
    // One route at a time. Asking for another replaces the one under way
    // rather than racing it: two of them landing would draw one line over
    // the other, and whichever finished last would answer for the one the
    // user is waiting on. Dropping the task is what stops it.
    tasks.fetched.retain(|index, _| !matches!(index, FetchIndex::Route(..)));

    let index = FetchIndex::Route(stops.clone(), range.clone());
    let now = time.last_update().unwrap_or(time.startup());
    let pool = AsyncComputeTaskPool::get();

    // Resolved against the resident names table before the walk, so a route
    // to a name that is not on record is nothing rather than a walk with
    // nowhere to end. Every stop, or none: a stop that is not on record is a
    // leg with nowhere to go, and the form has already been told which name
    // it was. The graph the walk rides is a cheap handle onto the resident
    // one.
    let stops: Option<Vec<i64>> =
        stops.iter().map(|stop| names.address(stop)).collect();
    let range = range.parse::<f64>().ok();
    let graph = jumps.0.clone();
    // Cheap Arc handles onto the resident tables, so the hops are named and
    // coloured on the task's own thread rather than on the main one.
    let names = Names::clone(names);
    let populated = Populated::clone(populated);

    // No moment. A route is a line through named systems rather than a
    // region, so there is no sky it leaves the map able to answer for.
    let task = pool.spawn(async move {
        let systems = match (stops, range) {
            (Some(stops), Some(range)) => graph
                .route_through(&stops, range, how)
                .into_iter()
                .map(|(address, position)| {
                    let raw = RawSystem {
                        address,
                        position,
                        magnitude: None,
                        temp_bucket: None,
                    };
                    build_system(&raw, &populated, &names)
                })
                .collect(),
            _ => Vec::new(),
        };
        (systems, None)
    });
    tasks.fetched.insert(index, (task, now));
}
