use crate::systems::fetch::{FetchIndex, FetchTasks, LastFetchedAt, RawSystem};
use crate::systems::route::graph::Jumps;
use crate::systems::spawn::build_system;
use crate::{Names, Populated};
use bevy::prelude::*;
use bevy::tasks::AsyncComputeTaskPool;

#[allow(clippy::too_many_arguments)]
pub fn fetch_route(
    start: String,
    end: String,
    range: String,
    tasks: &mut ResMut<FetchTasks>,
    time: &Res<Time<Real>>,
    last_fetched_at: &mut ResMut<LastFetchedAt>,
    jumps: &Res<Jumps>,
    names: &Res<Names>,
    populated: &Res<Populated>,
) {
    // One route at a time. Asking for another replaces the one under way
    // rather than racing it: two of them landing would draw one line over
    // the other, and whichever finished last would answer for the one the
    // user is waiting on. Dropping the task is what stops it.
    tasks.fetched.retain(|index, _| !matches!(index, FetchIndex::Route(..)));

    let index = FetchIndex::Route(start.clone(), end.clone(), range.clone());
    let now = time.last_update().unwrap_or(time.startup());
    let pool = AsyncComputeTaskPool::get();

    // Resolved against the resident names table before the walk, so a route to a
    // name that is not on record is nothing rather than a walk with nowhere to
    // end. The graph the walk rides is a cheap handle onto the resident one.
    let ends = names.address(&start).zip(names.address(&end));
    let range = range.parse::<f64>().ok();
    let graph = jumps.0.clone();
    // Cheap Arc handles onto the resident tables, so the hops are named and
    // coloured on the task's own thread rather than on the main one.
    let names = Names::clone(names);
    let populated = Populated::clone(populated);

    // No moment. A route is a line between two named systems rather than a
    // region, so there is no sky it leaves the map able to answer for.
    let task = pool.spawn(async move {
        let systems = match (ends, range) {
            (Some((start, end)), Some(range)) => graph
                .route(start, end, range)
                .map(|hops| {
                    hops.into_iter()
                        .map(|(address, position)| {
                            let raw = RawSystem { address, position };
                            build_system(&raw, &populated, &names)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        (systems, None)
    });
    tasks.fetched.insert(index, (task, now));
    **last_fetched_at = LastFetchedAt(now);
}
