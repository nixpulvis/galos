use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::filter::{Admitted, DimTo, Filters};
use crate::systems::selection::Selection;
use crate::systems::{Spyglass, System, route::fetch::fetch_route};
use crate::{Db, search::Search};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use chrono::Utc;
use galos_db::systems::System as DbSystem;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

pub fn plugin(app: &mut App) {
    app.insert_resource(Poll(Some(10.)));
    app.insert_resource(Throttle(100));

    app.init_resource::<LastFetchedAt>();
    app.init_resource::<FetchTasks>();

    app.add_systems(Update, (fetch, fetch_selected).in_set(MapSet::Fetch));
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

impl Poll {
    /// Whether enough has passed since `last` to ask again at `now`
    ///
    /// The one reading of the setting, put to it by everything refreshing on
    /// it. The spyglass asks about a region of the galaxy and
    /// [`crate::systems::bodies::fetch`] asks about the inside of one system,
    /// and the two share nothing else; what they do share is that the user set
    /// one number for how often the map goes back to the database, and it
    /// means the same thing to both.
    ///
    /// Never when the poll is off, so a caller only has to ask this to honour
    /// the checkbox.
    pub fn elapsed(&self, last: Instant, now: Instant) -> bool {
        self.0.is_some_and(|wait| {
            last + Duration::from_secs_f64(wait.max(0.)) < now
        })
    }
}

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

// TODO: Put region math inside custom Hash impl?
// TODO: once we have a hash impl let's save f64 instead of String for route
// range.
// TODO(#43): fetched regions should be cubes with `region_size` side length, they
// are currently spheres with `region_size` radius.
#[derive(Hash, Eq, PartialEq, Clone)]
pub enum FetchIndex {
    // System<String>
    /// Everywhere within a radius of a point, and what of it is wanted
    ///
    /// Nothing wanted in particular is the whole of what is there. Where the
    /// filters have said what they admit, the region is asked for that alone,
    /// which makes it a different question about the same place: adding or
    /// dropping a filter is somewhere new rather than a refresh, and is
    /// answered at the throttle rather than waiting out the poll.
    Region(IVec3, i32, Option<Admitted>),
    // View<Frustum>,
    Route(String, String, String),
    /// Named systems, by address
    ///
    /// What the map is asked for a row at a time rather than by where it is:
    /// a system the user picked out of a list is one the map may never have
    /// been near.
    Systems(Vec<i64>),
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
    /// A region refreshes another when it has the same center and reaches no
    /// further. A larger radius takes in systems that were never asked for, so
    /// it is a new question standing in the same place.
    ///
    /// A question and a predicate rather than an ordering. Two regions about
    /// different centers are each no answer to the other, which is a thing an
    /// [`Ord`] cannot say: it would have to call one of them the greater, and
    /// whichever it called it would be wrong the other way round.
    fn refreshes(&self, last: &FetchIndex) -> bool {
        match (self, last) {
            (
                FetchIndex::Region(center, radius, admitted),
                FetchIndex::Region(before, reached, asked),
            ) => center == before && radius <= reached && admitted == asked,
            // Only the spyglass records what it last fetched, so neither a
            // route nor a named system is ever on either side of this.
            // Somewhere new either way.
            _ => false,
        }
    }
}

impl fmt::Debug for FetchIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use FetchIndex::*;

        match self {
            Region(center, radius, admitted) => {
                write!(
                    f,
                    "<({},{},{}),{}",
                    center.x, center.y, center.z, radius
                )?;
                if let Some(admitted) = admitted {
                    write!(
                        f,
                        " admitting {} factions and {} systems",
                        admitted.factions.len(),
                        admitted.systems.len()
                    )?;
                }
                write!(f, ">")
            }
            Route(start, end, range) => {
                write!(f, "<{}-{}>{}>", start, end, range)
            }
            Systems(addresses) => write!(f, "<{} named>", addresses.len()),
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
    mut search_events: MessageReader<Search>,
    mut tasks: ResMut<FetchTasks>,
    mut spyglass: ResMut<Spyglass>,
    filters: Res<Filters>,
    dim: Res<DimTo>,
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
            &filters,
            &dim,
            &time,
            &mut last_fetched_at,
            &throttle,
            &poll,
            &db,
        );
    }

    for event in search_events.read() {
        match event {
            // A search finds and picks out nothing, so there is nothing
            // here to fetch yet. Whatever the user picks out of what it
            // found is asked for by `fetch_selected`.
            Search::System { .. } => {}
            Search::Route { start, end, range } => {
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

/// Ask for the region under the camera, or for what of it is admitted
///
/// Narrowed by the filters only where what they exclude is not drawn at all.
/// Anywhere above that the excluded systems are wanted on screen to be dimmed,
/// and what was never fetched cannot be drawn faintly.
fn fetch_spyglass(
    camera_query: &Query<&OrbitCamera>,
    tasks: &mut ResMut<FetchTasks>,
    spyglass: &ResMut<Spyglass>,
    filters: &Res<Filters>,
    dim: &Res<DimTo>,
    time: &Res<Time<Real>>,
    last_fetched_at: &mut ResMut<LastFetchedAt>,
    throttle: &Res<Throttle>,
    poll: &Res<Poll>,
    db: &Res<Db>,
) {
    let Ok(camera) = camera_query.single() else { return };
    let center = camera.center.as_ivec3();
    let admitted = if dim.0 == 0. { filters.admitted() } else { None };
    // Kept out of the index, unlike the two lists. The moment moves with the
    // clock, so a region that carried it would be somewhere new every time it
    // was worked out, and the map would ask again at the throttle rather than
    // waiting out the poll.
    let since =
        if dim.0 == 0. { filters.changed_since(Utc::now()) } else { None };
    let index =
        FetchIndex::Region(center, spyglass.radius as i32, admitted.clone());
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
            let range = radius.floor() as f64;
            let narrowed = admitted.as_ref().map(|admitted| {
                (admitted.factions.as_slice(), admitted.systems.as_slice())
            });
            DbSystem::fetch_in_range_of_point(&db, range, cent, narrowed, since)
                .await
                .unwrap_or_default()
        });
        tasks.fetched.insert(index.clone(), (task, now));
        tasks.last_fetched = Some(index);
        **last_fetched_at = LastFetchedAt(now);
    }
}

/// Ask for the systems that are picked out and have no star on the map
///
/// A system is picked out of what the database answered, which the map may
/// never have been near: a name searched for and flown to is exactly that.
/// Without this the camera arrives at empty space, and the ring and the name
/// that mark a selection have nothing to hang on.
///
/// Whatever the spyglass is set to. Fetching by region is what the user turns
/// off to stop the map filling itself in as they fly, and a system they
/// picked out by hand is not the map filling itself in.
///
/// Only when the selection changes, which is what keeps a system the database
/// cannot place from being asked for again every frame. Such a system never
/// spawns, so what is missing would go on being missing.
///
/// The spyglass's own memory of where it last fetched is left alone. This
/// asks for named rows rather than for somewhere, so it says nothing about
/// whether the region under the camera is worth asking for again.
fn fetch_selected(
    selection: Res<Selection>,
    systems: Query<&System>,
    mut tasks: ResMut<FetchTasks>,
    time: Res<Time<Real>>,
    db: Res<Db>,
) {
    if !selection.is_changed() {
        return;
    }

    let spawned = systems.iter().map(|system| system.address).collect();
    let wanted = unspawned(&selection.addresses(), &spawned);
    if wanted.is_empty() {
        return;
    }

    let now = time.last_update().unwrap_or(time.startup());
    let task_pool = AsyncComputeTaskPool::get();
    let asking = wanted.clone();
    let db = db.0.clone();
    let task = task_pool.spawn(async move {
        DbSystem::fetch_many(&db, &asking).await.unwrap_or_default()
    });
    tasks.fetched.insert(FetchIndex::Systems(wanted), (task, now));
}

/// Which of `selected` the map has no star for, by address
///
/// One query for all of them, so this answers a list rather than a verdict
/// per system.
fn unspawned(selected: &[i64], spawned: &HashSet<i64>) -> Vec<i64> {
    selected
        .iter()
        .copied()
        .filter(|address| !spawned.contains(address))
        .collect()
}

/// Whether the spyglass should ask for `index` now
///
/// One region query at a time. The throttle says how long to wait since the
/// last one was *started*, which is no answer at all when a region takes
/// longer to come back than the throttle waits: flying while zoomed out asks
/// again every throttle, and a query over most of the galaxy takes a second
/// or two, so a handful of them end up on the wire at once, each one a copy
/// of most of the table and each one crowding the rest.
///
/// Waited on rather than replaced, unlike a route. The regions asked for
/// while the camera moves are each a real answer about where it was, and
/// dropping the one under way for the next would leave nothing arriving at
/// all until the camera stopped.
pub fn spyglass_condition(
    index: &FetchIndex,
    tasks: &ResMut<FetchTasks>,
    now: Instant,
    last_fetched_at: &ResMut<LastFetchedAt>,
    throttle: &Res<Throttle>,
    poll: &Res<Poll>,
) -> bool {
    if region_asked(tasks.fetched.keys()) {
        return false;
    }

    tasks.last_fetched.as_ref().map_or(true, |last_fetched| {
        if index.refreshes(last_fetched) {
            poll.elapsed(last_fetched_at.0, now)
        } else {
            last_fetched_at.0 + Duration::from_millis(throttle.0) < now
        }
    })
}

/// Whether a region is among the queries already on the wire
///
/// Only a region. A route and a set of named systems are each asked for once
/// by something the user just did, and neither is the map asking again for
/// most of what it already holds.
fn region_asked<'a>(mut asked: impl Iterator<Item = &'a FetchIndex>) -> bool {
    asked.any(|index| matches!(index, FetchIndex::Region(..)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A region of `radius` about `center` on the x axis, asked for whole
    fn region(center: i32, radius: i32) -> FetchIndex {
        FetchIndex::Region(IVec3::new(center, 0, 0), radius, None)
    }

    /// The same region, narrowed to the faction at `id`
    fn region_admitting(center: i32, radius: i32, id: i32) -> FetchIndex {
        let admitted = Admitted { factions: vec![id], systems: Vec::new() };
        FetchIndex::Region(IVec3::new(center, 0, 0), radius, Some(admitted))
    }

    /// The map holding a star for each of `addresses`
    fn on_the_map(addresses: &[i64]) -> HashSet<i64> {
        addresses.iter().copied().collect()
    }

    /// A region narrowed to a filter is a different question about the place
    ///
    /// Not a refresh of the region asked for whole, so it is answered at the
    /// throttle rather than waiting out the poll. Adding a filter while the
    /// excluded are not drawn changes what the map is asking for, and the
    /// user is waiting on the answer.
    #[test]
    fn a_narrowed_region_does_not_refresh_the_whole_one() {
        assert!(!region_admitting(0, 10, 7).refreshes(&region(0, 10)));
        assert!(!region(0, 10).refreshes(&region_admitting(0, 10, 7)));
    }

    /// Nor does one narrowed to something else
    #[test]
    fn two_narrowings_are_two_questions() {
        assert!(
            !region_admitting(0, 10, 7).refreshes(&region_admitting(0, 10, 9))
        );
    }

    /// The same narrowing about the same place is a refresh
    #[test]
    fn the_same_narrowed_region_refreshes() {
        assert!(
            region_admitting(0, 10, 7).refreshes(&region_admitting(0, 10, 7))
        );
    }

    /// A region already on the wire is one the spyglass waits for
    ///
    /// The throttle measures from where the last query was sent rather than
    /// from where it came back, so a region that takes longer to answer than
    /// the throttle waits would otherwise be asked again over the top of
    /// itself, and again, while the camera moves.
    #[test]
    fn a_region_under_way_is_waited_for() {
        assert!(region_asked([region(0, 10)].iter()));
    }

    /// Nothing under way is nothing to wait for
    #[test]
    fn no_query_under_way_is_no_reason_to_wait() {
        assert!(!region_asked([].iter()));
    }

    /// A route does not hold the spyglass up
    ///
    /// It is asked for once by something the user just did, and it is not the
    /// map asking again for most of what it already holds.
    #[test]
    fn a_route_under_way_does_not_hold_the_spyglass_up() {
        let route = FetchIndex::Route("A".into(), "B".into(), "10".into());

        assert!(!region_asked([route].iter()));
    }

    /// Nor does a system that was picked out
    #[test]
    fn a_picked_system_does_not_hold_the_spyglass_up() {
        assert!(!region_asked([FetchIndex::Systems(vec![7])].iter()));
    }

    /// A system picked out with no star on the map is asked for
    ///
    /// The case the whole thing is for: a name searched for, picked out of
    /// what came back, and flown to, from a part of the sky the map has
    /// never fetched.
    #[test]
    fn a_selection_the_map_has_not_reached_is_asked_for() {
        assert_eq!(unspawned(&[7], &on_the_map(&[])), vec![7]);
    }

    /// One already on the map is not asked for again
    #[test]
    fn a_selection_already_drawn_is_left_alone() {
        assert_eq!(unspawned(&[7], &on_the_map(&[7])), Vec::<i64>::new());
    }

    /// A set half on the map asks only for the half that is not
    ///
    /// One query for the lot rather than one each, and the order they were
    /// picked in is what it goes out in.
    #[test]
    fn a_gathered_selection_asks_for_what_is_missing() {
        assert_eq!(unspawned(&[7, 9, 11], &on_the_map(&[9])), vec![7, 11]);
    }

    /// Nothing picked out asks for nothing
    #[test]
    fn an_empty_selection_asks_for_nothing() {
        assert_eq!(unspawned(&[], &on_the_map(&[7])), Vec::<i64>::new());
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
