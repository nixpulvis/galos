//! What it looks like to point at a system
//!
//! A system can be reached by its star or by its name, and both mean the
//! same thing, so both mark the system itself. Everything drawn for a system
//! then answers to one component and they cannot disagree: pointing at a
//! name rings its star, and pointing at a star lights its name.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::labels::{Label, NameBox};
use crate::systems::spawn::Star;
use bevy::picking::hover::HoverMap;
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};

pub fn plugin(app: &mut App) {
    app.add_systems(Update, point_the_cursor.in_set(MapSet::Present));
    // Reads where a star ended up rather than deciding it, so it waits for
    // the transforms to be worked out, as `labels::leaders` does.
    app.add_systems(PostUpdate, ring.after(TransformSystems::Propagate));
    app.add_observer(point_at);
    app.add_observer(look_away);
}

/// The colour everything pointed at is drawn in
///
/// One colour for the ring, the name and the line between them, so that a
/// system being pointed at reads as one thing rather than as three that
/// happen to have all changed at once.
pub const INDICATOR: Srgba = Srgba::new(1., 0.82, 0.35, 1.);

/// How much wider than its star a system's ring is drawn
///
/// Far enough out to read as something around the star rather than as part
/// of it.
const RING_MARGIN: f32 = 2.5;

/// The smallest a ring may be drawn, as a fraction of the orbit radius
///
/// A star is drawn small enough at a distance that a ring hugging it would
/// be a dot. This holds it to something the eye can find, and being a
/// fraction of how far out the camera is, it holds it there at every zoom.
const RING_FLOOR: f32 = 0.012;

/// A system the pointer is over
///
/// Carried by the system rather than by whatever the pointer actually
/// landed on, which may be its star or its name, so that anything wanting
/// to draw a system as pointed at asks one question.
#[derive(Component)]
pub struct PointedAt;

/// Mark the system behind whatever the pointer has come over
fn point_at(
    over: On<Pointer<Over>>,
    stars: Query<&ChildOf, With<Star>>,
    boxes: Query<&ChildOf, With<NameBox>>,
    names: Query<&ChildOf, With<Label>>,
    mut commands: Commands,
) {
    if let Some(system) = pointed_system(over.entity, &stars, &boxes, &names) {
        commands.entity(system).insert(PointedAt);
    }
}

/// And unmark it once the pointer has left
fn look_away(
    out: On<Pointer<Out>>,
    stars: Query<&ChildOf, With<Star>>,
    boxes: Query<&ChildOf, With<NameBox>>,
    names: Query<&ChildOf, With<Label>>,
    mut commands: Commands,
) {
    if let Some(system) = pointed_system(out.entity, &stars, &boxes, &names) {
        commands.entity(system).remove::<PointedAt>();
    }
}

/// Which system the pointer is really on, given what it landed on
///
/// A star hangs off its system. The box catching a name hangs off the name,
/// which hangs off the system in turn, so it is one step further up.
fn pointed_system(
    hit: Entity,
    stars: &Query<&ChildOf, With<Star>>,
    boxes: &Query<&ChildOf, With<NameBox>>,
    names: &Query<&ChildOf, With<Label>>,
) -> Option<Entity> {
    if let Ok(star) = stars.get(hit) {
        return Some(star.parent());
    }
    let name = boxes.get(hit).ok()?;
    Some(names.get(name.parent()).ok()?.parent())
}

/// Show a pointing cursor while anything worth clicking is under the pointer
///
/// Read from what is hovered rather than from coming and going, so that
/// moving straight from one system to the next cannot leave the cursor
/// behind whichever of the two events happens to arrive last.
pub fn point_the_cursor(
    hovered: Res<HoverMap>,
    clickable: Query<(), Or<(With<Star>, With<NameBox>)>>,
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
    pointed_at: Query<
        (&GlobalTransform, &Children),
        (With<System>, With<PointedAt>),
    >,
    stars: Query<&GlobalTransform, With<Star>>,
) {
    let Ok(camera) = camera.single() else { return };

    for (system, children) in &pointed_at {
        // The star is what is drawn there, and carries the size it is drawn
        // at, which the system deliberately does not.
        let drawn = children
            .iter()
            .filter_map(|child| stars.get(child).ok())
            .map(|star| star.scale().x)
            .fold(0., f32::max);

        let at = system.translation();
        let radius = (drawn * RING_MARGIN).max(camera.radius * RING_FLOOR);

        gizmos.circle(Isometry3d::new(at, camera.rotation), radius, INDICATOR);
    }
}
