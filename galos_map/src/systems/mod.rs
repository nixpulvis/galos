use bevy::prelude::*;
use bevy_panorbit_camera::PanOrbitCamera;
use galos_db::systems::System as DbSystem;

#[derive(Component)]
pub struct System {
    address: i64,
    name: String,
    population: u64,
}

pub mod fetch;
pub mod labels;
pub mod route;
pub mod scale;
pub mod spawn;

/// A global setting which controls the spyglass around the camera
#[derive(Resource)]
pub struct Spyglass {
    pub fetch: bool,
    pub radius: f32,
    pub filter: bool,
}

pub fn visibility(
    mut commands: Commands,
    camera: Query<&PanOrbitCamera>,
    systems: Query<(Entity, &Transform), With<System>>,
    spyglass: Res<Spyglass>,
) {
    // Make sure we make systems visible again.
    if spyglass.is_changed() && !spyglass.filter {
        for (entity, _) in &systems {
            commands.entity(entity).insert(Visibility::Visible);
        }
    }

    if spyglass.filter {
        let camera_translation = camera.single().target_focus;
        for (entity, system_transform) in &systems {
            let dist =
                camera_translation.distance(system_transform.translation);
            if dist <= spyglass.radius {
                commands.entity(entity).insert(Visibility::Visible);
            } else {
                commands.entity(entity).insert(Visibility::Hidden);
            }
        }
    }
}

pub fn system_to_vec(system: &DbSystem) -> Vec3 {
    Vec3::new(
        system.position.unwrap().x as f32,
        system.position.unwrap().y as f32,
        system.position.unwrap().z as f32,
    )
}
