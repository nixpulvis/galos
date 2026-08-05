//! What it looks like to point at a system
//!
//! A system can be reached by its star or by its name, and both mean the
//! same thing, so both mark the system itself. Everything drawn for a system
//! then answers to one component and they cannot disagree: pointing at a
//! name rings its star, and pointing at a star lights its name.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::filter::{self, Filtered};
use crate::systems::labels::{Label, NameBox, depth, world_per_pixel};
use crate::systems::selection::Selected;
use crate::systems::spawn::Shell;
use bevy::math::DVec3;
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerMap;
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};

pub fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (point_at, size_targets, point_the_cursor)
            .in_set(MapSet::Present)
            .after(super::scale::size_by_distance)
            .after(super::scale::size_uniformly),
    );
    // Reads where a star ended up rather than deciding it, so it waits for
    // the transforms to be worked out, as `labels::leaders` does.
    app.add_systems(PostUpdate, ring.after(TransformSystems::Propagate));
    app.add_observer(start_drag);
    app.add_observer(track_drag);
}

/// The colour everything pointed at is drawn in
///
/// One colour for the ring, the name and the line between them, so that a
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

/// What catches the pointer for a system
///
/// The same shape and size as the ring drawn around it, so that what can be
/// clicked is exactly what is shown. A star is drawn far too small to aim
/// at, and a target larger than the mark would be as misleading as one
/// smaller.
///
/// A sphere rather than a disc facing the camera, so that it presents the
/// same circle from wherever it is seen and nothing has to turn it.
/// Invisible, being drawn in a fully transparent material, since picking
/// only considers what is being drawn.
#[derive(Component)]
pub struct PointerTarget;

/// How long the pointer must rest on a system before it is asking about it
///
/// Crossing a system on the way to another is not pointing at it. A name
/// takes its place from the ones around it, so a claim staked in passing
/// takes away the very name that was being reached for.
const DWELL: f32 = 0.25;

/// The scale a thing that catches the pointer stands at before it is fitted
///
/// Small enough to catch nothing, since it does not yet stand for anything.
/// Not zero: a ray is put into the space of what it might hit by inverting
/// that thing's transform, and a zero scale has no inverse.
pub(super) const UNFITTED_SCALE: f32 = 1e-6;

/// The button that answers for whatever is under the pointer
///
/// Picking knows it as [`PointerButton::Primary`], and [`ButtonInput`] knows
/// it by where it sits, so the two names are put together here. What a press
/// selects and what a press clears are then the same button by construction
/// rather than by two files happening to agree.
pub(super) const PRIMARY: MouseButton = MouseButton::Left;

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
/// lie nearer the camera behind it. Between stars the nearest wins, as it
/// would if they blocked each other.
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
    boxes: Query<&ChildOf, With<NameBox>>,
    names: Query<&ChildOf, With<Label>>,
    targets: Query<&ChildOf, With<PointerTarget>>,
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
    let mut nearest: Option<(Entity, f32)> = None;

    for hits in hovered.values() {
        for (entity, hit) in hits.iter() {
            if let Ok(hit_box) = boxes.get(*entity) {
                if let Ok(name) = names.get(hit_box.parent()) {
                    named = Some(name.parent());
                }
            } else if let Ok(target) = targets.get(*entity) {
                if nearest.is_none_or(|(_, depth)| hit.depth < depth) {
                    nearest = Some((target.parent(), hit.depth));
                }
            }
        }
    }

    let wanted = named.or(nearest.map(|(system, _)| system));
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

/// Fit each system's target to what its indicator will be drawn at
///
/// One answer for both, worked out here and read back by [`ring`], so that
/// the mark and the area catching the pointer cannot come apart.
pub fn size_targets(
    camera: Query<(&OrbitCamera, &Camera)>,
    systems: Query<(&System, &Children)>,
    stars: Query<&Transform, With<Shell>>,
    mut targets: Query<&mut Transform, (With<PointerTarget>, Without<Shell>)>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (system, children) in &systems {
        let drawn = children
            .iter()
            .filter_map(|child| stars.get(child).ok())
            .map(|star| star.scale.x)
            .fold(0., f32::max);

        // A metre, which is as near as the camera may be pulled to anything.
        // What the floor is for is the sign rather than the distance.
        let into_view = depth(orbit, DVec3::from(system.position)).max(1.);
        let smallest = INDICATOR_MIN_RADIUS
            * world_per_pixel(cot_half_fov, viewport.y, into_view);
        let radius = (drawn * INDICATOR_MARGIN).max(smallest);

        for child in children.iter() {
            if let Ok(mut target) = targets.get_mut(child) {
                target.scale = Vec3::splat(radius);
            }
        }
    }
}

/// Show a pointing cursor while anything worth clicking is under the pointer
///
/// Read from what is hovered rather than from coming and going, so that
/// moving straight from one system to the next cannot leave the cursor
/// behind whichever of the two events happens to arrive last.
pub fn point_the_cursor(
    hovered: Res<HoverMap>,
    clickable: Query<(), Or<(With<PointerTarget>, With<NameBox>)>>,
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
    camera: Query<&OrbitCamera>,
    // A selected system is already ringed, in its own colour. Ringing it
    // again for being pointed at would draw one circle over the other and
    // read as the selection having been lost.
    pointed_at: Query<
        (&GlobalTransform, &Children, Has<Filtered>),
        (With<System>, With<PointedAt>, Without<Selected>),
    >,
    targets: Query<&GlobalTransform, With<PointerTarget>>,
) {
    let Ok(camera) = camera.single() else { return };

    for (system, children, filtered) in &pointed_at {
        // Drawn at whatever the target was fitted to, so the ring is the
        // outline of the very thing the pointer is tested against.
        let Some(radius) = children
            .iter()
            .filter_map(|child| targets.get(child).ok())
            .map(|target| target.scale().x)
            .next()
        else {
            continue;
        };

        gizmos.circle(
            Isometry3d::new(system.translation(), camera.rotation),
            radius,
            filter::dim(INDICATOR, filtered),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
