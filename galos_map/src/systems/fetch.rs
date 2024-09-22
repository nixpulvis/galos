use crate::systems::{route::fetch::fetch_route, Spyglass};
use crate::{search::Searched, Db};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use bevy_panorbit_camera::PanOrbitCamera;
use galos_db::systems::System as DbSystem;
use std::collections::HashMap;
use std::time::{Duration, Instant};

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

/// A region is as large as the current spyglass radius / this factor.
const REGION_FACTOR: i32 = 10;

/// The amount to throttle requests for old indices.
const LAST_FETCH_DELAY: Duration = Duration::from_secs(1);
/// The amount to throttle requests for new indices.
const NEW_FETCH_DELAY: Duration =
    Duration::from_millis((1. / 60. * 1000.) as u64);

/// Tasks for systems in the DB which will be spawned
#[derive(Resource, Default)]
pub struct FetchTasks {
    pub fetched: HashMap<FetchIndex, (Task<Vec<DbSystem>>, Instant)>,
    pub last_fetched: Option<FetchIndex>,
}

#[derive(Resource)]
pub struct LastFetchedAt(pub Instant);

impl Default for LastFetchedAt {
    fn default() -> LastFetchedAt {
        LastFetchedAt(Instant::now())
    }
}

/// Spawns tasks to load star systems from the DB
pub fn fetch(
    camera_query: Query<&mut PanOrbitCamera>,
    mut search_events: EventReader<Searched>,
    mut tasks: ResMut<FetchTasks>,
    mut spyglass: ResMut<Spyglass>,
    time: Res<Time<Real>>,
    mut last_fetched: ResMut<LastFetchedAt>,
    db: Res<Db>,
) {
    if spyglass.fetch {
        fetch_around_camera(
            &camera_query,
            &mut tasks,
            &mut spyglass,
            &time,
            &mut last_fetched,
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
                    &mut last_fetched,
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
                    &mut last_fetched,
                    &db,
                );
            }
        };
    }
}

fn fetch_around_camera(
    camera_query: &Query<&mut PanOrbitCamera>,
    tasks: &mut ResMut<FetchTasks>,
    spyglass: &ResMut<Spyglass>,
    time: &Res<Time<Real>>,
    last_fetched: &mut ResMut<LastFetchedAt>,
    db: &Res<Db>,
) {
    let camera = camera_query.single();
    let center = camera.focus.as_ivec3();
    // Regions need to be smaller than the spyglass radius. Once we load cubes,
    // we'll need to change things to hide the entities outside of the sphere.
    let scale = spyglass.radius as i32 / REGION_FACTOR;
    let index = if scale == 0 {
        FetchIndex::Region(IVec3::ZERO, spyglass.radius as i32)
    } else {
        FetchIndex::Region(center / scale, spyglass.radius as i32)
    };

    let now = time.last_update().unwrap_or(time.startup());
    if fetch_condition(&index, &tasks, now, &last_fetched) {
        let task_pool = AsyncComputeTaskPool::get();
        let db = db.0.clone();
        let radius = spyglass.radius;
        let task = task_pool.spawn(async move {
            let cent = [center.x as f64, center.y as f64, center.z as f64];
            DbSystem::fetch_in_range_of_point(&db, radius as f64, cent)
                .await
                .unwrap_or_default()
        });
        tasks.fetched.insert(index.clone(), (task, now));
        tasks.last_fetched = Some(index);
        **last_fetched = LastFetchedAt(now);
    }
}

fn fetch_faction(
    name: String,
    tasks: &mut ResMut<FetchTasks>,
    time: &Res<Time<Real>>,
    last_fetched: &mut ResMut<LastFetchedAt>,
    db: &Res<Db>,
) {
    let index = FetchIndex::Faction(name.clone());
    let now = time.last_update().unwrap_or(time.startup());
    if fetch_condition(&index, &tasks, now, &last_fetched) {
        let task_pool = AsyncComputeTaskPool::get();
        let db = db.0.clone();
        let task = task_pool.spawn(async move {
            DbSystem::fetch_faction(&db, &name).await.unwrap_or_default()
        });
        tasks.fetched.insert(index.clone(), (task, now));
        tasks.last_fetched = Some(index);
        **last_fetched = LastFetchedAt(now);
    }
}

pub fn fetch_condition(
    index: &FetchIndex,
    tasks: &ResMut<FetchTasks>,
    now: Instant,
    last_fetched_at: &ResMut<LastFetchedAt>,
) -> bool {
    tasks.last_fetched.as_ref().map_or(true, |last_fetched| {
        if *index == *last_fetched {
            last_fetched_at.0 + LAST_FETCH_DELAY < now
        } else {
            last_fetched_at.0 + NEW_FETCH_DELAY < now
        }
    })
}
