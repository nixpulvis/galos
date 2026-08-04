use crate::camera::MoveCamera;
use crate::schedule::MapSet;
use crate::search::Plot;
use crate::space::Galaxy;
use crate::systems::{
    System,
    fetch::FetchIndex,
    fetch::FetchTasks,
    filter::{self, Filtered, Filters},
    pointing::{
        DRAG_THRESHOLD, DragDistance, PRIMARY, PointedAt, PointerTarget,
        UNFITTED_SCALE,
    },
    route::Route,
    route::spawn::spawn_route,
    selection::Selection,
    system_to_vec,
};
use crate::ui::PointerOverUi;
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

    app.add_observer(select_on_click);
    // Answers what is pointed at this frame, which `point_at` decides.
    app.add_systems(
        Update,
        fly_on_double_click
            .in_set(MapSet::Present)
            .after(super::pointing::point_at),
    );
}

#[derive(Resource)]
pub struct SystemMesh(pub Handle<Mesh>);

/// What a star is drawn in, at full strength and dimmed
///
/// Two sets of the same colours rather than one recoloured per star, because
/// the colour lives on a shared asset. A star moves between the sets by
/// swapping which handle it points at, which repaints only that star, and the
/// two sets are built once, since how faintly to draw is one number rather
/// than a setting.
#[derive(Resource)]
pub struct SystemMaterials {
    /// One per colour, indexed as [`hue`] answers
    bright: Vec<Handle<StandardMaterial>>,
    /// The same colours, at [`filter::DIMMED`] of full
    dim: Vec<Handle<StandardMaterial>>,
}

impl SystemMaterials {
    /// The handle for `hue`, at the strength `dimmed` asks for
    fn get(&self, hue: Hue, dimmed: bool) -> Handle<StandardMaterial> {
        let set = if dimmed { &self.dim } else { &self.bright };
        set[hue as usize].clone()
    }
}

/// The colours a star may be drawn in
///
/// Named rather than numbered, so that a scheme below says which colour it
/// means. The two material sets are laid out in [`Hue::ALL`] order and
/// indexed by the hue itself, so there is one list of colours rather than a
/// list and a set of numbers agreeing with it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hue {
    Green,
    Cyan,
    Red,
    Orange,
    Yellow,
    Blue,
    Magenta,
    Grey,
}

impl Hue {
    /// Every hue, in the order the material sets hold them
    const ALL: [Hue; 8] = [
        Hue::Green,
        Hue::Cyan,
        Hue::Red,
        Hue::Orange,
        Hue::Yellow,
        Hue::Blue,
        Hue::Magenta,
        Hue::Grey,
    ];

    /// What the hue is painted in
    ///
    /// Alpha is part of it: a star is drawn as a translucent ball with a glow
    /// over it, and the grey a system with nothing on record comes out is
    /// fainter than the rest so that an unknown system does not read as a
    /// finding.
    const fn color(self) -> Color {
        match self {
            Hue::Green => Color::srgba(0., 1., 0., 0.4),
            Hue::Cyan => Color::srgba(0., 1., 1., 0.4),
            Hue::Red => Color::srgba(1., 0., 0., 0.4),
            Hue::Orange => Color::srgba(1., 0.5, 0., 0.4),
            Hue::Yellow => Color::srgba(1., 1., 0., 0.4),
            Hue::Blue => Color::srgba(0., 0., 1., 0.4),
            Hue::Magenta => Color::srgba(1., 0., 1., 0.4),
            Hue::Grey => Color::srgba(0.15, 0.15, 0.15, 0.3),
        }
    }
}

/// How a star is painted in `color`, at `strength` of full
///
/// Both the fill and the glow are scaled, since a star drawn dim but glowing
/// as brightly as the rest reads as no dimmer at all: the glow is most of
/// what is seen of a star at any distance.
fn star_material(color: Color, strength: f32) -> StandardMaterial {
    let faded = color.with_alpha(color.alpha() * strength);
    StandardMaterial {
        base_color: faded,
        alpha_mode: AlphaMode::Blend,
        emissive: LinearRgba::from(color.with_alpha(1.)) * 10. * strength,
        ..default()
    }
}

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

/// Pick out a clicked star system
///
/// Clicking says which system the user means and nothing more. Where the
/// camera goes is asked for separately, by the row that names what is picked
/// out, so that a system can be pointed out from wherever the user happens
/// to be looking without the map moving out from under them.
///
/// The left button orbits the camera as well as selecting, so an orbit that
/// happens to start and end on the same star has to be told apart from a
/// click on it. Picking calls it a drag after a single pixel of movement,
/// which is too eager to use by itself, so measure the travel instead.
//
// TODO: Spawn/despawn system label on Pointer<Over>/Pointer<Out>.
fn select_on_click(
    click: On<Pointer<Click>>,
    pointed_at: Query<&System, With<PointedAt>>,
    pointers: Res<PointerMap>,
    dragged: Query<&DragDistance>,
    frame: Res<FrameCount>,
    mut answered: Local<Option<u32>>,
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
    // several. They are all the same click, and only one system can be
    // picked out, so the first of them answers for the rest.
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
    let Ok(system) = pointed_at.single() else { return };
    selection.set(system.clone());
}

/// How long a second click may take to arrive and still make a double
///
/// Seconds. Long enough to be reached without hurrying, short enough that
/// two deliberate clicks on the same system are not read as one gesture.
const DOUBLE_CLICK: f32 = 0.4;

/// Fly the camera to a system the user double clicks
///
/// One click says which system is meant and a second says to go there, so
/// the map can be pointed at from where the user is without moving, and
/// travelled with the same hand when they do want to move.
///
/// A click is weighed by the same three questions everywhere on the map: the
/// primary button, travel short enough to be a click rather than a drag, and
/// the pointer's own business rather than the UI's. What is asked on top of
/// those is that the click before it landed on the same system, recently.
fn fly_on_double_click(
    buttons: Res<ButtonInput<MouseButton>>,
    over_ui: Res<PointerOverUi>,
    dragged: Query<&DragDistance>,
    pointed_at: Query<&System, With<PointedAt>>,
    time: Res<Time<Real>>,
    mut last: Local<LastClick>,
    mut camera: MessageWriter<MoveCamera>,
) {
    if !buttons.just_released(PRIMARY) || over_ui.0 {
        return;
    }
    if dragged.iter().any(|travelled| travelled.0 > DRAG_THRESHOLD) {
        return;
    }
    let Ok(system) = pointed_at.single() else { return };

    if last.doubled(system.address, time.elapsed_secs()) {
        camera
            .write(MoveCamera { position: Some(DVec3::from(system.position)) });
    }
}

/// The click a second one would be counted against
///
/// Which system as well as when, so that two clicks a moment apart on two
/// different stars are two answers rather than one gesture. Stars stand
/// close together on screen at any distance, and picking one out after
/// another is an ordinary thing to do quickly.
#[derive(Default)]
struct LastClick(Option<(i64, f32)>);

impl LastClick {
    /// Whether a click on `address` at `now` is the second of a pair
    ///
    /// A double is spent as soon as it is answered, so a third click starts
    /// counting afresh rather than making a second pair with the second.
    fn doubled(&mut self, address: i64, now: f32) -> bool {
        let doubled = matches!(self.0, Some((clicked, when))
            if clicked == address && now - when <= DOUBLE_CLICK);
        self.0 = if doubled { None } else { Some((address, now)) };
        doubled
    }
}

/// Polls the tasks in `FetchTasks` and spawns entities for each of the
/// resulting star systems
pub fn spawn(
    systems_query: Query<(Entity, &System)>,
    route_query: Query<Entity, With<Route>>,
    galaxy: Res<Galaxy>,
    grids: Query<&Grid, With<BigSpace>>,
    color_by: Res<ColorBy>,
    filters: Res<Filters>,
    mesh: Res<SystemMesh>,
    materials: Res<SystemMaterials>,
    invisible: Res<InvisibleMaterial>,
    time: Res<Time<Real>>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut material_assets: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut move_camera_events: MessageWriter<MoveCamera>,
    mut tasks: ResMut<FetchTasks>,
    mut plot: ResMut<Plot>,
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
                &filters,
                &mut commands,
                &mesh,
                &materials,
                &invisible,
                &time,
                fetched_at,
            );

            if let FetchIndex::Route(..) = index
                && let Some(position) =
                    new_systems.first().and_then(system_to_vec)
            {
                move_camera_events.write(MoveCamera { position: Some(position) });
            }

            match index {
                // TODO: Refactor into it's own system by spawning a new
                // Route component.
                FetchIndex::Route(start, end, range) => {
                    // A route is a line between systems, so one system is
                    // no route. Coming back with nothing is how the
                    // database says it could not get from one end to the
                    // other in jumps that long, and nothing drawn is the
                    // same nothing as a route still being worked out.
                    //
                    // Only ever an answer to a route still being waited on.
                    // A name that resolved to nothing is already said, and
                    // said more exactly than this could: the route was
                    // fetched anyway, and it comes back empty for the same
                    // reason, so without this the better answer is talked
                    // over a moment after it arrives.
                    if *plot == Plot::Working {
                        *plot = if new_systems.len() < 2 {
                            Plot::Trouble(format!(
                                "No route from {start} to {end} at {range} Ly"
                            ))
                        } else {
                            Plot::Nothing
                        };
                    }
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
///
/// The filters are asked here rather than left to [`filter::mark`], so that a
/// system arrives already marked and already drawn at the strength it should
/// be. A mark applied by a command lands at the next sync point, by which
/// time the star has been drawn once at full strength.
pub fn spawn_systems(
    db_systems: &[DbSystem],
    systems: &Query<(Entity, &System)>,
    galaxy: &Res<Galaxy>,
    grid: &Grid,
    color_by: &Res<ColorBy>,
    filters: &Res<Filters>,
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

            // Asked here as well as in `filter::mark`, since a mark applied
            // by a command lands at the next sync point and the star would
            // be drawn once at full strength before it arrived.
            let excluded = !filters.admit(&system);
            let drawn = star(&system, color_by, mesh, materials, excluded);
            let target = pointer_target(mesh, invisible);
            let mut spawned = commands.spawn((
                placement(&system, grid),
                system,
                // The star is what is shown or hidden; the mesh and any
                // labels inherit that from it.
                Visibility::default(),
                // A star outside the galaxy's grid is not placed by it,
                // and would be drawn wherever its bare transform happened
                // to put it rather than where the cell says.
                ChildOf(galaxy.0),
            ));
            if excluded {
                spawned.insert(Filtered);
            }
            spawned.with_child(drawn).with_child(target);
        }
    }
}

/// Carry a changed row, a changed colour scheme or a changed filter onto what
/// is drawn
///
/// The two halves of a star are refreshed from different things. Its
/// placement follows the row it was built from, and its material follows the
/// row, [`ColorBy`] and whether the filters exclude it, so the second is
/// checked against the star each mesh hangs off rather than a copy of it.
///
/// The material is decided afresh each frame and written only where it
/// differs, as [`super::labels::tint_marked_names`] does, rather than being
/// guarded by what has changed. A mark is applied by a command and so lands a
/// frame after the filter that asked for it, which leaves nothing that both
/// runs after the mark and can still see what changed.
fn update(
    systems_query: Query<(Entity, Ref<System>, Has<Filtered>)>,
    mut stars: Query<
        (&ChildOf, &mut MeshMaterial3d<StandardMaterial>),
        With<Star>,
    >,
    grids: Query<&Grid, With<BigSpace>>,
    color_by: Res<ColorBy>,
    materials: Res<SystemMaterials>,
    mut commands: Commands,
) {
    let Ok(grid) = grids.single() else { return };

    for (entity, system, _) in &systems_query {
        if system.is_changed() {
            commands.entity(entity).insert(placement(&system, grid));
        }
    }

    for (child_of, mut material) in &mut stars {
        let Ok((_, system, filtered)) = systems_query.get(child_of.parent())
        else {
            continue;
        };
        let wanted = materials.get(hue(&system, &color_by), filtered);
        if material.0 != wanted {
            material.0 = wanted;
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
/// on the system, so [`fly_on_double_click`] reads through to the parent.
fn star(
    system: &System,
    color_by: &Res<ColorBy>,
    mesh: &Res<SystemMesh>,
    materials: &Res<SystemMaterials>,
    dimmed: bool,
) -> impl Bundle {
    (
        Star,
        Mesh3d(mesh.0.clone()),
        MeshMaterial3d(materials.get(hue(system, color_by), dimmed)),
        Transform::default(),
        NotShadowCaster,
    )
}

/// Which colour a star is drawn in
fn hue(system: &System, color_by: &Res<ColorBy>) -> Hue {
    match color_by.deref() {
        ColorBy::Allegiance => allegiance_hue(system),
        ColorBy::Government => government_hue(system),
        ColorBy::Security => security_hue(system),
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
    let mut set = |strength: f32| {
        Hue::ALL
            .into_iter()
            .map(|hue| assets.add(star_material(hue.color(), strength)))
            .collect()
    };

    commands.insert_resource(SystemMaterials {
        bright: set(1.),
        dim: set(filter::DIMMED),
    });
    commands.insert_resource(InvisibleMaterial(assets.add(StandardMaterial {
        base_color: Color::NONE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    })));
}

fn allegiance_hue(system: &System) -> Hue {
    match system.allegiance {
        Some(Allegiance::Alliance) => Hue::Green,
        Some(Allegiance::Empire) => Hue::Cyan,
        Some(Allegiance::Federation) => Hue::Red,
        Some(Allegiance::PilotsFederation) => Hue::Orange,
        Some(Allegiance::PlayerPilots) => Hue::Yellow,
        Some(Allegiance::Independent) => Hue::Yellow,
        Some(Allegiance::Guardian) => Hue::Blue,
        Some(Allegiance::Thargoid) => Hue::Magenta,
        Some(Allegiance::None) | None => Hue::Grey,
    }
}

fn government_hue(system: &System) -> Hue {
    match system.government {
        Some(Government::Anarchy) => Hue::Yellow,
        Some(Government::Carrier) => Hue::Green,
        Some(Government::Communism) => Hue::Red,
        Some(Government::Confederacy) => Hue::Red,
        Some(Government::Cooperative) => Hue::Orange,
        Some(Government::Corporate) => Hue::Cyan,
        Some(Government::Democracy) => Hue::Blue,
        Some(Government::Dictatorship) => Hue::Red,
        Some(Government::Engineer) => Hue::Magenta,
        Some(Government::Feudal) => Hue::Red,
        Some(Government::Patronage) => Hue::Red,
        Some(Government::Prison) => Hue::Red,
        Some(Government::PrisonColony) => Hue::Red,
        Some(Government::Theocracy) => Hue::Blue,
        Some(Government::None) | None => Hue::Grey,
    }
}

fn security_hue(system: &System) -> Hue {
    match system.security {
        Some(Security::High) => Hue::Blue,
        Some(Security::Medium) => Hue::Cyan,
        Some(Security::Low) => Hue::Green,
        Some(Security::Anarchy) => Hue::Red,
        Some(Security::None) | None => Hue::Grey,
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
            factions: system.factions.clone(),
            updated_at: system.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One click on its own opens nothing
    #[test]
    fn a_single_click_is_not_a_double() {
        let mut last = LastClick::default();
        assert!(!last.doubled(1, 0.));
    }

    /// Two clicks in quick succession on one system make a double
    #[test]
    fn two_quick_clicks_on_one_system_are_a_double() {
        let mut last = LastClick::default();
        last.doubled(1, 0.);
        assert!(last.doubled(1, DOUBLE_CLICK));
    }

    /// Two clicks far enough apart are two singles
    #[test]
    fn two_slow_clicks_are_not_a_double() {
        let mut last = LastClick::default();
        last.doubled(1, 0.);
        assert!(!last.doubled(1, DOUBLE_CLICK + 0.01));
    }

    /// Two clicks on different systems are two singles
    ///
    /// Clicking a system flies the camera to it, so the star that lands
    /// under the pointer next is a different one often enough for this to be
    /// the usual way an accidental double would happen.
    #[test]
    fn two_clicks_on_different_systems_are_not_a_double() {
        let mut last = LastClick::default();
        last.doubled(1, 0.);
        assert!(!last.doubled(2, 0.1));
    }

    /// A third quick click does not make a second double
    ///
    /// Otherwise a held-down finger would open a panel per click, and there
    /// would be no way to close one without it coming straight back.
    #[test]
    fn a_third_quick_click_is_not_a_double() {
        let mut last = LastClick::default();
        last.doubled(1, 0.);
        assert!(last.doubled(1, 0.1));
        assert!(!last.doubled(1, 0.2));
    }

    /// A slow click after a double starts a fresh pair
    #[test]
    fn counting_starts_again_after_a_double() {
        let mut last = LastClick::default();
        last.doubled(1, 0.);
        last.doubled(1, 0.1);
        assert!(!last.doubled(1, 0.2));
        assert!(last.doubled(1, 0.3));
    }
}
