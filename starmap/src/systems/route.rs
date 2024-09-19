use bevy::prelude::*;
use bevy::render::mesh::PrimitiveTopology;
use bevy::render::render_asset::RenderAssetUsages;
use galos_db::systems::System as DbSystem;

use super::system_to_vec;

#[derive(Component)]
pub struct Route;

// TODO: Save another Local<Option<Handle<Mesh>>>?
pub(crate) fn spawn_route(
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

/// A list of points that will have a line drawn between each consecutive points
#[derive(Debug, Clone)]
struct LineStrip {
    points: Vec<Vec3>,
}

impl From<LineStrip> for Mesh {
    fn from(line: LineStrip) -> Self {
        Mesh::new(
            // This tells wgpu that the positions are a list of points
            // where a line will be drawn between each consecutive point
            PrimitiveTopology::LineStrip,
            RenderAssetUsages::RENDER_WORLD,
        )
        // Add the point positions as an attribute
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, line.points)
    }
}
