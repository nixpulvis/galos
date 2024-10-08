use crate::systems::Spyglass;
use bevy::core_pipeline::bloom::BloomSettings;
use bevy::prelude::*;
use bevy_mod_picking::prelude::*;
use bevy_panorbit_camera::{PanOrbitCamera, PanOrbitCameraPlugin};

pub fn plugin(app: &mut App) {
    app.add_plugins(PanOrbitCameraPlugin);
    app.add_event::<MoveCamera>();
    app.add_systems(Startup, spawn_camera);
    app.add_systems(Update, move_camera);
}

/// An event which triggers the movement of the camera
///
/// Send the camera to be focused on `position`.
#[derive(Event, Debug)]
pub struct MoveCamera {
    pub position: Option<Vec3>,
}

impl From<ListenerInput<Pointer<Click>>> for MoveCamera {
    fn from(click: ListenerInput<Pointer<Click>>) -> Self {
        MoveCamera { position: click.hit.position }
    }
}

/// Place a camera in space
pub fn spawn_camera(mut commands: Commands, spyglass: Res<Spyglass>) {
    commands.spawn((
        Camera3dBundle {
            transform: Transform::from_translation(Vec3::new(0., 0., 0.)),
            camera: Camera { hdr: true, ..default() },
            ..default()
        },
        PanOrbitCamera {
            pitch: Some(0.),
            yaw: Some(0.),
            radius: Some(spyglass.radius * 3.),
            focus: Vec3::splat(0.),
            zoom_sensitivity: 1.0,
            ..default()
        },
        BloomSettings::NATURAL,
    ));
}

/// Smoothly moves the camera on `MoveCamera` events
pub fn move_camera(
    mut query: Query<&mut PanOrbitCamera>,
    mut camera_events: EventReader<MoveCamera>,
) {
    for event in camera_events.read() {
        if let Some(position) = event.position {
            let mut camera = query.single_mut();
            camera.pan_smoothness = 0.6;
            camera.target_focus = position;
        }
    }
}
