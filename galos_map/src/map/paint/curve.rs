//! The field's curve, struck per pixel
//!
//! The splats of the map view are light, summed linearly — a crowd is the
//! light of its systems, wherever in the galaxy it stands — and that runs
//! over some thirty stops in one frame where a display carries a handful.
//! So the map view's field cameras draw the splats into two targets of their
//! own, the let-through and what the filters exclude — and the volume
//! ([`crate::map::paint::volume`]) into two more at half the resolution,
//! read back up and added to them — and this lays them over the galaxy
//! through one curve:
//!
//! - **The dial** ([`FieldExposure`]) is a gain, in stops, on the light
//!   before the curve reads it, **the glow's own dial** ([`GlowBrightness`])
//!   a gain on the field alone, and **the tilt** ([`EVEN_AT`]) a gain the
//!   reach sets on the field alone, so the field holds its level under the
//!   marks as the camera comes in and goes out.
//! - **The curve** ([`FieldCurve`]) takes a pixel's light, in stops over an
//!   average system's mark ([`Gains::average`]), to a display level: a
//!   cubic through knots the settings drag up and down, running one way
//!   only between each two of them, linear in the light under the first and
//!   held over the last.
//! - **The dim** is spent after it, on the excluded target as a whole, so a
//!   filter dims the field as far as it dims a mark.
//!
//! Struck on a pixel's brightest channel and applied to all three, so what
//! moves is the brightness and not the colour.
//!
//! **Per pixel and after the sum, which is the point.** A curve struck per
//! cell on its own count made a star's light depend on what else its cell
//! held: equal counts of two classes drew at a fifth of one another where one
//! stood among the well-scanned crowd and the other in the sparse disc. Per
//! pixel, the only thing that compresses a star's light is what actually
//! lands on its pixel.
//!
//! **The marks go through the same curve on their own** ([`Exposed`]), and
//! are laid over the field after it rather than summed into it. A system
//! drawn as itself is one system, at the level the curve gives one: summed
//! into the crowd behind it, a mark three thousand light years out sat
//! under the wash it stood in and the sky there read as a blur.
//!
//! The realistic view draws none of this: its field camera draws straight to
//! the window, where its own exposure and the bloom are what bring the sky
//! onto the display.

use crate::map::camera::{
    CURVE_LAYER, CURVE_ORDER, DIMMED_LAYER, FIELD_ORDER, VOLUME_DIMMED_LAYER,
    VOLUME_LAYER,
};
use crate::map::filter::DimTo;
use crate::map::paint::field::FieldCamera;
use crate::map::paint::glow::Gains;
use crate::map::paint::sizing::View;
use bevy::asset::embedded_asset;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{Hdr, ImageRenderTarget, RenderTarget, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::ecs::system::SystemParam;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureFormat,
};
use bevy::shader::ShaderRef;
use bevy::window::{PrimaryWindow, WindowRef};

pub fn plugin(app: &mut App) {
    embedded_asset!(app, "curve.wgsl");
    app.add_plugins(MaterialPlugin::<CurveMaterial>::default());
    app.init_resource::<FieldExposure>();
    app.init_resource::<GlowBrightness>();
    app.init_resource::<FieldCurve>();
    app.add_systems(Startup, spawn_curve);
    app.add_systems(Update, (fit_targets, route_field, set_curve).chain());
}

/// How many stops the field is lifted ahead of its curve
///
/// The one dial over how loud an unresolved galaxy is, which is a reading
/// and not a fact: the map is a political instrument at one setting and a
/// picture of where anybody has been at another, and neither is wrong. A
/// gain on the light before [`FieldCurve`] reads it, so opening it slides
/// the frame up the curve: the faint half comes up, and the shoulder goes on
/// holding the packed core under white.
///
/// The marks are on it, being in the same light: a system drawn as itself
/// and the field standing in for it are one picture, and a dial that moved
/// one of the two without the other would have them come apart as the
/// camera came in.
#[derive(Resource, Clone, Copy, PartialEq)]
pub struct FieldExposure(pub f32);

/// The dial rests at zero: neutral, the tuned look, stops either way from
/// there. The same rest and the same units the realistic sky's own exposure
/// is offered at ([`crate::map::galaxy::spawn::StarExposure`]).
impl Default for FieldExposure {
    fn default() -> FieldExposure {
        FieldExposure(0.)
    }
}

/// How many stops the glow is lifted or held down against the marks
///
/// The field alone, ahead of the curve: the marks stay where
/// [`FieldExposure`] puts them, so this says how loud the crowd behind a
/// system is and nothing about how the system itself reads. Where the glow
/// runs bright enough to wash out the marks laid over it — along an axis
/// whose hues come out strong, or in a dense patch — this holds it down
/// without dimming the marks with it, which the field exposure cannot. Rests
/// at nought, the tuned look.
#[derive(Resource, Clone, Copy, Default, PartialEq)]
pub struct GlowBrightness(pub f32);

/// The reach at which the field is exposed as the marks are, in light years
///
/// **The field's level tracks the reach, and the tilt takes that out of the
/// dial.** Every system is a mark's light wherever it is drawn, so a pixel
/// of the field holds as much light as there are systems behind it —
/// thousands with the galaxy seen whole, and a few inside the bubble — while
/// a mark is one system wherever it stands. Measured over `.index/full`, the
/// middling lit pixel of the field stands six stops over an average mark
/// with the galaxy seen whole and one stop over from three thousand light
/// years back. So the field is held down as the reach widens — to a stop
/// under an average mark with the galaxy seen whole, where the field is the
/// picture and the merged marks over it are a sampling of it, and to two and
/// a half under from three thousand light years, where the marks are the
/// systems themselves and read over it. Held three stops under at both, the
/// galaxy seen whole was a grain of merged marks over a dark wash.
///
/// A fixed function of a number the map already knows: nothing here adapts,
/// nothing lags, and the same reach is the same brightness every time it
/// comes round. Global to the frame, so it moves no star against another.
/// Under this reach the field is a handful of systems and the tilt holds.
const EVEN_AT: f32 = 16.;

/// And how many stops the field is held down for each octave the reach
/// widens: the three and a half stops the readings above are brought
/// together by, over the five and three quarter octaves between their
/// reaches.
const TILT: f32 = 0.59;

/// How many stops the field is moved at a reach of `reach` light years; see
/// [`EVEN_AT`]. Never lifted, and a reach of nothing, the spyglass not set,
/// moves it not at all.
fn tilt(reach: f32) -> f32 {
    if reach <= EVEN_AT {
        return 0.;
    }
    -TILT * (reach / EVEN_AT).log2()
}

/// How many knots the curve is drawn through
pub(crate) const KNOTS: usize = 7;

/// Where the knots stand, in stops of light over an average system's mark
///
/// Read against the average system along the axis drawn
/// ([`Gains::average`]), so one curve exposes every axis alike. An average
/// mark at nought, which is about where a star's own mark lands along star
/// class, and four stops apart either side of it: measured over
/// `.index/full` with the galaxy seen whole, the field's middling lit pixel
/// stands a stop under and its brightest thousandth thirteen to fourteen
/// over, so sixteen leaves the core room under its hold.
pub(crate) const STOPS: [f32; KNOTS] = [-8., -4., 0., 4., 8., 12., 16.];

/// The field's curve: a display level for every light the field lays
///
/// One level a knot, sRGB-encoded so equal steps are equal to the eye, at
/// [`STOPS`], each anywhere in the display's range. Between the knots a
/// cubic that runs one way only from one knot to the next, so dragging one
/// bends the curve smoothly through it — past its neighbours too, to a peak
/// or a trough — and never overshoots a knot; under the first, linear in
/// the light down to black; over the last, held.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct FieldCurve {
    pub levels: [f32; KNOTS],
}

impl Default for FieldCurve {
    fn default() -> FieldCurve {
        FieldCurve { levels: [0.03, 0.14, 0.4, 0.6, 0.76, 0.89, 0.97] }
    }
}

impl FieldCurve {
    /// Move knot `knot` to `level`, held to the display's range and to
    /// nothing else: a knot may be dragged past its neighbours, and the
    /// curve then falls between them, which is a reading the curve is free
    /// to give — a band of the field picked out, or a crowd let fall away.
    pub(crate) fn set(&mut self, knot: usize, level: f32) {
        self.levels[knot] = level.clamp(0., 1.);
    }

    /// The curve's slope through each knot, in levels a stop
    ///
    /// Fritsch and Carlson's: the mean of the two secants either side, nought
    /// where they disagree in sign or either is flat, and held inside the
    /// circle of radius three that keeps a cubic from overshooting its knots.
    /// So each span runs one way only, from one knot's level to the next's,
    /// and a knot dragged past its neighbour is a peak or a trough the curve
    /// turns flat at rather than a wiggle that overshoots it. Nought at the
    /// last knot, so the curve comes into its hold flat.
    fn tangents(&self) -> [f32; KNOTS] {
        let mut secant = [0f32; KNOTS - 1];
        for k in 0..KNOTS - 1 {
            secant[k] = (self.levels[k + 1] - self.levels[k])
                / (STOPS[k + 1] - STOPS[k]);
        }
        let mut slope = [0f32; KNOTS];
        slope[0] = secant[0];
        for k in 1..KNOTS - 1 {
            slope[k] = match secant[k - 1] * secant[k] > 0. {
                true => (secant[k - 1] + secant[k]) / 2.,
                false => 0.,
            };
        }
        slope[KNOTS - 1] = 0.;
        for k in 0..KNOTS - 1 {
            if secant[k] == 0. {
                slope[k] = 0.;
                slope[k + 1] = 0.;
                continue;
            }
            let a = slope[k] / secant[k];
            let b = slope[k + 1] / secant[k];
            let r = a * a + b * b;
            if r > 9. {
                let t = 3. / r.sqrt();
                slope[k] = t * a * secant[k];
                slope[k + 1] = t * b * secant[k];
            }
        }
        slope
    }

    /// The display level, sRGB-encoded, `stops` over an average mark
    ///
    /// The shader's own function, written twice: the settings draw this, and
    /// the shader is handed the knots and tangents it is drawn from.
    pub(crate) fn level(&self, stops: f32) -> f32 {
        self.level_along(&self.tangents(), stops)
    }

    /// [`Self::level`] with the tangents worked out already, for a caller
    /// asking the curve many times over.
    fn level_along(&self, slope: &[f32; KNOTS], stops: f32) -> f32 {
        if stops <= STOPS[0] {
            return encode(decode(self.levels[0]) * (stops - STOPS[0]).exp2());
        }
        for k in 0..KNOTS - 1 {
            if stops <= STOPS[k + 1] {
                let h = STOPS[k + 1] - STOPS[k];
                let t = (stops - STOPS[k]) / h;
                let (t2, t3) = (t * t, t * t * t);
                return (2. * t3 - 3. * t2 + 1.) * self.levels[k]
                    + (t3 - 2. * t2 + t) * h * slope[k]
                    + (-2. * t3 + 3. * t2) * self.levels[k + 1]
                    + (t3 - t2) * h * slope[k + 1];
            }
        }
        self.levels[KNOTS - 1]
    }

    /// The knots as the shader takes them: where, how high, and how steep
    fn knots(&self) -> [Vec4; KNOTS] {
        let slope = self.tangents();
        std::array::from_fn(|k| {
            Vec4::new(STOPS[k], self.levels[k], slope[k], 0.)
        })
    }
}

/// An sRGB-encoded level as linear light
fn decode(level: f32) -> f32 {
    let v = level.clamp(0., 1.);
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Linear light as an sRGB-encoded level
fn encode(light: f32) -> f32 {
    let v = light.clamp(0., 1.);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1. / 2.4) - 0.055 }
}

/// Everything the frame is exposed by: the curve, the dials, the reach the
/// field's tilt is read off, and the unit the curve is read in
#[derive(SystemParam)]
pub(crate) struct Exposing<'w> {
    curve: Res<'w, FieldCurve>,
    exposure: Res<'w, FieldExposure>,
    glow: Res<'w, GlowBrightness>,
    gains: Res<'w, Gains>,
    spyglass: Res<'w, crate::map::galaxy::Spyglass>,
}

impl Exposing<'_> {
    /// Whether what the marks are exposed by moved since the system asking
    /// last ran: the glow's dial and the tilt are the field's alone
    pub(crate) fn is_changed(&self) -> bool {
        self.curve.is_changed()
            || self.exposure.is_changed()
            || self.gains.is_changed()
    }

    /// How the marks are exposed: the dial, and no tilt
    pub(crate) fn marks(&self) -> Exposed {
        self.at(self.exposure.0)
    }

    /// How the field is exposed: the dial, the glow's own, and the tilt the
    /// reach sets
    fn field(&self) -> Exposed {
        self.at(self.exposure.0 + self.glow.0 + tilt(self.spyglass.radius))
    }

    fn at(&self, stops: f32) -> Exposed {
        Exposed {
            curve: *self.curve,
            slope: self.curve.tangents(),
            gain: stops.exp2() / (self.gains.mark * self.gains.average),
        }
    }
}

/// The curve and the gain ahead of it, as the frame is exposed
#[derive(Clone, Copy, Debug)]
pub(crate) struct Exposed {
    curve: FieldCurve,
    slope: [f32; KNOTS],
    /// Linear light to the curve's unit, the dial and the tilt in it.
    gain: f32,
}

impl Exposed {
    /// A light as the display carries it, through the curve on its own
    ///
    /// The shader's own `through`, for what is laid over the field after its
    /// curve rather than summed into it: a mark, at the level the curve gives
    /// one system standing alone. Struck on the brightest channel and applied
    /// to all three, as the field is.
    pub(crate) fn through(&self, light: Vec3) -> Vec3 {
        let c = light.max(Vec3::ZERO);
        let top = c.max_element();
        if top <= 0. || !top.is_finite() {
            return Vec3::ZERO;
        }
        let level =
            self.curve.level_along(&self.slope, (top * self.gain).log2());
        c * (decode(level) / top)
    }
}

/// The targets the map view's field is drawn into
#[derive(Resource)]
pub(crate) struct FieldTargets {
    /// The marks and splats the filters let through.
    pub(crate) lit: Handle<Image>,
    /// And what they exclude, at full; dimmed by [`CurveMaterial`].
    pub(crate) dimmed: Handle<Image>,
    /// The volume ([`crate::map::paint::volume`]), let through and
    /// excluded, at half the frame's resolution and added to the two above
    /// as the curve reads them.
    pub(crate) volume: [Handle<Image>; 2],
}

/// The camera that draws what the filters exclude into its target
#[derive(Component)]
struct DimmedCamera;

/// A camera that marches the volume into its half-resolution target
#[derive(Component)]
struct VolumeCamera;

/// The camera that lays the two targets over the galaxy through the curve
#[derive(Component)]
struct CurveCamera;

/// The one quad it lays them with, sized to the frame
#[derive(Component)]
struct CurveQuad;

/// The curve, the dial with the tilt in it, and the dim, as the shader
/// takes them
#[derive(ShaderType, Clone, Copy, Debug, PartialEq)]
struct CurveUniform {
    knots: [Vec4; KNOTS],
    gain: f32,
    dim: f32,
    _pad0: f32,
    _pad1: f32,
}

/// What lays the field's targets onto the frame
#[derive(Asset, TypePath, AsBindGroup, Clone)]
struct CurveMaterial {
    #[uniform(0)]
    curve: CurveUniform,
    #[texture(1)]
    lit: Handle<Image>,
    #[texture(2)]
    dimmed: Handle<Image>,
    /// The volume's let-through and excluded, sampled up from half the
    /// frame's resolution through the first one's sampler.
    #[texture(3)]
    #[sampler(5)]
    volume_lit: Handle<Image>,
    #[texture(4)]
    volume_dimmed: Handle<Image>,
}

impl Material for CurveMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://galos_map/map/paint/curve.wgsl".into()
    }

    /// Added over the galaxy, as the field was drawn straight onto it before
    /// there was a curve: light laid onto the scene, not a sheet over it.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
}

/// A target the field can be drawn into: half floats, as the frame itself
/// is, sized to the frame
fn target(size: UVec2) -> Image {
    let mut image = Image::new_target_texture(
        size.x.max(1),
        size.y.max(1),
        TextureFormat::Rgba16Float,
        None,
    );
    // Read between texels where it is read at another size: the volume's.
    image.sampler = ImageSampler::linear();
    image
}

/// How large the volume's targets are for a frame of `size`: half, rounded
/// up, and the scale factor that keeps them the frame's logical size, which
/// is what the volume is laid out in.
fn halved(size: UVec2, scale: f32) -> (UVec2, f32) {
    let half = (size + UVec2::ONE) / 2;
    (half, scale * half.y as f32 / size.y.max(1) as f32)
}

/// Put the targets, the dimmed camera, the volume's cameras and the curve's
/// camera and quad up
fn spawn_curve(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<CurveMaterial>>,
    curve: Res<FieldCurve>,
) {
    let lit = images.add(target(UVec2::ONE));
    let dimmed = images.add(target(UVec2::ONE));
    let volume =
        [images.add(target(UVec2::ONE)), images.add(target(UVec2::ONE))];

    // The field camera's twin, drawing what the filters exclude. Clears to
    // black, its target holding nothing but this frame's excluded light.
    commands.spawn((
        Camera3d::default(),
        Hdr,
        Tonemapping::None,
        Camera {
            order: FIELD_ORDER,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        RenderTarget::Image(ImageRenderTarget::from(dimmed.clone())),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::WindowSize,
            ..OrthographicProjection::default_3d()
        }),
        RenderLayers::layer(DIMMED_LAYER),
        DimmedCamera,
    ));

    // And the volume's two, each into a target at half the frame's
    // resolution, cleared to black.
    for (layer, into) in
        [VOLUME_LAYER, VOLUME_DIMMED_LAYER].into_iter().zip(&volume)
    {
        commands.spawn((
            Camera3d::default(),
            Hdr,
            Tonemapping::None,
            // Nothing on these layers has an edge to smooth.
            Msaa::Off,
            Camera {
                order: FIELD_ORDER,
                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
            RenderTarget::Image(ImageRenderTarget::from(into.clone())),
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::WindowSize,
                ..OrthographicProjection::default_3d()
            }),
            RenderLayers::layer(layer),
            VolumeCamera,
        ));
    }

    // Over the scene and under the annotations, as the field drew before
    // there was a curve; clearing nothing, so the galaxy stands under it.
    commands.spawn((
        Camera3d::default(),
        Hdr,
        Tonemapping::None,
        Camera {
            order: CURVE_ORDER,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::WindowSize,
            ..OrthographicProjection::default_3d()
        }),
        RenderLayers::layer(CURVE_LAYER),
        CurveCamera,
    ));
    commands.spawn((
        Mesh3d(meshes.add(Rectangle::new(1., 1.))),
        MeshMaterial3d(materials.add(CurveMaterial {
            curve: CurveUniform {
                knots: curve.knots(),
                gain: 0.,
                dim: 0.,
                _pad0: 0.,
                _pad1: 0.,
            },
            lit: lit.clone(),
            dimmed: dimmed.clone(),
            volume_lit: volume[0].clone(),
            volume_dimmed: volume[1].clone(),
        })),
        RenderLayers::layer(CURVE_LAYER),
        NoFrustumCulling,
        Transform::from_xyz(0., 0., -1.),
        CurveQuad,
    ));
    commands.insert_resource(FieldTargets { lit, dimmed, volume });
}

/// The field's two cameras, each drawing into its own target
type Drawing = Or<(With<FieldCamera>, With<DimmedCamera>)>;

/// The cameras that stand down outside the map view: the dimmed one, the
/// volume's, and the curve's own
type MapOnly = (
    Or<(With<DimmedCamera>, With<VolumeCamera>, With<CurveCamera>)>,
    Without<FieldCamera>,
);

/// Hold the targets at the frame's own size, the volume's at half of it,
/// and the quad over the frame
///
/// Only where the window has moved. A resize reallocates the targets, and
/// the curve reads a pixel of the frame as a texel of the first two.
///
/// **And the curve's material is touched when it does.** A material's bind
/// group holds the textures it was prepared with, and the renderer prepares
/// it again only when the material itself changes, never when an image it
/// reads does. Left alone, the curve went on reading the targets as they
/// stood before the resize — a field drawn for the old window — until
/// something else moved the material: the camera, by way of the reach.
fn fit_targets(
    window: Query<&Window, With<PrimaryWindow>>,
    targets: Option<Res<FieldTargets>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<CurveMaterial>>,
    mut quad: Query<
        (&mut Transform, &MeshMaterial3d<CurveMaterial>),
        With<CurveQuad>,
    >,
    mut cameras: Query<&mut RenderTarget, (Drawing, Without<VolumeCamera>)>,
    mut marching: Query<&mut RenderTarget, With<VolumeCamera>>,
) {
    let (Ok(window), Some(targets)) = (window.single(), targets) else {
        return;
    };
    let size = window.physical_size().max(UVec2::ONE);
    let scale = window.scale_factor();
    let (half, half_scale) = halved(size, scale);
    let sized = [&targets.lit, &targets.dimmed]
        .map(|handle| (handle, size))
        .into_iter()
        .chain(targets.volume.iter().map(|handle| (handle, half)));
    let mut resized = false;
    for (handle, size) in sized {
        if images.get(handle).is_some_and(|image| image.size() != size)
            && let Some(mut image) = images.get_mut(handle)
        {
            image.resize(Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            });
            resized = true;
        }
    }
    // An image target has no scale factor of its own, and the field is laid
    // out in the window's logical pixels; told the window's, its cameras see
    // the frame the marks were placed in. The volume's targets are half as
    // many pixels over the same frame, so half the scale.
    let told = cameras
        .iter_mut()
        .map(|target| (target, scale))
        .chain(marching.iter_mut().map(|target| (target, half_scale)));
    for (mut target, scale) in told {
        if let RenderTarget::Image(image) = &*target
            && image.scale_factor != scale
        {
            let handle = image.handle.clone();
            *target = RenderTarget::Image(ImageRenderTarget {
                handle,
                scale_factor: scale,
            });
        }
    }
    let logical = Vec3::new(window.width(), window.height(), 1.);
    for (mut transform, material) in &mut quad {
        if transform.scale != logical {
            transform.scale = logical;
        }
        if resized {
            // Taken as changed, which is all it takes to be prepared again
            // over the targets as they now stand. A borrow alone marks
            // nothing: only a mutable one through it does.
            let _ = materials.get_mut(&material.0).map(|it| it.into_inner());
        }
    }
}

/// Send the field camera where the view draws it, and the curve on or off
///
/// The map view draws the field into [`FieldTargets::lit`], clearing it to
/// black each frame, and lays it over the galaxy through the curve. The
/// realistic view draws it straight onto the window over the scene, as it
/// always has, and the curve and the dimmed camera stand down.
fn route_field(
    view: Res<View>,
    targets: Option<Res<FieldTargets>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut field: Query<(&mut Camera, &mut RenderTarget), With<FieldCamera>>,
    mut others: Query<&mut Camera, MapOnly>,
) {
    let Some(targets) = targets else { return };
    if !view.is_changed() && !targets.is_added() {
        return;
    }
    let map = matches!(*view, View::Map);
    let scale = window.single().map_or(1., Window::scale_factor);
    for (mut camera, mut target) in &mut field {
        if map {
            *target = RenderTarget::Image(ImageRenderTarget {
                handle: targets.lit.clone(),
                scale_factor: scale,
            });
            camera.clear_color = ClearColorConfig::Custom(Color::NONE);
        } else {
            *target = RenderTarget::Window(WindowRef::Primary);
            camera.clear_color = ClearColorConfig::None;
        }
    }
    for mut camera in &mut others {
        camera.is_active = map;
    }
}

/// Hand the curve, the dial, the tilt and the dim to the shader, where any
/// moved
///
/// Read every frame, the reach moving with the camera, and written only
/// where what the shader would be handed is new: a material written is a
/// material uploaded again.
fn set_curve(
    exposing: Exposing,
    dim: Res<DimTo>,
    quad: Query<&MeshMaterial3d<CurveMaterial>, With<CurveQuad>>,
    mut materials: ResMut<Assets<CurveMaterial>>,
) {
    let Ok(handle) = quad.single() else { return };
    let exposed = exposing.field();
    let wanted = CurveUniform {
        knots: exposed.curve.knots(),
        gain: exposed.gain,
        dim: dim.opacity(),
        _pad0: 0.,
        _pad1: 0.,
    };
    if materials.get(&handle.0).is_some_and(|it| it.curve == wanted) {
        return;
    }
    if let Some(mut material) = materials.get_mut(&handle.0) {
        material.curve = wanted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every span of the curve runs one way only, from one knot's level to
    /// the next's, wherever the knots are dragged — a peak and a trough
    /// included. A cubic through knots can overshoot them, and a field drawn
    /// through an overshoot is a band brighter or darker than any knot asked
    /// for, past white or under black at the ends of the range.
    #[test]
    fn every_span_runs_between_its_knots() {
        let curves = [
            FieldCurve::default(),
            FieldCurve { levels: [0., 0., 0.9, 0.9, 0.91, 1., 1.] },
            FieldCurve { levels: [0.3, 0.31, 0.32, 0.95, 0.96, 0.97, 0.98] },
            FieldCurve { levels: [0., 0.01, 0.02, 0.03, 0.04, 0.05, 1.] },
            FieldCurve { levels: [0.1, 0.9, 0.05, 1., 0., 0.6, 0.2] },
            FieldCurve { levels: [1., 0.8, 0.6, 0.4, 0.2, 0.1, 0.] },
        ];
        for curve in curves {
            for span in 0..KNOTS - 1 {
                let (a, b) = (curve.levels[span], curve.levels[span + 1]);
                let mut last = a;
                for step in 0..=200 {
                    let stops = STOPS[span]
                        + (STOPS[span + 1] - STOPS[span]) * step as f32 / 200.;
                    let level = curve.level(stops);
                    assert!(
                        level >= a.min(b) - 1e-5 && level <= a.max(b) + 1e-5,
                        "{curve:?} left {a}..{b} for {level} at {stops} stops"
                    );
                    assert!(
                        (level - last) * (b - a) >= -1e-5,
                        "{curve:?} turned back from {last} to {level} at \
                         {stops} stops"
                    );
                    last = level;
                }
            }
        }
    }

    /// Through every knot, and with no step anywhere along it: not at a
    /// knot, not where the linear toe meets the first, not where the hold
    /// takes over from the last.
    #[test]
    fn the_curve_meets_its_knots_without_a_step() {
        let curve = FieldCurve::default();
        for (stops, level) in STOPS.iter().zip(curve.levels) {
            assert!(
                (curve.level(*stops) - level).abs() < 1e-5,
                "{} at a knot set to {level}",
                curve.level(*stops)
            );
        }
        let mut last = curve.level(-20.);
        for step in 1..=4000 {
            let stops = -20. + step as f32 * 0.01;
            let level = curve.level(stops);
            assert!(
                (level - last).abs() < 0.01,
                "a step from {last} to {level} at {stops} stops"
            );
            last = level;
        }
        assert_eq!(curve.level(40.), curve.levels[KNOTS - 1], "the hold moved");
        assert!(curve.level(-60.) < 1e-6, "the toe does not reach black");
    }

    /// A wider reach is never a brighter field, and the field is never
    /// lifted over the marks: under the reach it meets them at, it holds,
    /// and a reach not set moves it not at all.
    #[test]
    fn the_tilt_holds_the_field_down_as_the_reach_widens() {
        assert_eq!(tilt(EVEN_AT), 0.);
        assert_eq!(tilt(EVEN_AT / 100.), 0.);
        assert_eq!(tilt(0.), 0., "an unset reach moved the field");
        let mut last = tilt(EVEN_AT);
        for step in 1..=64 {
            let reach = EVEN_AT * 1.2f32.powi(step);
            let here = tilt(reach);
            assert!(here < last, "no darker at a reach of {reach} ly");
            last = here;
        }
    }

    /// A knot drags past its neighbours either way, and is held only to the
    /// display's range
    #[test]
    fn a_knot_drags_past_its_neighbours() {
        let mut curve = FieldCurve::default();
        curve.set(3, 0.99);
        assert_eq!(curve.levels[3], 0.99);
        assert!(curve.levels[3] > curve.levels[4]);
        curve.set(3, 0.01);
        assert_eq!(curve.levels[3], 0.01);
        assert!(curve.levels[3] < curve.levels[2]);
        curve.set(0, -1.);
        assert_eq!(curve.levels[0], 0.);
        curve.set(KNOTS - 1, 2.);
        assert_eq!(curve.levels[KNOTS - 1], 1.);
    }
}
