use super::{
    spawn_route, system_to_vec, FetchIndex, FetchTasks, Route, System,
};
use crate::camera::MoveCamera;
use bevy::pbr::NotShadowCaster;
use bevy::prelude::*;
use bevy::tasks::block_on;
use bevy::tasks::futures_lite::future;
use bevy_mod_picking::prelude::*;
use elite_journal::{system::Security, Allegiance, Government};
use galos_db::systems::System as DbSystem;
use std::{collections::HashMap, ops::Deref};

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
    mut move_camera_events: EventWriter<MoveCamera>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut tasks: ResMut<FetchTasks>,
) {
    tasks.fetched.retain(|index, task| {
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
                &mut meshes,
                &mut materials,
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
                    spawn_route(
                        &new_systems,
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

    // TODO(#43): despawn stuff...
}

/// Generate all the star system entities.
pub(crate) fn spawn_systems(
    new_systems: &[DbSystem],
    systems_query: &Query<(Entity, &System)>,
    color_by: &Res<ColorBy>,
    commands: &mut Commands,
    mesh_asset: &mut ResMut<Assets<Mesh>>,
    material_assets: &mut ResMut<Assets<StandardMaterial>>,
) {
    let mut existing_systems: HashMap<i64, Entity> = systems_query
        .iter()
        .map(|(entity, system)| (system.address, entity))
        .collect();

    let mesh = init_meshes(mesh_asset);
    let materials = init_materials(material_assets);

    for new_system in new_systems {
        let color_idx = match color_by.deref() {
            ColorBy::Allegiance => allegiance_color_idx(&new_system),
            ColorBy::Government => government_color_idx(&new_system),
            ColorBy::Security => security_color_idx(&new_system),
        };
        if let Some(_enitity) = existing_systems.remove(&new_system.address) {
            // TODO(#42): update
        } else {
            commands.spawn((
                PbrBundle {
                    transform: Transform {
                        translation: Vec3::new(
                            new_system.position.unwrap().x as f32,
                            new_system.position.unwrap().y as f32,
                            new_system.position.unwrap().z as f32,
                        ),
                        scale: Vec3::splat(1.),
                        ..default()
                    },
                    mesh: mesh.clone(),
                    material: materials[color_idx].clone(),
                    ..default()
                },
                System {
                    address: new_system.address,
                    name: new_system.name.clone(),
                    population: new_system.population,
                    allegiance: new_system.allegiance,
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
}

// TODO(#42): update ColorBy

#[derive(Event)]
pub struct Despawn;

pub fn despawn(
    mut commands: Commands,
    systems: Query<(Entity, &System)>,
    mut events: EventReader<Despawn>,
) {
    for _ in events.read() {
        for (entity, _) in systems.iter() {
            commands.entity(entity).despawn_recursive();
        }
    }
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
        Some(Security::High) => 5,        // Blue
        Some(Security::Medium) => 1,      // Cyan
        Some(Security::Low) => 0,         // Green
        Some(Security::Anarchy) => 2,     // Red
        Some(Security::None) | None => 7, // Grey
    }
}
