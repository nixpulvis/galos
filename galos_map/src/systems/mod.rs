use crate::schedule::MapSet;
use bevy::prelude::*;
use bevy_panorbit_camera::PanOrbitCamera;
use chrono::{DateTime, Utc};
use elite_journal::{
    // TODO: Fix these imports, they should all be in system.
    Allegiance,
    Government,
    system::{Economy, Security},
};
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.insert_resource(Spyglass {
        radius: 10.,
        fetch: true,
        disabled: false,
        lock_camera: false,
    });

    app.add_plugins(fetch::plugin);
    app.add_plugins(spawn::plugin);
    app.add_plugins(despawn::plugin);
    app.add_plugins(scale::plugin);
    app.add_plugins(labels::plugin);

    // Both write the camera, though to different fields, so pick an order.
    app.add_systems(
        Update,
        zoom_with_spyglass
            .in_set(MapSet::Camera)
            .after(crate::camera::move_camera),
    );
    // Reads a star's transform, which the `scale` systems write.
    app.add_systems(
        Update,
        visibility
            .in_set(MapSet::Present)
            .after(scale::scale_systems)
            .after(scale::scale_stars),
    );
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

/// Show the systems inside the spyglass and hide the rest
///
/// Runs over every star every frame, so it writes only where the answer
/// actually changed. Assigning regardless would mark the whole sky as
/// changed each frame, and each star drags its name along with it.
pub fn visibility(
    camera: Query<&PanOrbitCamera>,
    mut systems: Query<(&Transform, &mut Visibility), With<System>>,
    spyglass: Res<Spyglass>,
) {
    // Make sure we make systems visible again.
    if spyglass.is_changed() && spyglass.disabled {
        for (_, mut visibility) in &mut systems {
            visibility.set_if_neq(Visibility::Visible);
        }
    }

    if !spyglass.disabled {
        let Ok(camera) = camera.single() else { return };
        let camera_translation = camera.focus;
        for (system_transform, mut visibility) in &mut systems {
            let dist =
                camera_translation.distance(system_transform.translation);
            visibility.set_if_neq(if dist <= spyglass.radius {
                Visibility::Visible
            } else {
                Visibility::Hidden
            });
        }
    }
}

pub fn zoom_with_spyglass(
    spyglass: Res<Spyglass>,
    mut camera: Query<&mut PanOrbitCamera>,
) {
    if spyglass.lock_camera {
        if let Ok(mut camera) = camera.single_mut() {
            camera.target_radius = spyglass.radius * 3.;
        }
    }
}

/// Where a system sits, if the database knows
///
/// Roughly three quarters of the systems on record have no coordinates, so
/// this has to be an answer the caller handles rather than an assumption.
pub fn system_to_vec(system: &DbSystem) -> Option<Vec3> {
    system.position.map(|p| Vec3::new(p.x as f32, p.y as f32, p.z as f32))
}
