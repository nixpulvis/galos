use super::fetch_route;
use crate::{search::Searched, Db};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use bevy_panorbit_camera::PanOrbitCamera;
use galos_db::systems::System as DbSystem;
use std::collections::{HashMap, HashSet};

/// Represents a single fetch request
//
// TODO: Put region math inside custom Hash impl?
// TODO: once we have a hash impl let's save f64 instead of String for route
// range.
// TODO: fetched regions should be cubes with `region_size` side length, they
// are currently spheres with `region_size` radius.
#[derive(Hash, Eq, PartialEq, Clone)]
pub enum FetchIndex {
    // System<String>
    Region(IVec3, i32),
    // View<Frustum>,
    Faction(String),
    Route(String, String, String),
}

// A region is as large as the current spyglass radius / this factor.
const REGION_FACTOR: i32 = 10;

/// Tasks for systems in the DB which will be spawned
#[derive(Resource)]
pub struct FetchTasks {
    pub fetched: HashMap<FetchIndex, Task<Vec<DbSystem>>>,
}

/// A representation of the spawned systems
#[derive(Resource)]
pub struct Fetched(pub HashSet<FetchIndex>);

/// A global setting which toggles the spyglass around the camera
#[derive(Resource)]
pub struct AlwaysFetch(pub bool);

/// Spawns tasks to load star systems from the DB
pub fn fetch(
    camera_query: Query<&mut PanOrbitCamera>,
    mut search_events: EventReader<Searched>,
    mut fetched: ResMut<Fetched>,
    mut tasks: ResMut<FetchTasks>,
    db: Res<Db>,
    mut always_fetch: ResMut<AlwaysFetch>,
    mut radius: ResMut<SpyglassRadius>,
) {
    if always_fetch.0 {
        fetch_around_camera(
            &camera_query,
            &mut fetched,
            &mut tasks,
            &mut radius,
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
                *always_fetch = AlwaysFetch(false);
                fetch_faction(name.into(), &mut fetched, &mut tasks, &db);
            }
            Searched::Route { start, end, range } => {
                fetch_route(
                    start.into(),
                    end.into(),
                    range.into(),
                    &mut fetched,
                    &mut tasks,
                    &db,
                );
            }
        };
    }
}

/// The radius searched around the camera
///
/// This does nothing while `AlwaysFetch` is false.
#[derive(Resource)]
pub struct SpyglassRadius(pub f64);

fn fetch_around_camera(
    camera_query: &Query<&mut PanOrbitCamera>,
    fetched: &mut ResMut<Fetched>,
    tasks: &mut ResMut<FetchTasks>,
    radius: &mut ResMut<SpyglassRadius>,
    db: &Res<Db>,
) {
    let camera = camera_query.single();
    let center = camera.focus.as_ivec3();
    // Regions need to be smaller than the spyglass radius. Once we load cubes,
    // we'll need to change things to hide the entities outside of the sphere.
    let scale = radius.0 as i32 / REGION_FACTOR;
    let region = if scale == 0 {
        FetchIndex::Region(IVec3::ZERO, radius.0 as i32)
    } else {
        FetchIndex::Region(center / scale, radius.0 as i32)
    };
    if !fetched.0.contains(&region) && !tasks.fetched.contains_key(&region) {
        let task_pool = AsyncComputeTaskPool::get();
        let db = db.0.clone();
        let radius = radius.0;
        let task = task_pool.spawn(async move {
            let cent = [center.x as f64, center.y as f64, center.z as f64];
            DbSystem::fetch_in_range_of_point(&db, radius, cent)
                .await
                .unwrap_or_default()
        });
        fetched.0.insert(region.clone());
        tasks.fetched.insert(region, task);
    }
}

fn fetch_faction(
    name: String,
    fetched: &mut ResMut<Fetched>,
    tasks: &mut ResMut<FetchTasks>,
    db: &Res<Db>,
) {
    let index = FetchIndex::Faction(name.clone());
    if !tasks.fetched.contains_key(&index) {
        let task_pool = AsyncComputeTaskPool::get();
        let db = db.0.clone();
        let task = task_pool.spawn(async move {
            DbSystem::fetch_faction(&db, &name).await.unwrap_or_default()
        });
        fetched.0.insert(index.clone());
        tasks.fetched.insert(index, task);
    }
}
