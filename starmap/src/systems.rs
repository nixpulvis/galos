use std::collections::{HashSet, HashMap};
use bevy::prelude::*;
use bevy::render::mesh::{PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::tasks::{block_on, AsyncComputeTaskPool, Task};
use bevy::tasks::futures_lite::future;
use bevy_panorbit_camera::PanOrbitCamera;
use bevy_mod_picking::prelude::*;
use galos_db::Database;
use galos_db::systems::System;
use elite_journal::prelude::*;
use async_std::task;
use crate::{DatabaseResource, Searched, MoveCamera, SystemMarker, RouteMarker};

#[derive(Resource)]
pub struct FetchTasks {
    // TODO: This IVec3 doesn't take zoom or rotation into account.
    // To generate all the stars in view we should. To start we'll
    // just load near the center of the camera.
    //
    // Also, using something else as the key could allow for
    // "search:faction:"New Pilots Initiative" => Task as well.
    pub regions: HashMap<IVec3, Task<Vec<System>>>
}

#[derive(Resource)]
pub struct LoadedRegions {
    pub centers: HashSet<IVec3>
}

pub fn fetch(
    camera_query: Query<&mut PanOrbitCamera>,
    mut loaded_regions: ResMut<LoadedRegions>,
    mut tasks: ResMut<FetchTasks>,
    db: Res<DatabaseResource>,
) {
    fetch_around_camera(&camera_query, &mut loaded_regions, &mut tasks, &db);
}

fn fetch_around_camera(
    camera_query: &Query<&mut PanOrbitCamera>,
    mut loaded_regions: &mut ResMut<LoadedRegions>,
    mut tasks: &mut ResMut<FetchTasks>,
    db: &Res<DatabaseResource>,
) {
    let camera = camera_query.single();
    let center = camera.focus.as_ivec3();
    if !loaded_regions.centers.contains(&center) &&
       !tasks.regions.contains_key(&center)
    {
        let task_pool = AsyncComputeTaskPool::get();
        let db = db.0.clone();
        let task = task_pool.spawn(async move {
            let radius = 50.;
            let cent = [
                center.x as f64,
                center.y as f64,
                center.z as f64,
            ];
            System::fetch_in_range_of_point(&db, radius, cent).await.unwrap_or_default()
        });
        loaded_regions.centers.insert(center);
        tasks.regions.insert(center, task);
    }
}

// TODO: How best to switch between camera orianted system loading and custom
// filters like faction and route searches, etc.
pub fn spawn(
    systems_query: Query<Entity, With<SystemMarker>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut mesh: Local<Option<Handle<Mesh>>>,
    mut tasks: ResMut<FetchTasks>,
) {
    tasks.regions.retain(|_, task| {
        let status = block_on(future::poll_once(task));
        let retain = status.is_none();
        if let Some(systems) = status {
            for entity in systems_query.iter() {
                commands.entity(entity).despawn_recursive();
            }
            spawn_entities(
                &systems,
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut mesh);
        }
        retain
    });

    // TODO: despawn stuff...
}

/// Queries the DB, then creates an entity for each star system in the search.
///
/// This function also moves the camera's position to be looking at the
/// searched system.
pub fn generate(
    systems_query: Query<Entity, With<SystemMarker>>,
    route_query: Query<Entity, With<RouteMarker>>,
    mut search_events: EventReader<Searched>,
    mut camera_events: EventWriter<MoveCamera>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut mesh: Local<Option<Handle<Mesh>>>,
) {
    for event in search_events.read() {
        // Load DB objects.
        let (origin, systems) = match event {
            Searched::System { name, .. } => query_systems(&name, "25"),
            Searched::Faction { name } => query_faction_systems(&name),
            Searched::Route { start, end, range } => query_route(&start, &end, &range),
        };

        // Move the camera to the origin of queried systems.
        if let Some(o) = &origin {
            camera_events.send(MoveCamera {
                position: system_to_vec(o)
            });
        }

        for entity in systems_query.iter() {
            commands.entity(entity).despawn_recursive();
        }
        for entity in route_query.iter() {
            commands.entity(entity).despawn_recursive();
        }

        spawn_entities(&systems, &mut commands, &mut meshes, &mut materials, &mut mesh);

        // After generating the star systems, if we're a route search, draw
        // the route lines and populate space around each system along the
        // route.
        if let Searched::Route { range, .. } = event {
            commands.spawn((MaterialMeshBundle {
                mesh: meshes.add(LineStrip {
                    points: systems.iter().map(system_to_vec).collect()
                }),
                transform: Transform::from_xyz(0., 0., 0.),
                material: materials.add(StandardMaterial {
                    base_color: Color::srgba(1., 1., 1., 0.1),
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                }),
                ..default()
            },
            RouteMarker));

            for system in systems {
                let (_, s) = query_systems(&system.name, range);
                spawn_entities(&s, &mut commands, &mut meshes, &mut materials, &mut mesh);
            }
        }
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

fn system_to_vec(system: &System) -> Vec3 {
    Vec3::new(
        system.position.unwrap().x as f32,
        system.position.unwrap().y as f32,
        system.position.unwrap().z as f32,
    )
}

// TODO: I'd like to avoid blocking, but I don't know how yet.
fn query_systems(name: &str, radius: &str) -> (Option<System>, Vec<System>) {
    task::block_on(async {
        let db = Database::new().await.unwrap();
        let radius = radius.parse().unwrap_or(100.);
        let origins = System::fetch_like_name(&db, &name).await.unwrap();
        match System::fetch_in_range_like_name(&db, radius, &name).await {
        // match System::fetch_sample(&db, 100., &name).await {
            Ok(systems) => (origins.first().map(ToOwned::to_owned), systems),
            _ => (None, vec![]),
        }
    })
}

fn query_faction_systems(faction: &str) -> (Option<System>, Vec<System>) {
    task::block_on(async {
        let db = Database::new().await.unwrap();
        match System::fetch_faction(&db, faction).await {
            Ok(systems) => (systems.first().map(ToOwned::to_owned), systems),
            _ => (None, vec![]),
        }
    })
}

fn query_route(start: &str, end: &str, range: &str) -> (Option<System>, Vec<System>) {
    task::block_on(async {
        let db = Database::new().await.unwrap();
        if let (Ok(a), Ok(b), Ok(r)) = (
            System::fetch_by_name(&db, start).await,
            System::fetch_by_name(&db, end).await,
            range.parse::<f64>())
        {
            if let Some(route) = a.route_to(&db, &b, r) {
                return (Some(a), route.0)
            }
        }

        (None, vec![])
    })
}

/// Generate all the star system entities.
fn spawn_entities(
    systems: &[System],
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
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

    for system in systems {
        commands.spawn((PbrBundle {
            transform: Transform {
                translation: Vec3::new(
                    system.position.unwrap().x as f32,
                    system.position.unwrap().y as f32,
                    system.position.unwrap().z as f32,
                ),
                // scale: Vec3::splat(0.25),
                scale: Vec3::splat(SYSTEM_SCALE),
                ..default()
            },
            // TODO: Use entries API to avoid unwrap.
            mesh: mesh.as_ref().unwrap().clone(),
            // TODO: Configure the material to be flatter when looking at allegiance,
            // or more realistic when looking at star class. Remember to check
            // partially overlapping systems.
            material: materials.add(allegiance_color(&system)),
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
