use crate::systems::fetch::{FetchIndex, FetchTasks, RawSystem};
use crate::systems::filter::{Filter, Filters};
use crate::systems::route::SelectedFilter;
use crate::systems::route::frontier::Frontiers;
use crate::systems::route::graph::{Drive, Frontier, Jumps, Routing, Tuning};
use crate::systems::spawn::build_system;
use crate::{Names, Populated};
use bevy::math::DVec3;
use bevy::prelude::*;
use elite_journal::Boxel;

use std::sync::Arc;

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
/// Stop every route being searched, and ask for nothing
///
/// Dropping the tasks is not enough on its own: a body the pool has begun
/// does not stop for being dropped, so each search is told to give up as
/// well ([`Frontiers::abandon_others`] with nothing to keep) and reads that
/// on its next expansion — measured at 90–150 µs for a ten-minute galactic
/// crossing.
///
/// Every leg of every trip at once, because that is what the gesture means:
/// the form is waiting on the plot as a whole, and a trip half stopped is a
/// spinner nothing will ever clear.
pub fn stop_routes(
    tasks: &mut ResMut<FetchTasks>,
    searching: &mut ResMut<Frontiers>,
    filters: &mut ResMut<Filters>,
) {
    tasks.fetched.retain(|index, _| !matches!(index, FetchIndex::Route(..)));
    searching.abandon_others(&[]);
    // The rows stand, each saying it was stopped. A leg that never landed
    // used to leave no row at all, so a trip of three legs stopped after two
    // read as a "3 Leg Route" over two rows with nothing to say where the
    // third went. See [`Filters::stopped_searching`].
    filters.stopped_searching();
}

/// Answers whether anything is now being searched: a route asked for while
/// it is still being searched is **taken back** rather than asked twice, so
/// a second click on the plot button stops the work and the form stops
/// waiting. A trip whose legs are not the ones under way cancels those and
/// asks for its own, which is the same gesture meaning the other thing.
#[allow(clippy::too_many_arguments)]
pub fn fetch_route(
    stops: Vec<String>,
    range: String,
    drive: Drive,
    tasks: &mut ResMut<FetchTasks>,
    searching: &mut ResMut<Frontiers>,
    time: &Res<Time<Real>>,
    jumps: &mut ResMut<Jumps>,
    how: Routing,
    tune: Tuning,
    names: &Res<Names>,
    boosts: &Res<crate::Boosts>,
    populated: &Res<Populated>,
    filters: &mut ResMut<Filters>,
    selected: &mut ResMut<SelectedFilter>,
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
                drive,
                how,
                tune,
            )
        })
        .collect();

    // One trip at a time, rather than one route at a time. The legs of this
    // trip stand; a leg left over from the trip before it goes, since two
    // trips landing at once would draw lines nobody asked for together and
    // the form has room to say how one of them is getting on.
    //
    // Dropping the task is half of stopping it: nothing will read the
    // answer, but a body the pool has already begun runs to its end, and a
    // route walk has nothing to await. So the frontier it was filling in is
    // told to give up as well — which takes its layers off the map and stops
    // the search where it is. See `Frontier::abandon`.
    tasks.fetched.retain(|index, _| {
        !matches!(index, FetchIndex::Route(..)) || legs.contains(index)
    });
    searching.abandon_others(&legs);

    // The legs of the trip before this one are stopped, not forgotten: their
    // rows stand saying so, and whichever of them this ask is also made of
    // are told they are searching again below.
    filters.stopped_searching();
    // A route just asked for is the one being looked at, so whichever was
    // picked out before it stands down. Cleared rather than set to this one,
    // the last route held being what [`super::active`] falls back to. Here
    // rather than where the answer lands, the row being up from now on.
    if !selected.0.is_empty() {
        selected.0.clear();
    }

    let now = time.last_update().unwrap_or(time.startup());

    for (leg, index) in stops.windows(2).zip(legs) {
        ask_leg(
            (&leg[0], &leg[1]),
            index,
            trip.clone(),
            &range,
            drive,
            how,
            tune,
            tasks,
            searching,
            filters,
            jumps,
            names,
            boosts,
            populated,
            now,
        );
    }
}

/// Ask one leg again, from nothing
///
/// What [`crate::search::Search::Replot`] comes to: the row's own ask, asked
/// over. The two ends, the ship, how hard the search is to work and how its
/// plan is to be made all come off the row — which is why the filter travels
/// rather than the stops, the form having possibly moved off every one of
/// them since.
///
/// **One leg, and from nothing.** Whatever search that leg has running is
/// taken back and told to give up before the new one starts, and a trip's
/// other legs are left exactly as they were. Answers whether anything was
/// asked: a route whose two ends are one system is no route to ask for.
#[allow(clippy::too_many_arguments)]
pub fn replot(
    route: &Filter,
    tasks: &mut ResMut<FetchTasks>,
    searching: &mut ResMut<Frontiers>,
    time: &Res<Time<Real>>,
    jumps: &mut ResMut<Jumps>,
    names: &Res<Names>,
    boosts: &Res<crate::Boosts>,
    populated: &Res<Populated>,
    filters: &mut ResMut<Filters>,
) -> bool {
    let Filter::Route { systems, range, trip, drive, how, tune, .. } = route
    else {
        return false;
    };
    let (Some(&start), Some(&end)) = (systems.first(), systems.last()) else {
        return false;
    };
    if start == end {
        return false;
    }

    // **The search to take back is found by what it asks, not by the string
    // it was asked with.** A leg is keyed on the two names as the reader
    // typed them and a row is named off the names table, so the key this
    // builds need not be the key the running search stands under — and a
    // leg left running would land its old answer in the row the new one is
    // for.
    let asks = |held: &FetchIndex| match held {
        FetchIndex::Route(from, to, at, under, fitted, worked, planned) => {
            names.address(from) == Some(start)
                && names.address(to) == Some(end)
                && at == range
                && under == trip
                && fitted == drive
                && worked == how
                && planned == tune
        }
        _ => false,
    };
    let running: Vec<FetchIndex> =
        tasks.fetched.keys().filter(|held| asks(held)).cloned().collect();
    for held in &running {
        tasks.fetched.remove(held);
        searching.abandon(held);
    }

    let (from, to) = (super::said(names, start), super::said(names, end));
    let index = FetchIndex::Route(
        from.clone(),
        to.clone(),
        range.clone(),
        trip.clone(),
        *drive,
        *how,
        *tune,
    );
    let now = time.last_update().unwrap_or(time.startup());
    ask_leg(
        (&from, &to),
        index,
        trip.clone(),
        range,
        *drive,
        *how,
        *tune,
        tasks,
        searching,
        filters,
        jumps,
        names,
        boosts,
        populated,
        now,
    );
    true
}

/// One leg asked for: its row put up, its frontier watched, and its walk
/// handed to the pool
///
/// The whole of what asking for a route comes to, once it is settled which
/// two systems and which ship. Everything either side of it — which legs a
/// trip is made of, which of them are already running and which to take back
/// — belongs to the caller, and there are two of those: a trip asked for
/// through the form ([`fetch_route`]) and one leg asked again on its own
/// ([`replot`]).
#[allow(clippy::too_many_arguments)]
fn ask_leg(
    leg: (&str, &str),
    index: FetchIndex,
    trip: Option<String>,
    range: &str,
    drive: Drive,
    how: Routing,
    tune: Tuning,
    tasks: &mut ResMut<FetchTasks>,
    searching: &mut ResMut<Frontiers>,
    filters: &mut ResMut<Filters>,
    jumps: &mut ResMut<Jumps>,
    names: &Res<Names>,
    boosts: &Res<crate::Boosts>,
    populated: &Res<Populated>,
    now: bevy::platform::time::Instant,
) {
    // Resolved against the resident names table before the walk, so a leg
    // to a name that is not on record is nothing rather than a walk with
    // nowhere to end. The form has already been told which name it was.
    //
    // The place comes with the address: the router reads the galaxy out
    // of the cell payloads, which are keyed by *where*, so the two ends
    // are the one thing it needs told — see [`JumpGraph::route`].
    //
    // **And the place comes from the galaxy rather than from the names
    // table**, which no longer holds one: the address names the boxel the
    // system sits in, so [`Sky::placed`] reads the exact position off the
    // payload after a sphere query the size of that boxel. Measured at
    // 0.8–5 ms by mass class, and a route asks it twice — against the
    // 2.4 GB of positions a row apiece that it replaces.
    let sky = jumps.sky.clone();
    let placed = |name: &str| {
        let address = names.address(name)?;
        // **The galaxy's answer, or nothing.** Where there is a tree to
        // ask, a system it cannot place is a system this cannot walk from:
        // the boxel middle is within half a boxel of the truth — 1,108
        // light years at the largest class — and a walk begun from it
        // would start at whatever system happens to lie near that point
        // and draw a line from there, which is worse than a leg that says
        // it found nothing. The middle stands in only where no galaxy is
        // open at all: a transport that cannot be mapped, or a test app,
        // where there is no exact place to be had from anywhere.
        let at = match sky.as_ref() {
            Some(sky) => sky.placed(address)?,
            None => Boxel::of(address).place().0,
        };
        Some((address, at))
    };
    let ends = placed(leg.0).zip(placed(leg.1));

    // The row goes up now rather than when the answer lands, so the leg
    // has something on screen for the seconds or minutes it takes: the
    // sky dims to its two ends and the search's own layers are drawn
    // against that. It carries the ends and no route between them until
    // [`Filters::landed`] hands it one — see [`filter::Plotted`].
    //
    // Named as the answer will be named, off the names table rather than
    // as the user typed it, so the row the answer lands in is the row
    // that asked and reads the same before and after.
    if let Some(((start, _), (end, _))) = ends {
        let asked = Filter::Route {
            label: format!(
                "{}{}{}",
                super::said(names, start),
                crate::ui::ARROW,
                super::said(names, end),
            ),
            systems: vec![start, end],
            range: range.to_owned(),
            trip,
            drive,
            how,
            tune,
        };
        let at = super::placed_at(&asked, filters);
        filters.searching(at, asked);
    }

    // Already under way: left alone. The same leg asked for twice is one
    // question, and starting it again would throw away the seconds it
    // has already spent. Stopping is [`Search::Stop`]'s, and asking from
    // nothing is [`replot`]'s, which takes the running search back before
    // it reaches this.
    if tasks.fetched.contains_key(&index) {
        return;
    }

    // What the search fills in as it runs, and what the map draws it from.
    // Both ends, since the drawing is measured out from the start and the
    // closed set is scaled by how far there is to go; a leg whose ends do
    // not resolve is not searched and is not watched.
    let watching = ends.map(|((_, from), (_, goal))| {
        Frontier::between(DVec3::from(from), DVec3::from(goal))
    });
    if let Some(watching) = &watching {
        searching.watch(index.clone(), Arc::clone(watching), now);
    }
    // Cheap Arc handles onto the graph and tables, so the hops are
    // walked, named and colored on the task's own thread rather than on
    // the main one. The graph is a handle on the mapped index, so this
    // is the first route's only cost.
    let graph = jumps.built(boosts);
    let reach = range.parse::<f64>().ok();
    let names = Names::clone(names);
    let populated = Populated::clone(populated);

    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        // The search as one zone, named with the leg's reach: this is the
        // longest thing the map does off the main thread — tens of millions
        // of neighbours for a galactic leg — and it never yields, being a walk
        // of a mapped graph with no read in it.
        let _zone = info_span!("route search", reach = ?reach).entered();
        match (graph, ends, reach) {
            (Some(graph), Some((start, end)), Some(range)) => graph
                .route(start, end, range, how, drive, tune, watching.as_ref())
                .map(|hops| {
                    hops.into_iter()
                        .map(|(address, position)| {
                            let raw = RawSystem {
                                address,
                                position,
                                magnitude: None,
                                temp_bucket: None,
                                // A stop comes out of the jump graph,
                                // which is places and nothing else.
                                updated_at: None,
                            };
                            build_system(&raw, &populated, &names)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    });
    tasks.fetched.insert(index, (task, now));
}
