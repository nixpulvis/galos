use crate::schedule::MapSet;
use crate::systems::Spyglass;
use crate::ui::PointerOverUi;
use bevy::camera::Hdr;
use bevy::input::mouse::{
    AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit,
};
use bevy::math::DVec3;
use bevy::picking::mesh_picking::MeshPickingCamera;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use big_space::prelude::*;
use std::f32::consts::FRAC_PI_2;

pub fn plugin(app: &mut App) {
    app.add_message::<MoveCamera>();
    app.add_systems(Update, move_camera.in_set(MapSet::Camera));
    // Reads what `move_camera` and the spyglass asked for, and is the only
    // thing that writes the camera's cell and transform.
    app.add_systems(
        Update,
        orbit_camera.in_set(MapSet::Camera).after(move_camera),
    );
}

/// How far the camera may be pitched from the horizontal
///
/// Stopping just short of straight up keeps the up vector from flipping when
/// the camera passes over the point it is orbiting.
const PITCH_LIMIT: f32 = FRAC_PI_2 - 1e-3;

/// Radians of orbit per pixel of pointer travel, at unit sensitivity
const ORBIT_RATE: f32 = 5e-3;

/// Fraction of the orbit radius panned per pixel of pointer travel
///
/// Panning covers ground in proportion to how far out the camera is, so the
/// map moves under the pointer at about the same rate at every zoom.
const PAN_RATE: f32 = 2e-3;

/// E-folds of zoom per line of scroll, at unit sensitivity
///
/// Zoom is multiplicative because the map spans nine orders of magnitude. A
/// fixed step would cross the whole bubble near the surface of a star and
/// barely register out at the rim.
const ZOOM_RATE: f32 = 0.15;

/// Pixels of scroll that count as one line, for pointers that report them
const PIXELS_PER_LINE: f32 = 16.;

/// How near and how far the camera may be pulled from what it looks at
///
/// The far end is past the width of the galaxy, so the whole map fits. The
/// near end is well inside a star system, ready for bodies to be drawn.
const MIN_RADIUS: f32 = 1e-6;
const MAX_RADIUS: f32 = 1e6;

/// A message which triggers the movement of the camera
///
/// Send the camera to be focused on `position`, in absolute galactic light
/// years. Positions are `f64` because a `f32` cannot tell two points at the
/// galactic rim apart any closer than a few thousand light seconds, which is
/// most of the way across a star system.
#[derive(Message, Debug)]
pub struct MoveCamera {
    pub position: Option<DVec3>,
}

/// A camera that orbits a point in the galaxy
///
/// Replaces `bevy_panorbit_camera`, which cannot be used here for two
/// reasons. Its focus is a `Vec3`, so it cannot name a point out at the rim
/// any more precisely than a star system is wide. And it writes an absolute
/// translation every frame, which is exactly what a floating origin asks you
/// not to do: `big_space` would spend every frame recentring a camera that
/// had already put itself back.
///
/// The orbit is instead kept as a focus, a radius and two angles, which is
/// what the controls actually manipulate. The cell and transform are
/// computed from those once per frame, so nothing is ever fought over.
#[derive(Component)]
pub struct OrbitCamera {
    /// Absolute galactic position the camera looks at, in light years
    pub focus: DVec3,
    /// Where the focus is heading, which it approaches smoothly
    pub target_focus: DVec3,
    /// Absolute galactic position of the camera itself, in light years
    ///
    /// Derived from the focus and the orbit, and published here because
    /// distances to stars are wanted by half the map. Reading it avoids
    /// having to undo the cell split to ask where the camera is.
    pub eye: DVec3,
    /// Which way the camera faces, for anything that wants to line up with it
    pub rotation: Quat,
    pub radius: f32,
    pub target_radius: f32,
    pub yaw: f32,
    pub pitch: f32,
    /// How much of the remaining distance to the target is left each frame
    ///
    /// Zero snaps, and values approaching one never arrive.
    pub pan_smoothness: f32,
    pub orbit_sensitivity: f32,
    pub pan_sensitivity: f32,
    pub zoom_sensitivity: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        OrbitCamera {
            focus: DVec3::ZERO,
            target_focus: DVec3::ZERO,
            eye: DVec3::ZERO,
            rotation: Quat::IDENTITY,
            radius: 1.,
            target_radius: 1.,
            yaw: 0.,
            pitch: 0.,
            pan_smoothness: 0.8,
            orbit_sensitivity: 1.,
            pan_sensitivity: 1.,
            zoom_sensitivity: 1.,
        }
    }
}

/// Everything the map's one camera is
///
/// Handed to [`crate::space`] to spawn, because a camera that is not a child
/// of the galaxy grid is not positioned by it.
pub fn camera(spyglass: &Spyglass) -> impl Bundle {
    (
        Camera3d::default(),
        Hdr,
        // Mesh picking requires markers, see `systems::spawn::plugin`.
        MeshPickingCamera,
        AmbientLight { color: Color::default(), brightness: 1e3, ..default() },
        // Every other entity is drawn relative to this one.
        FloatingOrigin,
        OrbitCamera {
            radius: spyglass.radius * 3.,
            target_radius: spyglass.radius * 3.,
            ..default()
        },
        Bloom::NATURAL,
    )
}

/// Smoothly moves the camera on `MoveCamera` messages
pub fn move_camera(
    mut query: Query<&mut OrbitCamera>,
    mut camera_events: MessageReader<MoveCamera>,
) {
    for event in camera_events.read() {
        if let Some(position) = event.position {
            let Ok(mut camera) = query.single_mut() else { continue };
            camera.pan_smoothness = 0.6;
            camera.target_focus = position;
        }
    }
}

/// Drive the orbit from the pointer, and place the camera where it lands
///
/// The orbit is worked out in absolute light years and only split into a
/// cell and a remainder at the very end, so the arithmetic never has to know
/// about grids and the camera never lands between two cells.
pub fn orbit_camera(
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    over_ui: Res<PointerOverUi>,
    grids: Query<&Grid, With<BigSpace>>,
    mut cameras: Query<(&mut OrbitCamera, &mut CellCoord, &mut Transform)>,
) {
    let Ok(grid) = grids.single() else { return };
    let Ok((mut orbit, mut cell, mut transform)) = cameras.single_mut() else {
        return;
    };

    // A drag that started on a slider is the user talking to the settings
    // window, not to the map behind it.
    if !over_ui.0 {
        if buttons.pressed(MouseButton::Left) {
            let rate = ORBIT_RATE * orbit.orbit_sensitivity;
            orbit.yaw -= motion.delta.x * rate;
            orbit.pitch = (orbit.pitch - motion.delta.y * rate)
                .clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }

        if buttons.pressed(MouseButton::Right) {
            let rate = PAN_RATE * orbit.pan_sensitivity * orbit.radius;
            let across = orbit.rotation * Vec3::X * -motion.delta.x * rate;
            let up = orbit.rotation * Vec3::Y * motion.delta.y * rate;
            orbit.target_focus += (across + up).as_dvec3();
        }

        let lines = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y / PIXELS_PER_LINE,
        };
        if lines != 0. {
            let zoom = -lines * ZOOM_RATE * orbit.zoom_sensitivity;
            orbit.target_radius = (orbit.target_radius * zoom.exp())
                .clamp(MIN_RADIUS, MAX_RADIUS);
        }
    }

    // Approach whatever was asked for, rather than jumping to it. A search
    // can send the focus clear across the galaxy, and arriving instantly
    // leaves no sense of where the new system is in relation to the old.
    let step = 1. - orbit.pan_smoothness.clamp(0., 1.);
    let focus = orbit.focus + (orbit.target_focus - orbit.focus) * step as f64;
    let radius = orbit.radius + (orbit.target_radius - orbit.radius) * step;

    let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.);
    let eye = focus + (rotation * Vec3::Z * radius).as_dvec3();

    orbit.focus = focus;
    orbit.radius = radius;
    orbit.rotation = rotation;
    orbit.eye = eye;

    let (eye_cell, eye_translation) = grid.translation_to_grid(eye);
    cell.set_if_neq(eye_cell);
    transform.translation = eye_translation;
    transform.rotation = rotation;
}
