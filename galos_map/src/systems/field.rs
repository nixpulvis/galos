//! The star field, drawn flat in screen space
//!
//! A system's mark cannot be a mesh at the system's true coordinate: at a
//! galaxy's scale the f32 clip transform `view_proj · model` tears it apart,
//! and which triangles survive turns with the camera, so the field blinks and
//! swims. So it is not drawn there. Every visible system is projected to a
//! pixel on the CPU in f64 — the stable anchor the names are already placed
//! by — and a screen-aligned quad is built at that pixel into one mesh, drawn
//! by a camera sitting at the world origin. Nothing that camera rasterises
//! carries a galaxy-scale coordinate, so there is no precision left to lose:
//! the field is exact and still at every zoom, pitch, and turn, and it is one
//! draw call however many stars there are.
//!
//! The [`Shell`] entities stay on the galaxy grid, where the map addresses
//! them for picking, filtering, and flying in, and they carry no mesh,
//! material or render layer of their own — nothing draws a shell where it
//! stands. This does all of the drawing, off a shell's position and the size
//! [`super::scale`] leaves on it.
//!
//! Which shells are drawn is `Visibility`, read as a plain value out of this
//! module's own query, and neither of the two the renderer settles will stand
//! in for it. A `ViewVisibility` is useless twice over: it is written during
//! rendering, a schedule after the `Update` this decides in, and it is never
//! written for a shell at all — nothing draws one, so nothing computes one,
//! and the answer is false for ever. An `InheritedVisibility` is propagated in
//! `PostUpdate`, so anything reading it during `Update` is reading last
//! frame's answer and a system just hidden or just shown is drawn a frame
//! wrong. Whether a shell is off the frame is a separate question again, and
//! [`screen_position`] answers it by giving nothing for what the camera cannot
//! see, so no visibility test is asked to cover it.

use crate::camera::{FIELD_LAYER, FIELD_ORDER, OrbitCamera, STAR_BLOOM};
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::bodies::spawn::Strength;
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::labels::{screen_position, world_per_pixel};
use crate::systems::scale::{Drawn, UNSEEN, View};
use crate::systems::spawn::{
    ColorBy, Shell, StarExposure, StarSprite, hue, mag_step,
    photometric_emissive,
};
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{Hdr, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::image::Image;
use bevy::math::DVec3;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use galos_photometry::{Distance, Magnitude};

pub fn plugin(app: &mut App) {
    // After `cut_star_psf`, whose `StarSprite` carries the point spread the
    // realistic glint is painted through.
    app.add_systems(
        Startup,
        spawn_field.after(crate::systems::spawn::cut_star_psf),
    );
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
pub(crate) struct FieldMark;

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
/// The map view's floor: a distant system is still a point of light, not
/// nothing, so its mark is held here rather than allowed to vanish. The
/// realistic view floors instead on the eye's own limit, drawing only the
/// stars that clear it (see [`build_field`]).
///
/// The last word over [`super::scale`]'s own far-field angle, which settles to
/// about half of this down an 1080 line window — so past roughly a thousand
/// light years this is what a mark is drawn at, and nearer than that the angle
/// is. The two are argued together in `scale::ANGULAR`, which is the place to
/// read before moving either.
///
/// The floor under a mark drawn by distance, and the last word over
/// `scale::ANGULAR` out where that angle falls under it.
///
/// Not the last word while the map is scaling by population; see [`floor`].
pub(crate) const SMALLEST: f32 = 0.75;

/// The smallest a mark is painted at, given whether the map is scaling
/// systems by population
///
/// [`SMALLEST`] ordinarily, and nothing at all in that mode: there a mark's
/// size is how many people live in the system, so a thinly populated one is
/// drawn smaller than an ordinary one — including smaller than a point of
/// light, out where an ordinary one is already that. Floored, every system
/// under some population came out the size of an ordinary one, which is a
/// floor saying something untrue about population.
///
/// Read by [`build_field`] and by [`super::pointing::size_indicators`], which
/// both put it through [`drawn_radius`], so what is painted and what is ringed
/// cannot come apart.
pub(crate) fn floor(scaling_by_population: bool) -> f32 {
    if scaling_by_population { 0. } else { SMALLEST }
}

/// The pixel radius to draw a system's mark at, or `None` to leave it undrawn
///
/// `raw` is the radius the view's sizing system settled, read off the world
/// size it left on the shell. The two views floor it apart:
///
/// - [`View::Map`] holds every system to `floor`, so a distant one stays a
///   point rather than a sub-pixel speck — unless the map is scaling by
///   population, where the size is the reading and there is no floor; see
///   [`floor`].
/// - [`View::Realistic`] draws only the stars that clear the eye's floor. One
///   that did not was shrunk by [`super::scale::size_photometrically`] to the
///   [`UNSEEN`] sliver, which is the sentinel that says it did not clear the
///   floor. Flooring it up to [`SMALLEST`] the way the map does would light
///   the whole sub-floor sky; so a sliver is dropped and every cleared star
///   keeps its own photometric radius, no floor.
fn mark_radius(view: &View, raw: f32, floor: f32) -> Option<f32> {
    match view {
        View::Map => Some(raw.max(floor)),
        // The sliver's own radius is `UNSEEN / 2`; anything larger cleared the
        // floor and is drawn at that radius.
        View::Realistic => (raw > UNSEEN * 0.5).then_some(raw),
    }
}

/// The pixel radius this draws a system's mark at, or `None` where it draws
/// none
///
/// `scale` is the world size the view's sizing system left on the shell and
/// `per_pixel` how much world a pixel covers where the system stands. The two
/// views write that size in different terms: the map's shell is a unit-radius
/// sphere, so its scale is the radius outright, where
/// [`super::scale::size_photometrically`] writes twice the star's pixel radius
/// as a world size on a unit quad. So the map reads the scale straight and the
/// realistic view halves it — read the same way, the map mark would come out
/// half the extent and stand inside the orbits it is meant to enclose.
///
/// `floor` is the smallest it may be painted at, which is [`floor`]'s to say.
///
/// The one answer, so that what the field paints and what
/// [`super::pointing::size_indicators`] rings and catches the pointer over
/// cannot come apart. A ring worked out from a second reading of the same
/// shell was drawn inside the mark it was meant to be around.
pub(crate) fn drawn_radius(
    view: &View,
    scale: f32,
    per_pixel: f32,
    floor: f32,
) -> Option<f32> {
    let raw = scale / per_pixel.max(f32::MIN_POSITIVE);
    mark_radius(
        view,
        match view {
            View::Map => raw,
            View::Realistic => raw * 0.5,
        },
        floor,
    )
}

/// Put the field's mesh, its two materials, and the origin camera up
fn spawn_field(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    sprite: Res<StarSprite>,
) {
    // A degenerate mesh to begin with; `build_field` swaps in a fresh one
    // holding the frame's stars each frame.
    let mesh =
        meshes.add(field_mesh(Vec::new(), Vec::new(), Vec::new(), Vec::new()));
    // The two marks are cut to a round profile, so a bare quad never shows as
    // a square. The map's is a disc: a flat solid dot, its fade in the alpha
    // for the blend over the galaxy. The realistic view's is the shared point
    // spread ([`super::spawn::star_psf`]) — a bright core falling to nothing —
    // so a star is a cored glow the camera's bloom spreads into a glint,
    // never the flat disc a hard mask draws. The profile peaks solid at its
    // centre, so a mark a pixel or two across still lands a bright point
    // rather than a sample of its faint edge.
    //
    // The per-vertex color is the rest: the map's fade in the alpha, the
    // realistic view's blackbody color at its HDR level, added for the bloom
    // to grow a bright star past its faint neighbours. Unlit either way: a
    // mark is a light, not a thing lit by one.
    let solid = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(
            images.add(crate::systems::glow::gaussian_mask()),
        ),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let glint = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(sprite.psf.clone()),
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
        Camera {
            order: FIELD_ORDER,
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

/// Turn the field to the view
///
/// The field draws every view, and what changes between them is which
/// material paints a mark: the map's flat solid disc or the realistic view's
/// photometric glint. Written only when the view moves.
///
/// **The bloom stays on in both**, where the map used to have it taken off.
/// The field camera draws [`super::glow`]'s aggregate splats beside the marks
/// now, and bloom over an additive HDR field is not a post-effect on it — it
/// is the point spread that turns a chain of few-pixel splats into a glow, and
/// without it the political field reads as a mosaic of dots. Bloom answers to
/// brightness, which is what makes one setting serve both: a splat's linear
/// deposit runs past one where it is dense and blooms hard, and the map's
/// marks are a blended colour under one and barely register — so the halo
/// [`STAR_BLOOM`] was tuned down to avoid lands on the field and not on the
/// symbols. If a mark's own halo ever does show, the escape is a camera and a
/// layer of its own for the field rather than a second tuning here, bloom
/// being per camera and not per pass.
fn tune_field(
    view: Res<View>,
    mut commands: Commands,
    field: Query<Entity, With<FieldCamera>>,
    mut mark: Query<&mut MeshMaterial3d<StandardMaterial>, With<FieldMark>>,
    palette: Res<FieldMaterials>,
) {
    if !view.is_changed() {
        return;
    }
    let realistic = matches!(*view, View::Realistic);
    if let Ok(entity) = field.single() {
        commands.entity(entity).insert(STAR_BLOOM);
    }
    if let Ok(mut material) = mark.single_mut() {
        let wanted = if realistic { &palette.glint } else { &palette.solid };
        if material.0 != *wanted {
            material.0 = wanted.clone();
        }
    }
}

/// Rebuild the field mesh from where every visible system falls on screen
#[expect(
    clippy::too_many_arguments,
    reason = "every shell, the view that sizes it, the palette and gains that \
              weigh it, and the mesh it is laid into"
)]
pub(crate) fn build_field(
    camera: Query<(&OrbitCamera, &Camera)>,
    shells: Query<
        (
            &System,
            &Drawn,
            &Visibility,
            &Strength,
            Has<Filtered>,
            Option<&crate::systems::route::Thinned>,
        ),
        With<Shell>,
    >,
    view: Res<View>,
    scale_population: Res<crate::systems::scale::ScalePopulation>,
    exposure: Res<StarExposure>,
    color_by: Res<ColorBy>,
    dim: Res<DimTo>,
    gains: Res<crate::systems::glow::Gains>,
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
    // Whether a mark may be painted under a pixel, which is the population
    // scale's to say; see [`floor`].
    let floor = floor(scale_population.0);

    let half = viewport * 0.5;
    for (system, drawn, visibility, strength, filtered, thinned) in &shells {
        // Out of the spyglass is not drawn; `super::visibility` says which.
        if *visibility == Visibility::Hidden {
            continue;
        }
        let position = DVec3::from(system.position);
        let Some(at) = screen_position(orbit, cot_half_fov, viewport, position)
        else {
            continue;
        };
        let away =
            crate::space::metres(orbit.eye_from(position)).length() as f32;
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, away.max(1.));
        // The pixel radius the view's sizing system settled, read back off the
        // world size it left on the shell, then floored or dropped by the
        // view; see [`drawn_radius`].
        let Some(radius) = drawn_radius(&view, drawn.0, per_pixel, floor)
        else {
            continue;
        };

        // How much of the mark is left: dimmed where the filters exclude it,
        // faded as it goes out.
        let mut fade = strength.0.clamp(0., 1.);
        if filtered {
            fade *= dim.opacity();
        }
        // What a route left of it, where it is on one: a hop that has closed
        // up on the hop before it is a mark over the line rather than a system
        // anyone can see. See [`crate::systems::route::Thinned`].
        if let Some(thinned) = thinned {
            fade *= thinned.0.clamp(0., 1.);
        }
        if fade <= 0. {
            continue;
        }
        let color = match *view {
            // A deposit, not a dot. A mark used to be an opaque disc in the
            // allegiance colour with its fade in the alpha, which over a field
            // that is itself light did not add to it but replaced it: a grey
            // system at `srgb(0.15)` composited over a bright region punched a
            // hole in the very light it was standing in. Laid down through the
            // field's own law instead, a mark adds what its system is worth and
            // the two read as one picture.
            View::Map => {
                let tone = hue(system, &color_by);
                let c = LinearRgba::from(tone.color());
                let weight = crate::systems::glow::mark_weight(
                    tone,
                    system.population > 0,
                    &gains,
                ) * fade;
                let p = crate::systems::glow::peak(
                    Vec3::new(c.red, c.green, c.blue) * weight,
                    radius,
                );
                [p.x, p.y, p.z, 1.]
            }
            // A photometric glint: the blackbody tint at its HDR level, spread
            // by the bloom, added rather than blended so the fade scales the
            // emission, not an alpha.
            View::Realistic => {
                let apparent = Magnitude(system.absolute_magnitude())
                    .apparent(Distance::light_years(
                        orbit.eye().distance(position),
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
///
/// [`RenderAssetUsages::RENDER_WORLD`], so the frame's copy is freed once it
/// has been extracted. Nothing in the main world reads these vertices back —
/// the mesh is written whole and never sampled, picking going through the
/// projected [`crate::systems::bodies::spawn::Places`] instead — and at four
/// vertices per visible system, rebuilt every frame, the retained copy is the
/// larger half of what the field costs in memory.
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
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The realistic view draws only the stars that clear the eye's floor
    ///
    /// A star below it is shrunk to the [`UNSEEN`] sliver by
    /// `size_photometrically`; drawn as a mark it lit the whole sub-floor sky.
    /// So a sliver is not drawn, a cleared star keeps its own radius with no
    /// floor, and only the map holds every system up to the floor it is given.
    #[test]
    fn the_realistic_view_drops_a_sub_floor_star() {
        let floor = floor(false);

        assert_eq!(mark_radius(&View::Realistic, UNSEEN * 0.5, floor), None);
        assert_eq!(mark_radius(&View::Realistic, 0.6, floor), Some(0.6));
        assert_eq!(mark_radius(&View::Realistic, 4.0, floor), Some(4.0));
        assert_eq!(
            mark_radius(&View::Map, UNSEEN * 0.5, floor),
            Some(SMALLEST)
        );
        assert_eq!(mark_radius(&View::Map, 3.0, floor), Some(3.0));
    }

    /// And scaling by population takes the map's floor off
    ///
    /// In that mode a mark's size is how many people live in the system, so a
    /// thinly populated one is drawn smaller than an ordinary one — under a
    /// pixel out where an ordinary one is already about that. Held up to the
    /// floor, every system under some population drew the size of an ordinary
    /// one.
    #[test]
    fn the_population_scale_paints_under_the_floor() {
        let floor = floor(true);
        let speck = SMALLEST * 0.2;

        assert_eq!(mark_radius(&View::Map, speck, floor), Some(speck));
        assert_eq!(mark_radius(&View::Map, 3.0, floor), Some(3.0));
    }
}
