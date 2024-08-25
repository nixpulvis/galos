use bevy::prelude::*;
use bevy_panorbit_camera::PanOrbitCamera;
use crate::MoveCamera;

pub fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3dBundle {
            transform: Transform::from_translation(Vec3::new(0., 0., 0.)),
            ..default()
        },
        PanOrbitCamera {
            pitch: Some(15.0f32.to_radians()),
            yaw: Some(30.0f32.to_radians()),
            radius: Some(25.0),

            zoom_sensitivity: 10.0,
            ..default()
        },
    ));
}

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
