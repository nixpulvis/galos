use bevy::prelude::*;
use bevy_panorbit_camera::PanOrbitCamera;
use chrono::{DateTime, Utc};
use elite_journal::{
    system::{Economy, Security},
    // TODO: Fix these imports, they should all be in system.
    Allegiance,
    Government,
};
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.insert_resource(Spyglass {
        radius: 50.,
        fetch: true,
        disabled: false,
        lock_camera: false,
    });

    app.add_plugins(fetch::plugin);
    app.add_plugins(spawn::plugin);
    app.add_plugins(despawn::plugin);
    app.add_plugins(scale::plugin);
    app.add_plugins(labels::plugin);

    app.add_systems(Update, visibility.after(spawn::spawn));
    app.add_systems(Update, zoom_with_spyglass);
}

#[derive(Component)]
pub struct System {
    address: i64,
    name: String,
    position: [f32; 3],
    population: u64,
    allegiance: Option<Allegiance>,
    government: Option<Government>,
    security: Option<Security>,
    primary_economy: Option<Economy>,
    secondary_economy: Option<Economy>,
    updated_at: DateTime<Utc>,
}

pub mod despawn;
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
    pub disabled: bool,
    pub lock_camera: bool,
}

pub fn visibility(
    mut commands: Commands,
    camera: Query<&PanOrbitCamera>,
    systems: Query<(Entity, &Transform), With<System>>,
    spyglass: Res<Spyglass>,
) {
    // Make sure we make systems visible again.
    if spyglass.is_changed() && spyglass.disabled {
        for (entity, _) in &systems {
            commands.entity(entity).insert(Visibility::Visible);
        }
    }

    if !spyglass.disabled {
        let camera_translation = camera.single().focus;
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

pub fn zoom_with_spyglass(
    spyglass: Res<Spyglass>,
    mut camera: Query<&mut PanOrbitCamera>,
) {
    if spyglass.lock_camera {
        camera.single_mut().target_radius = spyglass.radius * 3.;
    }
}

pub fn system_to_vec(system: &DbSystem) -> Vec3 {
    Vec3::new(
        system.position.unwrap().x as f32,
        system.position.unwrap().y as f32,
        system.position.unwrap().z as f32,
    )
}
