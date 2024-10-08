use crate::systems::{route::fetch::fetch_route, Spyglass};
use crate::{search::Searched, Db};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use bevy_panorbit_camera::PanOrbitCamera;
use galos_db::systems::System as DbSystem;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

pub fn plugin(app: &mut App) {
    app.insert_resource(Poll(Some(1.)));
    app.insert_resource(Throttle(50));

    app.init_resource::<LastFetchedAt>();
    app.init_resource::<FetchTasks>();

    app.add_systems(Update, fetch);
}

/// Controls the background systems fetch rate (Hz).
///
/// If this value is `None` it disables updates for existing [`FetchIndex`]s.
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
    Faction(String),
    Route(String, String, String),
}

impl Ord for FetchIndex {
    fn cmp(&self, other: &Self) -> Ordering {
        use FetchIndex::*;

        match (self, other) {
            (Region(sc, sr), Region(oc, or)) => {
                if sc == oc {
                    sr.cmp(or)
                } else {
                    // NOTE: It's critical that this be greater so
                    // comparisions on translating regions
                    Ordering::Greater
                }
            }
            (Faction(sn), Faction(on)) => sn.cmp(on),
            (Route(ss, se, sr), Route(os, oe, or)) => {
                ss.cmp(os).then(se.cmp(oe)).then(sr.cmp(or))
            }
            _ => Ordering::Less,
        }
    }
}

impl PartialOrd for FetchIndex {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
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
            Faction(name) => write!(f, "<{}>", name),
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
    camera_query: Query<&mut PanOrbitCamera>,
    mut search_events: EventReader<Searched>,
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
            Searched::Faction { name } => {
                fetch_faction(
                    name.into(),
                    &mut tasks,
                    &time,
                    &mut last_fetched_at,
                    &db,
                );
            }
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
    camera_query: &Query<&mut PanOrbitCamera>,
    tasks: &mut ResMut<FetchTasks>,
    spyglass: &ResMut<Spyglass>,
    time: &Res<Time<Real>>,
    last_fetched_at: &mut ResMut<LastFetchedAt>,
    throttle: &Res<Throttle>,
    poll: &Res<Poll>,
    db: &Res<Db>,
) {
    let camera = camera_query.single();
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

fn fetch_faction(
    name: String,
    tasks: &mut ResMut<FetchTasks>,
    time: &Res<Time<Real>>,
    last_fetched_at: &mut ResMut<LastFetchedAt>,
    db: &Res<Db>,
) {
    let index = FetchIndex::Faction(name.clone());
    let now = time.last_update().unwrap_or(time.startup());
    let task_pool = AsyncComputeTaskPool::get();
    let db = db.0.clone();
    let task = task_pool.spawn(async move {
        DbSystem::fetch_faction(&db, &name).await.unwrap_or_default()
    });
    tasks.fetched.insert(index.clone(), (task, now));
    tasks.last_fetched = Some(index);
    **last_fetched_at = LastFetchedAt(now);
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
        if *index <= *last_fetched {
            poll.0.map_or(false, |poll| {
                // Convert from Hz to millis.
                let poll = Duration::from_millis((1e3 / poll) as u64);
                last_fetched_at.0 + poll < now
            })
        } else {
            last_fetched_at.0 + Duration::from_millis(throttle.0) < now
        }
    })
}
