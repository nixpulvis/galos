use crate::camera::MoveCamera;
use crate::schedule::MapSet;
use crate::space::Galaxy;
use crate::systems::{
    System,
    fetch::FetchIndex,
    fetch::FetchTasks,
    pointing::{
        DRAG_THRESHOLD, DragDistance, PointedAt, PointerTarget, UNFITTED_SCALE,
    },
    route::Route,
    route::spawn::spawn_route,
    selection::Selection,
    system_to_vec,
};
use bevy::diagnostic::FrameCount;
use bevy::light::NotShadowCaster;
use bevy::math::DVec3;
use bevy::picking::mesh_picking::{MeshPickingPlugin, MeshPickingSettings};
use bevy::picking::pointer::PointerMap;
use bevy::prelude::*;
use bevy::tasks::block_on;
use bevy::tasks::futures_lite::future;
use big_space::prelude::*;
use elite_journal::{Allegiance, Government, system::Security};
use galos_db::systems::System as DbSystem;
use std::{collections::HashMap, ops::Deref, time::Instant};

pub fn plugin(app: &mut App) {
    app.add_plugins(MeshPickingPlugin);
    // A star and its name are worth clicking; the route line is not. Marking
    // what is worth hitting keeps the ray cast off every mesh in the world.
    // Requires `MeshPickingCamera` on the camera and `Pickable` on each.
    app.insert_resource(MeshPickingSettings {
        require_markers: true,
        ..default()
    });
    app.insert_resource(ColorBy::Allegiance);
    app.insert_resource(ShowNames(false));

    app.add_systems(Startup, (init_mesh, init_materials));
    app.add_systems(Update, spawn.in_set(MapSet::Populate));
    app.add_systems(Update, update.in_set(MapSet::Populate).before(spawn));

    app.add_observer(focus_camera_on_click);
}

#[derive(Resource)]
pub struct SystemMesh(pub Handle<Mesh>);

#[derive(Resource)]
pub struct SystemMaterials(pub Vec<Handle<StandardMaterial>>);

/// A material that draws nothing, for what only has to be hit
#[derive(Resource)]
pub struct InvisibleMaterial(pub Handle<StandardMaterial>);
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

/// A star, drawn inside the [`System`] that holds it
///
/// A system is a place and a star is a thing in it. Only one is drawn per
/// system today, but a system can have several, and they will differ in
/// where they sit and how large they are.
///
/// [`super::scale`] writes a size onto this entity rather than onto the
/// system, because a star is drawn far larger than it is so as to stay
/// visible from light years away. Scale is inherited, so anything sharing an
/// entity with a star would be stretched by the same exaggeration; keeping
/// stars on children of their own leaves the system's transform meaning what
/// it says, and lets labels and, later, bodies sit at their true size.
#[derive(Component)]
pub struct Star;

/// Focus the camera on clicked star systems
///
/// The left button orbits the camera as well as selecting, so an orbit that
/// happens to start and end on the same star has to be told apart from a
/// click on it. Picking calls it a drag after a single pixel of movement,
/// which is too eager to use by itself, so measure the travel instead.
//
// TODO: Spawn/despawn system label on Pointer<Over>/Pointer<Out>.
fn focus_camera_on_click(
    click: On<Pointer<Click>>,
    pointed_at: Query<&System, With<PointedAt>>,
    pointers: Res<PointerMap>,
    dragged: Query<&DragDistance>,
    frame: Res<FrameCount>,
    mut answered: Local<Option<u32>>,
    mut move_camera_events: MessageWriter<MoveCamera>,
    mut selection: ResMut<Selection>,
) {
    let travelled = pointers
        .get_entity(click.pointer_id)
        .and_then(|pointer| dragged.get(pointer).ok())
        .map_or(0., |travelled| travelled.0);
    if click.button != PointerButton::Primary || travelled > DRAG_THRESHOLD {
        return;
    }

    // One click is reported once for everything under the pointer, and
    // since a star stopped blocking what lies behind it there are usually
    // several. They are all the same click, and there is only one place to
    // be sent, so the first of them answers for the rest.
    //
    // Counted by frame rather than by which of them is the one that won:
    // picking reports a click before `pointing` has looked at the frame it
    // belongs to, so anything recorded about the winner is a frame old, and
    // a pointer that has just moved would leave the click unanswered.
    if *answered == Some(frame.0) {
        return;
    }
    *answered = Some(frame.0);
    // Whatever is being pointed at is what a click is for, and `pointing`
    // has already settled which system that is, weighing a name over a star
    // lying nearer behind it. Asking it rather than working the hit out
    // again keeps the click on whatever the ring and the tint are on.
    //
    // The system, rather than where on it the ray landed. A hit is reported
    // in rendering coordinates, which are relative to whichever grid cell
    // the camera is in and so mean nothing once it has moved on.
    let Ok(system) = pointed_at.single() else { return };
    selection.set(system.clone());
    move_camera_events
        .write(MoveCamera { position: Some(DVec3::from(system.position)) });
}

/// Polls the tasks in `FetchTasks` and spawns entities for each of the
/// resulting star systems
pub fn spawn(
    systems_query: Query<(Entity, &System)>,
    route_query: Query<Entity, With<Route>>,
    galaxy: Res<Galaxy>,
    grids: Query<&Grid, With<BigSpace>>,
    color_by: Res<ColorBy>,
    mesh: Res<SystemMesh>,
    materials: Res<SystemMaterials>,
    invisible: Res<InvisibleMaterial>,
    time: Res<Time<Real>>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut material_assets: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut move_camera_events: MessageWriter<MoveCamera>,
    mut tasks: ResMut<FetchTasks>,
) {
    let Ok(grid) = grids.single() else { return };

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
                &galaxy,
                grid,
                &color_by,
                &mut commands,
                &mesh,
                &materials,
                &invisible,
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
                        &galaxy,
                        grid,
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

/// Create or refresh the entities for each row fetched
///
/// A [`System`] carries the database row and the grid placement, and is what
/// the rest of the map addresses. What is drawn hangs off it: a [`Star`]
/// today, and labels alongside. Nothing there inherits a size, so each is
/// drawn at whatever size suits it.
///
/// A row already on the map has its [`System`] replaced rather than being
/// respawned, which [`update`] then acts on.
pub fn spawn_systems(
    db_systems: &[DbSystem],
    systems: &Query<(Entity, &System)>,
    galaxy: &Res<Galaxy>,
    grid: &Grid,
    color_by: &Res<ColorBy>,
    commands: &mut Commands,
    mesh: &Res<SystemMesh>,
    materials: &Res<SystemMaterials>,
    invisible: &Res<InvisibleMaterial>,
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

            let drawn = star(&system, color_by, mesh, materials);
            let target = pointer_target(mesh, invisible);
            commands
                .spawn((
                    placement(&system, grid),
                    system,
                    // The star is what is shown or hidden; the mesh and any
                    // labels inherit that from it.
                    Visibility::default(),
                    // A star outside the galaxy's grid is not placed by it,
                    // and would be drawn wherever its bare transform happened
                    // to put it rather than where the cell says.
                    ChildOf(galaxy.0),
                ))
                .with_child(drawn)
                .with_child(target);
        }
    }
}

/// Carry a changed row, or a changed colour scheme, onto what is drawn
///
/// The two halves of a star are refreshed from different things. Its
/// placement follows the row it was built from, and its material follows
/// both the row and [`ColorBy`], so the second is checked against the star
/// each mesh hangs off rather than a copy of it.
fn update(
    systems_query: Query<(Entity, Ref<System>)>,
    stars: Query<(Entity, &ChildOf), With<Star>>,
    grids: Query<&Grid, With<BigSpace>>,
    color_by: Res<ColorBy>,
    materials: Res<SystemMaterials>,
    mut commands: Commands,
) {
    let Ok(grid) = grids.single() else { return };

    for (entity, system) in &systems_query {
        if system.is_changed() {
            commands.entity(entity).insert(placement(&system, grid));
        }
    }

    for (entity, child_of) in &stars {
        let Ok((_, system)) = systems_query.get(child_of.parent()) else {
            continue;
        };
        if system.is_changed() || color_by.is_changed() {
            let idx = color_idx(&system, &color_by);
            commands
                .entity(entity)
                .insert(MeshMaterial3d(materials.0[idx].clone()));
        }
    }
}

/// Where a star sits, as the galaxy's grid wants it
///
/// Split into the cell the position falls in and how far into that cell it
/// sits. The cell is an integer, so it stays exact however far out the system
/// is, and the transform left over is small enough to be carried without
/// losing anything.
///
/// The scale is left alone. This is the star's own transform, and everything
/// hung off it is placed relative to a light year meaning a light year.
fn placement(system: &System, grid: &Grid) -> (CellCoord, Transform) {
    let (cell, translation) =
        grid.translation_to_grid(DVec3::from(system.position));

    (cell, Transform::from_translation(translation))
}

/// What catches the pointer for a system
///
/// Sized each frame by [`super::pointing`] to match the ring it draws, so a
/// system is as easy to hit as the mark says it is. Sits at the system's own
/// position and draws nothing.
fn pointer_target(
    mesh: &Res<SystemMesh>,
    invisible: &Res<InvisibleMaterial>,
) -> impl Bundle {
    (
        PointerTarget,
        Mesh3d(mesh.0.clone()),
        MeshMaterial3d(invisible.0.clone()),
        // Fitted by `pointing::size_targets` before the first draw.
        Transform::from_scale(Vec3::splat(UNFITTED_SCALE)),
        NotShadowCaster,
        // Mesh picking requires markers, see `plugin`. A star does not
        // block what lies behind it, so a name drawn over one is reported
        // as well and `pointing` can weigh the two.
        Pickable { should_block_lower: false, is_hoverable: true },
    )
}

/// The one star a system is drawn with
///
/// Sits at the system's own position with an identity transform, since there
/// is nothing yet to tell one star of a system from another. [`super::scale`]
/// writes a size onto it each frame, and picking hits land here rather than
/// on the system, so [`focus_camera_on_click`] reads through to the parent.
fn star(
    system: &System,
    color_by: &Res<ColorBy>,
    mesh: &Res<SystemMesh>,
    materials: &Res<SystemMaterials>,
) -> impl Bundle {
    (
        Star,
        Mesh3d(mesh.0.clone()),
        MeshMaterial3d(materials.0[color_idx(system, color_by)].clone()),
        Transform::default(),
        NotShadowCaster,
    )
}

/// Which of the [`SystemMaterials`] a star is drawn in
fn color_idx(system: &System, color_by: &Res<ColorBy>) -> usize {
    match color_by.deref() {
        ColorBy::Allegiance => allegiance_color_idx(system),
        ColorBy::Government => government_color_idx(system),
        ColorBy::Security => security_color_idx(system),
    }
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
    commands.insert_resource(InvisibleMaterial(assets.add(StandardMaterial {
        base_color: Color::NONE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    })));
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
        let pos = [position.x, position.y, position.z];

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
