//! What it looks like to point at a system
//!
//! A system can be reached by its star or by its name, and both mean the
//! same thing, so both mark the system itself. Everything drawn for a system
//! then answers to one component and they cannot disagree: pointing at a
//! name rings its star, and pointing at a star lights its name.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::labels::{
    Label, depth, name_rect, screen_position, world_per_pixel,
};
use crate::systems::selection::Selected;
use crate::systems::spawn::Shell;
use bevy::camera::RenderTarget;
use bevy::math::DVec3;
use bevy::picking::backend::{HitData, PointerHits};
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::{PointerId, PointerLocation, PointerMap};
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};

pub fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (point_at, size_indicators, point_the_cursor)
            .in_set(MapSet::Present)
            .after(super::scale::size_by_distance)
            .after(super::scale::size_uniformly),
    );
    // Answers where the pointer is before anything asks, which is what a
    // picking backend is and where bevy expects one to run.
    app.add_systems(
        PreUpdate,
        hits.in_set(bevy::picking::PickingSystems::Backend),
    );
    // Reads where a star ended up rather than deciding it, so it waits for
    // the transforms to be worked out, as `labels::leaders` does.
    app.add_systems(PostUpdate, ring.after(TransformSystems::Propagate));
    app.add_observer(start_drag);
    app.add_observer(track_drag);
}

/// The color everything pointed at is drawn in
///
/// One color for the ring, the name and the line between them, so that a
/// system being pointed at reads as one thing rather than as three that
/// happen to have all changed at once.
pub const INDICATOR: Srgba = Srgba::new(1., 0.82, 0.35, 1.);

/// How much wider than its star a system's indicator is drawn
///
/// Far enough out to read as something around the star rather than as part
/// of it.
const INDICATOR_MARGIN: f32 = 1.5;

/// The smallest an indicator may be, as a radius in logical pixels
///
/// A star draws small enough at a distance that an indicator hugging it
/// would be a dot, and it is the indicator that has to be aimed at. Held in
/// pixels because that is what aiming is done in, so the target stays the
/// same size to the hand at every zoom.
const INDICATOR_MIN_RADIUS: f32 = 9.5;

/// How large a system's mark is, as a radius in logical pixels
///
/// The one answer behind both the ring drawn around a system and the area
/// that answers the pointer over it, so what can be clicked is exactly what
/// is shown.
///
/// Pixels, because that is what the mark is specified in and what aiming is
/// done in: [`INDICATOR_MIN_RADIUS`] is a distance to the hand rather than a
/// distance in the world. A ring is drawn in the world and so converts this
/// back at the moment of drawing, which is the only place the two units meet.
///
/// Held on the system itself. It once sat on an invisible sphere hung off the
/// system for a ray to be thrown at, and that sphere had to be a size in
/// metres, which is what a system is not: a mark is nine pixels across
/// whether the camera is a light year away or fifty thousand.
#[derive(Component, Default)]
pub struct Indicator(pub f32);

/// How long the pointer must rest on a system before it is asking about it
///
/// Crossing a system on the way to another is not pointing at it. A name
/// takes its place from the ones around it, so a claim staked in passing
/// takes away the very name that was being reached for.
const DWELL: f32 = 0.25;

/// The button that answers for whatever is under the pointer
///
/// Picking knows it as [`PointerButton::Primary`], and [`ButtonInput`] knows
/// it by where it sits, so the two names are put together here. What a press
/// selects and what a press clears are then the same button by construction
/// rather than by two files happening to agree.
pub const PRIMARY: MouseButton = MouseButton::Left;

/// How far a pointer may travel while pressed before it is dragging
///
/// The line between the two things a press can mean. Short of it the press
/// is a click, and answers with whatever is under the pointer; past it the
/// press is moving the map, and asks nothing of what it sweeps across.
///
/// Logical pixels, so it is the same distance to the hand whatever the
/// display density.
pub(super) const DRAG_THRESHOLD: f32 = 5.;

/// How far a pointer has travelled since it was last pressed
///
/// Kept on the pointer rather than in one shared slot, so a second pointer
/// cannot answer for the first and the measurement dies with the pointer.
#[derive(Component, Default)]
pub(super) struct DragDistance(pub f32);

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

/// A system the pointer is over
///
/// Carried by the system rather than by whatever the pointer actually
/// landed on, which may be its star or its name, so that anything wanting
/// to draw a system as pointed at asks one question.
#[derive(Component)]
pub struct PointedAt {
    /// When the pointer came to rest here, in seconds since startup
    since: f32,
}

impl PointedAt {
    /// Reached at `now`, in seconds since startup
    pub fn reached(now: f32) -> Self {
        Self { since: now }
    }

    /// Whether the pointer has rested here long enough to be asking
    ///
    /// What is shown of a system costs nothing and answers at once. What is
    /// shown of its neighbours does not, so that waits.
    pub fn settled(&self, now: f32) -> bool {
        now - self.since >= DWELL
    }
}

/// Mark the one system the pointer is on
///
/// Read from what is hovered rather than from coming and going, so that the
/// choice between two things under the pointer at once is made in one place
/// instead of falling to whichever event happened to arrive last.
///
/// A name wins over a star. Names are drawn over everything, so a name under
/// the pointer is what the eye says is being pointed at, whatever happens to
/// lie nearer the camera behind it.
///
/// Between stars, an admitted one wins, and only then the nearer of the two,
/// as it would if they blocked each other. A filter says which systems the
/// user is working with, and the rest are drawn faintly to be the space those
/// are read against; letting that space take the pointer off an admitted
/// system would have the background answer for the thing in front of it.
///
/// Reachable across [`super`], since what is drawn for a system is ordered
/// after it: a ring, a tint and a selection all answer what this decides, and
/// reading it a frame late is a mark trailing the pointer. No further than
/// that, though, which is as far as [`DragDistance`] goes and as far as
/// anything asks.
pub(super) fn point_at(
    hovered: Res<HoverMap>,
    time: Res<Time<Real>>,
    buttons: Res<ButtonInput<MouseButton>>,
    dragged: Query<&DragDistance>,
    names: Query<&ChildOf, With<Label>>,
    marked: Query<(), With<Indicator>>,
    filtered: Query<(), With<Filtered>>,
    pointed_at: Query<Entity, With<PointedAt>>,
    mut commands: Commands,
) {
    // A pointer dragging the map is holding the view, not asking about
    // what it happens to sweep over, and rings and names lighting up under
    // a turning map are noise.
    //
    // Past the same travel that tells a click from a drag, so the two agree:
    // within it the press is a click and what it is on stays pointed at,
    // and beyond it the press is a drag and answers nothing. Only while a
    // button is down, since the distance a pointer last travelled outlives
    // the press that measured it.
    let holding = buttons.any_pressed([
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
    ]);
    if holding && dragged.iter().any(|far| far.0 > DRAG_THRESHOLD) {
        for system in &pointed_at {
            commands.entity(system).remove::<PointedAt>();
        }
        return;
    }

    let mut named: Option<Entity> = None;
    // What is admitted, and only then what is nearest. A system the filters
    // admit is what the user asked to be looking at, so one lying behind
    // another they did not ask for is still the one they are pointing at:
    // the dim star in front is the background the filter is read against, and
    // background that answers the pointer is background in the way.
    let mut nearest: Option<(Entity, bool, f32)> = None;

    for hits in hovered.values() {
        for (entity, hit) in hits.iter() {
            if let Ok(name) = names.get(*entity) {
                named = Some(name.parent());
            } else if marked.contains(*entity) {
                let system = *entity;
                let dim = filtered.contains(system);
                let better = nearest.is_none_or(|(_, was_dim, depth)| {
                    (dim, hit.depth) < (was_dim, depth)
                });
                if better {
                    nearest = Some((system, dim, hit.depth));
                }
            }
        }
    }

    let wanted = named.or(nearest.map(|(system, ..)| system));
    let mut already = false;
    for system in &pointed_at {
        if Some(system) == wanted {
            // Left as it is, so that resting somewhere goes on counting
            // rather than starting over every frame.
            already = true;
        } else {
            commands.entity(system).remove::<PointedAt>();
        }
    }
    if let Some(system) = wanted
        && !already
    {
        commands.entity(system).insert(PointedAt::reached(time.elapsed_secs()));
    }
}

/// Work out how large each system's mark is, in pixels
///
/// One answer, read back off [`Indicator`] both by what draws the ring and by
/// what answers the pointer, so the mark and the area that catches cannot
/// come apart.
///
/// A shell is drawn in metres and holds a size that changes with the camera,
/// so it is measured into pixels here and the larger of that and the floor
/// wins. Where the shell is too small to aim at, which is nearly everywhere,
/// the floor is the whole of the answer.
pub fn size_indicators(
    camera: Query<(&OrbitCamera, &Camera)>,
    mut systems: Query<(&System, &Children, &mut Indicator)>,
    shells: Query<&Transform, With<Shell>>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (system, children, mut indicator) in &mut systems {
        let drawn = children
            .iter()
            .filter_map(|child| shells.get(child).ok())
            .map(|shell| shell.scale.x)
            .fold(0., f32::max);

        // A metre, which is as near as the camera may be pulled to anything.
        // What the floor is for is the sign rather than the distance.
        let into_view = depth(orbit, DVec3::from(system.position)).max(1.);
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, into_view);
        let shell = drawn * INDICATOR_MARGIN / per_pixel;

        indicator.0 = shell.max(INDICATOR_MIN_RADIUS);
    }
}

/// How wide a mark of `radius` pixels is out where its system stands
///
/// A ring is world geometry, so what is held in pixels is spoken back into
/// metres at the moment of drawing. The one place the two meet, and both
/// rings go through it, so neither can disagree with what is caught.
pub(super) fn drawn_radius(
    orbit: &OrbitCamera,
    cot_half_fov: f32,
    viewport: Vec2,
    position: DVec3,
    radius: f32,
) -> f32 {
    let into_view = depth(orbit, position).max(1.);
    radius * world_per_pixel(cot_half_fov, viewport.y, into_view)
}

/// Say what the pointer is over, measured on screen
///
/// A picking backend, which is to say it answers one question: which entities
/// lie under each pointer. Everything downstream of that — what is hovered,
/// what a click lands on, whether the cursor turns — is bevy's and is not
/// touched by how the answer is arrived at.
///
/// Arrived at here by projecting each system to the screen and comparing
/// distances in pixels, rather than by throwing a ray at a mesh standing in
/// for it. What is being aimed at is a mark nine pixels across; putting that
/// in the world only to ask a ray about it is a long way round, and at this
/// map's scale it does not survive the trip. A ray is built from a point at
/// the near plane and one at `near / f32::EPSILON`, and it is met by
/// inverting the target's transform and multiplying three of its lengths
/// together. In metres, with a system's mark drawn `1e15` across and stars
/// `1e17` apart, all three of those overflow a float. The map was briefly
/// unclickable in three separate ways for this reason.
///
/// Screen space has none of that: a projection and a distance, both in
/// pixels, both small. It is also where the sizes were decided to begin with.
///
/// Bodies are still met by rays, and rightly. They are drawn at their own
/// size, they have shapes worth hitting rather than a disc standing in for
/// them, and the numbers stay ordinary.
fn hits(
    pointers: Query<(&PointerId, &PointerLocation)>,
    window: Query<Entity, With<PrimaryWindow>>,
    cameras: Query<(Entity, &Camera, &RenderTarget, &OrbitCamera)>,
    systems: Query<(Entity, &System, &Indicator, &ViewVisibility)>,
    labels: Query<(Entity, &ChildOf), With<Label>>,
    mut hits: MessageWriter<PointerHits>,
) {
    let Ok((eye, camera, target, orbit)) = cameras.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    let caught = |camera: Entity, position: DVec3| HitData {
        camera,
        depth: depth(orbit, position),
        position: None,
        normal: None,
        extra: None,
    };

    for (pointer, at) in &pointers {
        let Some(at) = at.location() else { continue };
        if !at.is_in_viewport(camera, target, &window) {
            continue;
        }
        let at = at.position;
        let mut picks = Vec::new();

        for (entity, system, indicator, drawn) in &systems {
            // What is not drawn is not there to be pointed at. The spyglass
            // hides a system by writing its visibility, and one hidden is one
            // the user has said they are not looking at.
            if !drawn.get() {
                continue;
            }
            let position = DVec3::from(system.position);
            let Some(on_screen) =
                screen_position(orbit, cot_half_fov, viewport, position)
            else {
                continue;
            };
            if on_screen.distance(at) <= indicator.0 {
                picks.push((entity, caught(eye, position)));
            }
        }

        // A name is caught over the rectangle it was given, which is the same
        // one [`super::labels::choose_names`] laid out to keep names from
        // touching. So the areas that catch cannot overlap either, and a name
        // is clickable over exactly the room it was granted.
        for (label, child_of) in &labels {
            let Ok((_, system, ..)) = systems.get(child_of.parent()) else {
                continue;
            };
            let position = DVec3::from(system.position);
            let Some(on_screen) =
                screen_position(orbit, cot_half_fov, viewport, position)
            else {
                continue;
            };
            if name_rect(on_screen, &system.name).contains(at) {
                picks.push((label, caught(eye, position)));
            }
        }

        hits.write(PointerHits::new(*pointer, picks, camera.order as f32));
    }
}

/// Show a pointing cursor while anything worth clicking is under the pointer
///
/// Read from what is hovered rather than from coming and going, so that
/// moving straight from one system to the next cannot leave the cursor
/// behind whichever of the two events happens to arrive last.
pub fn point_the_cursor(
    hovered: Res<HoverMap>,
    clickable: Query<(), Or<(With<Indicator>, With<Label>)>>,
    window: Query<Entity, With<PrimaryWindow>>,
    mut commands: Commands,
) {
    let Ok(window) = window.single() else { return };
    let over_something = hovered
        .values()
        .flat_map(|hits| hits.keys())
        .any(|entity| clickable.contains(*entity));

    commands.entity(window).insert(if over_something {
        CursorIcon::System(SystemCursorIcon::Pointer)
    } else {
        CursorIcon::default()
    });
}

/// Ring the system the pointer is over
///
/// Drawn as a gizmo rather than a mesh because it lasts exactly as long as
/// the pointer rests there and follows the camera while it does.
///
/// Turned to face the camera, so it reads as a ring around the star rather
/// than as a hoop the star is sitting inside.
pub fn ring(
    mut gizmos: Gizmos,
    camera: Query<(&OrbitCamera, &Camera)>,
    // A selected system is already ringed, in its own color. Ringing it
    // again for being pointed at would draw one circle over the other and
    // read as the selection having been lost.
    pointed_at: Query<
        (&GlobalTransform, &System, &Indicator, Has<Filtered>),
        (With<PointedAt>, Without<Selected>),
    >,
    dim: Res<DimTo>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (at, system, indicator, filtered) in &pointed_at {
        // Drawn at what the pointer is tested against, so the ring is the
        // outline of the very area that catches.
        let radius = drawn_radius(
            orbit,
            cot_half_fov,
            viewport,
            DVec3::from(system.position),
            indicator.0,
        );

        gizmos.circle(
            Isometry3d::new(at.translation(), orbit.rotation),
            radius,
            dim.against(INDICATOR, filtered),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::entity::EntityHashMap;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::PointerId;

    /// Crossing a system does not count as pointing at it
    ///
    /// This is the whole reason for the wait. A name takes its place from
    /// the ones around it, so a system claiming one in passing takes it
    /// from whatever the pointer was on its way to.
    #[test]
    fn passing_over_a_system_is_not_pointing_at_it() {
        let brushed = PointedAt::reached(10.);

        assert!(!brushed.settled(10.), "counted the instant it was reached");
        assert!(
            !brushed.settled(10. + DWELL * 0.9),
            "counted before the pointer had settled"
        );
    }

    /// Resting on one does
    ///
    /// Measured a shade past the wait rather than exactly on it, since a
    /// subtraction of two floats does not land on the boundary and it is
    /// not the boundary that matters.
    #[test]
    fn resting_on_a_system_is_pointing_at_it() {
        let rested = PointedAt::reached(10.);

        assert!(rested.settled(10. + DWELL * 1.1));
        assert!(rested.settled(10. + DWELL * 10.));
    }

    /// A map with `stars` under the pointer at once, each `(dim, depth)`
    ///
    /// Answers which system each of them stands for, in the order given.
    /// Every star is a system carrying the child the pointer actually hits,
    /// as the sky is built, and the hits are what a picking backend would
    /// have reported for those children.
    fn pointed(stars: &[(bool, f32)]) -> (App, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ButtonInput<MouseButton>>();
        app.add_systems(Update, point_at);

        let mut over = EntityHashMap::default();
        let systems = stars
            .iter()
            .map(|(dim, depth)| {
                let system = app.world_mut().spawn(Indicator(0.)).id();
                if *dim {
                    app.world_mut().entity_mut(system).insert(Filtered);
                }
                over.insert(
                    system,
                    HitData {
                        camera: Entity::PLACEHOLDER,
                        depth: *depth,
                        position: None,
                        normal: None,
                        extra: None,
                    },
                );
                system
            })
            .collect();

        let mut hovered = HoverMap::default();
        hovered.insert(PointerId::Mouse, over);
        app.insert_resource(hovered);
        app.update();
        (app, systems)
    }

    /// Which of `stars` came out pointed at
    fn points_at(app: &App, stars: &[Entity]) -> Vec<bool> {
        stars
            .iter()
            .map(|star| app.world().entity(*star).contains::<PointedAt>())
            .collect()
    }

    /// An admitted system is pointed at through a dim one nearer the camera
    ///
    /// The dim ones are the space a filter is read against. One of them
    /// answering the pointer over an admitted system behind it would put the
    /// background in the way of the thing it is there to set off.
    #[test]
    fn an_admitted_system_is_pointed_at_through_a_dim_one() {
        let (app, stars) = pointed(&[(true, 1.), (false, 50.)]);

        assert_eq!(points_at(&app, &stars), vec![false, true]);
    }

    /// Between two admitted, the nearer is still the one pointed at
    ///
    /// Being admitted decides between a dim star and a lit one, and depth
    /// goes on deciding everything it decided before.
    #[test]
    fn between_two_admitted_the_nearer_is_pointed_at() {
        let (app, stars) = pointed(&[(false, 1.), (false, 50.)]);

        assert_eq!(points_at(&app, &stars), vec![true, false]);
    }

    /// And between two dim ones, likewise
    ///
    /// Neither of them admitted, so the rule that decided it before is the
    /// whole of what is left.
    #[test]
    fn between_two_dim_ones_the_nearer_is_pointed_at() {
        let (app, stars) = pointed(&[(true, 1.), (true, 50.)]);

        assert_eq!(points_at(&app, &stars), vec![true, false]);
    }

    /// A camera at the origin, looking down `-Z`
    fn looking() -> OrbitCamera {
        OrbitCamera { eye: DVec3::ZERO, rotation: Quat::IDENTITY, ..default() }
    }

    /// The cotangent of half the vertical field of view, for a default lens
    ///
    /// What `Camera::clip_from_view` answers, worked out here rather than
    /// built from a window so the sizing can be tested without one.
    fn cot_half_fov() -> f32 {
        1. / (PerspectiveProjection::default().fov / 2.).tan()
    }

    /// A mark is the same size to the hand however far off its system is
    ///
    /// The whole reason it is held in pixels. Nine pixels at the near end of
    /// the map and nine at the far end, where the two ends are fourteen
    /// orders of magnitude apart.
    #[test]
    fn a_mark_is_the_same_size_to_the_hand_at_every_zoom() {
        let camera = looking();
        let viewport = Vec2::new(1280., 720.);
        let mark = INDICATOR_MIN_RADIUS;

        // A metre off, and the width of the galaxy off.
        for away in [1f64, 1e6, 1e12, 1e18, 1e21] {
            let position = DVec3::new(0., 0., -away);
            let drawn =
                drawn_radius(&camera, cot_half_fov(), viewport, position, mark);
            // Back into pixels, the way the ring's size is arrived at.
            let per_pixel = world_per_pixel(
                cot_half_fov(),
                viewport.y,
                depth(&camera, position).max(1.),
            );

            assert!(
                (drawn / per_pixel - mark).abs() < mark * 1e-3,
                "a {mark} pixel mark {away}m off came back {} pixels",
                drawn / per_pixel
            );
        }
    }

    /// A mark stays a number at the far end of the map
    ///
    /// What the ray this replaced could not do. Aiming at a system meant
    /// inverting a transform scaled to metres and multiplying three of its
    /// lengths together, and at these distances all of that overflows a
    /// float and comes back as an infinity or as nothing at all.
    #[test]
    fn a_mark_across_the_galaxy_is_still_a_number() {
        let camera = looking();
        let viewport = Vec2::new(1280., 720.);

        // A hundred thousand light years, in the metres the map is drawn in.
        let across = DVec3::new(0., 0., -9.46e20);
        let drawn = drawn_radius(
            &camera,
            cot_half_fov(),
            viewport,
            across,
            INDICATOR_MIN_RADIUS,
        );

        assert!(drawn.is_finite(), "the mark came back {drawn}");
        assert!(drawn > 0., "the mark collapsed to {drawn}");
    }

    /// A system dead ahead lands in the middle of the screen
    ///
    /// Which is what the pointer is measured against, so a mark that is not
    /// where its system is drawn is a mark that catches somewhere else.
    #[test]
    fn a_system_ahead_is_marked_where_it_is_drawn() {
        let camera = looking();
        let viewport = Vec2::new(1280., 720.);
        let ahead = DVec3::new(0., 0., -9.46e17);

        let at = screen_position(&camera, cot_half_fov(), viewport, ahead)
            .expect("a system in front of the camera is on screen");

        assert!(
            at.distance(viewport / 2.) < 1e-3,
            "a system dead ahead landed at {at}"
        );
    }
}
