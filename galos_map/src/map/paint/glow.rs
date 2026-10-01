//! The political field, splatted from the aggregates
//!
//! [`crate::map::galaxy::plan`] walks the index into the cells whose systems separate
//! on screen and the cells that stand for their whole subtree. The first half
//! has been drawn since the bounded fetch landed; this is the second. A splat
//! needs nothing fetched — the aggregates are resident and a splatted cell's
//! payload is deliberately never asked for — so the field is the one part of
//! the map that is never absent and never patchy, and what a fetch changes is
//! only how much of a region has condensed into marks.
//!
//! **Two channels, two distributions.** A cell carries stellar moments and
//! inhabited moments, and they are nothing alike: measured over
//! `.galos_index`, the root's count-weighted centroid and its inhabited
//! centroid are 12.5 thousand light years apart and their spreads differ
//! tenfold. So the systems nobody lives in splat at the stellar centroid with
//! the stellar extent, and the colonies splat at their own, which is what
//! keeps a colonisation filament a filament instead of smearing it over the
//! cell that holds it. [`galos_index::read::inhabited::Inhabited`] is the second distribution
//! and exists for this.
//!
//! **One light per system, and nothing else.** A system is worth the same
//! light whether the map draws it as its own mark or the field stands in for
//! it — [`mark_light`] is the one figure — and that is what makes the
//! handoff between them invisible: as a cell's payload lands, its light
//! moves out of the field and into the marks with nothing added and nothing
//! lost. The field is *linear*: a splat lays its systems' summed light over
//! its footprint and is held down by nothing of its own, so a thousand
//! systems are a thousand times one, wherever in the galaxy they stand.
//!
//! **The display's range is one curve, struck per pixel.** What the field
//! lays runs over some thirty stops in one frame, and a display carries a
//! handful, so the marks and the field are drawn into a target of their own
//! and brought onto the display by [`crate::map::paint::curve`] after they
//! have been summed. A correction struck per cell — a crowd held down by its
//! own count, and a curve on each splat's own peak — is what this replaced:
//! a cell's light then depended on what else the cell held, so equal counts
//! of two star classes drew at a fifth of one another where one stood among
//! the well-scanned crowd and the other in the sparse disc. Per pixel, the
//! only thing that compresses a star's light is what actually lands on its
//! pixel.
//!
//! **Two targets, the let-through and the dimmed.** What the filters and the
//! color mask exclude is laid into a target of its own, and the curve brings
//! each onto the display apart: the excluded at the dim's share of what the
//! curve made of them. A dim spent on the light ahead of a curve that spans
//! thirty stops is a few stops of thirty, which is no dim at all.
//!
//! **Resolved per cell and added in linear light.** Each splat's mix is summed
//! on the CPU — eight multiply-adds against a histogram that is already the
//! local composition, at the level the walk chose to draw it — and deposited as
//! one additive Gaussian. Addition is order-independent, so neighbouring cells
//! sum into a continuous field with no seam and no draw-order artifact, and a
//! fine cell beside a coarse one needs no stitching: the weights the walk
//! hands out conserve, so the total is right whatever level each region
//! resolved at. Where two cells of different dominant colour overlap they add
//! toward white, which is the mixing reading; an offscreen per-category
//! resolve would instead desaturate by measured entropy and keep the
//! intensity, and nothing here forecloses it — the
//! deposited quantity is the same either way.
//!
//! **Every splat is a pixel or two by construction**, which is what makes
//! one isotropic Gaussian enough. A cell hands its light to its children
//! once its contents subtend [`galos_index::read::walk::SPLIT_PX`] — half a pixel,
//! so the frontier follows the pixel grid down as far as the tree goes — and
//! a filament is carried by a chain of pixel-wide splats rather than by one
//! elongated blob. It also bounds the cost of a centroid that has left the
//! frame: the field is drawn off projected centres, so a splat whose centre
//! goes behind the camera drops, and what drops is a blob of a pixel or two
//! at the very edge.
//!
//! **How tight is the frame's to say, not the data's.** A cell knows where
//! its systems are to its own edge and no better, so a splat is spread over
//! half its cell ([`COVERAGE`]) — the width at which neighbours sum flat —
//! and never under half a pixel ([`FINEST`]), which is all a display can
//! carry. Between them the field is as sharp as the frame allows wherever
//! the tree has depth, and an honest wash where it has not.
//!
//! Drawn on [`FIELD_LAYER`] and [`DIMMED_LAYER`], into the curve's two
//! targets; [`crate::map::paint::field`]'s marks are laid over it after the
//! curve, each through it on its own.

use crate::map::camera::{DIMMED_LAYER, FIELD_LAYER, OrbitCamera};
use crate::map::filter::mask::Keeps;
use crate::map::galaxy::plan::Planned;
use crate::map::galaxy::spawn::{ColorBy, Hue};
use crate::map::index::{ResidentIndex, Settled};
use crate::map::paint::sizing::View;
use crate::map::schedule::MapSet;
use crate::map::screen::{screen_position, world_per_pixel};
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::image::{Image, ImageSampler};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat,
};
use bevy::tasks::ComputeTaskPool;
use galos_index::prelude::StarKind;
use galos_index::read::inhabited::Inhabited;
use galos_index::tree::cell::UNIFORM_SPAN;

pub fn plugin(app: &mut App) {
    app.init_resource::<Gains>();
    app.init_resource::<Laid>();
    app.add_systems(Startup, spawn_glow);
    app.add_systems(
        Update,
        (settle_gains, build_glow.after(crate::map::galaxy::plan::plan))
            .chain()
            .in_set(MapSet::Present),
    );
}

/// What the field laid down last frame
///
/// The two channels counted apart, because which of them is carrying a view
/// is the whole question the gains answer: a frame of nothing but backdrop is
/// a political field that has been buried, and a frame of no backdrop at all
/// is one that has lost the galaxy behind it.
///
/// **In the curve's unit.** Every level here is linear light over an
/// average system's mark ([`Gains::mark`] times [`Gains::average`]), which
/// is what the field's curve is read in
/// ([`crate::map::paint::curve::FieldCurve`]): a level's `log2` is how many
/// stops along the curve it lands before the dial moves it.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Laid {
    /// Quads laid at a cell's inhabited centroid.
    pub colonies: u32,
    /// Quads laid at a cell's stellar centroid, for the systems nobody lives
    /// in.
    pub backdrop: u32,
    /// Quads laid into the dimmed target, for what the filters and the
    /// color mask exclude.
    pub dimmed: u32,
    /// The light the let-through quads laid, all told: each one's brightest
    /// channel over its footprint, in the curve's unit. Zero where the marks have
    /// accounted for everything.
    pub light: f32,
    /// The brightest peak any one let-through quad was laid at, in the
    /// curve's unit.
    ///
    /// Where the top of the frame stands on the curve, before quads that
    /// overlap have summed — so the pixels themselves run brighter still.
    pub peak: f32,
    /// The faintest and the middling peak, in the curve's unit.
    ///
    /// **What says whether the field is a picture.** The brightest splat is
    /// one cell in the galaxy's core and is bright at every zoom, so a field
    /// that has faded to nothing everywhere else reads the same by it. The
    /// median is what the frame is actually made of.
    ///
    /// The faintest is the tail and not the reading: there is always some
    /// cell holding a handful of systems across a degree of sky, and what
    /// it lays down underflows to nothing at any exposure worth having.
    pub faintest: f32,
    pub typical: f32,
    /// The smallest, median-ish and largest footprint radius laid, in pixels,
    /// and how many were laid on [`FINEST`], the pixel floor — what says
    /// whether the field is resolving structure or has run out of frame to
    /// resolve it on.
    pub thinnest: f32,
    pub tenth: f32,
    pub quarter: f32,
    pub median: f32,
    pub widest: f32,
    pub floored: u32,
    /// Quads whose systems have separated on screen: their marks would cover
    /// less of the footprint than they are spread over.
    ///
    /// These are the cells the map is about to draw as marks and has not
    /// fetched yet, so the number is how much of the field is waiting on a
    /// payload.
    pub separated: u32,
}

impl Laid {
    /// Count one quad in, off what [`Quads::deposit`] laid.
    fn took(&mut self, lit: Lit) {
        self.separated += u32::from(lit.separated);
        self.dimmed += u32::from(lit.dimmed);
    }
}

impl Default for Laid {
    fn default() -> Laid {
        Laid {
            colonies: 0,
            backdrop: 0,
            dimmed: 0,
            light: 0.,
            peak: 0.,
            faintest: 0.,
            typical: 0.,
            separated: 0,
            thinnest: f32::INFINITY,
            tenth: 0.,
            quarter: 0.,
            median: 0.,
            widest: 0.,
            floored: 0,
        }
    }
}

/// One of the two meshes the field is laid into, one quad a channel a splat:
/// the let-through on [`FIELD_LAYER`], and what the filters exclude on
/// [`DIMMED_LAYER`]
#[derive(Component)]
struct GlowMark {
    dimmed: bool,
}

/// What a system is worth, in the linear light its mark and the field
/// standing in for it both lay
///
/// **One light per system and nothing for the crowd.** A system is worth the
/// same light whether it is drawn as itself or stood in for by the field —
/// that is what makes the handoff between them invisible — and a crowd is
/// worth the sum of what its systems are worth, with nothing struck off for
/// being one: how much of a crowd's range reaches the display is
/// [`crate::map::paint::curve`]'s to say, per pixel and after everything has
/// summed. So what is left here is the *balance* between kinds of system,
/// which is a ratio of per-system levels and the same at any density.
///
/// Held as a resource rather than as constants because the balance between
/// the colonies and the ungoverned galaxy behind them is a reading and not a
/// fact, and because [`settle_gains`] derives three of them from the index.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Gains {
    /// How bright an inhabited system with a reading is drawn, in linear
    /// light
    ///
    /// The unit the others are shares of.
    pub mark: f32,
    /// How much of [`mark`](Self::mark) an inhabited system with nothing
    /// political on record is worth, and a star on record along star class
    /// before its class's [`star_level`]
    ///
    /// Not zero. A colony whose allegiance nobody has reported is still a
    /// colony, and the neutral light it draws in is how the map says so;
    /// what it must not do is out-shout the systems that do have a reading.
    pub unaligned: f32,
    /// How much of [`mark`](Self::mark) a system nobody lives in is worth
    ///
    /// Derived from the index rather than chosen: [`settle_gains`] sets it
    /// so the systems nobody lives in lay, all told, the light the colonies
    /// lay — for a populated share `p`, `p / (1 - p)` of a mark. Measured
    /// over `.galos_index`, 77,061 inhabited of 3,399,743: one in
    /// forty-four, so an uninhabited system is worth a forty-third of one.
    ///
    /// Without it the map goes grey: there are forty-odd uninhabited systems
    /// for every colony, and laid down any brighter than this they bury
    /// every shell colour wherever the two are seen together.
    pub empty: f32,
    /// How much of [`mark`](Self::mark) a star nothing has recorded is
    /// worth along star class
    ///
    /// Star class's [`empty`](Self::empty), derived the same way by
    /// [`settle_gains`]: the unscanned stars lay, all told, the light the
    /// scanned ones lay — for scanned stars worth
    /// [`unaligned`](Self::unaligned) times a mean [`star_level`] of `l`
    /// and a scanned share `s`, `unaligned · l · s / (1 - s)`. Weighed
    /// against a full mark instead, the gray came out four times the
    /// scanned colors over it.
    pub unscanned: f32,
    /// What a system is worth on average along the axis the map is colored
    /// by, as a share of [`mark`](Self::mark): the galaxy's whole light over
    /// its count
    ///
    /// **The unit the field's curve is read in**, so one curve exposes
    /// every axis alike. The axes lay very different light all told — star
    /// class every system at a quarter of a mark, the political axes the
    /// colonies at full and the uninhabited at a share that balances them —
    /// and measured over `.index/full` the political sky lays a
    /// hundred-and-eighteenth of what the star classes do. Read against a
    /// mark, a curve that exposed one left the other seven stops under it.
    /// Derived by [`settle_gains`]; a global gain, the same for every pixel,
    /// so it moves no star against another.
    pub average: f32,
}

impl Default for Gains {
    fn default() -> Gains {
        let unaligned = 0.25;
        Gains {
            mark: 0.6,
            unaligned,
            // Off the share `.galos_index` measures, until `settle_gains`
            // reads the directory actually in front of the map: one system
            // in forty-four is inhabited, so forty-three are not.
            empty: 1. / 43.,
            // Off the share `.index/full` measures: 124 million of 200
            // million systems have no star on record. The scanned taken at
            // a mean level of one until the directory says otherwise.
            unscanned: unaligned * 76. / 124.,
            // A mark until the directory says otherwise.
            average: 1.,
        }
    }
}

/// How far out a splat's quad reaches, in standard deviations
///
/// Four. Three holds 98.9 % of a Gaussian's weight, which is plenty of the
/// *light* — but the question at the rim is not how much is left outside it,
/// it is how large a step the quad ends on. At three sigma the profile still
/// stands at `exp(-4.5)`, 1.1 % of peak, so every quad finishes on a cliff;
/// ten overlapping stack those cliffs into a tenth of the brightness, laid
/// out on the cell lattice with darker seams where fewer overlap. That is the
/// checker, and it reads as squares rather than as lumps because it is the
/// quads' own boundaries and not their profiles.
///
/// Four sigma is `exp(-8)`, three ten-thousandths, and the pedestal
/// [`gaussian_mask`] subtracts takes even that to nothing.
const REACH: f32 = 4.0;

/// The radius a system's own mark is painted at, in pixels
///
/// [`crate::map::paint::field::SMALLEST`] itself and not a second copy of the figure: it
/// is what one system's light fills out where the map floors a mark, which is
/// what [`splat`] measures a cell's footprint against through [`MARK_AREA`].
///
/// Not the floor a *splat* is laid at; that is [`FINEST`]. The two were one
/// constant, and flooring a splat's whole quad at a 0.75 px radius put its
/// kernel at 0.19 px of sigma — a fifth of a pixel, which is not a
/// distribution but a hard dot, and a lattice of them is the beading the
/// field was reported for.
const SMALLEST: f32 = crate::map::paint::field::SMALLEST;

/// The pixels one system's mark covers, at the floor it is drawn at
///
/// The bridge between the two halves of the map. The field stands in for
/// systems that are not drawn yet, and what says whether it should read as a
/// wash or as a point of light is how much of its footprint those systems
/// would cover if they were: `count · MARK_AREA` against the footprint's own
/// area. See [`splat`].
const MARK_AREA: f32 = std::f32::consts::PI * SMALLEST * SMALLEST;

/// The finest a splat is laid, as a standard deviation in pixels
///
/// **Half a pixel, because the display is the field's own resolution
/// limit.** Light laid tighter than the pixel grid cannot be seen as
/// structure; it can only alias, and a lattice of sub-pixel spikes reads as
/// stipple and crawls as the camera moves. Half a pixel is the widest kernel
/// that costs no sharpness a pixel could have shown, and the tightest that
/// still sums flat across neighbours about a pixel apart: the ripple over a
/// lattice of pitch `d` goes as `2 exp(-2 pi^2 sigma^2 / d^2)`, 1.4 % at
/// `sigma = d/2`.
///
/// It is the last word under [`COVERAGE`], which says what a cell may claim
/// of *itself*; this says what a frame can show. With
/// [`galos_index::walk`]'s split at half a pixel of cell the two meet: the
/// frontier's cells are about a pixel apart, their own floors land just
/// under this, and the field comes out as tight as the frame allows and
/// still continuous.
const FINEST: f32 = 0.5;

/// What a cell's own spread is worth as a screen sigma: one over the square
/// root of three
///
/// **A cell reports a radius in three dimensions and the field draws in
/// two.** [`galos_index::prelude::Moments::rms_radius`] is the RMS *distance*
/// of a cell's systems from their centroid, so for an isotropic cloud it is
/// `sqrt(3)` times the deviation along any one axis — and one axis is what a
/// screen Gaussian's sigma is. Laid as the radius it stands at, every splat
/// came out 73 % too wide and spread its light over three times the area it
/// owned, which is the blur the field was reported for, and the same factor
/// then read three times too little `fill` and pressed a crowd as though it
/// were a scatter.
///
/// Isotropy is the only assumption a centroid and one radius can carry, and
/// it is the unbiased one: averaged over orientations, the variance a cloud
/// projects onto any axis is a third of the trace of its own, whatever shape
/// it is. A filament seen along its length is drawn wider than it is and
/// seen across it narrower; per-axis moments are what would tell the two
/// apart, and a cell carries none.
///
/// Applied to the *measured* spread alone, and never to [`COVERAGE`]. That
/// is a share of a cell's edge and already a per-axis figure, so flattening
/// it a second time would spread a lone cell over a third less than its own
/// box — which stippled the sparse half of the disc into a lattice of dots
/// when it was tried.
const FLATTENED: f64 = 1. / 1.732_050_807_568_877_2;

/// The side of the Gaussian mask, in texels
///
/// Sixty-four. A splat's footprint runs to thousands of pixels, so the mask
/// is magnified and bilinear magnification creases at every texel boundary —
/// which is a real artifact, and for a while it was my answer for the
/// checker. It was not: raising this sixteenfold moved the reported pitch not
/// at all. It is back where it was rather than left high on a hypothesis that
/// did not survive, and the four megabytes with it.
///
/// What removes the class of it, if a crease ever does show, is evaluating
/// the profile per fragment instead of sampling it.
const GLOW_TEXELS: u32 = 64;

/// The least a splat is spread over, as a share of its cell's edge
///
/// **A cell cannot assert structure finer than itself.** Its moments say
/// where its systems sit and how far they spread, but the finest thing it
/// stands for is the box, and laying its light down tighter than that
/// claims a precision the tree does not have.
///
/// **A half, because that is what sums flat.** Neighbouring splats sit
/// about a cell edge apart, and Gaussians on a lattice of pitch `d` only
/// sum flat once `sigma` is half of it: the ripple goes as
/// `2 exp(-2 pi^2 sigma^2 / d^2)`, 1.4 % at `sigma = d/2`, 34 % at `d/3`
/// and total at `d/5`. A box taken as evenly filled deviates by
/// `edge / sqrt(12)`, three tenths of it, and three tenths is exactly where
/// the ripple is a third — so a lattice of cells laid at their own honest
/// width stipples, which is what it did: at three tenths, with the split
/// band brought down to half a pixel, the sparse half of the disc came out
/// as a regular grid of dots. Half a cell over-spreads a splat by the same
/// third it costs to be rid of them.
///
/// **What buys the resolution is the split and not this.** Three tenths was
/// reached when a splatted cell's contents spanned two to four pixels, where
/// the floor was the only lever on how tight a filament could be drawn and
/// this one had to carry it. With [`galos_index::read::walk::SPLIT_PX`] cutting at
/// half a pixel the frontier's cells are about a pixel across, so half of
/// one is half a pixel — [`FINEST`], the display's own limit — and the floor
/// costs nothing where the tree has depth to give. Where it has not, in the
/// thinly visited outer disc whose leaves are hundreds of light years wide,
/// this is what keeps the field a field: a wash over the cell, which is all
/// the map knows.
///
/// One figure for both channels. The colonies were given a half of their own
/// while the backdrop kept three tenths, on the reasoning that a filament is
/// a line of cells with nothing beside it to fill the gaps and needs the
/// overlap more than a fog does. It does — and so does the fog, for the same
/// arithmetic, wherever its cells are the same size.
const COVERAGE: f64 = 0.5;

/// How a splat is laid: the radius it is drawn at, and the light it peaks at
///
/// **The one deposit law, and it is linear.** `light` is what the systems
/// this stands for would be drawn at as marks, summed, and `spread` is how
/// far the cell says its systems are scattered, as a standard deviation in
/// pixels. The mask integrates to `2 pi sigma^2` at unit peak, so the light
/// divided by that area is what lands on the target, and the quantity is
/// conserved: a parent's one splat and its children's several integrate to
/// the same total, the cross-fade between them neither pumps brightness nor
/// loses any, and a splat whose systems have come apart on screen lays
/// exactly the light their marks will.
///
/// Nothing here holds a crowd down. A crowd's light running off the top of
/// the display is the per-pixel curve's to settle
/// ([`crate::map::paint::curve`]); struck here, on the cell's own count, it
/// made a star's light depend on what else its cell held.
///
/// **The spread is the cell's, always**, and the light is never more than
/// the marks'. Cutting the spread to what the light can fill — so a sparse
/// cell keeps its marks' peak instead of their mean — holds the brightness
/// at any zoom, and it was tried, and it is the one thing this must not do.
/// A splat tight enough to read as an object is one the user tries to
/// click; and a cell's centroid moves as its neighbours resolve and as its
/// own systems are drawn out of it, so those objects slide about under a
/// zoom. The field says where its systems are to a cell's precision and no
/// better, and it says so as a wash.
pub(crate) fn splat(light: Vec3, spread: f32) -> (f32, Vec3) {
    let sigma = spread.max(FINEST);
    let radius = sigma * REACH;
    let area = std::f32::consts::TAU * sigma * sigma;
    (radius, light / area)
}

/// What one system is worth, in linear light
///
/// The light its mark is painted at, and the light the field deposits for it
/// where it is not drawn yet — one figure, which is what makes the two
/// halves of the map one picture. A system nobody lives in is worth
/// [`Gains::empty`], an inhabited one with nothing political on record is
/// held down by [`Gains::unaligned`] so a crowd of them cannot bury a shell,
/// and a system with a reading draws at full.
pub(crate) fn mark_light(hue: Hue, peopled: bool, gains: &Gains) -> f32 {
    let share = match (peopled, hue) {
        (false, _) => gains.empty,
        (true, Hue::Grey) => gains.unaligned,
        (true, _) => 1.0,
    };
    share * gains.mark
}

/// What one system is worth along star class, in linear light
///
/// A star nothing has recorded at [`Gains::unscanned`], which balances the
/// unscanned two thirds of the sky against the scanned third over it. A star
/// on record is worth what a colony with no politics on record is
/// ([`Gains::unaligned`]): the axis is every system and not a few thousand
/// picked out of them — times its class's [`star_level`].
pub(crate) fn star_light(hue: Hue, gains: &Gains) -> f32 {
    let share = match hue {
        Hue::Grey => gains.unscanned,
        _ => gains.unaligned * star_level(hue),
    };
    share * gains.mark
}

/// How much brighter or dimmer a star class's hue is drawn than its share
/// alone, so the main sequence dims hot to cool
///
/// **A level and not a color.** The hue's light is scaled whole, so its
/// chromaticity is untouched and a class reads as the color the key names.
/// What it fixes is that the hues are not equally bright to the eye: yellow
/// carries 0.93 of the Rec. 709 luminance, cyan 0.79, orange 0.37, the blue
/// 0.28 and red 0.21, so drawn at one level G outshone O and B, and A and F
/// read dimmer than K.
///
/// Laid on a geometric ladder instead, cyan's luminance down to red's in
/// four equal ratios of 0.72 — 0.79, 0.57, 0.41, 0.29, 0.21 for O and B, A
/// and F, G, K and M — which is equal steps as the eye weighs brightness.
/// The two ends stand where their hues put them; the three between are
/// brought onto the rungs. What cannot be scooped, and what is not on
/// record, are left at their share: they are off the sequence.
pub(crate) const fn star_level(hue: Hue) -> f32 {
    match hue {
        Hue::Blue => 2.056,
        Hue::Yellow => 0.441,
        Hue::Orange => 0.8065,
        Hue::Cyan | Hue::Red | Hue::Magenta | Hue::Green | Hue::Grey => 1.,
    }
}

/// The brightest [`star_level`], which the key's swatches are drawn under so
/// the brightest of them is the full color and the rest stand below it as
/// they do on the map
pub(crate) const STAR_LEVEL_TOP: f32 = star_level(Hue::Blue);

/// What one system is worth along `color_by`, in linear light: a mark's
/// level, and the light laid in its place
pub(crate) fn system_light(
    color_by: ColorBy,
    hue: Hue,
    peopled: bool,
    gains: &Gains,
) -> f32 {
    match color_by.every_system() {
        true => star_light(hue, gains),
        false => mark_light(hue, peopled, gains),
    }
}

/// Put the field's two meshes and their additive material up
fn spawn_glow(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Added and never blended. A splat is light laid into a field, so what a
    // vertex carries is an emission and not an opacity: the alpha is one
    // throughout and the whole of the weight is in the three channels, which
    // is the same shape `crate::map::paint::field`'s realistic glint is painted in.
    // Unlit, because a splat is a light and not a thing lit by one.
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(images.add(gaussian_mask())),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    for (dimmed, layer) in [(false, FIELD_LAYER), (true, DIMMED_LAYER)] {
        let mesh = meshes.add(glow_mesh(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            RenderLayers::layer(layer),
            // Every vertex is placed by hand each frame; there is no bound
            // to cull against and the whole field is one draw regardless.
            NoFrustumCulling,
            Transform::default(),
            Visibility::Visible,
            GlowMark { dimmed },
        ));
    }
}

/// Set what a system nobody lives in, and a star nobody has recorded, is
/// worth from the index's own composition, and what a system is worth on
/// average along the axis drawn
///
/// The first two are set so their crowd lays, all told, the light of the
/// crowd it stands behind: the uninhabited the colonies'
/// ([`Gains::empty`]), and the unscanned the scanned stars'
/// ([`Gains::unscanned`]). Derived rather than chosen, so a directory of a
/// different composition is exposed for what it holds. The third is the
/// curve's unit ([`Gains::average`]).
///
/// Only when the directory or the axis moves. A directory with no colonies
/// in it, or no stars on record, leaves the default standing rather than
/// dividing by nothing.
fn settle_gains(
    index: Res<ResidentIndex>,
    settled: Res<Settled>,
    color_by: Res<ColorBy>,
    mut gains: ResMut<Gains>,
) {
    if !index.is_changed() && !settled.is_changed() && !color_by.is_changed() {
        return;
    }
    let mut settling = *gains;
    let root = galos_index::prelude::CellId::ROOT;
    let stellar = index.0.get(root).map_or(0, |cell| cell.aggregate.count());
    let held = settled.0.get(root);
    let peopled = held.map_or(0, Inhabited::count);
    if stellar > peopled && peopled > 0 {
        settling.empty = (peopled as f64 / (stellar - peopled) as f64) as f32;
    }
    let Some(cell) = index.0.get(root) else { return };
    let kinds = cell.aggregate.kinds();
    let unknown = u64::from(kinds[0]);
    // What the scanned stars are worth all told, each class at its level.
    let levelled: f64 = kinds
        .iter()
        .enumerate()
        .skip(1)
        .map(|(code, n)| {
            f64::from(*n)
                * f64::from(star_level(ColorBy::StarClass.hue_of(code)))
        })
        .sum();
    if unknown > 0 && levelled > 0. {
        settling.unscanned =
            settling.unaligned * (levelled / unknown as f64) as f32;
    }
    // And what a system is worth on the whole along the axis drawn, which
    // is the unit the curve is read in.
    let mut total = 0f64;
    if color_by.every_system() {
        for (code, n) in kinds.iter().enumerate() {
            let hue = ColorBy::StarClass.hue_of(code);
            total += f64::from(*n) * f64::from(star_light(hue, &settling));
        }
    } else {
        if let Some(held) = held {
            political(held, *color_by, |_, hue, n| {
                total +=
                    f64::from(n) * f64::from(mark_light(hue, true, &settling));
            });
        }
        total += stellar.saturating_sub(peopled) as f64
            * f64::from(mark_light(Hue::Grey, false, &settling));
    }
    if stellar > 0 && total > 0. {
        settling.average =
            (total / stellar as f64 / f64::from(settling.mark)) as f32;
    }
    gains.set_if_neq(settling);
}

/// What a cell's political histogram comes to: the light it lays down, summed
/// over its buckets
///
/// Premultiplied — the colour returned is already scaled by the counts — so
/// the caller deposits one quad for the whole cell instead of one a bucket,
/// which is the same sum with an eighth of the vertices. The axis the histogram
/// is read off is the one the map is coloured by, through the very mapping a
/// mark is painted by, so the field and the symbols over it cannot disagree
/// about what a colour means.
///
/// In shares of [`Gains::mark`], so a bucket's light here is exactly what its
/// systems' own marks are painted at; the caller spends the level. The colour
/// is [`Hue::light`], which is a chromaticity — an unreported colony is
/// neutral at [`Gains::unaligned`] and not a dark grey paint dimmed a second
/// time by a gain that already said it was dim.
///
/// `keeps` is how much of each bucket to count, which is the color mask's
/// say ([`crate::map::filter::mask::Mask::keeps`]): [`Keeps::ALL`] for the
/// cell as it stands.
fn composition(
    held: &Inhabited,
    color_by: ColorBy,
    unaligned: f32,
    keeps: Keeps,
) -> Vec3 {
    let mut light = Vec3::ZERO;
    political(held, color_by, |bucket, hue, count| {
        let gain = if hue == Hue::Grey { unaligned } else { 1.0 };
        light += hue.light() * count as f32 * gain * keeps.of(bucket);
    });
    light
}

/// What a cell's stars come to along star class: the light they lay down,
/// premultiplied, in shares of [`Gains::mark`] as [`composition`]'s is
///
/// `kinds` is by [`StarKind::code`], the cell's aggregate less what is drawn
/// as itself, and `keeps` the color mask's say along star class. The field
/// hands this the scanned stars alone and lays the unscanned as a channel of
/// their own.
pub(crate) fn starlight(
    kinds: &[u32; StarKind::COUNT],
    gains: &Gains,
    keeps: Keeps,
) -> Vec3 {
    let mut light = Vec3::ZERO;
    for (code, count) in kinds.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let hue = ColorBy::StarClass.hue_of(code);
        light += hue.light()
            * (*count as f32 * star_light(hue, gains) / gains.mark
                * keeps.of(code));
    }
    light
}

/// Walk a cell's political histogram along the axis the map is colored by,
/// handing each bucket, its color and its count over
///
/// The axis is read through the very mapping a mark is painted by, so the
/// field, a merged mark and a drawn system cannot disagree about what a
/// colour means. Shared with [`crate::map::galaxy::blobs`], which paints a merged mark
/// as the average of the marks it stands for and needs the same walk.
///
/// Empty buckets are skipped, most of them being empty for most cells.
pub(crate) fn political(
    held: &Inhabited,
    color_by: ColorBy,
    mut lay: impl FnMut(usize, Hue, u32),
) {
    for (bucket, count) in color_by.counts(held).iter().enumerate() {
        if *count > 0 {
            lay(bucket, color_by.hue_of(bucket), *count);
        }
    }
}

/// What the filters and the color mask leave of a channel: the light let
/// through, and the light they exclude
///
/// **Every system is let through at full or drawn at the dim**, as its own
/// mark would be. Along the mask the let-through light is `kept`, which can
/// be another color entirely — a cell of Federation and Empire with the
/// Federation hidden is cyan — and the picking filters and a span let
/// through `share` of whatever the mask left, the two taken as independent.
/// What is not let through is the rest of `whole`, laid into the dimmed
/// target where the excluded are drawn at all (`excluded`) and nowhere
/// where they are not.
///
/// Both at full: the dim is spent after the curve, on the dimmed target as
/// a whole ([`crate::map::paint::curve`]), so a filter dims the field as far
/// as it dims a mark.
fn let_through(
    whole: Vec3,
    kept: Vec3,
    share: f32,
    excluded: bool,
) -> (Vec3, Vec3) {
    let through = kept * share;
    let dimmed = match excluded {
        true => (whole - through).max(Vec3::ZERO),
        false => Vec3::ZERO,
    };
    (through, dimmed)
}

/// What [`build_glow`] writes, and where the camera stood when it last did:
/// one parameter, a system taking sixteen at most.
type Written<'w, 's> = (
    ResMut<'w, Laid>,
    Query<'w, 's, (&'static GlowMark, &'static mut Mesh3d)>,
    ResMut<'w, Assets<Mesh>>,
    Local<'s, Option<(galos_index::read::walk::View, DVec3)>>,
);

/// Rebuild the field from the cells the walk said to splat
///
/// Off a plan re-walked only when the eye moves: what a splat is drawn
/// *from* is a pure function of position, and where it is drawn *to* turns
/// with the camera. The same split [`crate::map::paint::field::build_field`]
/// makes.
///
/// **Not at all where nothing it reads has moved.** Measured over
/// `.index/full`, a still view at thirty thousand light years spent 15 ms
/// of every frame laying down the field it laid the frame before, and the
/// renderer another 4 taking in the fresh mesh. What it reads is the
/// resources it is handed, whose change marks say whether they moved, and
/// the camera, which is written every frame whether it moves or not and so
/// is kept by value and compared.
#[expect(
    clippy::too_many_arguments,
    reason = "the plan, the two aggregations it reads, the palette, the gains \
              and the mesh it writes"
)]
fn build_glow(
    camera: Query<(&OrbitCamera, &Camera)>,
    planned: Res<Planned>,
    drawn: Res<crate::map::galaxy::plan::Drawn>,
    index: Res<ResidentIndex>,
    settled: Res<Settled>,
    named: Res<crate::map::galaxy::blobs::Named>,
    filtering: crate::map::filter::Filtering,
    color_by: Res<ColorBy>,
    gains: Res<Gains>,
    view: Res<View>,
    scale_population: Res<crate::map::paint::sizing::ScalePopulation>,
    spyglass: Res<crate::map::galaxy::Spyglass>,
    (mut laid, mut glow, mut meshes, mut last): Written<'_, '_>,
) {
    if glow.is_empty() {
        return;
    }
    // Where the camera stands and how it sees, which is all of it the field
    // is laid from; see the doc above.
    let seen = camera.single().ok().and_then(|(orbit, lens)| {
        crate::map::galaxy::plan::view(orbit, lens)
            .map(|seen| (seen, orbit.center()))
    });
    let moved = planned.is_changed()
        || drawn.is_changed()
        || index.is_changed()
        || settled.is_changed()
        || named.is_changed()
        || filtering.filters.is_changed()
        || filtering.dim.is_changed()
        || color_by.is_changed()
        || gains.is_changed()
        || view.is_changed()
        || scale_population.is_changed()
        || spyglass.is_changed();
    if !moved && last.is_some() && *last == seen {
        return;
    }
    *last = seen;
    // Whether the sky is being read as populations, in which the crowd
    // nobody lives in is not drawn at all.
    //
    // **The field stands in for marks and has to say what they say.**
    // [`mark_light`] is one figure for a mark and for the light laid down
    // in its place, and in that mode an uninhabited system's mark is
    // never drawn — so the light standing in for one is not standing in
    // for anything. Laid anyway, the galaxy kept its grey while the marks
    // over it were only the colonies, which is the two halves of the
    // picture answering different questions.
    let populated_only =
        crate::map::paint::sizing::by_population(&view, &scale_population);
    // Whether what the filters exclude is drawn at all: below the dim it is
    // not, and nothing is laid into the dimmed target.
    let excluded = filtering.excluded_are_drawn();
    let mask = filtering.filters.mask();
    let mut quads = Quads::default();
    let mut counted = Laid::default();

    // The realistic sky's own glow is the summed light below the visibility
    // floor, read off the flux buckets and the luminosity moments rather than
    // off the political ones. Same deposit, a different weight; not this.
    //
    // Laid in a block rather than behind an early return, so the mesh below is
    // written on every frame either way: a view that draws no field writes an
    // empty one, where leaving the last frame's standing would keep a political
    // field over the realistic sky.
    'lay: {
        if *view != View::Map {
            break 'lay;
        }
        let Ok((orbit, camera)) = camera.single() else { break 'lay };
        let Some(viewport) = camera.logical_viewport_size() else { break 'lay };
        let frame = Frame {
            orbit,
            cot_half_fov: camera.clip_from_view().y_axis.y,
            viewport,
            half: viewport * 0.5,
            mark: gains.mark * gains.average,
        };

        // What the walk hands out shares *of*. `SplatRef::blend` is a share of
        // the whole galaxy and not of the cell it names — the walk splits a
        // parent's weight among its children by their counts, so the factors
        // telescope and the blends over a field sum to one, which `walk`'s own
        // conservation test asserts. So the quantity spent against a blend is
        // the galaxy's, and a frontier cell carrying its own whole subtree
        // comes out at `blend * total == count`, which is exactly its own
        // content.
        //
        // Multiplying a blend by the cell's own count instead spends
        // `count^2 / total` and takes the field to nothing: measured, the
        // brightest splat over the bubble came out at seven ten-thousandths of
        // a unit and nothing was visible at all.
        let total = index
            .0
            .get(galos_index::prelude::CellId::ROOT)
            .map_or(0, |cell| cell.aggregate.count())
            as f32;
        if total <= 0. {
            break 'lay;
        }

        // To bound the view is to clear away what the reach does not hold, and
        // the field is part of the view. The same predicate the marks are held
        // to ([`Spyglass::reaches`], which `crate::map::galaxy::visibility` asks of every
        // drawn system), so the spyglass is one boundary over the whole map
        // rather than a sphere of symbols standing in a field that carries on
        // past it. Answers true for everything while the spyglass is not
        // clearing, which is what that mode means.
        //
        // Asked of each channel at its own centroid, since a cell's colonies
        // and its stars do not sit in the same place: a cell whose colonies
        // are inside the reach draws them even where its stellar centre is
        // outside.
        //
        // **And the centroid is not the splat.** A splat is a Gaussian four
        // deviations wide, and a cell wide enough to carry one that is
        // hundreds of pixels across — measured over `.index/full`, the
        // widest runs to 2,200 px — paints most of the frame from a centre
        // well inside the bubble. What the map showed for it was a sphere
        // of marks standing in a wash that ran out to the corners. So each
        // splat is also told how much room the reach leaves it, and fades
        // by how much of itself that still holds; see `Quads::deposit`.
        let in_reach =
            |at: [f64; 3]| spyglass.reaches(orbit.center(), DVec3::from(at));
        let room = |at: [f64; 3]| {
            spyglass.clear.then(|| {
                f64::from(spyglass.radius)
                    - orbit.center().distance(DVec3::from(at))
            })
        };

        // One chunk of the splats laid down on its own: each splat is its
        // own quads and the field is additive, so
        // the chunks are laid on their own threads and joined in any
        // order. Measured over `.index/full` at a wide zoom, 152,199 splats
        // laid one after another were 12 ms of every frame.
        let lay = |chunk: &[galos_index::read::walk::SplatRef]| {
            let mut quads = Quads::default();
            let mut counted = Laid::default();
            for splat in chunk {
                let Some(cell) = index.0.get(splat.id) else { continue };
                let count = cell.aggregate.count();
                if count == 0 {
                    continue;
                }
                // What the marks have already taken out of this cell. A cell can
                // be marked and splatted by the same walk, so without this its
                // systems are drawn twice: once as themselves and again inside the
                // field behind them. Absent for a cell with nothing drawn out of
                // it, which is the ordinary far case and leaves the total standing.
                let taken = drawn.0.get(&splat.id).copied().unwrap_or_default();
                // The share of this cell's own content the splat carries: one where
                // it stands for its whole subtree, less where it is part way into a
                // cross-fade with its children.
                let carried = splat.blend as f32 * total / count as f32;

                // The systems nobody lives in, at the stellar moments: one
                // uncolored channel, because a backdrop is a density question and
                // not a composition one.
                //
                // Neutral, and worth [`Gains::empty`] a system. Painting it in the
                // palette's own grey discounted it twice — that colour is `0.15` in
                // sRGB, a fiftieth in the linear light this adds in, so the channel
                // came out four ten-thousandths of its weight and the galaxy behind
                // the shells went black. [`Hue::light`] is neutral for the same
                // reason, which is what leaves this channel and a mark of the same
                // system agreeing.
                //
                // The residual's own moments, not the total's: a cell half drawn
                // has its drawn half subtracted out of the geometry as well as out
                // of the weight, so the light that is left sits where the systems
                // that are left sit. The backdrop's weight counts only the systems
                // nobody lives in while its moments count every system in the cell,
                // which is a fiftieth of a difference at one populated system in
                // forty-four and not worth a fourth weighting to carry.
                let peopled =
                    settled.0.get(splat.id).map_or(0, Inhabited::count);
                let empty = count.saturating_sub(peopled).saturating_sub(
                    taken.count.saturating_sub(taken.inhabited.count()),
                );
                // The finest either channel may claim of this cell, in light
                // years, and the same rule for both ([`COVERAGE`]) — but never
                // wider than the contents themselves reach.
                //
                // **The floor is an admission of ignorance, and it has to stop
                // where the ignorance does.** Half a cell is what sums flat
                // over a lattice of *filled* cells, where the map knows a
                // cell's systems only to its own edge; for a cell holding one
                // system the map knows exactly where that system is, and
                // spreading it over half a cell invents a region of sky that
                // is not there. Measured over `.index/full`: `HIP 58832` is
                // the one inhabited system more than two thousand light years
                // off the galactic plane, it sits alone in a level 5 cell, and
                // the floor drew it as a ball of colony light 2,048 light
                // years in radius reaching seven thousand light years up — a
                // bright political region over sky holding, in the whole
                // galaxy, seven systems. Every other colony splat in that
                // frame reached 820.
                //
                // Capped by the support of what is held — the RMS radius read
                // as the span of an even spread, [`UNIFORM_SPAN`], which is
                // the same figure the walk's own merge rule measures a cell's
                // contents by. A cell whose systems fill it has a support of
                // about its whole edge and keeps the floor it had; a cell
                // holding a knot keeps the knot; a cell holding one system
                // floors at nothing and is laid at half a pixel
                // ([`FINEST`]), which is what one system looks like.
                let covered = |spread: f64| {
                    (cell.id.edge_ly() * COVERAGE).min(spread * UNIFORM_SPAN)
                };
                // What the filters leave of each channel. The same share a
                // merged mark is drawn at ([`crate::map::galaxy::blobs`]) and spent the
                // same way: the admitted part of a cell at full and the rest
                // at the dim, which is what its systems' own marks would come
                // to if every one of them were drawn. Nothing asked is a share
                // of one and leaves the field exactly as it was.
                //
                // Per channel, because the two stand for different halves of a
                // cell: a faction names none but populated systems, so the
                // colonies keep their share of the light while the grey
                // backdrop — which holds no member of it at all — falls to the
                // dim. One figure for both would keep the backdrop lit for
                // systems the filter could never admit.
                //
                // **The field has to answer the filters or the picture changes
                // as it resolves.** [`mark_light`] is one figure for a mark and
                // for the light laid down in its place, so a filter that
                // reached the marks and not the field would appear to take
                // effect only where the camera had come in far enough to draw
                // the systems themselves.
                //
                // The color mask's backdrop is all or nothing, uninhabited
                // being one flag, less what star class lets through of it
                // where the map is not colored by star class; its colonies
                // are [`let_through`]'s, less what the axes not drawn let
                // through of them ([`Mask::off_axis`]). Those are asked of
                // what is left once the marks have taken theirs, which is
                // what this splat lays down: the near systems drawn as
                // themselves are not the far ones' share.
                //
                // [`Mask::off_axis`]: crate::map::filter::mask::Mask::off_axis
                let held_named = named.admitted(splat.id);
                let aged = cell.aggregate.aged();
                let colonies = settled.0.get(splat.id).map(|held| {
                    if taken.inhabited.count() < held.count() {
                        held.remove(taken.inhabited)
                    } else {
                        // Every colony under the cell is on the map as itself.
                        Inhabited::ZERO
                    }
                });
                let mut kinds = *cell.aggregate.kinds();
                for (kind, drawn) in kinds.iter_mut().zip(taken.kinds) {
                    *kind = kind.saturating_sub(drawn);
                }
                let off = mask.off_axis(colonies.as_ref(), Some(&kinds));
                let backdrop_share = filtering.filters.admitted_share(
                    aged,
                    held_named.alone,
                    count.saturating_sub(peopled),
                ) * mask.keeps_uninhabited()
                    * off.backdrop;
                let colony_share = filtering.filters.admitted_share(
                    aged,
                    held_named.populated,
                    peopled,
                );
                let mass = cell.aggregate.mass().remove(taken.mass);

                // Star class stands for every system alike, so the two
                // channels split the cell by its stars rather than by who
                // lives there: the unscanned are its backdrop, neutral, and
                // the scanned its colonies, in the color their kinds come
                // to. Both at the cell's own moments, there being no
                // weighting of the scanned apart; the colonies' kinds are
                // among these, and a political channel as well would count
                // them twice.
                if color_by.every_system() {
                    let unscanned = u64::from(std::mem::take(&mut kinds[0]));
                    let scanned: u64 =
                        kinds.iter().map(|n| u64::from(*n)).sum();
                    let Some(at) = mass.centroid().filter(|at| in_reach(*at))
                    else {
                        continue;
                    };
                    let spread = (mass.rms_radius() * FLATTENED)
                        .max(covered(mass.rms_radius()));
                    let keeps = mask.keeps(*color_by);
                    // The political axes hide colonies, which are among both
                    // channels' stars and told apart in neither.
                    let share = filtering.filters.admitted_share(
                        aged,
                        held_named.whole(),
                        count,
                    ) * off.over(
                        colonies.map_or(0, |held| held.count()),
                        unscanned + scanned,
                    );
                    if unscanned > 0 {
                        let systems = unscanned as f32 * carried;
                        let whole = Vec3::splat(
                            systems * gains.unscanned * gains.mark * MARK_AREA,
                        );
                        let (through, dimmed) = let_through(
                            whole,
                            whole,
                            share * keeps.of(0),
                            excluded,
                        );
                        if let Some(lit) = quads.deposit(
                            &frame,
                            at,
                            spread,
                            through,
                            dimmed,
                            systems * MARK_AREA,
                            room(at),
                        ) {
                            counted.backdrop += 1;
                            counted.took(lit);
                        }
                    }
                    if scanned > 0 {
                        let whole = starlight(&kinds, &gains, Keeps::ALL);
                        let kept = match mask.narrows() {
                            true => starlight(&kinds, &gains, keeps),
                            false => whole,
                        };
                        let (through, dimmed) =
                            let_through(whole, kept, share, excluded);
                        let scale = carried * gains.mark * MARK_AREA;
                        if let Some(lit) = quads.deposit(
                            &frame,
                            at,
                            spread,
                            through * scale,
                            dimmed * scale,
                            scanned as f32 * carried * MARK_AREA,
                            room(at),
                        ) {
                            counted.colonies += 1;
                            counted.took(lit);
                        }
                    }
                    continue;
                }
                if empty > 0
                    && !populated_only
                    && let Some(at) = mass.centroid()
                    && in_reach(at)
                {
                    let systems = empty as f32 * carried;
                    let whole = Vec3::splat(
                        systems * gains.empty * gains.mark * MARK_AREA,
                    );
                    let (through, dimmed) =
                        let_through(whole, whole, backdrop_share, excluded);
                    if let Some(lit) = quads.deposit(
                        &frame,
                        at,
                        (mass.rms_radius() * FLATTENED)
                            .max(covered(mass.rms_radius())),
                        through,
                        dimmed,
                        systems * MARK_AREA,
                        room(at),
                    ) {
                        counted.backdrop += 1;
                        counted.took(lit);
                    }
                }

                // And the colonies, at their own. Absent where there are none,
                // and never stood in for by the stellar centroid: that is how a
                // colony is drawn where there is not one.
                if let Some(held) = colonies
                    && let Some(at) = held.centroid()
                    && in_reach(at)
                {
                    let whole = composition(
                        &held,
                        *color_by,
                        gains.unaligned,
                        Keeps::ALL,
                    );
                    let kept = match mask.narrows() {
                        true => {
                            composition(
                                &held,
                                *color_by,
                                gains.unaligned,
                                mask.keeps(*color_by),
                            ) * off.colonies
                        }
                        false => whole,
                    };
                    let (through, dimmed) =
                        let_through(whole, kept, colony_share, excluded);
                    let scale = carried * gains.mark * MARK_AREA;
                    if let Some(lit) = quads.deposit(
                        &frame,
                        at,
                        (held.spread() * FLATTENED).max(covered(held.spread())),
                        through * scale,
                        dimmed * scale,
                        held.count() as f32 * carried * MARK_AREA,
                        room(at),
                    ) {
                        counted.colonies += 1;
                        counted.took(lit);
                    }
                }
            }
            (quads, counted)
        };
        let splats = &planned.0.splats[..];
        let laid_down: Vec<(Quads, Laid)> = if splats.len() < CHUNK * 2 {
            vec![lay(splats)]
        } else {
            let lay = &lay;
            ComputeTaskPool::get().scope(|scope| {
                for chunk in splats.chunks(CHUNK) {
                    scope.spawn(async move { lay(chunk) });
                }
            })
        };
        for (laid_here, counted_here) in laid_down {
            quads.join(laid_here);
            counted.backdrop += counted_here.backdrop;
            counted.colonies += counted_here.colonies;
            counted.dimmed += counted_here.dimmed;
            counted.separated += counted_here.separated;
        }
    }

    for radius in &quads.radii {
        counted.thinnest = counted.thinnest.min(*radius);
        counted.widest = counted.widest.max(*radius);
        counted.floored += u32::from(*radius <= FINEST * REACH + 1e-3);
    }
    if !quads.radii.is_empty() {
        counted.median = percentile(&mut quads.radii, 0.5);
        counted.tenth = percentile(&mut quads.radii, 0.1);
        counted.quarter = percentile(&mut quads.radii, 0.25);
    }
    counted.light = quads.light;
    if !quads.peaks.is_empty() {
        let peaks = &mut quads.peaks;
        counted.faintest = peaks.iter().copied().fold(f32::INFINITY, f32::min);
        counted.peak = peaks.iter().copied().fold(0., f32::max);
        counted.typical = percentile(peaks, 0.5);
    }
    laid.set_if_neq(counted);

    let Quads { lit, dimmed, .. } = quads;
    let mut strips = [Some(lit), Some(dimmed)];
    for (mark, mut mesh3d) in &mut glow {
        if let Some(strip) = strips[usize::from(mark.dimmed)].take() {
            mesh3d.0 = meshes.add(strip.mesh());
        }
    }
}

/// What one quad came to, for [`Laid`]
#[derive(Clone, Copy)]
struct Lit {
    /// Whether its systems' marks would cover less than the footprint it was
    /// spread over: the cells the map is about to draw as marks.
    separated: bool,
    /// Whether any of it went into the dimmed target.
    dimmed: bool,
}

/// Where the frame is seen from, which is everything a splat's centroid
/// and spread need to become a quad on screen
struct Frame<'a> {
    orbit: &'a OrbitCamera,
    cot_half_fov: f32,
    viewport: Vec2,
    half: Vec2,
    /// An average system's mark ([`Gains::average`]), which the
    /// diagnostics are read in units of.
    mark: f32,
}

/// One target's quads, as its mesh wants them
#[derive(Default)]
struct Strip {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Strip {
    /// One quad `radius` about `(cx, cy)`, at `peak` in the middle.
    fn quad(&mut self, cx: f32, cy: f32, radius: f32, peak: Vec3) {
        let color = [peak.x, peak.y, peak.z, 1.];
        let base = self.positions.len() as u32;
        for (dx, dy, u, v) in [
            (-radius, -radius, 0., 1.),
            (radius, -radius, 1., 1.),
            (radius, radius, 1., 0.),
            (-radius, radius, 0., 0.),
        ] {
            // In front of the origin camera, clear of its near plane.
            self.positions.push([cx + dx, cy + dy, -2.]);
            self.uvs.push([u, v]);
            self.colors.push(color);
        }
        self.indices.extend_from_slice(&[
            base,
            base + 1,
            base + 2,
            base,
            base + 2,
            base + 3,
        ]);
    }

    /// Take `other`'s quads in after these.
    fn join(&mut self, mut other: Strip) {
        let base = self.positions.len() as u32;
        self.positions.append(&mut other.positions);
        self.uvs.append(&mut other.uvs);
        self.colors.append(&mut other.colors);
        self.indices.extend(other.indices.iter().map(|at| at + base));
    }

    fn mesh(self) -> Mesh {
        glow_mesh(self.positions, self.uvs, self.colors, self.indices)
    }
}

/// The frame's quads, the let-through and the dimmed, and what the
/// let-through came to for [`Laid`]
#[derive(Default)]
struct Quads {
    lit: Strip,
    dimmed: Strip,
    /// Footprint radii laid this frame.
    radii: Vec<f32>,
    /// The peak each let-through quad was laid at, in the curve's unit.
    peaks: Vec<f32>,
    /// The light the let-through quads laid, all told, in the curve's unit.
    light: f32,
}

impl Quads {
    /// Lay `lit` and `dimmed` down as one Gaussian about `at`, each into its
    /// own target
    ///
    /// `spread` is how far the cell says its systems are scattered, in light
    /// years, and `covered` the pixels their marks would cover, which says
    /// only whether they have come apart on screen. [`splat`] is the law;
    /// this is the projection either side of it, and the quads.
    ///
    /// [`None`] where nothing was laid: no light to lay, or a centroid the
    /// camera cannot see. The second is bounded by the walk — a cell drawn as
    /// a splat spans a few pixels at most — so what a centroid off the frame
    /// costs is a few pixels at the very edge.
    #[allow(clippy::too_many_arguments)]
    fn deposit(
        &mut self,
        frame: &Frame,
        at: [f64; 3],
        spread: f64,
        lit: Vec3,
        dimmed: Vec3,
        covered: f32,
        // How far the reach leaves this splat to spread, light years, or
        // `None` where the spyglass is not clearing and it may spread as
        // far as it likes. See `build_glow`.
        room: Option<f64>,
    ) -> Option<Lit> {
        let lights = lit.max_element() > 0.0;
        let dims = dimmed.max_element() > 0.0;
        if !lights && !dims {
            return None;
        }
        let position = DVec3::from(at);
        let screen = screen_position(
            frame.orbit,
            frame.cot_half_fov,
            frame.viewport,
            position,
        )?;
        let away =
            crate::map::space::metres(frame.orbit.eye_from(position)).length();
        let per_pixel = world_per_pixel(
            frame.cot_half_fov,
            frame.viewport.y,
            (away as f32).max(1.),
        );
        // The spread comes in as a per-axis deviation in light years, the
        // caller having flattened its cell's own RMS radius ([`FLATTENED`])
        // and floored it on the cell. The pixel scale is in metres, which is
        // the one conversion the map makes and the only place a light year is
        // spoken to the grid.
        let spread_px =
            (spread * crate::map::space::LIGHT_YEAR / per_pixel as f64) as f32;
        if !spread_px.is_finite() {
            return None;
        }

        let (radius, peak) = splat(lit, spread_px);
        let (_, dim_peak) = splat(dimmed, spread_px);
        if !radius.is_finite() || !peak.is_finite() || !dim_peak.is_finite() {
            return None;
        }

        // **Faded to the room the reach leaves, never cut to it.** The quad
        // is square and the profile inside it is not, so a splat stopped at
        // the boundary ends on whatever the Gaussian is worth there — which
        // is most of its peak for anything centred near the edge, laid
        // along four straight sides. The map drew squares.
        //
        // So the whole splat dims instead, by how much of its own footprint
        // the reach still holds: untouched while it fits inside, fading to
        // nothing as its centre reaches the edge. It is smooth in position
        // and smooth in zoom, it has no edge of its own anywhere, and it is
        // what takes the wash off the frame — a splat two thousand pixels
        // wide is barely holding any of itself inside a bubble a few
        // hundred across, whatever its centre is doing.
        let fade = match room {
            Some(room) if room <= 0.0 => return None,
            Some(room) => {
                let room_px = (room * crate::map::space::LIGHT_YEAR
                    / per_pixel as f64) as f32;
                (room_px / radius.max(f32::MIN_POSITIVE)).clamp(0., 1.)
            }
            None => 1.,
        };
        if fade <= 0. {
            return None;
        }

        let cx = screen.x - frame.half.x;
        let cy = frame.half.y - screen.y;
        if lights {
            let peak = peak * fade;
            self.lit.quad(cx, cy, radius, peak);
            let sigma = radius / REACH;
            let level = peak.max_element() / frame.mark;
            self.peaks.push(level);
            self.light += level * std::f32::consts::TAU * sigma * sigma;
        }
        if dims {
            self.dimmed.quad(cx, cy, radius, dim_peak * fade);
        }
        self.radii.push(radius);

        Some(Lit {
            separated: covered < std::f32::consts::TAU * spread_px * spread_px,
            dimmed: dims,
        })
    }

    /// Take `other`'s quads in after these.
    fn join(&mut self, mut other: Quads) {
        self.lit.join(other.lit);
        self.dimmed.join(other.dimmed);
        self.radii.append(&mut other.radii);
        self.peaks.append(&mut other.peaks);
        self.light += other.light;
    }
}

/// How many splats one thread lays: enough that a frame is a few dozen
/// chunks at a wide zoom and none of them is mostly overhead.
const CHUNK: usize = 4096;

/// The value `f` of the way up `values`, by selection rather than a sort:
/// three of these over a frame's hundred thousand radii are a third of what
/// sorting them was.
fn percentile(values: &mut [f32], f: f32) -> f32 {
    let at = ((values.len() - 1) as f32 * f) as usize;
    *values.select_nth_unstable_by(at, f32::total_cmp).1
}

/// Build the field's mesh from the frame's quads, as a fresh asset each frame
///
/// The same reasoning as [`crate::map::paint::field`]'s: a mesh whose size moves trips
/// Bevy's allocator into copying into the slab it has just freed, and a fresh
/// handle costs no more than the in-place path, which reallocates anyway.
/// Never empty, an empty mesh drawing the same error a zero-sized one does.
fn glow_mesh(
    mut positions: Vec<[f32; 3]>,
    mut uvs: Vec<[f32; 2]>,
    mut colors: Vec<[f32; 4]>,
    mut indices: Vec<u32>,
) -> Mesh {
    if positions.is_empty() {
        positions = vec![[0., 0., -2.]; 3];
        uvs = vec![[0., 0.]; 3];
        colors = vec![[0., 0., 0., 0.]; 3];
        indices = vec![0, 1, 2];
    }
    let mut mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(bevy::mesh::Indices::U32(indices));
    mesh
}

/// A Gaussian profile for a splat's footprint: peaking at one in the middle
/// and falling to nothing by the quad's edge
///
/// A splat is a distribution and not a disc, which is the difference between
/// a field that reads as a gradient and one that reads as a mosaic of circles.
/// Cut to [`REACH`] standard deviations across the half-width, so the value at
/// the rim is a percent of the peak and the seam where the quad ends does not
/// show.
pub(crate) fn gaussian_mask() -> Image {
    /// What the bare profile still stands at when the quad ends, and so what
    /// is taken off it everywhere: `exp(-REACH^2 / 2)`. Subtracting it costs
    /// the deposit about a part in three thousand and removes an edge step
    /// outright, which is the trade the whole constant exists to make.
    const RIM: f32 = 0.000_335_462_63;
    let n = GLOW_TEXELS;
    let centre = (n as f32 - 1.) / 2.;
    let sigma = centre / REACH;
    let mut data = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let dx = x as f32 - centre;
            let dy = y as f32 - centre;
            let r2 = dx * dx + dy * dy;
            // Pedestal-subtracted, so the profile reaches exactly zero at the
            // rim rather than stepping off whatever it still had there. A
            // quad ending on a discontinuity draws its own edge, and a field
            // is thousands of quads whose edges agree.
            let mask = (((-r2 / (2. * sigma * sigma)).exp() - RIM).max(0.))
                / (1. - RIM);
            let value = (mask * 255.).round().clamp(0., 255.) as u8;
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
    use elite_journal::prelude::Allegiance;
    use galos_index::read::inhabited::Readings;

    fn empire() -> Inhabited {
        Inhabited::of_system(
            [0.0; 3],
            Readings {
                allegiance: Some(Allegiance::Empire),
                ..Readings::default()
            },
        )
    }

    fn unreported() -> Inhabited {
        Inhabited::of_system([0.0; 3], Readings::default())
    }

    /// A cell of one Empire system lays down the Empire's colour at full
    /// weight, which is what makes a shell read as a shell.
    #[test]
    fn a_colony_lays_down_its_own_colour() {
        let light =
            composition(&empire(), ColorBy::Allegiance, 0.25, Keeps::ALL);
        assert!((light - Hue::Cyan.light()).length() < 1e-6);
    }

    /// Four unreported colonies against one aligned one: under equal weight
    /// the grey would out-deposit the colour four to one, and the gain is what
    /// stops it.
    #[test]
    fn grey_does_not_bury_a_shell() {
        let mut crowd = Inhabited::ZERO;
        for _ in 0..4 {
            crowd = crowd.merge(unreported());
        }
        let cell = crowd.merge(empire());
        let light = composition(&cell, ColorBy::Allegiance, 0.25, Keeps::ALL);
        let grey = 4.0 * 0.25;
        // The red channel carries only the grey, which is neutral; blue and
        // green carry the Empire's cyan over it.
        assert!((light.x - grey).abs() < 1e-6);
        assert!(
            light.z > light.x,
            "grey {} buried the shell {}",
            light.x,
            light.z
        );
        assert!((light.z - (Hue::Cyan.light().z + grey)).abs() < 1e-6);
    }

    /// The kinds of system keep the order the gains mean: nobody home under
    /// a colony with nothing on record, under a colony with a reading
    #[test]
    fn the_gains_keep_their_order() {
        let gains = Gains::default();
        let empty = mark_light(Hue::Grey, false, &gains);
        let unaligned = mark_light(Hue::Grey, true, &gains);
        let aligned = mark_light(Hue::Cyan, true, &gains);
        assert!(empty > 0., "an uninhabited system is still a system");
        assert!(empty < unaligned, "an empty system outshone a colony");
        assert!(
            unaligned < aligned,
            "a colony with nothing on record outshone one with a reading"
        );
    }

    /// A splat lays exactly the light the systems it stands for would as
    /// marks, whatever it is spread over and however many it holds, and is
    /// always spread over its cell
    ///
    /// **This is the handoff, and the whole of the law.** A cell's light
    /// moves from the field to its own marks as its payload arrives, and the
    /// two halves have to meet at the crossing or the map lies twice: a
    /// filament fades out as the camera comes in, and then lights up again
    /// when its systems land. And a crowd is the sum of its systems: a
    /// correction struck on a cell's own count is what made a star's light
    /// depend on what else its cell held.
    #[test]
    fn a_splat_lays_what_its_marks_would() {
        let gains = Gains::default();
        let level = mark_light(Hue::Cyan, true, &gains);
        for spread in [0.1f32, 0.5, 5., 50., 500., 5_000.] {
            for systems in [1f32, 10., 1_000., 100_000.] {
                let marks = systems * level * MARK_AREA;
                let (radius, peak) = splat(Hue::Cyan.light() * marks, spread);
                assert_eq!(
                    radius,
                    spread.max(FINEST) * REACH,
                    "{systems} systems over {spread} px were not laid over \
                     their cell"
                );
                let sigma = radius / REACH;
                let laid =
                    peak.max_element() * std::f32::consts::TAU * sigma * sigma;
                assert!(
                    (laid - marks).abs() <= marks * 1e-4,
                    "{systems} systems over {spread} px laid {laid} where \
                     their marks lay {marks}"
                );
            }
        }
    }

    /// Two star classes of the same count lay the same light, whatever else
    /// shares their cells
    ///
    /// What the field was reported for: a key showing only brown dwarfs and
    /// one showing only the white dwarfs and neutron stars, near enough the
    /// same count each, drew the first at a fifth of the second — the dwarfs
    /// sit among the well-scanned crowd and the remnants in the sparse disc,
    /// and the field held each cell down by its own crowd.
    #[test]
    fn equal_counts_of_two_classes_lay_equal_light() {
        let gains = Gains::default();
        let mut crowded = [0u32; StarKind::COUNT];
        crowded[usize::from(StarKind::BrownDwarf.code())] = 10;
        crowded[usize::from(StarKind::M.code())] = 50_000;
        crowded[usize::from(StarKind::K.code())] = 30_000;
        let mut sparse = [0u32; StarKind::COUNT];
        sparse[usize::from(StarKind::Neutron.code())] = 10;

        let only = |kind: StarKind| {
            let mut mask = crate::map::filter::mask::Mask::default();
            mask.draw(Some(ColorBy::StarClass));
            let hidden = (0..StarKind::COUNT)
                .filter(|code| *code != usize::from(kind.code()));
            mask.set(ColorBy::StarClass, hidden, true);
            mask.keeps(ColorBy::StarClass)
        };
        let lay = |kinds: &[u32; StarKind::COUNT], keeps: Keeps| {
            let whole = starlight(kinds, &gains, Keeps::ALL);
            let kept = starlight(kinds, &gains, keeps);
            let (through, _) = let_through(whole, kept, 1., true);
            // The same cell drawn at two sizes: what a splat lays all told
            // does not turn on its spread.
            [3f32, 300.].map(|spread| {
                let (radius, peak) = splat(through, spread);
                let sigma = radius / REACH;
                peak.max_element() * std::f32::consts::TAU * sigma * sigma
            })
        };
        let dwarfs = lay(&crowded, only(StarKind::BrownDwarf));
        let remnants = lay(&sparse, only(StarKind::Neutron));
        for (dwarfs, remnants) in dwarfs.into_iter().zip(remnants) {
            assert!(
                (dwarfs - remnants).abs() <= remnants * 1e-4,
                "ten brown dwarfs among eighty thousand laid {dwarfs}, ten \
                 neutron stars alone {remnants}"
            );
        }
    }

    /// The composition is a sum, so it composes the way the aggregate does: a
    /// cell's mix is its parts' mixes added, with no normalisation in between
    /// to lose.
    #[test]
    fn composition_is_additive() {
        let a = empire();
        let b = unreported();
        let all = Keeps::ALL;
        let whole = composition(&a.merge(b), ColorBy::Allegiance, 0.25, all);
        let left = composition(&a, ColorBy::Allegiance, 0.25, all);
        let right = composition(&b, ColorBy::Allegiance, 0.25, all);
        assert!((whole - (left + right)).length() < 1e-6);
    }

    /// An empty cell deposits nothing at all rather than a black quad, and has
    /// no place to deposit it at.
    #[test]
    fn nobody_home_deposits_nothing() {
        let light = composition(
            &Inhabited::ZERO,
            ColorBy::Allegiance,
            0.25,
            Keeps::ALL,
        );
        assert_eq!(light, Vec3::ZERO);
        assert_eq!(Inhabited::ZERO.centroid(), None);
    }

    /// A cell whose every colony is hidden lets nothing through, and lays
    /// the whole of itself into the dimmed target — or nowhere, where the
    /// excluded are not drawn
    #[test]
    fn a_fully_masked_cell_lets_nothing_through() {
        let mut mask = crate::map::filter::mask::Mask::default();
        mask.set(ColorBy::Allegiance, 0..11, true);
        let cell = empire().merge(unreported());
        let keeps = mask.keeps(ColorBy::Allegiance);
        let kept = composition(&cell, ColorBy::Allegiance, 0.25, keeps);
        assert_eq!(kept, Vec3::ZERO);

        let whole = composition(&cell, ColorBy::Allegiance, 0.25, Keeps::ALL);
        let (through, dimmed) = let_through(whole, kept, 1.0, true);
        assert_eq!(through, Vec3::ZERO, "a hidden cell was let through");
        assert!((dimmed - whole).length() < 1e-6, "dimmed {dimmed}");
        let (_, dimmed) = let_through(whole, kept, 1.0, false);
        assert_eq!(dimmed, Vec3::ZERO, "laid where the excluded are dropped");
    }

    /// **The light a hidden colony takes out of the field is the light its
    /// mark takes off the map.** A cell of an Empire and an unreported colony
    /// with the Empire hidden lets the unreported one through and dims the
    /// Empire, as their two marks would.
    #[test]
    fn a_hidden_colony_leaves_the_field_as_its_mark_does() {
        let mut mask = crate::map::filter::mask::Mask::default();
        mask.set(ColorBy::Allegiance, [2], true);
        let cell = empire().merge(unreported());
        let whole = composition(&cell, ColorBy::Allegiance, 0.25, Keeps::ALL);
        let kept = composition(
            &cell,
            ColorBy::Allegiance,
            0.25,
            mask.keeps(ColorBy::Allegiance),
        );
        let (through, dimmed) = let_through(whole, kept, 1.0, true);
        assert!((through - Hue::Grey.light() * 0.25).length() < 1e-6);
        assert!((dimmed - Hue::Cyan.light()).length() < 1e-6);

        // And with nothing hidden, the filters' share of the cell is let
        // through and the rest dimmed.
        let (through, dimmed) = let_through(whole, whole, 0.25, true);
        assert!((through - whole * 0.25).length() < 1e-6);
        assert!((dimmed - whole * 0.75).length() < 1e-6);
    }

    /// The mask is a distribution: brightest in the middle, under a percent of
    /// that at the rim, so the quad's edge does not show as a seam.
    #[test]
    fn the_mask_falls_to_nothing_at_the_rim() {
        let image = gaussian_mask();
        let n = GLOW_TEXELS as usize;
        let data = image.data.as_ref().unwrap();
        let at = |x: usize, y: usize| data[(y * n + x) * 4];
        // The centre falls between texels at an even width, so the brightest
        // texel sits half a texel off the peak rather than on it.
        assert!(at(n / 2, n / 2) >= 250, "the middle is not the peak");
        assert!(at(0, n / 2) < 4, "the rim is not dark: {}", at(0, n / 2));
        assert!(at(n / 2, 0) < 4);
    }
}

/// What the field actually lays down over a real directory.
///
/// The exposure cannot be read off a unit test with three systems in it: what
/// decides whether the field is a picture or a white sheet is how much content
/// a splatted cell holds and how many pixels its footprint covers, and both
/// are properties of a built galaxy. So this stands the walk and the field up
/// over one and reports the numbers, the way [`crate::map::galaxy::flight`] does for the
/// fetch.
///
/// It earns its place because the field has no other check. A render change is
/// judged by looking at it, and looking requires a live display — the window's
/// surface is where the map exists — so on a machine whose screen has slept
/// every capture comes back black and says nothing. These numbers hold either
/// way.
///
/// Stands down without `GALOS_PERF_DIR` naming a built index:
///
/// ```sh
/// GALOS_PERF_DIR=.galos_index cargo test -p galos_map --lib glow -- --nocapture
/// ```
#[cfg(test)]
mod exposure {
    use super::*;
    use crate::map::camera::OrbitCamera;
    use crate::map::galaxy::plan::Planned;
    use crate::map::index::{Populated, ResidentIndex, Settled};
    use galos_index::prelude::{FsSource, Source};
    use galos_index::read::inhabited::Inhabitance;
    use std::path::PathBuf;

    fn measured() -> Option<PathBuf> {
        let dir = PathBuf::from(std::env::var("GALOS_PERF_DIR").ok()?);
        dir.join("index.bin").exists().then_some(dir)
    }

    /// The field's own numbers with the eye `away` light years off Sol.
    ///
    /// The eye is placed and not the orbit radius: `OrbitCamera::stood_back`
    /// sets a radius the camera's own placement system derives an eye from,
    /// and that system does not run here — so three radii measured three times
    /// at the origin and read as one answer.
    /// How the map is set up for a measurement: how wide the spyglass is
    /// clearing at, and whether the marks have already accounted for every
    /// system the field would otherwise draw.
    #[derive(Clone)]
    struct Set {
        /// The spyglass radius in light years, or [`None`] for not clearing.
        reach: Option<f32>,
        /// Whether every splatted cell is taken as drawn in full.
        accounted: bool,
        /// A filter to put on the map before the field is laid.
        asked: Option<crate::map::filter::Filter>,
        /// Whether the sky is read as populations.
        peopled: bool,
        /// The axis the map is colored by.
        color_by: ColorBy,
    }

    impl Set {
        /// The far case the exposure is judged on: no boundary, nothing drawn.
        fn open() -> Set {
            Set {
                reach: None,
                accounted: false,
                asked: None,
                peopled: false,
                color_by: ColorBy::Allegiance,
            }
        }

        /// The same, with a filter on the map.
        fn asking(filter: crate::map::filter::Filter) -> Set {
            Set { asked: Some(filter), ..Set::open() }
        }
    }

    fn laid_at(dir: &PathBuf, away: f64, set: Set) -> (Laid, usize) {
        let source = FsSource::new(dir);
        let (index, populated) = pollster::block_on(async {
            (
                source
                    .index()
                    .await
                    .expect("the index should read")
                    .lit(&source.lights().await.unwrap_or_default()),
                source.populated().await.unwrap_or_default(),
            )
        });
        let settled = Inhabitance::of(&index, populated.iter());

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<Mesh>();
        app.insert_resource(ResidentIndex(index));
        app.insert_resource(Settled(std::sync::Arc::new(settled)));
        app.insert_resource(Populated::default());
        // Nothing is drawn as itself in this harness, so nothing is accounted
        // for and the field lays every cell's aggregate down whole. That is
        // the far case the exposure is judged on.
        app.init_resource::<crate::map::galaxy::plan::Drawn>();
        app.insert_resource(crate::map::galaxy::Spyglass {
            // What `reach_with_camera` would set from this distance where
            // the test is not clearing at one of its own: the tilt reads
            // the reach, so a measurement has to stand where the client
            // would. See `crate::map::galaxy::reach_with_camera`.
            radius: set.reach.unwrap_or((away * 0.37) as f32),
            clear: set.reach.is_some(),
            lock_camera: false,
            follow_camera: true,
        });
        app.insert_resource(View::Map);
        app.insert_resource(set.color_by);
        app.insert_resource(crate::map::paint::sizing::ScalePopulation(
            set.peopled,
        ));
        app.init_resource::<Gains>();
        app.init_resource::<Laid>();
        app.insert_resource(Planned(galos_index::prelude::Needed {
            mode: galos_index::prelude::Mode::Shell,
            marks: Vec::new(),
            blobs: Vec::new(),
            splats: Vec::new(),
        }));

        // The mesh the field is laid into, and a camera that can say what it
        // sees. `spawn_glow` wants an image and material store this does not
        // stand up, so the entity is made by hand with the one component
        // `build_glow` writes.
        let mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(glow_mesh(Vec::new(), Vec::new(), Vec::new(), Vec::new()));
        for dimmed in [false, true] {
            app.world_mut().spawn((Mesh3d(mesh.clone()), GlowMark { dimmed }));
        }
        // Aimed, and not merely placed. `walk_screen` is a pure function of
        // where the eye *is* and never of where it points, so the plan comes
        // back full whatever the rotation — but the field is laid at projected
        // centres, so a camera left on the identity rotation looks away from
        // the galaxy and every splat in the plan falls off the frame. Measured
        // before this line went in: 3,376 splats at thirty thousand light
        // years and not one quad laid.
        let eye = DVec3::new(0., 0., -away);
        let mut camera = OrbitCamera::standing_at(eye);
        camera.rotation =
            Quat::from_rotation_arc(Vec3::NEG_Z, (-eye.as_vec3()).normalize());
        app.world_mut().spawn((
            camera,
            Projection::Perspective(PerspectiveProjection::default()),
            crate::map::galaxy::tests::seeing(),
        ));
        // What the filters leave of each channel, and the cells holding
        // what they name — empty here, so a filter that names systems
        // names none of these.
        app.init_resource::<crate::map::galaxy::blobs::Named>();
        // The sky's cut, which the plan reads for the photometric mode.
        app.init_resource::<crate::map::galaxy::spawn::StarExposure>();
        app.init_resource::<crate::map::filter::DimTo>();
        let mut filters = crate::map::filter::Filters::default();
        if let Some(asked) = set.asked.clone() {
            filters.add(asked);
        }
        app.insert_resource(filters);

        let world = app.world_mut();
        let plan = world.register_system(crate::map::galaxy::plan::plan);
        let build = world.register_system(build_glow);
        let gains = world.register_system(settle_gains);
        world.run_system(gains).expect("the gains settle");
        world.run_system(plan).expect("the walk plans");
        // Every splatted cell taken as fully drawn, by handing the field
        // exactly the totals it is about to subtract. Built from the
        // aggregates rather than from points so the residual is zero to the
        // bit, which is what the claim under test is.
        if set.accounted {
            let splats: Vec<_> = world
                .resource::<Planned>()
                .0
                .splats
                .iter()
                .map(|splat| splat.id)
                .collect();
            let index = world.resource::<ResidentIndex>().0.clone();
            let settled = world.resource::<Settled>().0.clone();
            let mut drawn =
                world.resource_mut::<crate::map::galaxy::plan::Drawn>();
            for id in splats {
                let Some(cell) = index.get(id) else { continue };
                drawn.0.insert(
                    id,
                    crate::map::galaxy::plan::Accounted {
                        count: cell.aggregate.count(),
                        mass: cell.aggregate.mass(),
                        inhabited: settled.get(id).copied().unwrap_or_default(),
                        kinds: *cell.aggregate.kinds(),
                    },
                );
            }
        }
        world.run_system(build).expect("the field builds");
        let planned = world.resource::<Planned>().0.splats.len();
        (*world.resource::<Laid>(), planned)
    }

    /// Reading the sky as populations puts the backdrop out, as it puts
    /// the marks of the same systems out
    ///
    /// The two halves of the picture are one light: an uninhabited
    /// system's mark is not drawn in that mode, so the field standing in
    /// for one must not be either. Reported as the galaxy keeping its grey
    /// while the marks over it were only the colonies.
    #[test]
    fn the_populated_sky_lays_no_backdrop() {
        let Some(dir) = measured() else { return };
        let (whole, _) = laid_at(&dir, 30_000., Set::open());
        assert!(whole.backdrop > 0, "the ordinary sky laid no backdrop");

        let (peopled, _) =
            laid_at(&dir, 30_000., Set { peopled: true, ..Set::open() });
        assert_eq!(
            peopled.backdrop, 0,
            "the crowd nobody lives in was laid where no mark of it is drawn",
        );
        assert_eq!(
            peopled.colonies, whole.colonies,
            "the colonies went with it",
        );
    }

    /// A filter reaches the field, and dims what it excludes rather than
    /// dropping it
    ///
    /// **The field has to answer the filters or the picture changes as it
    /// resolves.** [`mark_light`] is one figure for a mark and for the
    /// light laid down in a system's place, so a filter that reached the
    /// marks and not the field would appear to take effect only where the
    /// camera had come in far enough to draw the systems themselves — and
    /// at galaxy scale that is nowhere.
    ///
    /// A faction nobody is in names no system in any cell, so nothing is
    /// let through and every quad is laid into the dimmed target: the field
    /// goes faint rather than dark, which is what says the sky is still
    /// there and is not what was asked for.
    #[test]
    fn a_filter_dims_the_field() {
        let Some(dir) = measured() else { return };
        let (whole, splats) = laid_at(&dir, 30_000., Set::open());
        assert!(splats > 0, "nothing was planned to lay");
        assert!(whole.peak > 0., "the field laid nothing unfiltered");
        assert_eq!(whole.dimmed, 0, "an open map dimmed something");

        let (filtered, _) = laid_at(
            &dir,
            30_000.,
            Set::asking(crate::map::filter::Filter::Faction {
                id: -1,
                name: "Nobody".into(),
            }),
        );
        assert_eq!(
            filtered.backdrop, whole.backdrop,
            "a filter dropped the field instead of dimming it",
        );
        assert_eq!(
            filtered.dimmed,
            filtered.backdrop + filtered.colonies,
            "a quad the filter excludes was not dimmed",
        );
        assert_eq!(
            filtered.peak, 0.,
            "a faction nobody is in let light through"
        );
    }

    /// The field is laid at every zoom and along both kinds of axis: both
    /// channels down, and a middling splat that lays something
    ///
    /// Read on the median peak and not the brightest one. The brightest
    /// splat is a cell in the galaxy's core and is bright from anywhere, so
    /// a field that has faded to nothing everywhere else passes on it.
    /// Where the peaks stand on the curve is printed, in its unit: the
    /// curve's knots are placed against these.
    ///
    /// Spending `blend * count` rather than `blend * total` took the peak
    /// to four thousandths and the field was invisible, and nothing said
    /// so but this.
    #[test]
    fn the_field_is_exposed() {
        let Some(dir) = measured() else { return };
        for color_by in [ColorBy::Allegiance, ColorBy::StarClass] {
            for away in [20., 200., 2_000., 30_000.] {
                let (laid, splats) =
                    laid_at(&dir, away, Set { color_by, ..Set::open() });
                println!(
                    "{color_by:?} {away:>8} ly out: {splats:>5} splats, {:>5} colonies, \
                 {:>5} backdrop, light {:.1}, \
                 peak {:.5}|{:.5}|{:.3} average marks, \
                 radius {:.2}|{:.2}|{:.2}|{:.2}|{:.2} px, {} floored, \
                 {} separated",
                    laid.colonies,
                    laid.backdrop,
                    laid.light,
                    laid.faintest,
                    laid.typical,
                    laid.peak,
                    laid.thinnest,
                    laid.tenth,
                    laid.quarter,
                    laid.median,
                    laid.widest,
                    laid.floored,
                    laid.separated,
                );
                assert!(
                    laid.backdrop > 0,
                    "the galaxy behind the shells was not drawn at {away} ly"
                );
                assert!(
                    laid.typical > 0.,
                    "the middling splat laid nothing at {away} ly"
                );
                // The field resolves to the frame and not to the split. A
                // splat's kernel is its cell's own spread ([`FLATTENED`]),
                // floored on half the cell ([`COVERAGE`]) and then on half a
                // pixel ([`FINEST`]) — and with [`galos_index::read::walk::SPLIT_PX`]
                // cutting at half a pixel of contents, the last of those three
                // is what catches a frontier cell wherever the tree has depth
                // to give. So some of every frame is laid on the pixel floor,
                // and a frame with none is one whose splats are all wider than
                // the display can show: the two-to-four-pixel band this used to
                // cut at laid not one splat on it at any zoom.
                assert!(
                    laid.floored > 0,
                    "nothing at {away} ly was laid at the frame's own resolution: \
                 thinnest radius {} px",
                    laid.thinnest
                );
            }
        }
    }

    /// A cell whose systems are all on the map as themselves lays down no
    /// field, so the two halves of the walk never draw one system twice.
    ///
    /// The walk marks a cell and splats it in the same pass, deliberately, so
    /// without the residual the field carries every marked cell's whole
    /// subtree a second time behind the marks standing in front of it. This is
    /// that subtraction, at its limit: account for everything and nothing is
    /// left to lay.
    #[test]
    fn what_the_marks_draw_the_field_does_not() {
        let Some(dir) = measured() else { return };
        let (whole, splats) = laid_at(&dir, 2_000., Set::open());
        let (residual, same) =
            laid_at(&dir, 2_000., Set { accounted: true, ..Set::open() });

        assert_eq!(splats, same, "the plan moved between the two");
        assert!(
            whole.colonies > 0 && whole.backdrop > 0,
            "nothing to subtract"
        );
        println!(
            "{splats} splats: {} + {} quads whole, {} + {} after the marks",
            whole.colonies,
            whole.backdrop,
            residual.colonies,
            residual.backdrop
        );
        assert_eq!(
            (residual.colonies, residual.backdrop),
            (0, 0),
            "the field drew systems the marks had already drawn"
        );
        assert_eq!(residual.light, 0., "light was laid twice");
    }

    /// To bound the view is to clear away what the reach does not hold, and
    /// the field is part of the view.
    ///
    /// A spyglass that clears is a boundary over the whole map, not a sphere
    /// of symbols standing in a field that carries on past it. Measured from
    /// far enough out that most of the galaxy is on screen, so there is plenty
    /// outside the reach for the field to have drawn.
    #[test]
    fn the_spyglass_holds_the_field_in() {
        let Some(dir) = measured() else { return };
        let (open, splats) = laid_at(&dir, 30_000., Set::open());
        let (held, _) =
            laid_at(&dir, 30_000., Set { reach: Some(200.), ..Set::open() });

        println!(
            "{splats} splats: {} + {} quads unbounded, {} + {} inside 200 ly \
             (peak {:.2} average marks)",
            open.colonies,
            open.backdrop,
            held.colonies,
            held.backdrop,
            held.peak,
        );
        assert!(
            held.backdrop < open.backdrop / 10,
            "the field drew past the reach: {} of {} quads",
            held.backdrop,
            open.backdrop
        );
        // And nothing wide is left standing. A splat's own footprint is a
        // cell wide and the widest of them runs to two thousand pixels
        // unbounded, which is a wash over the whole frame laid from a
        // centre well inside the reach; faded by how much of itself the
        // reach holds, a splat that size is gone and the field ends about
        // where the marks do.
        assert!(
            held.widest < open.widest / 10.,
            "a splat spread past the reach: {} px against {} px unbounded",
            held.widest,
            open.widest
        );
        assert!(
            held.light < open.light,
            "clearing the view laid no less light down"
        );
    }

    /// The colonies are drawn where the colonies are, which is not where the
    /// stars are. Measured over `.galos_index`, the root's two centroids are
    /// 12.5 thousand light years apart.
    #[test]
    fn the_colonies_are_not_drawn_at_the_stellar_centre() {
        let Some(dir) = measured() else { return };
        let source = FsSource::new(&dir);
        let (index, populated) = pollster::block_on(async {
            use galos_index::prelude::Source as _;
            (
                source
                    .index()
                    .await
                    .expect("the index should read")
                    .lit(&source.lights().await.unwrap_or_default()),
                source.populated().await.unwrap_or_default(),
            )
        });
        let settled = Inhabitance::of(&index, populated.iter());
        let root = galos_index::prelude::CellId::ROOT;
        let stellar = index
            .get(root)
            .and_then(|cell| cell.aggregate.count_centroid())
            .expect("a root centroid");
        let held = settled.get(root).expect("somebody lives in the galaxy");
        let colonies = held.centroid().expect("a colony centroid");

        let apart = ((stellar[0] - colonies[0]).powi(2)
            + (stellar[1] - colonies[1]).powi(2)
            + (stellar[2] - colonies[2]).powi(2))
        .sqrt();
        println!(
            "stellar {stellar:?} vs colonies {colonies:?}: {apart:.0} ly apart"
        );
        assert!(
            apart > 1_000.,
            "the two centroids agreed, so the field has nothing to gain by \
             keeping them apart: {apart} ly"
        );
        assert!(
            held.spread() < index.get(root).unwrap().aggregate.count_extent(),
            "the colonies spread wider than the stars they sit among"
        );
    }
}
