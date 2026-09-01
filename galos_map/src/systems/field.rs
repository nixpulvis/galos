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

use crate::camera::{
    FIELD_LAYER, OrbitCamera, SHELLS_LAYER, STAR_BLOOM, ShellsView,
};
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::bodies::spawn::Strength;
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::labels::{screen_position, world_per_pixel};
use crate::systems::scale::View;
use crate::systems::spawn::{
    ColorBy, Shell, StarExposure, hue, mag_step, photometric_emissive,
};
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{Exposure, Hdr, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::math::DVec3;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use galos_photometry::{Distance, Magnitude};

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn_field);
    app.add_systems(Update, tune_field);
    app.add_systems(
        Update,
        build_field
            .in_set(MapSet::Present)
            // Reads the world size the view's own sizing system leaves on each
            // shell to recover its pixel size, so it runs after both.
            .after(crate::systems::scale::size_by_distance)
            .after(crate::systems::scale::size_photometrically),
    );
}

/// The camera at the world origin that draws the field flat
#[derive(Component)]
struct FieldCamera;

/// The one mesh the whole field is rebuilt into each frame
#[derive(Resource)]
struct FieldMesh(Handle<Mesh>);

/// The mesh entity, so a view change can swap which material paints it
#[derive(Component)]
struct FieldMark;

/// The two ways the one field is painted, chosen by the view
///
/// The map's flat solid mark, blended over the galaxy, and the realistic
/// view's photometric glint, shaped by the point spread and added for the
/// camera's bloom to spread into a star.
#[derive(Resource)]
struct FieldMaterials {
    solid: Handle<StandardMaterial>,
    glint: Handle<StandardMaterial>,
}

/// The smallest a mark is drawn, as a radius in pixels
///
/// A star too far to resolve is still a point of light, not nothing, so it is
/// held to a sliver of a pixel rather than allowed to vanish.
const SMALLEST: f32 = 0.75;

/// Put the field's mesh, its two materials, and the origin camera up
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
    // out. Both are points a pixel or two across — too small to carry the
    // point-spread texture, which at that size samples its own faint edge and
    // vanishes; the shape is the point and, in the realistic view, the bloom
    // the field's camera spreads over it. The map's is a flat solid dot, its
    // fade in the alpha, blended over the galaxy. The realistic view's is the
    // blackbody colour at its HDR level, added and left for the bloom to grow
    // a bright star past its faint neighbours. Unlit either way: a mark is a
    // light, not a thing lit by one.
    let solid = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let glint = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Add,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(solid.clone()),
        RenderLayers::layer(FIELD_LAYER),
        // Every vertex is placed by hand each frame; there is no bound to cull
        // against and the whole field is one draw regardless.
        NoFrustumCulling,
        Transform::default(),
        Visibility::Visible,
        FieldMark,
    ));
    commands.insert_resource(FieldMesh(mesh));
    commands.insert_resource(FieldMaterials { solid, glint });

    // The camera that draws it, at the world origin so nothing it rasterises
    // sits at a galaxy coordinate. Orthographic at one unit to the pixel, over
    // the scene and under the annotation overlays, clearing neither so the map
    // shows through. `tune_field` gives it the realistic view's bloom.
    commands.spawn((
        Camera3d::default(),
        Hdr,
        Tonemapping::None,
        Exposure::SUNLIGHT,
        Camera {
            order: SHELLS_LAYER as isize,
            clear_color: ClearColorConfig::None,
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

/// Turn the field to the view and leave the shells' camera off
///
/// The field draws every view. The map wants flat solid marks and no bloom;
/// the realistic view wants the glint material and the star bloom to spread
/// its HDR points. Written only when the view moves.
fn tune_field(
    view: Res<View>,
    mut commands: Commands,
    mut field: Query<
        (Entity, &mut Camera),
        (With<FieldCamera>, Without<ShellsView>),
    >,
    mut shells: Query<&mut Camera, (With<ShellsView>, Without<FieldCamera>)>,
    mut mark: Query<&mut MeshMaterial3d<StandardMaterial>, With<FieldMark>>,
    palette: Res<FieldMaterials>,
) {
    if !view.is_changed() {
        return;
    }
    let realistic = matches!(*view, View::Realistic);
    if let Ok((entity, mut camera)) = field.single_mut() {
        camera.is_active = true;
        if realistic {
            commands.entity(entity).insert(STAR_BLOOM);
        } else {
            commands.entity(entity).remove::<Bloom>();
        }
    }
    if let Ok(mut camera) = shells.single_mut() {
        camera.is_active = false;
    }
    if let Ok(mut material) = mark.single_mut() {
        let wanted = if realistic { &palette.glint } else { &palette.solid };
        if material.0 != *wanted {
            material.0 = wanted.clone();
        }
    }
}

/// Rebuild the field mesh from where every visible system falls on screen
fn build_field(
    camera: Query<(&OrbitCamera, &Camera)>,
    shells: Query<
        (&System, &Transform, &Visibility, &Strength, Has<Filtered>),
        With<Shell>,
    >,
    view: Res<View>,
    exposure: Res<StarExposure>,
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
        // The pixel size the view's sizing system settled, read back off the
        // world size it left on the shell.
        let radius = (drawn.scale.x / per_pixel * 0.5).max(SMALLEST);

        // How much of the mark is left: dimmed where the filters exclude it,
        // faded as it goes out.
        let mut fade = strength.0.clamp(0., 1.);
        if filtered {
            fade *= dim.opacity();
        }
        let color = match *view {
            // A flat solid dot in the allegiance colour, its fade in the
            // alpha for the blend over the galaxy.
            View::Map => {
                let c = LinearRgba::from(hue(system, &color_by).color());
                [c.red, c.green, c.blue, fade]
            }
            // A photometric glint: the blackbody tint at its HDR level, shaped
            // by the point spread and spread by the bloom. The blend is
            // additive, so the fade scales the emission, not an alpha.
            View::Realistic => {
                let apparent = Magnitude(system.absolute_magnitude())
                    .apparent(Distance::light_years(
                        orbit.eye.distance(position),
                    ))
                    .0;
                let e = photometric_emissive(
                    system.temp_bucket(),
                    mag_step(apparent),
                    exposure.factor(),
                );
                [e.red * fade, e.green * fade, e.blue * fade, 1.]
            }
        };

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
