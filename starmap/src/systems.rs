use std::collections::{HashSet, HashMap};
use bevy::prelude::*;
use bevy::render::mesh::{PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{AsBindGroup, ShaderRef};
use bevy::tasks::{block_on, AsyncComputeTaskPool, Task};
use bevy::tasks::futures_lite::future;
use bevy_panorbit_camera::PanOrbitCamera;
use bevy_mod_picking::prelude::*;
use galos_db::systems::System;
use elite_journal::prelude::*;
use crate::{
    Db,
    ui::Searched,
    MoveCamera,
    SystemMarker,
    RouteMarker
};

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
    pub fetched: HashMap<FetchIndex, Task<Vec<System>>>
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
            &db);
    }

    for event in search_events.read() {
        match event {
            // TODO: Ensure at least the searched star is fetched. I don't do it
            // again here because it was already fetched (syncronously) in
            // `search`. That needs to be refactored anyway. So for now, if
            // you search for a system with AlwaysFetch(false) it may take you
            // to a part of empty space. Setting AlwaysFetch(true) will
            // populate it.
            Searched::System { .. } => {},
            Searched::Faction { name } => {
                *always_fetch = AlwaysFetch(false);
                fetch_faction(
                    name.into(),
                    &mut fetched,
                    &mut tasks,
                    &db);
            },
            Searched::Route { start, end, range } => {
                fetch_route(
                    start.into(),
                    end.into(),
                    range.into(),
                    &mut fetched,
                    &mut tasks,
                    &db);
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
    if !fetched.0.contains(&region) &&
       !tasks.fetched.contains_key(&region)
    {
        let task_pool = AsyncComputeTaskPool::get();
        let db = db.0.clone();
        let radius = radius.0;
        let task = task_pool.spawn(async move {
            let cent = [
                center.x as f64,
                center.y as f64,
                center.z as f64,
            ];
            System::fetch_in_range_of_point(&db, radius, cent).await.unwrap_or_default()
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
            System::fetch_faction(&db, &name).await.unwrap_or_default()
        });
        fetched.0.insert(index.clone());
        tasks.fetched.insert(index, task);
    }
}

fn fetch_route(
    start: String,
    end: String,
    range: String,
    fetched: &mut ResMut<Fetched>,
    tasks: &mut ResMut<FetchTasks>,
    db: &Res<Db>,
) {
    let index = FetchIndex::Route(start.clone(), end.clone(), range.clone());
    if !tasks.fetched.contains_key(&index) {
        let task_pool = AsyncComputeTaskPool::get();
        let db = db.0.clone();
        let task = task_pool.spawn(async move {
            if let (Ok(a), Ok(b), Ok(r)) = (
                System::fetch_by_name(&db, &start).await,
                System::fetch_by_name(&db, &end).await,
                range.parse::<f64>())
            {
                if let Some(route) = a.route_to(&db, &b, r) {
                    return route.0
                }
            }
            vec![]
        });
        fetched.0.insert(index.clone());
        tasks.fetched.insert(index, task);
    }
}

/// Toggles star system despawning
//
// TODO: We still need to come up with a strategy for despawning in general.
// always despawning everything isn't going to be the only option. We'll have
// frustum culling etc.
#[derive(Resource)]
pub struct AlwaysDespawn(pub bool);

/// Polls the tasks in `FetchTasks` and spawns entities for each of the
/// resulting star systems
pub fn spawn(
    systems_query: Query<Entity, With<SystemMarker>>,
    route_query: Query<Entity, With<RouteMarker>>,
    mut move_camera_events: EventWriter<MoveCamera>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard_materials: ResMut<Assets<StandardMaterial>>,
    mut star_materials: ResMut<Assets<StarMaterial>>,
    mut mesh: Local<Option<Handle<Mesh>>>,
    mut tasks: ResMut<FetchTasks>,
    mut fetched: ResMut<Fetched>,
    always_despawn: Res<AlwaysDespawn>,
) {
    tasks.fetched.retain(|index, task| {
        let status = block_on(future::poll_once(task));
        let retain = status.is_none();
        if let Some(systems) = status {
            if always_despawn.0 {
                // TODO: send despawn event, same as in the always_despawn
                // ui checkbox.
                fetched.0.clear();
                fetched.0.insert(index.clone());
                for entity in systems_query.iter() {
                    commands.entity(entity).despawn_recursive();
                }
            }

            // TODO: Pass FetchIndex along. I'd like to have index.marker() or
            // similar so I can mark entities with some info about where they
            // were fetched from.
            spawn_systems(
                &systems,
                &mut commands,
                &mut meshes,
                &mut star_materials,
                &mut mesh);

            match index {
                FetchIndex::Faction(..) |
                FetchIndex::Route(..) => {
                    if let Some(system) = systems.first() {
                        let position = system_to_vec(&system);
                        move_camera_events.send(MoveCamera { position });
                    }
                }
                _ => {}
            }

            match index {
                FetchIndex::Route(..) => {
                    spawn_route(
                        &systems,
                        &route_query,
                        &mut commands,
                        &mut meshes,
                        &mut standard_materials);
                },
                _ => {}
            }
        }
        retain
    });

    // TODO: despawn stuff...
}

// TODO: Save another Local<Option<Handle<Mesh>>>?
fn spawn_route(
    systems: &[System],
    route_query: &Query<Entity, With<RouteMarker>>,
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    standard_materials: &mut ResMut<Assets<StandardMaterial>>,
) {
    for entity in route_query.iter() {
        commands.entity(entity).despawn_recursive();
    }

    commands.spawn((MaterialMeshBundle {
        mesh: meshes.add(LineStrip {
            points: systems.iter().map(system_to_vec).collect()
        }),
        transform: Transform::from_xyz(0., 0., 0.),
        material: standard_materials.add(StandardMaterial {
            base_color: Color::srgba(1., 1., 1., 0.1),
            alpha_mode: AlphaMode::Blend,
            ..default()
        }),
        ..default()
    },
    RouteMarker));
}

/// Generate all the star system entities.
fn spawn_systems(
    systems: &[System],
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    star_materials: &mut ResMut<Assets<StarMaterial>>,
    mesh: &mut Local<Option<Handle<Mesh>>>,
    ) {
    // TODO: Scale systems based on the distance from the camera.
    // This may follow some kind of log curve, or generally effect closer
    // systems less. The goal is to have systems never become smaller than a
    // pixel in size. I'm not sure if we can implement blending modes or
    // something to handle partially overlapping systems.
    const SYSTEM_SCALE:  f32 = 1.;
    const SYSTEM_RADIUS: f32 = SYSTEM_SCALE/2.5;

    // Make sure our sphere mesh is loaded. This is the "shape" of the star.
    if mesh.is_none() {
        **mesh = Some(meshes.add(Sphere::new(SYSTEM_RADIUS).mesh().ico(3).unwrap()));
    }

    let material = star_materials.add(StarMaterial {});

    for system in systems {
        commands.spawn((MaterialMeshBundle {
            transform: Transform {
                translation: Vec3::new(
                    system.position.unwrap().x as f32,
                    system.position.unwrap().y as f32,
                    system.position.unwrap().z as f32,
                ),
                scale: Vec3::splat(SYSTEM_SCALE),
                ..default()
            },
            // TODO: Use entries API to avoid unwrap.
            mesh: mesh.as_ref().unwrap().clone(),
            // TODO: Configure the material to be flatter when looking at allegiance,
            // or more realistic when looking at star class. Remember to check
            // partially overlapping systems.
            material: material.clone(),
            ..default()
        },
        SystemMarker,
        PickableBundle::default(),

        On::<Pointer<Click>>::target_commands_mut(|click, _target_commands| {
            dbg!(click);
            // TODO: toggle system info.
            // TODO: double click to center camera... use events instead
            // of the code below which doesn't work.
            // if let Some(position) = click.event.hit.position {
            //     camera::move_camera(camera_query, position);
            // }
        }),

        On::<Pointer<Over>>::target_commands_mut(|_hover, _target_commands| {
            dbg!(_hover);
            // TODO: Spawn system label.
        }),

        On::<Pointer<Out>>::target_commands_mut(|_hover, _target_commands| {
            dbg!(_hover);
            // TODO: Despawn system label.
        })));
    }
}

fn system_to_vec(system: &System) -> Vec3 {
    Vec3::new(
        system.position.unwrap().x as f32,
        system.position.unwrap().y as f32,
        system.position.unwrap().z as f32,
    )
}

#[derive(AsBindGroup, Debug, Clone, Asset, TypePath)]
pub struct StarMaterial {}

impl Material for StarMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/star.wgsl".into()
    }
}

/// Maps system allegiance to a color for the sphere on the map.
fn allegiance_color(system: &System) -> Color {
    match system.allegiance {
        Some(Allegiance::Alliance)         => Color::srgb(0., 1., 0.),   // Green
        Some(Allegiance::Empire)           => Color::srgb(0., 1., 1.),   // Cyan
        Some(Allegiance::Federation)       => Color::srgb(1., 0., 0.),   // Red
        Some(Allegiance::PilotsFederation) => Color::srgb(1., 0.5, 0.),  // Orange
        Some(Allegiance::Independent)      => Color::srgb(1., 1., 0.),   // Yellow
        Some(Allegiance::Guardian)         => Color::srgb(0., 0., 1.),   // Blue
        Some(Allegiance::Thargoid)         => Color::srgb(1., 0., 1.),  // Blue
        Some(_)                            => Color::srgb(1., 1., 1.),   // White
        None                               => Color::srgb(0., 0., 0.),   // Black
    }
}

/// A list of points that will have a line drawn between each consecutive points
#[derive(Debug, Clone)]
struct LineStrip {
    points: Vec<Vec3>,
}

impl From<LineStrip> for Mesh {
    fn from(line: LineStrip) -> Self {
        Mesh::new(
            // This tells wgpu that the positions are a list of points
            // where a line will be drawn between each consecutive point
            PrimitiveTopology::LineStrip,
            RenderAssetUsages::RENDER_WORLD,
        )
        // Add the point positions as an attribute
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, line.points)
    }
}
