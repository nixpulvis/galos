use crate::systems::fetch::{FetchIndex, FetchTasks, RawSystem};
use crate::systems::route::graph::{Jumps, Routing};
use crate::systems::spawn::build_system;
use crate::{Names, Populated};
use bevy::prelude::*;
use bevy::tasks::AsyncComputeTaskPool;

/// Ask for a trip through `stops`, in order, at a ship's jump `range`
///
/// One route per leg, from each stop to the next, each asked for and answered
/// on its own. A trip through five systems is four routes: four lines, four
/// panels, four sets of stops, and four answers about whether the ship can fly
/// it. So a leg that cannot be flown says so and takes nothing with it — the
/// legs that can are drawn — and each leg carries the one figure that matters
/// about it, the longest jump it asks the ship to make.
///
/// The legs run at once. They share the resident graph and the resident tables
/// behind `Arc`s, and each walk is its own task on the compute pool, so a trip
/// costs about what its longest leg costs rather than the sum of them.
///
/// No clock is written. [`LastFetchedAt`](crate::systems::fetch::LastFetchedAt)
/// is the spyglass region fetch's own, measuring the throttle and the poll from
/// the last region asked for; a route is not a region, and resetting it here
/// put off the next region read by the throttle for no better reason than that
/// the user had plotted something.
#[allow(clippy::too_many_arguments)]
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
    // Every leg this trip is made of, in the order flown. The key is the leg
    // rather than the trip, so a leg asked for twice — the same pair turning
    // up in two trips, or a trip asked for again while it is still landing —
    // is the one question and the one answer.
    // What the legs are one trip under, where there is more than one of
    // them. A route asked for on its own is a trip of one leg and has nothing
    // to be grouped with, so it carries no trip and its row stands alone.
    //
    // Named for its stops, as a leg is named for its two ends, so that two
    // trips between the same ends read apart in the bar. Part of the key as
    // well: the same leg flown in two trips is two answers, each belonging to
    // its own trip, and one shared between them would land in whichever
    // asked first.
    let trip = (stops.len() > 2).then(|| stops.join(crate::ui::ARROW));
    let legs: Vec<FetchIndex> = stops
        .windows(2)
        .map(|leg| {
            FetchIndex::Route(
                leg[0].clone(),
                leg[1].clone(),
                range.clone(),
                trip.clone(),
            )
        })
        .collect();

    // One trip at a time, rather than one route at a time. The legs of this
    // trip stand; a leg left over from the trip before it goes, since two
    // trips landing at once would draw lines nobody asked for together and
    // the form has room to say how one of them is getting on. Dropping the
    // task is what stops it.
    tasks.fetched.retain(|index, _| {
        !matches!(index, FetchIndex::Route(..)) || legs.contains(index)
    });

    let now = time.last_update().unwrap_or(time.startup());
    let pool = AsyncComputeTaskPool::get();
    let range = range.parse::<f64>().ok();

    for (leg, index) in stops.windows(2).zip(legs) {
        // Already under way or already answered. The retain above kept it,
        // and asking again would drop the answer on the floor and walk the
        // same leg a second time.
        if tasks.fetched.contains_key(&index) {
            continue;
        }

        // Resolved against the resident names table before the walk, so a leg
        // to a name that is not on record is nothing rather than a walk with
        // nowhere to end. The form has already been told which name it was.
        let ends = names.address(&leg[0]).zip(names.address(&leg[1]));
        // Cheap Arc handles onto the resident graph and tables, so the hops
        // are walked, named and coloured on the task's own thread rather than
        // on the main one.
        let graph = jumps.0.clone();
        let names = Names::clone(names);
        let populated = Populated::clone(populated);

        // No moment. A route is a line between two named systems rather than
        // a region, so there is no sky it leaves the map able to answer for.
        let task = pool.spawn(async move {
            let systems = match (ends, range) {
                (Some((start, end)), Some(range)) => graph
                    .route(start, end, range, how)
                    .map(|hops| {
                        hops.into_iter()
                            .map(|(address, position)| {
                                let raw = RawSystem {
                                    address,
                                    position,
                                    magnitude: None,
                                    temp_bucket: None,
                                };
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
    }
}
