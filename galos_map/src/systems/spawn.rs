use crate::camera::MoveCamera;
use crate::schedule::MapSet;
use crate::systems::{
    System, fetch::FetchIndex, fetch::FetchTasks, route::Route,
    route::spawn::spawn_route, system_to_vec,
};
use bevy::light::NotShadowCaster;
use bevy::picking::mesh_picking::{MeshPickingPlugin, MeshPickingSettings};
use bevy::picking::pointer::PointerMap;
use bevy::prelude::*;
use bevy::tasks::block_on;
use bevy::tasks::futures_lite::future;
use elite_journal::{Allegiance, Government, system::Security};
use galos_db::systems::System as DbSystem;
use std::{collections::HashMap, ops::Deref, time::Instant};

pub fn plugin(app: &mut App) {
    app.add_plugins(MeshPickingPlugin);
    // Stars are the only thing worth clicking, so they are the only thing
    // ray cast against. The alternative is every mesh in the world, which
    // means the route line and a name label per nearby star. Requires
    // `MeshPickingCamera` on the camera and `Pickable` on each star.
    app.insert_resource(MeshPickingSettings {
        require_markers: true,
        ..default()
    });
    app.insert_resource(ColorBy::Allegiance);
    app.insert_resource(ShowNames(false));

    app.add_systems(Startup, (init_mesh, init_materials));
    app.add_systems(Update, spawn.in_set(MapSet::Populate));
    app.add_systems(Update, update.in_set(MapSet::Populate).before(spawn));

    app.add_observer(start_drag);
    app.add_observer(track_drag);
    app.add_observer(focus_camera_on_click);
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

/// How far a pointer may travel while pressed and still count as a click
///
/// Logical pixels, so the same physical slack whatever the display density.
const CLICK_SLOP: f32 = 5.;

/// How far a pointer has travelled since it was last pressed
///
/// Kept on the pointer rather than in one shared slot, so a second pointer
/// cannot answer for the first and the measurement dies with the pointer.
#[derive(Component, Default)]
struct DragDistance(f32);

/// Start measuring a pointer's travel when one of its buttons goes down
fn start_drag(
    press: On<Pointer<Press>>,
    pointers: Res<PointerMap>,
    mut commands: Commands,
) {
    let Some(pointer) = pointers.get_entity(press.pointer_id) else { return };
    commands.entity(pointer).insert(DragDistance(0.));
}

/// Keep the furthest a pointer has been from where it was pressed
fn track_drag(
    moved: On<Pointer<Drag>>,
    pointers: Res<PointerMap>,
    mut dragged: Query<&mut DragDistance>,
) {
    let Some(pointer) = pointers.get_entity(moved.pointer_id) else { return };
    let Ok(mut travelled) = dragged.get_mut(pointer) else { return };
    travelled.0 = travelled.0.max(moved.distance.length());
}

/// Focus the camera on clicked star systems
///
/// The left button orbits the camera as well as selecting, so an orbit that
/// happens to start and end on the same star has to be told apart from a
/// click on it. Picking calls it a drag after a single pixel of movement,
/// which is too eager to use by itself, so measure the travel instead.
//
// TODO: toggle system info as well.
// TODO: Spawn/despawn system label on Pointer<Over>/Pointer<Out>.
fn focus_camera_on_click(
    click: On<Pointer<Click>>,
    systems: Query<(), With<System>>,
    pointers: Res<PointerMap>,
    dragged: Query<&DragDistance>,
    mut move_camera_events: MessageWriter<MoveCamera>,
) {
    let travelled = pointers
        .get_entity(click.pointer_id)
        .and_then(|pointer| dragged.get(pointer).ok())
        .map_or(0., |travelled| travelled.0);
    if click.button != PointerButton::Primary || travelled > CLICK_SLOP {
        return;
    }
    if systems.contains(click.entity) {
        move_camera_events.write(MoveCamera { position: click.hit.position });
    }
}

/// Polls the tasks in `FetchTasks` and spawns entities for each of the
/// resulting star systems
pub fn spawn(
    systems_query: Query<(Entity, &System)>,
    route_query: Query<Entity, With<Route>>,
    color_by: Res<ColorBy>,
    mesh: Res<SystemMesh>,
    materials: Res<SystemMaterials>,
    time: Res<Time<Real>>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut material_assets: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut move_camera_events: MessageWriter<MoveCamera>,
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
                    if let Some(position) =
                        new_systems.first().and_then(system_to_vec)
                    {
                        move_camera_events
                            .write(MoveCamera { position: Some(position) });
                    }
                }
                _ => {}
            }

            match index {
                // TODO: Refactor into it's own system by spawning a new
                // Route component.
                FetchIndex::Route(..) => {
                    spawn_route(
                        &new_systems,
                        &route_query,
                        &mut commands,
                        &mut mesh_assets,
                        &mut material_assets,
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
        let Ok(system) = System::try_from(db_system) else {
            debug!("skipping {}, no position on record", db_system.address);
            continue;
        };

        if let Some(enitity) = existing_systems.remove(&db_system.address) {
            debug!(
                "updating {} @ {:?}",
                db_system.address,
                fetched_at.duration_since(time.startup())
            );

            commands.entity(enitity).insert(system);
        } else {
            debug!(
                "spawning {} {:?}",
                db_system.address,
                fetched_at.duration_since(time.startup())
            );

            commands.spawn((
                pbr_components(&system, color_by, mesh, materials),
                system,
                NotShadowCaster,
                Pickable::default(),
            ));
        }
    }
}

fn update(
    systems_query: Query<(Entity, Ref<System>)>,
    color_by: Res<ColorBy>,
    mesh: Res<SystemMesh>,
    materials: Res<SystemMaterials>,
    mut commands: Commands,
) {
    for (entity, system) in &systems_query {
        if system.is_changed() {
            commands
                .entity(entity)
                .insert(pbr_components(&system, &color_by, &mesh, &materials));
        } else if color_by.is_changed() {
            let color_idx = match color_by.deref() {
                ColorBy::Allegiance => allegiance_color_idx(&system),
                ColorBy::Government => government_color_idx(&system),
                ColorBy::Security => security_color_idx(&system),
            };
            commands
                .entity(entity)
                .insert(MeshMaterial3d(materials.0[color_idx].clone()));
        }
    }
}

fn pbr_components(
    system: &System,
    color_by: &Res<ColorBy>,
    mesh: &Res<SystemMesh>,
    materials: &Res<SystemMaterials>,
) -> (Mesh3d, MeshMaterial3d<StandardMaterial>, Transform) {
    let color_idx = match color_by.deref() {
        ColorBy::Allegiance => allegiance_color_idx(&system),
        ColorBy::Government => government_color_idx(&system),
        ColorBy::Security => security_color_idx(&system),
    };

    (
        Mesh3d(mesh.0.clone()),
        MeshMaterial3d(materials.0[color_idx].clone()),
        Transform {
            translation: Vec3::new(
                system.position[0],
                system.position[1],
                system.position[2],
            ),
            scale: Vec3::splat(1.),
            ..default()
        },
    )
}

fn init_mesh(mut assets: ResMut<Assets<Mesh>>, mut commands: Commands) {
    let handle = assets.add(Sphere::new(1.).mesh().ico(1).unwrap());
    commands.insert_resource(SystemMesh(handle));
}

fn init_materials(
    mut assets: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
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

    commands.insert_resource(SystemMaterials(handles));
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

/// A system the database has no coordinates for
///
/// The map is a map. A system it cannot place is not something it can draw.
pub struct Unplaceable;

impl TryFrom<&DbSystem> for System {
    type Error = Unplaceable;

    fn try_from(system: &DbSystem) -> Result<System, Unplaceable> {
        let position = system.position.ok_or(Unplaceable)?;
        let pos = [position.x as f32, position.y as f32, position.z as f32];

        Ok(System {
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
        })
    }
}
