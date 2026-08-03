//! What it looks like to point at a system
//!
//! A system can be reached by its star or by its name, and both mean the
//! same thing, so both mark the system itself. Everything drawn for a system
//! then answers to one component and they cannot disagree: pointing at a
//! name rings its star, and pointing at a star lights its name.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::labels::{Label, NameBox, depth, world_per_pixel};
use crate::systems::spawn::Star;
use bevy::math::DVec3;
use bevy::picking::hover::HoverMap;
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};

pub fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (size_targets, point_the_cursor)
            .in_set(MapSet::Present)
            .after(super::scale::size_by_distance)
            .after(super::scale::size_uniformly),
    );
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
    targets: Query<&ChildOf, With<PointerTarget>>,
    boxes: Query<&ChildOf, With<NameBox>>,
    names: Query<&ChildOf, With<Label>>,
    mut commands: Commands,
) {
    if let Some(system) = pointed_system(over.entity, &targets, &boxes, &names)
    {
        commands.entity(system).insert(PointedAt);
    }
}

/// And unmark it once the pointer has left
fn look_away(
    out: On<Pointer<Out>>,
    targets: Query<&ChildOf, With<PointerTarget>>,
    boxes: Query<&ChildOf, With<NameBox>>,
    names: Query<&ChildOf, With<Label>>,
    mut commands: Commands,
) {
    if let Some(system) = pointed_system(out.entity, &targets, &boxes, &names) {
        commands.entity(system).remove::<PointedAt>();
    }
}

/// Which system the pointer is really on, given what it landed on
///
/// A system's target hangs off it directly. The box catching a name hangs
/// off the name, which hangs off the system in turn, so it is one step
/// further up.
fn pointed_system(
    hit: Entity,
    targets: &Query<&ChildOf, With<PointerTarget>>,
    boxes: &Query<&ChildOf, With<NameBox>>,
    names: &Query<&ChildOf, With<Label>>,
) -> Option<Entity> {
    if let Ok(target) = targets.get(hit) {
        return Some(target.parent());
    }
    let name = boxes.get(hit).ok()?;
    Some(names.get(name.parent()).ok()?.parent())
}

/// Fit each system's target to what its indicator will be drawn at
///
/// One answer for both, worked out here and read back by [`ring`], so that
/// the mark and the area catching the pointer cannot come apart.
pub fn size_targets(
    camera: Query<(&OrbitCamera, &Camera)>,
    systems: Query<(&System, &Children)>,
    stars: Query<&Transform, With<Star>>,
    mut targets: Query<&mut Transform, (With<PointerTarget>, Without<Star>)>,
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

        let into_view = depth(orbit, DVec3::from(system.position)).max(1e-3);
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
    pointed_at: Query<
        (&GlobalTransform, &Children),
        (With<System>, With<PointedAt>),
    >,
    targets: Query<&GlobalTransform, With<PointerTarget>>,
) {
    let Ok(camera) = camera.single() else { return };

    for (system, children) in &pointed_at {
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
            INDICATOR,
        );
    }
}
