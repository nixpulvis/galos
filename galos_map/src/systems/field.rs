//! The star field, drawn flat in screen space
//!
//! A system's mark cannot be a mesh at the system's true coordinate: at a
//! galaxy's scale the f32 clip transform `view_proj · model` tears it apart,
//! and which triangles survive turns with the camera, so the field blinks and
//! swims (see `docs/night-sky.md`). So it is not drawn there. Every visible
//! system is projected to a pixel on the CPU in f64 — the stable anchor the
//! names are already placed by — and a screen-aligned quad is built at that
//! pixel into one mesh, drawn by a camera sitting at the world origin. Nothing
//! that camera rasterises carries a galaxy-scale coordinate, so there is no
//! precision left to lose: the field is exact and still at every zoom, pitch,
//! and turn, and it is one draw call however many stars there are.
//!
//! The [`Shell`] entities stay on the galaxy grid, where the map addresses them
//! for picking, filtering, and flying in. They simply stop being what draws the
//! star; this does the drawing, off their position alone.

use crate::camera::{FIELD_LAYER, OrbitCamera, SHELLS_LAYER, ShellsView};
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::bodies::spawn::Strength;
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::labels::{screen_position, world_per_pixel};
use crate::systems::scale::View;
use crate::systems::spawn::{ColorBy, Shell, hue};
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{Exposure, Hdr, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::math::DVec3;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::asset::RenderAssetUsages;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn_field);
    app.add_systems(Update, tune_field);
    app.add_systems(
        Update,
        build_field
            .in_set(MapSet::Present)
            // Reads the world size `size_by_distance` leaves on each shell to
            // recover its pixel size.
            .after(crate::systems::scale::size_by_distance)
            .run_if(resource_equals(View::Map)),
    );
}

/// The camera at the world origin that draws the field flat
#[derive(Component)]
struct FieldCamera;

/// The one mesh the whole field is rebuilt into each frame
#[derive(Resource)]
struct FieldMesh(Handle<Mesh>);

/// The smallest a mark is drawn, as a radius in pixels
///
/// A star too far to resolve is still a point of light, not nothing, so it is
/// held to a sliver of a pixel rather than allowed to vanish.
const SMALLEST: f32 = 0.75;

/// Put the field's mesh, its material, and the origin camera up
fn spawn_field(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh = meshes.add(Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    ));
    // White base, so the per-vertex colour is the whole of what a mark comes
    // out. Flat and solid — a map mark reads as a dot, not a glint; the
    // realistic view is where the point spread will shape a star. Unlit: a
    // mark is a light, not a thing lit by one.
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(material),
        RenderLayers::layer(FIELD_LAYER),
        // Every vertex is placed by hand each frame; there is no bound to cull
        // against and the whole field is one draw regardless.
        NoFrustumCulling,
        Transform::default(),
        Visibility::Visible,
    ));
    commands.insert_resource(FieldMesh(mesh));

    // The camera that draws it, at the world origin so nothing it rasterises
    // sits at a galaxy coordinate. Orthographic at one unit to the pixel, over
    // the scene and under the annotation overlays, clearing neither so the map
    // shows through. Inactive to begin with; `tune_field` turns it on for the
    // view it belongs to.
    commands.spawn((
        Camera3d::default(),
        Hdr,
        Tonemapping::None,
        Exposure::SUNLIGHT,
        Camera {
            order: SHELLS_LAYER as isize,
            clear_color: ClearColorConfig::None,
            is_active: false,
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::WindowSize,
            ..OrthographicProjection::default_3d()
        }),
        RenderLayers::layer(FIELD_LAYER),
        FieldCamera,
    ));
}

/// One camera or the other draws the sky
///
/// The origin camera draws the field flat in the map view. The realistic view
/// keeps the shells' own camera for now, until the field paints its stars too.
/// Written only when the view moves.
fn tune_field(
    view: Res<View>,
    mut field: Query<&mut Camera, (With<FieldCamera>, Without<ShellsView>)>,
    mut shells: Query<&mut Camera, (With<ShellsView>, Without<FieldCamera>)>,
) {
    if !view.is_changed() {
        return;
    }
    let map = matches!(*view, View::Map);
    if let Ok(mut camera) = field.single_mut() {
        camera.is_active = map;
    }
    if let Ok(mut camera) = shells.single_mut() {
        camera.is_active = !map;
    }
}

/// Rebuild the field mesh from where every visible system falls on screen
fn build_field(
    camera: Query<(&OrbitCamera, &Camera)>,
    shells: Query<
        (&System, &Transform, &Visibility, &Strength, Has<Filtered>),
        With<Shell>,
    >,
    color_by: Res<ColorBy>,
    dim: Res<DimTo>,
    field: Res<FieldMesh>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;
    let Some(mut mesh) = meshes.get_mut(&field.0) else { return };

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let half = viewport * 0.5;
    for (system, drawn, visibility, strength, filtered) in &shells {
        // Out of the spyglass is not drawn; `super::visibility` says which.
        if *visibility == Visibility::Hidden {
            continue;
        }
        let position = DVec3::from(system.position);
        let Some(at) = screen_position(orbit, cot_half_fov, viewport, position)
        else {
            continue;
        };
        let away = crate::space::metres(orbit.eye - position).length() as f32;
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, away.max(1.));
        // The pixel size `size_by_distance` settled, read back off the world
        // size it left on the shell.
        let radius = (drawn.scale.x / per_pixel * 0.5).max(SMALLEST);

        // The colour of the ring around the star. The palette carries a low
        // alpha for the old translucent ball; a flat mark wants to read solid,
        // so the point spread alone shapes it. Dimmed if the filters exclude
        // it and faded as its mark goes out.
        let mut tint = LinearRgba::from(hue(system, &color_by).color());
        tint.alpha = 1.;
        if filtered {
            tint.alpha *= dim.opacity();
        }
        tint.alpha *= strength.0.clamp(0., 1.);
        let color = [tint.red, tint.green, tint.blue, tint.alpha];

        // Pixel-centred, y up, one unit to a pixel: the frame the origin camera
        // reads. A metre in front of it, clear of its near plane.
        let cx = at.x - half.x;
        let cy = half.y - at.y;
        let base = positions.len() as u32;
        for (dx, dy, u, v) in [
            (-radius, -radius, 0., 1.),
            (radius, -radius, 1., 1.),
            (radius, radius, 1., 0.),
            (-radius, radius, 0., 0.),
        ] {
            positions.push([cx + dx, cy + dy, -1.]);
            uvs.push([u, v]);
            colors.push(color);
        }
        indices.extend_from_slice(&[
            base,
            base + 1,
            base + 2,
            base,
            base + 2,
            base + 3,
        ]);
    }

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
}
