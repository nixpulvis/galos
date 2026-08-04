use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::{Spyglass, route::fetch::fetch_route};
use crate::{Db, search::Searched};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use galos_db::systems::System as DbSystem;
use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

pub fn plugin(app: &mut App) {
    app.insert_resource(Poll(Some(1.)));
    app.insert_resource(Throttle(50));

    app.init_resource::<LastFetchedAt>();
    app.init_resource::<FetchTasks>();

    app.add_systems(Update, fetch.in_set(MapSet::Fetch));
}

/// How long the map waits before asking again for what it already has
///
/// Seconds, which is how the question is put: how often should this be
/// refreshed. A rate would want 0.167 of a box that will be typed 6 into.
///
/// `None` never asks again, which is what the checkbox beside it turns off.
/// Zero asks every frame, which the two ends being different values is what
/// makes sayable at all.
///
/// Only what has already been fetched waits this long. Somewhere new is a
/// question the map has not put yet, and waits on [`Throttle`] instead.
#[derive(Resource)]
pub struct Poll(pub Option<f64>);

/// The amount to throttle requests for new indices (millis).
#[derive(Resource)]
pub struct Throttle(pub u64);

/// A resource which keeps the instant the last fetch was made
#[derive(Resource)]
pub struct LastFetchedAt(pub Instant);

impl Default for LastFetchedAt {
    fn default() -> LastFetchedAt {
        LastFetchedAt(Instant::now())
    }
}

/// Represents a single fetch request
//
// TODO: Put region math inside custom Hash impl?
// TODO: once we have a hash impl let's save f64 instead of String for route
// range.
// TODO(#43): fetched regions should be cubes with `region_size` side length, they
// are currently spheres with `region_size` radius.
#[derive(Hash, Eq, PartialEq, Clone)]
pub enum FetchIndex {
    // System<String>
    Region(IVec3, i32),
    // View<Frustum>,
    Route(String, String, String),
}

impl FetchIndex {
    /// Whether this asks again for what `last` already fetched
    ///
    /// Which is what decides how long the map waits: asking again for what it
    /// has is a refresh and waits out [`Poll`], while asking for somewhere new
    /// is a question it has not put yet and waits only on [`Throttle`]. Flying
    /// somewhere should bring stars promptly however slowly the map is set to
    /// refresh.
    ///
    /// A region refreshes another when it has the same centre and reaches no
    /// further. A larger radius takes in systems that were never asked for, so
    /// it is a new question standing in the same place.
    ///
    /// A question and a predicate rather than an ordering. Two regions about
    /// different centres are each no answer to the other, which is a thing an
    /// [`Ord`] cannot say: it would have to call one of them the greater, and
    /// whichever it called it would be wrong the other way round.
    fn refreshes(&self, last: &FetchIndex) -> bool {
        match (self, last) {
            (
                FetchIndex::Region(centre, radius),
                FetchIndex::Region(before, reached),
            ) => centre == before && radius <= reached,
            // Only the spyglass records what it last fetched, so a route is
            // never on either side of this. Somewhere new either way.
            _ => false,
        }
    }
}

impl fmt::Debug for FetchIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use FetchIndex::*;

        match self {
            Region(center, radius) => write!(
                f,
                "<({},{},{}),{}>",
                center.x, center.y, center.z, radius
            ),
            Route(start, end, range) => {
                write!(f, "<{}-{}>{}>", start, end, range)
            }
        }
    }
}

/// Tasks for systems in the DB which will be spawned
#[derive(Resource, Default)]
pub struct FetchTasks {
    pub fetched: HashMap<FetchIndex, (Task<Vec<DbSystem>>, Instant)>,
    pub last_fetched: Option<FetchIndex>,
}

/// Spawns tasks to load star systems from the DB
pub fn fetch(
    camera_query: Query<&OrbitCamera>,
    mut search_events: MessageReader<Searched>,
    mut tasks: ResMut<FetchTasks>,
    mut spyglass: ResMut<Spyglass>,
    time: Res<Time<Real>>,
    mut last_fetched_at: ResMut<LastFetchedAt>,
    throttle: Res<Throttle>,
    poll: Res<Poll>,
    db: Res<Db>,
) {
    if spyglass.fetch {
        fetch_spyglass(
            &camera_query,
            &mut tasks,
            &mut spyglass,
            &time,
            &mut last_fetched_at,
            &throttle,
            &poll,
            &db,
        );
    }

    for event in search_events.read() {
        match event {
            // TODO: Ensure at least the searched star is fetched. I don't do it
            // again here because it was already fetched (syncronously) in
            // `search`. That needs to be refactored anyway. So for now, if
            // you search for a system with AlwaysFetch(false) it may take you
            // to a part of empty space. Setting AlwaysFetch(true) will
            // populate it.
            Searched::System { .. } => {}
            Searched::Route { start, end, range } => {
                fetch_route(
                    start.into(),
                    end.into(),
                    range.into(),
                    &mut tasks,
                    &time,
                    &mut last_fetched_at,
                    &db,
                );
            }
        };
    }
}

fn fetch_spyglass(
    camera_query: &Query<&OrbitCamera>,
    tasks: &mut ResMut<FetchTasks>,
    spyglass: &ResMut<Spyglass>,
    time: &Res<Time<Real>>,
    last_fetched_at: &mut ResMut<LastFetchedAt>,
    throttle: &Res<Throttle>,
    poll: &Res<Poll>,
    db: &Res<Db>,
) {
    let Ok(camera) = camera_query.single() else { return };
    let center = camera.focus.as_ivec3();
    let index = FetchIndex::Region(center, spyglass.radius as i32);
    let now = time.last_update().unwrap_or(time.startup());
    if spyglass_condition(&index, tasks, now, last_fetched_at, throttle, poll) {
        debug!(
            "fetching {:?} @ {:?}",
            index,
            now.duration_since(time.startup())
        );

        let task_pool = AsyncComputeTaskPool::get();
        let db = db.0.clone();
        let radius = spyglass.radius;
        let task = task_pool.spawn(async move {
            let cent = [center.x as f64, center.y as f64, center.z as f64];
            DbSystem::fetch_in_range_of_point(&db, radius.floor() as f64, cent)
                .await
                .unwrap_or_default()
        });
        tasks.fetched.insert(index.clone(), (task, now));
        tasks.last_fetched = Some(index);
        **last_fetched_at = LastFetchedAt(now);
    }
}

pub fn spyglass_condition(
    index: &FetchIndex,
    tasks: &ResMut<FetchTasks>,
    now: Instant,
    last_fetched_at: &ResMut<LastFetchedAt>,
    throttle: &Res<Throttle>,
    poll: &Res<Poll>,
) -> bool {
    tasks.last_fetched.as_ref().map_or(true, |last_fetched| {
        if index.refreshes(last_fetched) {
            poll.0.map_or(false, |wait| {
                last_fetched_at.0 + Duration::from_secs_f64(wait.max(0.)) < now
            })
        } else {
            last_fetched_at.0 + Duration::from_millis(throttle.0) < now
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A region of `radius` about `centre` on the x axis
    fn region(centre: i32, radius: i32) -> FetchIndex {
        FetchIndex::Region(IVec3::new(centre, 0, 0), radius)
    }

    /// The same region asked for again is a refresh
    #[test]
    fn the_same_region_refreshes() {
        assert!(region(0, 10).refreshes(&region(0, 10)));
    }

    /// Somewhere else is a new question, whichever way the camera went
    ///
    /// Both directions, since the two regions are symmetric: neither is an
    /// answer to the other, and a test of one direction alone would pass for
    /// something that called one of them the greater.
    #[test]
    fn another_region_is_not_a_refresh() {
        assert!(!region(1, 10).refreshes(&region(0, 10)));
        assert!(!region(0, 10).refreshes(&region(1, 10)));
    }

    /// Reaching no further is still a refresh
    ///
    /// Everything a narrower spyglass asks for has already been fetched, so
    /// there is nothing new to hurry for.
    #[test]
    fn a_smaller_radius_refreshes() {
        assert!(region(0, 5).refreshes(&region(0, 10)));
    }

    /// Reaching further is a new question
    ///
    /// It takes in systems that were never asked for, so it waits on the
    /// throttle rather than the poll and the sky fills as the user widens it.
    #[test]
    fn a_larger_radius_is_not_a_refresh() {
        assert!(!region(0, 20).refreshes(&region(0, 10)));
    }

    /// A route is never a refresh of anything, nor refreshed by one
    #[test]
    fn a_route_is_always_a_new_question() {
        let route = FetchIndex::Route("A".into(), "B".into(), "10".into());
        assert!(!route.refreshes(&region(0, 10)));
        assert!(!region(0, 10).refreshes(&route));
        assert!(!route.refreshes(&route));
    }
}
