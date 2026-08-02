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

    // A strip needs two ends to join. A search that found nothing comes back
    // empty, and handing the renderer a mesh with no vertices leaves its slab
    // allocator referring to something that was never allocated:
    //
    //     ERROR bevy_render::slab_allocator: Use-after-free: attempted to
    //     copy element data for an unallocated key
    let points: Vec<Vec3> = systems.iter().filter_map(system_to_vec).collect();
    if points.len() < 2 {
        return;
    }

    commands.spawn((
        Mesh3d(meshes.add(LineStrip { points })),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1., 1., 1., 0.25),
            alpha_mode: AlphaMode::Blend,
            ..default()
        })),
        Transform::from_xyz(0., 0., 0.),
        Route,
    ));
}
