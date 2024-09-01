use bevy::prelude::*;
use bevy::core_pipeline::{
    bloom::{BloomCompositeMode, BloomSettings},
    tonemapping::Tonemapping,
};
use bevy_panorbit_camera::PanOrbitCamera;
use bevy_infinite_grid::InfiniteGridBundle;
use crate::MoveCamera;

/// Place a camera in space
pub fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3dBundle {
            transform: Transform::from_translation(Vec3::new(0., 0., 0.)),
            tonemapping: Tonemapping::TonyMcMapface,
            ..default()
        },
        PanOrbitCamera {
            pitch: Some(15.0f32.to_radians()),
            yaw: Some(30.0f32.to_radians()),
            radius: Some(25.0),

            // Achenar, home of the Empire!
            focus: Vec3::new(67.5, -119.46875, 24.84375),

            zoom_sensitivity: 10.0,
            ..default()
        },
        BloomSettings::NATURAL,
    ));
    commands.spawn(InfiniteGridBundle::default());
}

/// Smoothly moves the camera on `MoveCamera` event
pub fn move_camera(
    mut query: Query<&mut PanOrbitCamera>,
    mut camera_events: EventReader<MoveCamera>,
) {
    for event in camera_events.read() {
        let mut camera = query.single_mut();
        camera.pan_smoothness = 0.85;
        camera.target_focus = event.position;
    }
}
