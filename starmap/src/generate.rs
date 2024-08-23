use bevy::prelude::*;
use bevy_mod_picking::prelude::*;
use galos_db::Database;
use galos_db::systems::System;
use elite_journal::Allegiance;
use async_std::task;
use crate::{SystemsSearch, SystemMarker};
use crate::camera::{self, PanOrbitState};

/// Queries the DB, then creates an entity for each star system in the search.
///
/// This function also moves the camera's position to be looking at the
/// searched system.
pub fn star_systems(
    camera_query: Query<&mut PanOrbitState>,
    search: Res<SystemsSearch>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut mesh: Local<Option<Handle<Mesh>>>,
) {
    // TODO: Scale systems based on the distance from the camera.
    // This may follow some kind of log curve, or generally effect closer
    // systems less. The goal is to have systems never become smaller than a
    // pixel in size. I'm not sure if we can implement blending modes or
    // something to handle partially overlapping systems.
    const SYSTEM_SCALE:  f32 = 1.;
    const SYSTEM_RADIUS: f32 = SYSTEM_SCALE/10.;

    // Make sure our sphere mesh is loaded. This is the "shape" of the star.
    if mesh.is_none() {
        *mesh = Some(meshes.add(Sphere::new(SYSTEM_RADIUS).mesh().ico(3).unwrap()));
    }

    // Load DB objects.
    let (origin, systems) = query_systems(search);

    // Move the camera to the origin of queried systems.
    if let Some(o) = origin {
        let position = Vec3::new(
            o.position.unwrap().x as f32,
            o.position.unwrap().y as f32,
            o.position.unwrap().z as f32,
        );
        camera::move_camera(camera_query, position);
    }

    // Generate all the star system entities.
    for system in systems {
        commands.spawn((PbrBundle {
            transform: Transform {
                translation: Vec3::new(
                    system.position.unwrap().x as f32,
                    system.position.unwrap().y as f32,
                    system.position.unwrap().z as f32,
                ),
                // scale: Vec3::splat(0.25),
                scale: Vec3::splat(SYSTEM_SCALE),
                ..default()
            },
            // TODO: Use entries API to avoid unwrap.
            mesh: mesh.as_ref().unwrap().clone(),
            // TODO: Configure the material to be flatter when looking at allegiance,
            // or more realistic when looking at star class. Remember to check
            // partially overlapping systems.
            material: materials.add(allegiance_color(&system)),
            ..default()
        },
        SystemMarker,
        PickableBundle::default(),

        On::<Pointer<Click>>::target_commands_mut(|click, _target_commands| {
            dbg!(click);
            // TODO: toggle system info.
            // TODO: double click to center camera... use events instead
            // of the code below which doesn't work.
            // if let Some(position) = click.event.hit.position {
            //     camera::move_camera(camera_query, position);
            // }
        }),

        On::<Pointer<Over>>::target_commands_mut(|_hover, _target_commands| {
            dbg!(_hover);
            // TODO: Spawn system label.
        }),

        On::<Pointer<Out>>::target_commands_mut(|_hover, _target_commands| {
            dbg!(_hover);
            // TODO: Despawn system label.
        }),

        ));
    }
}

// TODO: This absolutely needs to be async and to avoid blocking the rendering
// pipeline.
fn query_systems(search: Res<SystemsSearch>) -> (Option<System>, Vec<System>) {
    task::block_on(async {
        let db = Database::new().await.unwrap();
        let radius = search.radius.parse().unwrap_or(100.);
        let origins = System::fetch_like_name(&db, &search.name).await.unwrap();
        match System::fetch_in_range_like_name(&db, radius, &search.name).await {
        // match System::fetch_sample(&db, 100., &search.name).await {
            Ok(systems) => (origins.first().map(ToOwned::to_owned), systems),
            _ => (None, vec![]),
        }
    })
}

/// Maps system allegiance to a color for the sphere on the map.
fn allegiance_color(system: &System) -> Color {
    match system.allegiance {
        Some(Allegiance::Alliance)         => Color::srgb(0., 1., 0.),   // Green
        Some(Allegiance::Empire)           => Color::srgb(0., 1., 1.),   // Cyan
        Some(Allegiance::Federation)       => Color::srgb(1., 0., 0.),   // Red
        Some(Allegiance::PilotsFederation) => Color::srgb(1., 0.5, 0.),  // Orange
        Some(Allegiance::Independent)      => Color::srgb(1., 1., 0.),   // Yellow
        Some(Allegiance::Guardian)         => Color::srgb(0., 0., 1.),   // Blue
        Some(Allegiance::Thargoid)         => Color::srgb(1., 0., 1.),  // Blue
        Some(_)                            => Color::srgb(1., 1., 1.),   // White
        None                               => Color::srgb(0., 0., 0.),   // Black
    }
}
