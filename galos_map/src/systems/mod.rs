use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use bevy::math::DVec3;
use bevy::prelude::*;
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

    // Both ask the camera for something, and `orbit_camera` then works out
    // where it lands, so both have to have spoken by the time it runs.
    app.add_systems(
        Update,
        zoom_with_spyglass
            .in_set(MapSet::Camera)
            .after(crate::camera::move_camera)
            .before(crate::camera::orbit_camera),
    );
    app.add_systems(Update, visibility.in_set(MapSet::Present));
}

#[derive(Component)]
pub struct System {
    address: i64,
    name: String,
    /// Absolute galactic position, in light years
    ///
    /// The grid this is drawn in splits a position into a cell and an offset
    /// within it, which is what the renderer needs but an awkward thing to
    /// measure distances between. The database's own answer is kept here,
    /// undiminished, and everything that wants to know how far apart two
    /// systems are asks this instead of unpicking the split.
    position: [f64; 3],
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
    camera: Query<&OrbitCamera>,
    mut systems: Query<(&System, &mut Visibility)>,
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
        let radius = spyglass.radius as f64;
        for (system, mut visibility) in &mut systems {
            let dist = camera.focus.distance(DVec3::from(system.position));
            visibility.set_if_neq(if dist <= radius {
                Visibility::Visible
            } else {
                Visibility::Hidden
            });
        }
    }
}

pub fn zoom_with_spyglass(
    spyglass: Res<Spyglass>,
    mut camera: Query<&mut OrbitCamera>,
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
pub fn system_to_vec(system: &DbSystem) -> Option<DVec3> {
    system.position.map(|p| DVec3::new(p.x, p.y, p.z))
}
