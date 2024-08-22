use bevy::prelude::*;
use galos_db::Database;
use galos_db::systems::System;
use elite_journal::Allegiance;
use async_std::task;
use crate::{SystemsSearch, SystemMarker};
use crate::camera::{self, PanOrbitState};

pub fn bodies(
    camera_query: Query<&mut PanOrbitState>,
    search: Res<SystemsSearch>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh = meshes.add(Sphere::new(1.0).mesh().ico(3).unwrap());

    let systems = task::block_on(async {
        let db = Database::new().await.unwrap();
        match System::fetch_in_range_like_name(&db, 100., &search.name).await {
            Ok(systems) if !systems.is_empty() => {
                let origins = System::fetch_like_name(&db, &search.name).await.unwrap();
                let origin = origins.first().unwrap();
                let position = Vec3::new(
                    origin.position.unwrap().x as f32,
                    origin.position.unwrap().y as f32,
                    origin.position.unwrap().z as f32,
                );
                camera::move_camera(camera_query, position);
                systems
            },
            _ => vec![],
        }
    });

    for system in systems {
        let radius: f32 = 0.25;

        let position = Vec3::new(
            system.position.unwrap().x as f32,
            system.position.unwrap().y as f32,
            system.position.unwrap().z as f32,
        );

        commands.spawn((PbrBundle {
            transform: Transform {
                translation: position,
                scale: Vec3::splat(radius),
                ..default()
            },
            mesh: mesh.clone(),
            material: materials.add(system_color(&system)),
            ..default()
        }, SystemMarker));
    }
}

fn system_color(system: &System) -> Color {
    match system.allegiance {
        Some(Allegiance::Alliance)    => Color::srgb(0., 1., 0.),
        Some(Allegiance::Empire)      => Color::srgb(0., 0., 1.),
        Some(Allegiance::Federation)  => Color::srgb(1., 0., 0.),
        Some(Allegiance::Independent) => Color::srgb(1., 1., 0.),
        Some(_)                       => Color::srgb(1., 1., 1.),
        None                          => Color::srgb(0., 0., 0.),
    }
}
