use super::{LineStrip, Route, system_to_vec};
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
        commands.entity(entity).despawn();
    }

    commands.spawn((
        Mesh3d(meshes.add(LineStrip {
            points: systems.iter().map(system_to_vec).collect(),
        })),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1., 1., 1., 0.25),
            alpha_mode: AlphaMode::Blend,
            ..default()
        })),
        Transform::from_xyz(0., 0., 0.),
        Route,
    ));
}
