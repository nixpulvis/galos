use crate::{camera::MoveCamera, search::Searched, Db};
use bevy::pbr::NotShadowCaster;
use bevy::prelude::*;
use bevy::render::mesh::PrimitiveTopology;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{block_on, AsyncComputeTaskPool, Task};
use bevy_mod_picking::prelude::*;
use bevy_panorbit_camera::PanOrbitCamera;
use elite_journal::prelude::*;
use galos_db::systems::System as DbSystem;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;

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
                DbSystem::fetch_by_name(&db, &start).await,
                DbSystem::fetch_by_name(&db, &end).await,
                range.parse::<f64>(),
            ) {
                if let Some(route) = a.route_to(&db, &b, r) {
                    return route.0;
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
    systems_query: Query<Entity, With<System>>,
    route_query: Query<Entity, With<Route>>,
    always_despawn: Res<AlwaysDespawn>,
    color_by: Res<ColorBy>,
    mut move_camera_events: EventWriter<MoveCamera>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut tasks: ResMut<FetchTasks>,
    mut fetched: ResMut<Fetched>,
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
            }

            // TODO: Pass FetchIndex along. I'd like to have index.marker() or
            // similar so I can mark entities with some info about where they
            // were fetched from.
            spawn_systems(
                &systems,
                &systems_query,
                &color_by,
                &always_despawn,
                &mut commands,
                &mut meshes,
                &mut materials,
            );

            match index {
                FetchIndex::Faction(..) | FetchIndex::Route(..) => {
                    if let Some(system) = systems.first() {
                        let position = system_to_vec(&system);
                        move_camera_events
                            .send(MoveCamera { position: Some(position) });
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
                        &mut materials,
                    );
                }
                _ => {}
            }
        }
        retain
    });

    // TODO: despawn stuff...
}

#[derive(Component)]
pub struct Route;

// TODO: Save another Local<Option<Handle<Mesh>>>?
fn spawn_route(
    systems: &[DbSystem],
    route_query: &Query<Entity, With<Route>>,
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
) {
    for entity in route_query.iter() {
        commands.entity(entity).despawn_recursive();
    }

    commands.spawn((
        MaterialMeshBundle {
            mesh: meshes.add(LineStrip {
                points: systems.iter().map(system_to_vec).collect(),
            }),
            transform: Transform::from_xyz(0., 0., 0.),
            material: materials.add(StandardMaterial {
                base_color: Color::srgba(1., 1., 1., 0.1),
                alpha_mode: AlphaMode::Blend,
                ..default()
            }),
            ..default()
        },
        Route,
    ));
}

#[derive(Component)]
pub struct System {
    address: i64,
    name: String,
    population: u64,
    allegiance: Option<Allegiance>,
}

/// Generate all the star system entities.
fn spawn_systems(
    systems: &[DbSystem],
    systems_query: &Query<Entity, With<System>>,
    color_by: &Res<ColorBy>,
    always_despawn: &Res<AlwaysDespawn>,
    commands: &mut Commands,
    mesh_asset: &mut ResMut<Assets<Mesh>>,
    material_assets: &mut ResMut<Assets<StandardMaterial>>,
) {
    if always_despawn.0 {
        for entity in systems_query.iter() {
            commands.entity(entity).despawn_recursive();
        }
    }

    let mesh = init_meshes(mesh_asset);
    let materials = init_materials(material_assets);

    for system in systems {
        let color_idx = match color_by.deref() {
            ColorBy::Allegiance => allegiance_color_idx(&system),
            ColorBy::Government => government_color_idx(&system),
            ColorBy::Security => security_color_idx(&system),
        };
        commands.spawn((
            PbrBundle {
                transform: Transform {
                    translation: Vec3::new(
                        system.position.unwrap().x as f32,
                        system.position.unwrap().y as f32,
                        system.position.unwrap().z as f32,
                    ),
                    scale: Vec3::splat(1.),
                    ..default()
                },
                mesh: mesh.clone(),
                material: materials[color_idx].clone(),
                ..default()
            },
            System {
                address: system.address,
                name: system.name.clone(),
                population: system.population,
                allegiance: system.allegiance,
            },
            NotShadowCaster,
            PickableBundle::default(),
            // TODO: toggle system info as well.
            On::<Pointer<Click>>::send_event::<MoveCamera>(),
            On::<Pointer<Over>>::target_commands_mut(
                |_hover, _target_commands| {
                    // dbg!(_hover);
                    // TODO: Spawn system label.
                },
            ),
            On::<Pointer<Out>>::target_commands_mut(
                |_hover, _target_commands| {
                    // dbg!(_hover);
                    // TODO: Despawn system label.
                },
            ),
        ));
    }
}

#[derive(Resource, Debug, PartialEq)]
pub enum View {
    // #[default]
    Systems,
    Stars, // TODO: bodies?
}

#[derive(Resource, Debug)]
pub struct ScalePopulation(pub bool);

pub fn scale_systems(
    scale_population: Res<ScalePopulation>,
    mut set: ParamSet<(
        Query<(&mut Transform, &System)>,
        Query<&Transform, With<PanOrbitCamera>>,
    )>,
) {
    if !set.p0().is_empty() {
        let camera_translation = set.p1().single().translation;
        let pop_avg = if scale_population.0 {
            // TODO: This is *very* slow and should be precomputed when the set of systems changes.
            set.p0().iter().map(|(_, s)| s.population).sum::<u64>()
                / set.p0().iter().len() as u64
        } else {
            0
        };

        // The goal is to avoid fading out any stars, but scale them as the
        // camera moves further away from them.
        // TODO: We should still change rgba color/emmisivity as needed.
        for (mut system_transform, system) in set.p0().iter_mut() {
            let dist =
                camera_translation.distance(system_transform.translation);
            let mut scale = 4e-4 * dist + 8.5e-2;
            if scale_population.0 {
                let pop_factor = system.population as f32 / pop_avg as f32;
                scale *= 0.2 * pop_factor.ln();
            }
            system_transform.scale = Vec3::splat(scale);
        }
    }
}

pub fn scale_stars(mut query: Query<(&mut Transform, &System)>) {
    if !query.is_empty() {
        // TODO: Change rgba color/emmisivity. The goal is to fade out to
        // transparent when they are too far away.
        for (mut system_transform, system) in query.iter_mut() {
            system_transform.scale = Vec3::splat(1e-2);
        }
    }
}

#[derive(Resource, Copy, Clone, Debug, PartialEq)]
pub enum ColorBy {
    Allegiance,
    Government,
    Security,
}

fn init_meshes(assets: &mut Assets<Mesh>) -> Handle<Mesh> {
    assets.add(Sphere::new(1.).mesh().ico(3).unwrap())
}

fn init_materials(
    assets: &mut Assets<StandardMaterial>,
) -> Vec<Handle<StandardMaterial>> {
    let colors = vec![
        Color::srgba(0., 1., 0., 0.4),    // Green
        Color::srgba(0., 1., 1., 0.4),    // Cyan
        Color::srgba(1., 0., 0., 0.4),    // Red
        Color::srgba(1., 0.5, 0., 0.4),   // Orange
        Color::srgba(1., 1., 0., 0.4),    // Yellow
        Color::srgba(0., 0., 1., 0.4),    // Blue
        Color::srgba(1., 0., 1., 0.4),    // Magenta
        Color::srgba(0.1, 0.1, 0.1, 0.1), // Grey
    ];

    colors
        .into_iter()
        .map(|color| {
            assets.add(StandardMaterial {
                base_color: color,
                alpha_mode: AlphaMode::Blend,
                emissive: LinearRgba::from(color.with_alpha(1.0)) * 10.,
                ..default()
            })
        })
        .collect()
}

fn allegiance_color_idx(system: &DbSystem) -> usize {
    match system.allegiance {
        Some(Allegiance::Alliance) => 0,         // Green
        Some(Allegiance::Empire) => 1,           // Cyan
        Some(Allegiance::Federation) => 2,       // Red
        Some(Allegiance::PilotsFederation) => 3, // Orange
        Some(Allegiance::PlayerPilots) => 4,     // Yellow
        Some(Allegiance::Independent) => 4,      // Yellow
        Some(Allegiance::Guardian) => 5,         // Blue
        Some(Allegiance::Thargoid) => 6,         // Magenta
        Some(Allegiance::None) | None => 7,      // Grey
    }
}

fn government_color_idx(system: &DbSystem) -> usize {
    match system.government {
        Some(Government::Anarchy) => 4,      // Yellow
        Some(Government::Carrier) => 0,      // Green
        Some(Government::Communism) => 2,    // Red
        Some(Government::Confederacy) => 2,  // Red
        Some(Government::Cooperative) => 3,  // Orange
        Some(Government::Corporate) => 1,    // Cyan
        Some(Government::Democracy) => 5,    // Blue
        Some(Government::Dictatorship) => 2, // Red
        Some(Government::Engineer) => 6,     // Magenta
        Some(Government::Feudal) => 2,       // Red
        Some(Government::Patronage) => 2,    // Red
        Some(Government::Prison) => 2,       // Red
        Some(Government::PrisonColony) => 2, // Red
        Some(Government::Theocracy) => 5,    // Blue
        Some(Government::None) | None => 7,  // Grey
    }
}

fn security_color_idx(system: &DbSystem) -> usize {
    match system.security {
        Some(Security::High) => 1,        // Cyan
        Some(Security::Medium) => 5,      // Blue
        Some(Security::Low) => 4,         // Yellow
        Some(Security::Anarchy) => 2,     // Red
        Some(Security::None) | None => 7, // Grey
    }
}

fn system_to_vec(system: &DbSystem) -> Vec3 {
    Vec3::new(
        system.position.unwrap().x as f32,
        system.position.unwrap().y as f32,
        system.position.unwrap().z as f32,
    )
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
