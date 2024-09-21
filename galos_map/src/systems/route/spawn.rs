use super::{system_to_vec, LineStrip, Route};
use bevy::prelude::*;
use galos_db::systems::System as DbSystem;

// TODO: Save another Local<Option<Handle<Mesh>>>?
pub fn spawn_route(
    systems: &[DbSystem],
    route_query: &Query<Entity, With<Route>>,
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
) {
    for entity in route_query.iter() {
        commands.entity(entity).despawn_recursive();
    }

    commands.spawn((
        MaterialMeshBundle {
            mesh: meshes.add(LineStrip {
                points: systems.iter().map(system_to_vec).collect(),
            }),
            transform: Transform::from_xyz(0., 0., 0.),
            material: materials.add(StandardMaterial {
                base_color: Color::srgba(1., 1., 1., 0.1),
                alpha_mode: AlphaMode::Blend,
                ..default()
            }),
            ..default()
        },
        Route,
    ));
}
