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
use bevy::image::{Image, ImageSampler};
use bevy::math::DVec3;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::post_process::bloom::Bloom;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat,
};
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

/// The side of the disc-mask texture, in texels
///
/// A mark is a handful of pixels at most until the camera is close enough to
/// descend, so the mask needs no more than this to round it, and small enough
/// that a linear fetch at a pixel or two still lands on the solid centre.
const MARK_TEXELS: u32 = 64;

/// Put the field's mesh, its two materials, and the origin camera up
fn spawn_field(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // A degenerate mesh to begin with; `build_field` swaps in a fresh one
    // holding the frame's stars each frame.
    let mesh =
        meshes.add(field_mesh(Vec::new(), Vec::new(), Vec::new(), Vec::new()));
    // A disc mask, so a mark is a round dot rather than the square its bare
    // quad would draw. White and opaque at the centre and clear at the
    // corners in every channel: the map's solid dot reads the alpha for its
    // blend over the galaxy, the realistic view's additive glint reads the
    // colour, so masking both keeps the corners off whichever way it is
    // painted.
    //
    // The per-vertex colour is the rest of what a mark comes out. The map's is
    // a flat solid dot, its fade in the alpha; the realistic view's is the
    // blackbody colour at its HDR level, added and left for the bloom to grow
    // a bright star past its faint neighbours. Unlit either way: a mark is a
    // light, not a thing lit by one.
    let mask = images.add(disc_mask());
    let solid = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(mask.clone()),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let glint = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(mask),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(solid.clone()),
        RenderLayers::layer(FIELD_LAYER),
        // Every vertex is placed by hand each frame; there is no bound to cull
        // against and the whole field is one draw regardless.
        NoFrustumCulling,
        Transform::default(),
        Visibility::Visible,
        FieldMark,
    ));
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
    mut field: Query<&mut Mesh3d, With<FieldMark>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;
    let Ok(mut mesh3d) = field.single_mut() else { return };

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

    mesh3d.0 = meshes.add(field_mesh(positions, uvs, colors, indices));
}

/// Build the field's mesh from the frame's quads, as a fresh asset each frame
///
/// Swapped in for the last rather than rewritten in place. Bevy's mesh
/// allocator frees and reallocates a mesh whose size changes and then copies
/// into the slab it just freed, which spends the frame logging a use-after-free
/// as the star count moves. A new handle sidesteps it — allocated, filled once,
/// and the old one dropped — at no more cost than the in-place path, which
/// reallocates anyway. Never empty: an empty mesh takes the same zero-size path
/// and draws the same error, so a field standing in for nothing carries one
/// degenerate triangle that rasterises to nothing.
fn field_mesh(
    mut positions: Vec<[f32; 3]>,
    mut uvs: Vec<[f32; 2]>,
    mut colors: Vec<[f32; 4]>,
    mut indices: Vec<u32>,
) -> Mesh {
    if positions.is_empty() {
        positions = vec![[0., 0., -1.]; 3];
        uvs = vec![[0., 0.]; 3];
        colors = vec![[0., 0., 0., 0.]; 3];
        indices = vec![0, 1, 2];
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// A round mask for the field's marks: white and opaque at the centre, clear
/// at the rim
///
/// The marks are screen-aligned quads, and painted bare they are squares.
/// Sampled as a material's base colour, this cuts each quad to the disc
/// inscribed in it — in every channel, so it rounds the solid mark's alpha and
/// the glint's colour alike. A texel and a half of fade at the rim antialiases
/// the edge; the centre holds solid at any size, so a mark a pixel across is
/// still a point of light rather than a sample of a faint edge that vanishes.
fn disc_mask() -> Image {
    let n = MARK_TEXELS;
    let centre = (n as f32 - 1.) / 2.;
    let mut data = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let dx = x as f32 - centre;
            let dy = y as f32 - centre;
            let dist = (dx * dx + dy * dy).sqrt();
            // Solid out to the quad's edge, then a texel and a half to clear.
            let mask = ((centre - dist) / 1.5).clamp(0., 1.);
            let value = (mask * 255.) as u8;
            let texel = ((y * n + x) * 4) as usize;
            data[texel] = value;
            data[texel + 1] = value;
            data[texel + 2] = value;
            data[texel + 3] = value;
        }
    }
    let mut image = Image::new(
        Extent3d { width: n, height: n, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mark is a disc, not the square its quad would draw bare
    ///
    /// A solid centre and clear corners, in every channel, so the box the eye
    /// used to read is gone whichever way the mark is painted.
    #[test]
    fn the_mark_mask_is_round() {
        let image = disc_mask();
        let n = MARK_TEXELS as usize;
        let data = image.data.expect("the mask carries its texels");

        let centre = ((n / 2) * n + n / 2) * 4;
        assert_eq!(
            &data[centre..centre + 4],
            &[255, 255, 255, 255],
            "the centre of the mark is not solid"
        );
        assert_eq!(
            &data[0..4],
            &[0, 0, 0, 0],
            "the corner of the quad is still drawn"
        );
    }
}
