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
        radius: 10.,
        fetch: true,
        disabled: false,
        lock_camera: false,
    });
    app.insert_resource(Target(None));

    app.add_plugins(fetch::plugin);
    app.add_plugins(spawn::plugin);
    app.add_plugins(despawn::plugin);
    app.add_plugins(scale::plugin);
    app.add_plugins(labels::plugin);

    app.add_systems(
        Update,
        visibility.after(spawn::spawn).after(despawn::despawn),
    );
    app.add_systems(Update, zoom_with_spyglass);
}

#[derive(Component, Clone, Debug)]
pub struct System {
    pub address: i64,
    pub name: String,
    pub position: [f32; 3],
    pub population: u64,
    pub allegiance: Option<Allegiance>,
    pub government: Option<Government>,
    pub security: Option<Security>,
    pub primary_economy: Option<Economy>,
    pub secondary_economy: Option<Economy>,
    pub updated_at: DateTime<Utc>,
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

#[derive(Resource)]
pub struct Target(pub Option<System>);

pub fn system_to_vec(system: &DbSystem) -> Vec3 {
    Vec3::new(
        system.position.unwrap().x as f32,
        system.position.unwrap().y as f32,
        system.position.unwrap().z as f32,
    )
}
