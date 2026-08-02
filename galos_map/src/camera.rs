use crate::systems::Spyglass;
use bevy::camera::Hdr;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy_panorbit_camera::{PanOrbitCamera, PanOrbitCameraPlugin};

pub fn plugin(app: &mut App) {
    app.add_plugins(PanOrbitCameraPlugin);
    app.add_message::<MoveCamera>();
    app.add_systems(Startup, spawn_camera);
    app.add_systems(Update, move_camera);
}

/// A message which triggers the movement of the camera
///
/// Send the camera to be focused on `position`.
#[derive(Message, Debug)]
pub struct MoveCamera {
    pub position: Option<Vec3>,
}

/// Place a camera in space
pub fn spawn_camera(mut commands: Commands, spyglass: Res<Spyglass>) {
    commands.spawn((
        Camera3d::default(),
        Hdr,
        AmbientLight { color: Color::default(), brightness: 1e3, ..default() },
        Transform::from_translation(Vec3::new(0., 0., 0.)),
        PanOrbitCamera {
            pitch: Some(0.),
            yaw: Some(0.),
            radius: Some(spyglass.radius * 3.),
            focus: Vec3::splat(0.),
            zoom_sensitivity: 1.0,
            ..default()
        },
        Bloom::NATURAL,
    ));
}

/// Smoothly moves the camera on `MoveCamera` messages
pub fn move_camera(
    mut query: Query<&mut PanOrbitCamera>,
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
