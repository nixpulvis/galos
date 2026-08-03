use super::{LineStrip, Route, system_to_vec};
use crate::space::Galaxy;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use galos_db::systems::System as DbSystem;

// TODO: Save another Local<Option<Handle<Mesh>>>?
pub fn spawn_route(
    systems: &[DbSystem],
    route_query: &Query<Entity, With<Route>>,
    galaxy: &Res<Galaxy>,
    grid: &Grid,
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
    let points: Vec<DVec3> = systems.iter().filter_map(system_to_vec).collect();
    if points.len() < 2 {
        return;
    }

    // Mesh vertices are floats, with no cell to lean on, so a route drawn in
    // galactic coordinates would be quantised to whatever precision is left
    // at that distance from the centre. Hanging the line off its own midpoint
    // leaves the vertices holding only how far each end is from that, which
    // is at most the length of the route.
    let midpoint = points.iter().fold(DVec3::ZERO, |sum, p| sum + *p)
        / points.len() as f64;
    let (cell, translation) = grid.translation_to_grid(midpoint);
    let points = points.iter().map(|p| (*p - midpoint).as_vec3()).collect();

    commands.spawn((
        Mesh3d(meshes.add(LineStrip { points })),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1., 1., 1., 0.25),
            alpha_mode: AlphaMode::Blend,
            ..default()
        })),
        cell,
        Transform::from_translation(translation),
        Route,
        ChildOf(galaxy.0),
    ));
}
