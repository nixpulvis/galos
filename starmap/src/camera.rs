use bevy::prelude::*;
use bevy::core_pipeline::bloom::BloomSettings;
use bevy_panorbit_camera::PanOrbitCamera;
use bevy_mod_picking::prelude::*;

/// Sends the camera to be focused on `position`
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
pub fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3dBundle {
            transform: Transform::from_translation(Vec3::new(0., 0., 0.)),
            camera: Camera {
                hdr: true,
                ..default()
            },
            ..default()
        },
        PanOrbitCamera {
            pitch: Some(0.),
            yaw: Some(0.),
            radius: Some(25.0),

            // Achenar, home of the Empire!
            focus: Vec3::new(67.5, -119.46875, 24.84375),

            zoom_sensitivity: 1.0,
            ..default()
        },
        BloomSettings::NATURAL,
    ));
}

/// Smoothly moves the camera on `MoveCamera` event
pub fn move_camera(
    mut query: Query<&mut PanOrbitCamera>,
    mut camera_events: EventReader<MoveCamera>,
) {
    for event in camera_events.read() {
        if let Some(position) = event.position {
            let mut camera = query.single_mut();
            camera.pan_smoothness = 0.85;
            camera.target_focus = position;
        }
    }
}

pub fn keyboard(
    mut query: Query<&mut PanOrbitCamera>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let mut camera = query.single_mut();
    const ZOOM_FACTOR: f32 = 50.;

    if keys.pressed(KeyCode::KeyW) {
        if let Some(radius) = camera.radius {
            camera.target_focus -= Vec3::new(0., 0., radius / ZOOM_FACTOR);
        }
    }
    if keys.pressed(KeyCode::KeyS) {
        if let Some(radius) = camera.radius {
            camera.target_focus += Vec3::new(0., 0., radius / ZOOM_FACTOR);
        }
    }
    if keys.pressed(KeyCode::KeyA) {
        if let Some(radius) = camera.radius {
            camera.target_focus -= Vec3::new(radius / ZOOM_FACTOR, 0., 0.);
        }
    }
    if keys.pressed(KeyCode::KeyD) {
        if let Some(radius) = camera.radius {
            camera.target_focus += Vec3::new(radius / ZOOM_FACTOR, 0., 0.);
        }
    }
    if keys.pressed(KeyCode::KeyQ) {
        if let Some(radius) = camera.radius {
            camera.target_focus -= Vec3::new(0., radius / ZOOM_FACTOR, 0.);
        }
    }
    if keys.pressed(KeyCode::KeyE) {
        if let Some(radius) = camera.radius {
            camera.target_focus += Vec3::new(0., radius / ZOOM_FACTOR, 0.);
        }
    }
    if keys.pressed(KeyCode::KeyR) {
        camera.target_radius *= 0.9;
    }
    if keys.pressed(KeyCode::KeyF) {
        camera.target_radius *= 1.1;
    }
}
