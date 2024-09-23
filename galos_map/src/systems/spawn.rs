use crate::camera::MoveCamera;
use crate::systems::{
    fetch::FetchIndex, fetch::FetchTasks, route::spawn::spawn_route,
    route::Route, system_to_vec, System,
};
use bevy::pbr::NotShadowCaster;
use bevy::prelude::*;
use bevy::tasks::block_on;
use bevy::tasks::futures_lite::future;
use bevy_mod_picking::prelude::*;
use elite_journal::{system::Security, Allegiance, Government};
use galos_db::systems::System as DbSystem;
use std::{collections::HashMap, ops::Deref, time::Instant};

pub fn plugin(app: &mut App) {
    app.add_plugins(DefaultPickingPlugins);
    app.insert_resource(ColorBy::Allegiance);
    app.insert_resource(ShowNames(false));

    app.add_systems(Startup, (init_mesh, init_materials));
    app.add_systems(Update, spawn);
    app.add_systems(Update, update.after(spawn));
}

#[derive(Resource)]
pub struct SystemMesh(pub Handle<Mesh>);

#[derive(Resource)]
pub struct SystemMaterials(pub Vec<Handle<StandardMaterial>>);
// pub struct SystemMaterials(pub HashMap<String, Handle<StandardMaterial>>);

/// Determains what color to draw in system view mode.
#[derive(Resource, Copy, Clone, Debug, PartialEq)]
pub enum ColorBy {
    Allegiance,
    Government,
    Security,
}

/// Determains whether or not to show system name labels
#[derive(Resource)]
pub struct ShowNames(pub bool);

/// Polls the tasks in `FetchTasks` and spawns entities for each of the
/// resulting star systems
pub fn spawn(
    systems_query: Query<(Entity, &System)>,
    route_query: Query<Entity, With<Route>>,
    color_by: Res<ColorBy>,
    mesh: Res<SystemMesh>,
    materials: Res<SystemMaterials>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut move_camera_events: EventWriter<MoveCamera>,
    mut tasks: ResMut<FetchTasks>,
) {
    tasks.fetched.retain(|index, (task, fetched_at)| {
        let status = block_on(future::poll_once(task));
        let retain = status.is_none();
        if let Some(new_systems) = status {
            // TODO: Pass FetchIndex along. I'd like to have index.marker() or
            // similar so I can mark entities with some info about where they
            // were fetched from.
            spawn_systems(
                &new_systems,
                &systems_query,
                &color_by,
                &mut commands,
                &mesh,
                &materials,
                &time,
                fetched_at,
            );

            match index {
                FetchIndex::Faction(..) | FetchIndex::Route(..) => {
                    if let Some(system) = new_systems.first() {
                        let position = system_to_vec(&system);
                        move_camera_events
                            .send(MoveCamera { position: Some(position) });
                    }
                }
                _ => {}
            }

            match index {
                FetchIndex::Route(..) => {
                    // spawn_route(
                    //     &new_systems,
                    //     &route_query,
                    //     &mut commands,
                    //     &mesh,
                    //     &materials,
                    // );
                }
                _ => {}
            }
        }
        retain
    });

    // TODO(#43): despawn stuff...
}

/// Generate all the star system entities.
pub fn spawn_systems(
    db_systems: &[DbSystem],
    systems: &Query<(Entity, &System)>,
    color_by: &Res<ColorBy>,
    commands: &mut Commands,
    mesh: &Res<SystemMesh>,
    materials: &Res<SystemMaterials>,
    time: &Res<Time<Real>>,
    fetched_at: &Instant,
) {
    let mut existing_systems: HashMap<i64, Entity> = systems
        .iter()
        .map(|(entity, system)| (system.address, entity))
        .collect();

    for db_system in db_systems {
        if let Some(enitity) = existing_systems.remove(&db_system.address) {
            debug!(
                "updating {} @ {:?}",
                db_system.address,
                fetched_at.duration_since(time.startup())
            );

            commands.entity(enitity).insert(System::from(db_system));
        } else {
            debug!(
                "spawning {} {:?}",
                db_system.address,
                fetched_at.duration_since(time.startup())
            );

            let system = System::from(db_system);
            let color_idx = match color_by.deref() {
                ColorBy::Allegiance => allegiance_color_idx(&system),
                ColorBy::Government => government_color_idx(&system),
                ColorBy::Security => security_color_idx(&system),
            };
            commands.spawn((
                PbrBundle {
                    transform: Transform {
                        translation: Vec3::new(
                            system.position[0],
                            system.position[1],
                            system.position[2],
                        ),
                        scale: Vec3::splat(1.),
                        ..default()
                    },
                    mesh: mesh.0.clone(),
                    material: materials.0[color_idx].clone(),
                    ..default()
                },
                system,
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
}

fn update(
    systems_query: Query<(Entity, Ref<System>)>,
    color_by: Res<ColorBy>,
    commands: Commands,
    mesh: Res<SystemMesh>,
    materials: Res<SystemMaterials>,
) {
    for (entity, system) in &systems_query {
        if system.is_changed() {
            info!("hit");
            // dbg!(materials);
            // commands
            //     .entity(entity)
            //     .insert()
        }
    }
}

fn init_mesh(
    mut mesh: ResMut<SystemMesh>,
    mut assets: ResMut<Assets<Mesh>>,
) {
    mesh.0 = assets.add(Sphere::new(1.).mesh().ico(3).unwrap());
}

fn init_materials(
    mut materials: ResMut<SystemMaterials>,
    mut assets: ResMut<Assets<StandardMaterial>>,
) {
    let colors = vec![
        Color::srgba(0., 1., 0., 0.4),       // Green
        Color::srgba(0., 1., 1., 0.4),       // Cyan
        Color::srgba(1., 0., 0., 0.4),       // Red
        Color::srgba(1., 0.5, 0., 0.4),      // Orange
        Color::srgba(1., 1., 0., 0.4),       // Yellow
        Color::srgba(0., 0., 1., 0.4),       // Blue
        Color::srgba(1., 0., 1., 0.4),       // Magenta
        Color::srgba(0.15, 0.15, 0.15, 0.3), // Grey
    ];

    let handles = colors
        .into_iter()
        .map(|color| {
            assets.add(StandardMaterial {
                base_color: color,
                alpha_mode: AlphaMode::Blend,
                emissive: LinearRgba::from(color.with_alpha(1.0)) * 10.,
                ..default()
            })
        })
        .collect();

    materials.0 = handles;
}

fn allegiance_color_idx(system: &System) -> usize {
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

fn government_color_idx(system: &System) -> usize {
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

fn security_color_idx(system: &System) -> usize {
    match system.security {
        Some(Security::High) => 5,        // Blue
        Some(Security::Medium) => 1,      // Cyan
        Some(Security::Low) => 0,         // Green
        Some(Security::Anarchy) => 2,     // Red
        Some(Security::None) | None => 7, // Grey
    }
}

impl From<&DbSystem> for System {
    fn from(system: &DbSystem) -> System {
        let pos = [
            system.position.unwrap().x as f32,
            system.position.unwrap().y as f32,
            system.position.unwrap().z as f32,
        ];

        System {
            address: system.address,
            position: pos,
            name: system.name.clone(),
            population: system.population,
            allegiance: system.allegiance,
            government: system.government,
            security: system.security,
            primary_economy: system.primary_economy,
            secondary_economy: system.secondary_economy,
            updated_at: system.updated_at,
        }
    }
}
