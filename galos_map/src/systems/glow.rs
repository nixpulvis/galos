//! The political field, splatted from the aggregates
//!
//! [`super::aggregate`] walks the index into the cells whose systems separate
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
//! cell that holds it. [`galos_index::Inhabited`] is the second distribution
//! and exists for this.
//!
//! **One light per system, and a correction for the crowd.** A system is
//! worth the same light whether the map draws it as its own mark or the
//! field stands in for it — [`mark_light`] is the one figure — and that is
//! what makes the handoff between them invisible: as a cell's payload lands,
//! its light moves out of the field and into the marks with nothing added
//! and nothing lost. What the field does differently is hold a *crowd*
//! down. An uninhabited system is a density fact and not a political one,
//! and there are forty-odd of them for every colony, so laid down at equal
//! weight they bury every shell colour under grey — which is what the map
//! did. So a crowded splat is divided by [`Gains::crowd`], and the crowd
//! nobody lives in by [`Gains::backdrop`] again, derived from the index's
//! own populated share rather than guessed. Neither is stored: a weight
//! baked into an aggregate cannot be retuned and makes a residual wrong
//! under any other gain.
//!
//! **The correction is spent as a cell resolves**, in proportion to the
//! crowding it corrects for, so by the time a cell's systems have come apart
//! on screen the field is laying down exactly the light their marks will.
//! Flat, it left the field seventeen times under those marks *and* falling
//! as the square of the zoom, so a filament faded out as the camera came in
//! and lit up again when its payload landed. See [`splat`], which is the
//! whole of the law.
//!
//! **Resolved per cell and added in linear light.** Each splat's mix is summed
//! on the CPU — eight multiply-adds against a histogram that is already the
//! local composition, at the level the walk chose to draw it — and deposited as
//! one additive Gaussian. Addition is order-independent, so neighbouring cells
//! sum into a continuous field with no seam and no draw-order artifact, and a
//! fine cell beside a coarse one needs no stitching: the weights the walk
//! hands out conserve, so the total is right whatever level each region
//! resolved at. Where two cells of different dominant colour overlap they add
//! toward white, which is the mixing reading; `doc/galaxy.md`'s offscreen
//! per-category resolve would instead desaturate by measured entropy and keep
//! the intensity, and nothing here forecloses it — the deposited quantity is
//! the same either way.
//!
//! **Every splat is a pixel or two by construction**, which is what makes
//! one isotropic Gaussian enough. A cell hands its light to its children
//! once its contents subtend [`galos_index::walk::SPLIT_PX`] — half a pixel,
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
//! Drawn on [`FIELD_LAYER`] beside [`super::field`]'s marks, under them: the
//! quads sit a unit further from the origin camera so the symbols composite
//! over the field they stand in rather than the other way about.

use crate::camera::{FIELD_LAYER, OrbitCamera};
use crate::schedule::MapSet;
use crate::systems::aggregate::Planned;
use crate::systems::labels::{screen_position, world_per_pixel};
use crate::systems::scale::View;
use crate::systems::spawn::{
    ColorBy, Hue, allegiance_hue, government_hue, security_hue,
};
use crate::{ResidentIndex, Settled};
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::image::{Image, ImageSampler};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat,
};
use galos_index::inhabited::{
    Inhabited, allegiance_at, government_at, security_at,
};

pub fn plugin(app: &mut App) {
    app.init_resource::<Gains>();
    app.init_resource::<FieldExposure>();
    app.init_resource::<Laid>();
    app.add_systems(Startup, spawn_glow);
    app.add_systems(
        Update,
        (settle_gains, build_glow.after(crate::systems::aggregate::plan))
            .chain()
            .in_set(MapSet::Present),
    );
}

/// How many stops the political field is lifted to the display
///
/// The one dial over the whole field, and the reason it exists is that how
/// loud an unresolved galaxy should be is a reading and not a fact: the map
/// is a political instrument at one setting and a picture of where anybody
/// has been at another, and neither is wrong. What it scales is the light
/// the field deposits, both channels together, before the crowding
/// correction and the ceiling — so opening it lifts the faint half of the
/// frame and the roll-off goes on holding the packed core down, which is
/// what makes it usable rather than a way of washing the frame out.
///
/// The marks are not on it. A system drawn as itself is an object at a set
/// brightness ([`mark_light`]), and the two coming apart is exactly what
/// this module spent a rewrite closing — so the dial moves what stands in
/// for the systems that are not drawn, and once a region resolves, the
/// setting stops mattering there.
#[derive(Resource)]
pub struct FieldExposure(pub f32);

/// The dial rests at zero: neutral, the tuned look, stops either way from
/// there. The same rest and the same units the realistic sky's own exposure
/// is offered at ([`super::spawn::StarExposure`]).
impl Default for FieldExposure {
    fn default() -> FieldExposure {
        FieldExposure(0.)
    }
}

impl FieldExposure {
    /// The linear gain the stops come to: a doubling per stop.
    pub(crate) fn factor(&self) -> f32 {
        2f32.powf(self.0)
    }
}

/// What the field laid down last frame
///
/// The two channels counted apart, because which of them is carrying a view
/// is the whole question the gains answer: a frame of nothing but backdrop is
/// a political field that has been buried, and a frame of no backdrop at all
/// is one that has lost the galaxy behind it.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Laid {
    /// Quads laid at a cell's inhabited centroid.
    pub colonies: u32,
    /// Quads laid at a cell's stellar centroid, for the systems nobody lives
    /// in.
    pub backdrop: u32,
    /// Linear light deposited, summed over every channel of every quad.
    pub light: f32,
    /// The brightest peak any one quad was laid at, in linear light.
    ///
    /// The top of the field: at [`CEILING`] for everything it is a white
    /// sheet, and the core over the bubble is what is meant to clip.
    pub peak: f32,
    /// The faintest and the middling peak, in linear light.
    ///
    /// **What says whether the field is a picture.** The brightest splat is
    /// one cell in the galaxy's core and is bright at every zoom, so a field
    /// that has faded to nothing everywhere else reads the same by it. The
    /// median is what the frame is actually made of, and the fade the
    /// crowding correction is spent to stop showed up here and nowhere
    /// else: measured over `.galos_index` under a flat correction, three
    /// thousandths with the galaxy seen whole and a hundred and sixty
    /// *millionths* from twenty light years out — one level off black on an
    /// eight-bit display — with the brightest splat sat at a third of a
    /// unit throughout, saying nothing was wrong.
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
    /// less of the footprint than they are spread over (see [`splat`]).
    ///
    /// The resolving half of the frame, and what the crowding correction is
    /// spent over. These are the cells the map is about to draw as marks and
    /// has not fetched yet, so the number is also how much of the field is
    /// waiting on a payload.
    pub separated: u32,
    /// Quads whose peak hit [`CEILING`] and clipped.
    ///
    /// A handful is the bright core doing what the bright core does; most of
    /// the frame is an over-exposed field, and the number is what tells the
    /// two apart without a picture.
    pub clipped: u32,
}

impl Laid {
    /// Count one quad in, off what [`Quads::deposit`] laid.
    fn took(&mut self, lit: Lit) {
        self.light += lit.light;
        self.peak = self.peak.max(lit.peak);
        self.clipped += u32::from(lit.peak >= CEILING);
        self.separated += u32::from(lit.separated);
    }
}

impl Default for Laid {
    fn default() -> Laid {
        Laid {
            colonies: 0,
            backdrop: 0,
            light: 0.,
            peak: 0.,
            faintest: 0.,
            typical: 0.,
            clipped: 0,
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

/// The mesh the field is laid into, one quad a channel a splat
#[derive(Component)]
struct GlowMark;

/// What each channel of the field is worth, in the linear light it deposits
///
/// **One light per system, and a suppression for the crowd.** A system is
/// worth the same light whether it is drawn as itself or stood in for by the
/// field — that is what makes the handoff between them invisible — so the
/// three levels below are the mark's and the field's alike. What the field
/// then does differently is hold a *crowd* down: ten thousand systems in a
/// pixel are not ten thousand marks' worth of light, they are a backdrop,
/// and [`crowd`](Self::crowd) is how hard that backdrop is pressed. It is
/// undone as the crowd resolves (see [`splat`]), so what the field lays down
/// where its systems separate is exactly what their marks will draw.
///
/// Held as a resource rather than as constants because the balance between
/// the colonies and the ungoverned galaxy behind them is a reading and not a
/// fact, and because [`settle_gains`] derives one of them from the index.
#[derive(Resource, Debug, Clone, Copy)]
pub struct Gains {
    /// How bright an ordinary inhabited system is drawn, in linear light
    ///
    /// The unit the other two are shares of, and the field's exposure with
    /// it: a crowded cell is this divided by [`crowd`](Self::crowd), and a
    /// resolved one is this outright.
    pub mark: f32,
    /// How much of [`mark`](Self::mark) an inhabited system with nothing
    /// political on record is worth
    ///
    /// Not zero. A colony whose allegiance nobody has reported is still a
    /// colony, and the neutral light it draws in is how the map says so;
    /// what it must not do is out-shout the systems that do have a reading.
    pub unaligned: f32,
    /// How much of [`mark`](Self::mark) a system nobody lives in is worth
    ///
    /// Dimmer, not absent: an uninhabited system is most of the galaxy and
    /// still a system, so it is a faint point rather than nothing.
    pub faint: f32,
    /// What a crowd of colonies is held down by, against the light their
    /// marks would carry
    ///
    /// The field's exposure, and the one number that says how loud the
    /// unresolved galaxy is. Seventeen, which is where the field already
    /// stood: it is what the old pair of weights came to — a sixteenth of a
    /// unit against a mark's own light over a mark's own footprint — so the
    /// crowded end of the law is the exposure that was measured and looked
    /// at, and not a fresh guess. At one, the field and the marks at equal
    /// weight, the galaxy is a white sheet with the marks reading as dirt on
    /// it.
    ///
    /// It is a *crowding* correction and not a scale, so it is spent in
    /// proportion to the crowding it corrects for and is gone by the time a
    /// cell's systems separate on screen. Flat, it left the field seventeen
    /// times dimmer than the very marks that replace it, so a region faded
    /// out as the camera came in and then lit up again when its payload
    /// landed — the field saying there was nothing where there was about to
    /// be a shell.
    pub crowd: f32,
    /// How much harder the crowd nobody lives in is held down, over and
    /// above [`crowd`](Self::crowd)
    ///
    /// Derived from the index rather than chosen: [`settle_gains`] sets it
    /// so the two channels reach the same brightness where the colonies are
    /// dense, which for a populated share `p` and an uninhabited system
    /// worth [`faint`](Self::faint) of a colony is `faint · (1/p - 1)`.
    /// Measured over `.galos_index`, 77,061 inhabited of 3,399,743 — one in
    /// forty-four, so a crowd of the uninhabited is pressed three and a half
    /// times harder than a crowd of colonies.
    ///
    /// Without it the map goes grey: there are forty-odd uninhabited systems
    /// for every colony, so laid down at equal weight they bury every shell
    /// colour. It rides on the crowd and not on the system, because one
    /// uninhabited system standing on its own is not a crowd and is drawn at
    /// [`faint`](Self::faint) either way — as its own mark, and as the
    /// field's stand-in for it.
    pub backdrop: f32,
}

impl Default for Gains {
    fn default() -> Gains {
        let faint = 0.08;
        Gains {
            mark: 0.6,
            unaligned: 0.25,
            faint,
            crowd: 17.,
            // Off the share `.galos_index` measures, until `settle_gains`
            // reads the directory actually in front of the map: one system
            // in forty-four is inhabited, so forty-three are not.
            backdrop: faint * 43.,
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
/// [`super::field::SMALLEST`] itself and not a second copy of the figure: it
/// is what one system's light fills out where the map floors a mark, which is
/// what [`splat`] measures a cell's footprint against through [`MARK_AREA`].
///
/// Not the floor a *splat* is laid at; that is [`FINEST`]. The two were one
/// constant, and flooring a splat's whole quad at a 0.75 px radius put its
/// kernel at 0.19 px of sigma — a fifth of a pixel, which is not a
/// distribution but a hard dot, and a lattice of them is the beading the
/// field was reported for.
const SMALLEST: f32 = super::field::SMALLEST;

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
/// two.** [`galos_index::Moments::rms_radius`] is the RMS *distance* of a
/// cell's systems from their centroid, so for an isotropic cloud it is
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
/// this one had to carry it. With [`galos_index::walk::SPLIT_PX`] cutting at
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

/// Where a splat's deposit stops being laid straight and starts rolling
/// off, in linear light
///
/// One, which is white. Everything under it is the field as it was and is
/// untouched: the disc, the arms, the web between them and the whole faint
/// half of the frame sit three to four orders of magnitude below this.
const SHOULDER: f32 = 1.0;

/// The brightest a single splat may peak at, in linear light
///
/// **A shoulder and not a clip.** The deposit is conserved, so a coarse
/// cell holding a hundred thousand systems whose footprint has been floored
/// to one pixel asks for its whole weight in that pixel, and what it asks
/// for runs to five figures. That has to be bounded — bloom's downsample
/// chain turns a value that large into `inf` and then into `NaN`, and a
/// `NaN` in the chain spreads across the target, which blackened the whole
/// frame the one time it happened — and how it is bounded is what the
/// bubble looks like from far off.
///
/// It was a hard `min` at eight, and the bubble blew out: measured
/// over `.index/full`, the middling splat peaks at 3.8 units from thirty
/// thousand light years out, four times white before a second splat is
/// added to it, so the whole core landed past the display's top with no
/// gradation left in it and bloom spread that white over everything near
/// it. Nothing was clipping — the clip was never the problem; laying four
/// units into a framebuffer that shows one was.
///
/// So the top of the scale is a curve. Under [`SHOULDER`] the deposit is
/// laid as it stands, and above it the excess is compressed toward this
/// ceiling along `1 - exp(-x)`, which meets the straight part with the same
/// slope — so there is no knee to see — and never reaches the ceiling at
/// all. Two, so the core keeps a stop of gradation above white instead of
/// being one flat sheet of it, and bloom has a stop to spread into rather
/// than several.
///
/// Struck on the brightest channel and applied to all three, so what comes
/// down is the brightness and not the colour. Per channel it is the red
/// that survives a red-dominated core being pressed while the blue is let
/// through, which is a hue shift with the zoom, and it is also exactly how
/// a clip desaturates everything bright to white.
const CEILING: f32 = 2.0;

/// The deposit with the top of the scale rolled off
///
/// See [`CEILING`]. Identity under [`SHOULDER`], and asymptotic to the
/// ceiling above it.
fn shouldered(peak: Vec3) -> Vec3 {
    let top = peak.max_element();
    if !(top > SHOULDER) {
        return peak;
    }
    let room = CEILING - SHOULDER;
    let rolled = SHOULDER + room * (1. - (-(top - SHOULDER) / room).exp());
    peak * (rolled / top)
}

/// How far past touching a crowd is let alone, as a multiple of `fill`
///
/// Where the roll-off starts. Below it a crowd is a plain density, held
/// down by [`Gains::crowd`] and no more: this is where the galaxy's own
/// structure lives — the web of filaments and voids between the arms, laid
/// by cells whose marks would cover their footprint a few times over — and
/// pressing it is what took that web off the map. Above it the correction
/// goes on growing as the square root of the crowding, which is what keeps
/// the bubble and the galactic core from blowing out to white discs.
///
/// Thirty-two, measured over `.index/full` at the two zooms the complaints
/// came from. Against no roll-off at all, the middling splat is untouched
/// from twenty light years out to two thousand, the galaxy's disc keeps
/// two thirds of what it lays, and the white on the frame halves: 1,190
/// pixels past white with the galaxy seen whole against 2,578, and 1,264
/// against 2,130 from two thousand. Pressing from the touching point
/// instead — where it started — took the disc to a sixth and the web with
/// it.
const PACKED: f32 = 32.0;

/// How a splat is laid: the radius it is drawn at, and the light it peaks at
///
/// **The one deposit law.** `light` is what the systems this stands for
/// would be drawn at as marks, summed; `covered` is the pixels those marks
/// would cover; `spread` is how far the cell says its systems are
/// scattered, as a standard deviation in pixels. The mask integrates to
/// `2 pi sigma^2` at unit peak, so the light divided by that area is what
/// lands on the framebuffer, and the quantity is conserved: a parent's one
/// splat and its children's several integrate to the same total, and the
/// cross-fade between them neither pumps brightness nor loses any.
///
/// What the law settles on top of that is the level, off `fill` — the share
/// of the footprint those marks would cover.
///
/// - **A crowd** (`fill >= 1`) is a density: its marks would cover the
///   footprint over and over. Ten thousand systems in a pixel are a
///   backdrop and not ten thousand marks' worth of light, so a crowd is
///   held down by `crowd`, which is the field's whole exposure and what
///   keeps the galaxy a picture rather than a white sheet.
/// - **A packed crowd** (`fill >= PACKED`) is held harder still, the
///   correction growing as the square root of the crowding from there up.
///   A line of sight through the bubble carries the whole bubble's light,
///   and conserved, that is a white disc however the rest of the frame is
///   exposed: measured over `.galos_index` from ten thousand light years
///   out it summed to 10.1 units over 1,633 pixels past white. What the
///   roll-off must not do is take the galaxy's own web of filaments with
///   it, which is why it starts where it does rather than at the touching
///   point; see [`PACKED`].
/// - **A scatter** (`fill < 1`) is not a crowd, and there is nothing to
///   correct. Its systems have come apart on screen: every one of them is a
///   mark the map draws at full as soon as its payload lands, and the field
///   is only standing in until it does. So the correction is spent in
///   proportion to the crowding it corrects for, and by the time a cell has
///   separated the field is laying down exactly the light its marks will.
///
/// That is the handoff, and it is what the map was getting wrong. Under a
/// flat correction the field stood seventeen times under the very marks
/// that replace it — and, being a density, it dimmed as the square of the
/// zoom while they did not — so a filament faded out as the camera came in
/// and then lit up again when its payload landed, the map saying there was
/// nothing where there was about to be a shell. Measured over
/// `.galos_index`, the middling splat fell nineteenfold between the galaxy
/// seen whole and twenty light years out, to one level off black.
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
pub(crate) fn splat(
    light: Vec3,
    covered: f32,
    spread: f32,
    crowd: f32,
) -> (f32, Vec3) {
    let sigma = spread.max(FINEST);
    let radius = sigma * REACH;
    let area = std::f32::consts::TAU * sigma * sigma;
    // Off the radius actually drawn, so a cell floored to a point is read as
    // the crowd it is rather than as a scatter over an area it was not given.
    let fill = covered / area;
    let pressed = if fill <= 1. {
        fill
    } else if fill <= PACKED {
        1.
    } else {
        (fill / PACKED).sqrt()
    };
    let crowding = 1. + (crowd - 1.) * pressed;
    let peak = light / (crowding * area);
    (radius, shouldered(peak))
}

/// What one system is worth, in linear light
///
/// The light its mark is painted at, and the light the field deposits for it
/// where it is not drawn yet — one figure, which is what makes the two
/// halves of the map one picture. A system nobody lives in is faint, an
/// inhabited one with nothing political on record is held down by
/// [`Gains::unaligned`] so a crowd of them cannot bury a shell, and a system
/// with a reading draws at full.
///
/// What the field does to a *crowd* of them is [`Gains::crowd`]'s, and is
/// spent as the crowd resolves; see [`splat`].
pub(crate) fn mark_light(hue: Hue, peopled: bool, gains: &Gains) -> f32 {
    let share = match (peopled, hue) {
        (false, _) => gains.faint,
        (true, Hue::Grey) => gains.unaligned,
        (true, _) => 1.0,
    };
    share * gains.mark
}

/// Put the field's mesh and its additive material up
fn spawn_glow(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let mesh =
        meshes.add(glow_mesh(Vec::new(), Vec::new(), Vec::new(), Vec::new()));
    // Added and never blended. A splat is light laid into a field, so what a
    // vertex carries is an emission and not an opacity: the alpha is one
    // throughout and the whole of the weight is in the three channels, which
    // is the same shape `super::field`'s realistic glint is painted in.
    // Unlit, because a splat is a light and not a thing lit by one.
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(images.add(gaussian_mask())),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material),
        RenderLayers::layer(FIELD_LAYER),
        // Every vertex is placed by hand each frame; there is no bound to cull
        // against and the whole field is one draw regardless.
        NoFrustumCulling,
        Transform::default(),
        Visibility::Visible,
        GlowMark,
    ));
}

/// Set the backdrop's own suppression from the index's populated share
///
/// The crowd nobody lives in is held down until it weighs the same as the
/// crowd of colonies standing in it, which for a populated share `p` and an
/// uninhabited system worth [`Gains::faint`] of a colony is
/// `faint · (1/p - 1)`. Derived rather than chosen, so a directory of a
/// different composition is exposed for what it holds.
///
/// Only when either side of it moves. A directory with no colonies in it
/// leaves the default standing rather than dividing by nothing.
fn settle_gains(
    index: Res<ResidentIndex>,
    settled: Res<Settled>,
    mut gains: ResMut<Gains>,
) {
    if !index.is_changed() && !settled.is_changed() {
        return;
    }
    let stellar = index
        .0
        .get(galos_index::CellId::ROOT)
        .map_or(0, |cell| cell.aggregate.count());
    let peopled =
        settled.0.get(galos_index::CellId::ROOT).map_or(0, Inhabited::count);
    if stellar > peopled && peopled > 0 {
        let empty = (stellar - peopled) as f64 / peopled as f64;
        gains.backdrop = gains.faint * empty as f32;
    }
}

/// What a cell's political histogram comes to: the light it lays down, summed
/// over its buckets, and how many systems that light is
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
fn composition(
    held: &Inhabited,
    color_by: ColorBy,
    unaligned: f32,
) -> (Vec3, f32) {
    let mut light = Vec3::ZERO;
    let mut weight = 0.0;
    let mut lay = |hue: Hue, count: u32| {
        if count == 0 {
            return;
        }
        let gain = if hue == Hue::Grey { unaligned } else { 1.0 };
        let w = count as f32 * gain;
        light += hue.light() * w;
        weight += w;
    };
    match color_by {
        ColorBy::Allegiance => {
            for (bucket, count) in held.allegiance().iter().enumerate() {
                lay(allegiance_hue(allegiance_at(bucket)), *count);
            }
        }
        ColorBy::Government => {
            for (bucket, count) in held.government().iter().enumerate() {
                lay(government_hue(government_at(bucket)), *count);
            }
        }
        ColorBy::Security => {
            for (bucket, count) in held.security().iter().enumerate() {
                lay(security_hue(security_at(bucket)), *count);
            }
        }
    }
    (light, weight)
}

/// Rebuild the field from the cells the walk said to splat
///
/// Every frame, off a plan re-walked only when the eye moves: what a splat is
/// drawn *from* is a pure function of position, and where it is drawn *to*
/// turns with the camera. The same split [`super::field::build_field`] makes.
#[expect(
    clippy::too_many_arguments,
    reason = "the plan, the two aggregations it reads, the palette, the gains \
              and the mesh it writes"
)]
fn build_glow(
    camera: Query<(&OrbitCamera, &Camera)>,
    planned: Res<Planned>,
    drawn: Res<crate::systems::aggregate::Drawn>,
    index: Res<ResidentIndex>,
    settled: Res<Settled>,
    color_by: Res<ColorBy>,
    gains: Res<Gains>,
    exposure: Res<FieldExposure>,
    view: Res<View>,
    spyglass: Res<crate::systems::Spyglass>,
    mut laid: ResMut<Laid>,
    mut glow: Query<&mut Mesh3d, With<GlowMark>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Ok(mut mesh3d) = glow.single_mut() else { return };
    // The dial, as a linear gain on everything the field lays. Read once:
    // it says nothing about where a splat goes, only how bright it lands.
    let opened = exposure.factor();
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
        let cot_half_fov = camera.clip_from_view().y_axis.y;
        let half = viewport * 0.5;

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
            .get(galos_index::CellId::ROOT)
            .map_or(0, |cell| cell.aggregate.count())
            as f32;
        if total <= 0. {
            break 'lay;
        }

        // To bound the view is to clear away what the reach does not hold, and
        // the field is part of the view. The same predicate the marks are held
        // to ([`Spyglass::reaches`], which `super::visibility` asks of every
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
        // splat is also told how much room the reach leaves it, and its
        // quad is cut to that: the profile inside the bubble is untouched
        // and the light outside is not drawn, which is what clearing the
        // view means.
        let in_reach =
            |at: [f64; 3]| spyglass.reaches(orbit.center(), DVec3::from(at));
        let room = |at: [f64; 3]| {
            spyglass.clear.then(|| {
                f64::from(spyglass.radius)
                    - orbit.center().distance(DVec3::from(at))
            })
        };

        for splat in &planned.0.splats {
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
            // Neutral, and held down as a crowd alone. Painting it in the
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
            let peopled = settled.0.get(splat.id).map_or(0, Inhabited::count);
            let empty = count.saturating_sub(peopled).saturating_sub(
                taken.count.saturating_sub(taken.inhabited.count()),
            );
            // The finest either channel may claim of this cell, in light
            // years, and the same figure for both ([`COVERAGE`]).
            let finest = cell.id.edge_ly() * COVERAGE;
            let mass = cell.aggregate.mass().remove(taken.mass);
            if empty > 0
                && let Some(at) = mass.centroid()
                && in_reach(at)
            {
                let systems = empty as f32 * carried;
                let light = Vec3::splat(
                    systems * gains.faint * gains.mark * MARK_AREA * opened,
                );
                if let Some(lit) = quads.deposit(
                    orbit,
                    cot_half_fov,
                    viewport,
                    half,
                    at,
                    (mass.rms_radius() * FLATTENED).max(finest),
                    light,
                    systems * MARK_AREA,
                    gains.crowd * gains.backdrop,
                    room(at),
                ) {
                    counted.backdrop += 1;
                    counted.took(lit);
                }
            }

            // And the colonies, at their own. Absent where there are none,
            // and never stood in for by the stellar centroid: that is how a
            // colony is drawn where there is not one.
            let colonies = settled.0.get(splat.id).map(|held| {
                if taken.inhabited.count() < held.count() {
                    held.remove(taken.inhabited)
                } else {
                    // Every colony under the cell is on the map as itself.
                    Inhabited::ZERO
                }
            });
            if let Some(held) = colonies
                && let Some(at) = held.centroid()
                && in_reach(at)
            {
                let (mix, _) = composition(&held, *color_by, gains.unaligned);
                let systems = held.count() as f32 * carried;
                if let Some(lit) = quads.deposit(
                    orbit,
                    cot_half_fov,
                    viewport,
                    half,
                    at,
                    (held.spread() * FLATTENED).max(finest),
                    mix * carried * gains.mark * MARK_AREA * opened,
                    systems * MARK_AREA,
                    gains.crowd,
                    room(at),
                ) {
                    counted.colonies += 1;
                    counted.took(lit);
                }
            }
        }
    }

    for radius in &quads.radii {
        counted.thinnest = counted.thinnest.min(*radius);
        counted.widest = counted.widest.max(*radius);
        counted.floored += u32::from(*radius <= FINEST * REACH + 1e-3);
    }
    if !quads.radii.is_empty() {
        quads.radii.sort_unstable_by(f32::total_cmp);
        let at =
            |f: f32| quads.radii[((quads.radii.len() - 1) as f32 * f) as usize];
        counted.median = at(0.5);
        counted.tenth = at(0.1);
        counted.quarter = at(0.25);
    }
    if !quads.peaks.is_empty() {
        quads.peaks.sort_unstable_by(f32::total_cmp);
        counted.faintest = quads.peaks[0];
        counted.typical = quads.peaks[(quads.peaks.len() - 1) / 2];
    }
    laid.set_if_neq(counted);

    mesh3d.0 = meshes.add(glow_mesh(
        quads.positions,
        quads.uvs,
        quads.colors,
        quads.indices,
    ));
}

/// What one quad came to, for [`Laid`]
#[derive(Clone, Copy)]
struct Lit {
    /// The brightest channel it was laid at, in linear light.
    peak: f32,
    /// The linear light it actually lays on the framebuffer, summed over its
    /// three channels — after the crowding and after any clip, so a quad that
    /// asked for more than [`CEILING`] is counted for what it got.
    light: f32,
    /// Whether its systems' marks would cover less than the footprint it was
    /// spread over, which is the resolving end of [`splat`]'s law.
    separated: bool,
}

/// The frame's quads, as the mesh wants them
#[derive(Default)]
struct Quads {
    /// Footprint radii laid this frame, for [`Laid`].
    radii: Vec<f32>,
    /// And the peak each was laid at.
    peaks: Vec<f32>,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Quads {
    /// Lay `light` down as a Gaussian about `at`
    ///
    /// `spread` is how far the cell says its systems are scattered, in light
    /// years; `covered` is the pixels their marks would cover, and `crowd`
    /// what a crowd of them is held down by while they are one. [`splat`] is
    /// where the three meet; this is the projection either side of it, and
    /// the quad.
    ///
    /// [`None`] where nothing was laid: no light to lay, or a centroid the
    /// camera cannot see. The second is bounded by the walk — a cell drawn as
    /// a splat spans a few pixels at most — so what a centroid off the frame
    /// costs is a few pixels at the very edge.
    #[allow(clippy::too_many_arguments)]
    fn deposit(
        &mut self,
        orbit: &OrbitCamera,
        cot_half_fov: f32,
        viewport: Vec2,
        half: Vec2,
        at: [f64; 3],
        spread: f64,
        light: Vec3,
        covered: f32,
        crowd: f32,
        // How far the reach leaves this splat to spread, light years, or
        // `None` where the spyglass is not clearing and it may spread as
        // far as it likes. See `build_glow`.
        room: Option<f64>,
    ) -> Option<Lit> {
        if light.max_element() <= 0.0 {
            return None;
        }
        let position = DVec3::from(at);
        let screen = screen_position(orbit, cot_half_fov, viewport, position)?;
        let away = crate::space::metres(orbit.eye_from(position)).length();
        let per_pixel =
            world_per_pixel(cot_half_fov, viewport.y, (away as f32).max(1.));
        // The spread comes in as a per-axis deviation in light years, the
        // caller having flattened its cell's own RMS radius ([`FLATTENED`])
        // and floored it on the cell. The pixel scale is in metres, which is
        // the one conversion the map makes and the only place a light year is
        // spoken to the grid.
        let spread_px =
            (spread * crate::space::LIGHT_YEAR / per_pixel as f64) as f32;
        if !spread_px.is_finite() {
            return None;
        }

        let (radius, peak) = splat(light, covered, spread_px, crowd);
        if !radius.is_finite() || !peak.is_finite() {
            return None;
        }
        let color = [peak.x, peak.y, peak.z, 1.];

        // Cut to the room the reach leaves, the profile inside it standing
        // as it is: the mask is sampled over the same `radius` and the quad
        // simply stops early, so what is lost is the tail that would have
        // been drawn where the view has been cleared.
        let drawn = match room {
            Some(room) if room <= 0.0 => return None,
            Some(room) => {
                let room_px =
                    (room * crate::space::LIGHT_YEAR / per_pixel as f64) as f32;
                radius.min(room_px)
            }
            None => radius,
        };
        // Where the mask is sampled to, so a cut quad shows the middle of
        // the profile rather than the whole of it squeezed.
        let uv = 0.5 * drawn / radius;

        let cx = screen.x - half.x;
        let cy = half.y - screen.y;
        let base = self.positions.len() as u32;
        for (dx, dy, u, v) in [
            (-drawn, -drawn, 0.5 - uv, 0.5 + uv),
            (drawn, -drawn, 0.5 + uv, 0.5 + uv),
            (drawn, drawn, 0.5 + uv, 0.5 - uv),
            (-drawn, drawn, 0.5 - uv, 0.5 - uv),
        ] {
            // A unit further out than a mark, so the symbols composite over
            // the field rather than the field over them.
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
        // The radius as *drawn*, so the diagnostics read the footprint the
        // frame carries and not the one the profile was worked out over.
        self.radii.push(drawn);
        self.peaks.push(peak.max_element());
        let sigma = radius / REACH;
        Some(Lit {
            peak: peak.max_element(),
            light: peak.element_sum() * std::f32::consts::TAU * sigma * sigma,
            separated: covered < std::f32::consts::TAU * spread_px * spread_px,
        })
    }
}

/// Build the field's mesh from the frame's quads, as a fresh asset each frame
///
/// The same reasoning as [`super::field`]'s: a mesh whose size moves trips
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

    fn empire() -> Inhabited {
        Inhabited::of_system([0.0; 3], Some(Allegiance::Empire), None, None)
    }

    fn unreported() -> Inhabited {
        Inhabited::of_system([0.0; 3], None, None, None)
    }

    /// A cell of one Empire system lays down the Empire's colour at full
    /// weight, which is what makes a shell read as a shell.
    #[test]
    fn a_colony_lays_down_its_own_colour() {
        let (light, weight) = composition(&empire(), ColorBy::Allegiance, 0.25);
        assert_eq!(weight, 1.0);
        assert!((light - Hue::Cyan.light()).length() < 1e-6);
    }

    /// The whole point of the gain: a colony nobody has reported an allegiance
    /// for weighs less than one somebody has, so a crowd of them cannot bury a
    /// shell.
    #[test]
    fn an_unreported_colony_weighs_less() {
        let (_, aligned) = composition(&empire(), ColorBy::Allegiance, 0.25);
        let (_, grey) = composition(&unreported(), ColorBy::Allegiance, 0.25);
        assert!(grey < aligned, "grey weighed {grey}, aligned {aligned}");
        assert!(grey > 0.0, "an unreported colony is still a colony");
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
        let (light, _) = composition(&cell, ColorBy::Allegiance, 0.25);
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

    /// Every kind of system is worth a level the eye can find, and they keep
    /// the order the gains mean.
    ///
    /// The regression this guards is the whole galaxy going black at the
    /// default zoom. Grey was painted as a dark grey *and* held down by the
    /// gain that already said an unknown system is dim, so an uninhabited
    /// mark came out at nine ten-thousandths of a unit — three levels off
    /// black on an eight-bit display.
    #[test]
    fn a_mark_is_bright_enough_to_find() {
        let gains = Gains::default();
        let empty = mark_light(Hue::Grey, false, &gains);
        let unaligned = mark_light(Hue::Grey, true, &gains);
        let aligned = mark_light(Hue::Cyan, true, &gains);

        for (what, level) in
            [("empty", empty), ("unaligned", unaligned), ("aligned", aligned)]
        {
            let painted = Hue::Grey.light() * level;
            assert!(
                painted.max_element() > 0.01,
                "an {what} system is painted at {painted}, which is black"
            );
            assert!(level <= 1.0, "an {what} system draws past white");
        }
        assert!(empty < unaligned, "an empty system outshone a colony");
        assert!(
            unaligned < aligned,
            "a colony with nothing on record outshone one with a reading"
        );
    }

    /// A splat of systems that have separated on screen lays down exactly
    /// the light their marks will lay down.
    ///
    /// **This is the handoff.** A cell's light moves from the field to its
    /// own marks as its payload arrives, and the two halves have to meet at
    /// the crossing or the map lies twice: a filament fades out as the
    /// camera comes in, and then lights up again when its systems land.
    ///
    /// The light and not the peak. A wash over the cell and a scatter of
    /// marks inside it cannot have the same peak and the same total, and
    /// which of the two the field keeps is the whole argument in [`splat`]:
    /// it keeps the total, because the cell is all it knows about where the
    /// systems are.
    #[test]
    fn a_resolved_splat_meets_the_marks_it_stands_for() {
        let gains = Gains::default();
        let level = mark_light(Hue::Cyan, true, &gains);
        // A hundred colonies scattered over a cell three hundred pixels
        // across: their marks would cover a five-hundredth of it, which is
        // the far side of resolved.
        let systems = 100.;
        let covered = systems * MARK_AREA;
        let marks = systems * level * MARK_AREA;
        let (radius, peak) =
            splat(Hue::Cyan.light() * marks, covered, 300., gains.crowd);

        assert_eq!(radius, 300. * REACH, "the splat left its cell's own size");
        let sigma = radius / REACH;
        let laid = peak.max_element() * std::f32::consts::TAU * sigma * sigma;
        assert!(
            (laid - marks).abs() < marks * 0.01,
            "a resolved splat laid {laid} where its marks lay {marks}"
        );
    }

    /// A crowd is laid as a density, thinning as the footprint it is spread
    /// over grows — until it is packed, where the correction takes over.
    ///
    /// The other end of the same law, and the reason there is a `crowd` at
    /// all. A hundred thousand systems inside a pixel are a backdrop and
    /// not a hundred thousand marks' worth of light.
    ///
    /// Both halves are load-bearing and they were tuned against each other.
    /// Ordinary crowds — the galaxy's own arms and the web of filaments
    /// between them — are a plain density and thin with their footprint,
    /// and pressing *them* is what took the web off the map; a packed one
    /// is pressed, which is what keeps a line of sight through the bubble
    /// off the top of the scale.
    #[test]
    fn a_crowd_is_laid_as_a_density() {
        let gains = Gains::default();
        let level = mark_light(Hue::Cyan, true, &gains);
        let systems = 100_000.;
        let covered = systems * MARK_AREA;
        let light = Hue::Cyan.light() * systems * level * MARK_AREA;
        // A spread that puts the crowd exactly at the top of the flat band,
        // so four times wider is sixteen times the area and still this side
        // of the touching point, and four times tighter is past [`PACKED`].
        let wide = (covered / (std::f32::consts::TAU * PACKED)).sqrt();
        // Read an eighth of the light down. The deposit is linear in the
        // light and neither law turns on it, and an eighth is what keeps
        // every reading below [`SHOULDER`]: the top of the scale is a
        // second curve over these two, and a packed crowd at full light is
        // up against it.
        let dim = light * 0.125;

        let (tight, close) = splat(dim, covered, wide, gains.crowd);
        let (broad, far) = splat(dim, covered, wide * 4., gains.crowd);
        assert_eq!(tight, wide * REACH, "a crowded splat covers its cell");
        assert_eq!(broad, wide * 4. * REACH);
        // Four times the spread is sixteen times the area, and the same
        // light over it: a crowd this side of the roll-off is a density and
        // nothing else.
        let thinning = close.max_element() / far.max_element();
        assert!(
            (thinning - 16.).abs() < 0.1,
            "a crowd did not thin with its footprint: {} to {} is {thinning}",
            close.max_element(),
            far.max_element()
        );

        // And past the roll-off it stops keeping up with its own density:
        // sixteen times packed together is four times the light, not
        // sixteen.
        let (_, packed) = splat(dim, covered, wide / 4., gains.crowd);
        let steepness = packed.max_element() / close.max_element();
        assert!(
            (steepness - 4.).abs() < 0.2,
            "a packed crowd was not held down: {} against {} is {steepness}",
            packed.max_element(),
            close.max_element()
        );

        // At the light it is really laid at, a crowd spread over its own
        // cell is still worth less than one system's mark.
        let (_, spread) = splat(light, covered, wide * 4., gains.crowd);
        assert!(
            spread.max_element() < level,
            "a crowd of systems was laid brighter than one mark: {}",
            spread.max_element()
        );
    }

    /// A splat always covers its cell, and never lays down more light than
    /// the systems it stands for would as marks.
    ///
    /// The two bounds that keep the field honest at every density and every
    /// zoom. It is a wash over the cell — never a tighter thing the eye
    /// would read as an object — and the crowding correction only ever takes
    /// light away, so the field cannot claim more systems than are there.
    #[test]
    fn a_splat_lays_no_more_light_than_its_marks_would() {
        let gains = Gains::default();
        let level = mark_light(Hue::Grey, false, &gains);
        for spread in [0.5f32, 5., 50., 500., 5_000.] {
            for systems in [1f32, 10., 1_000., 100_000.] {
                let covered = systems * MARK_AREA;
                let marks = systems * level * MARK_AREA;
                let (radius, peak) = splat(
                    Vec3::splat(marks),
                    covered,
                    spread,
                    gains.crowd * gains.backdrop,
                );
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
                    laid <= marks * 1.001,
                    "{systems} systems over {spread} px laid {laid} where \
                     their marks lay {marks}"
                );
            }
        }
    }

    /// The composition is a sum, so it composes the way the aggregate does: a
    /// cell's mix is its parts' mixes added, with no normalisation in between
    /// to lose.
    #[test]
    fn composition_is_additive() {
        let a = empire();
        let b = unreported();
        let (whole, _) = composition(&a.merge(b), ColorBy::Allegiance, 0.25);
        let (left, _) = composition(&a, ColorBy::Allegiance, 0.25);
        let (right, _) = composition(&b, ColorBy::Allegiance, 0.25);
        assert!((whole - (left + right)).length() < 1e-6);
    }

    /// An empty cell deposits nothing at all rather than a black quad, and has
    /// no place to deposit it at.
    #[test]
    fn nobody_home_deposits_nothing() {
        let (light, weight) =
            composition(&Inhabited::ZERO, ColorBy::Allegiance, 0.25);
        assert_eq!(weight, 0.0);
        assert_eq!(light, Vec3::ZERO);
        assert_eq!(Inhabited::ZERO.centroid(), None);
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
/// over one and reports the numbers, the way [`super::flight`] does for the
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
    use crate::camera::OrbitCamera;
    use crate::systems::aggregate::Planned;
    use crate::{Populated, ResidentIndex, Settled};
    use galos_index::{FsSource, Inhabitance, Source};
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
    #[derive(Clone, Copy)]
    struct Set {
        /// The spyglass radius in light years, or [`None`] for not clearing.
        reach: Option<f32>,
        /// Whether every splatted cell is taken as drawn in full.
        accounted: bool,
    }

    impl Set {
        /// The far case the exposure is judged on: no boundary, nothing drawn.
        fn open() -> Set {
            Set { reach: None, accounted: false }
        }
    }

    fn laid_at(dir: &PathBuf, away: f64, set: Set) -> (Laid, usize) {
        let source = FsSource::new(dir);
        let (index, populated) = pollster::block_on(async {
            (
                source.index().await.expect("the index should read"),
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
        app.init_resource::<crate::systems::aggregate::Drawn>();
        app.insert_resource(crate::systems::Spyglass {
            radius: set.reach.unwrap_or(0.),
            clear: set.reach.is_some(),
            lock_camera: false,
            follow_camera: true,
        });
        app.insert_resource(View::Map);
        app.insert_resource(ColorBy::Allegiance);
        app.init_resource::<Gains>();
        app.init_resource::<FieldExposure>();
        app.init_resource::<Laid>();
        app.insert_resource(Planned(galos_index::Needed {
            mode: galos_index::Mode::Shell,
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
        app.world_mut().spawn((Mesh3d(mesh), GlowMark));
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
            crate::systems::tests::seeing(),
        ));
        let world = app.world_mut();
        let plan = world.register_system(crate::systems::aggregate::plan);
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
                world.resource_mut::<crate::systems::aggregate::Drawn>();
            for id in splats {
                let Some(cell) = index.get(id) else { continue };
                drawn.0.insert(
                    id,
                    crate::systems::aggregate::Accounted {
                        count: cell.aggregate.count(),
                        mass: cell.aggregate.mass(),
                        inhabited: settled.get(id).copied().unwrap_or_default(),
                    },
                );
            }
        }
        world.run_system(build).expect("the field builds");
        let planned = world.resource::<Planned>().0.splats.len();
        (*world.resource::<Laid>(), planned)
    }

    /// The field is exposed at every zoom: it lays both channels down, and
    /// the splat the frame is *made* of lands in the range a luminance field
    /// is read in rather than vanishing or saturating.
    ///
    /// Read on the median peak and not the brightest one. The brightest
    /// splat is a cell in the galaxy's core and is bright from anywhere, so
    /// a field that has faded to nothing everywhere else passes on it —
    /// which is exactly what happened. Measured over `.galos_index` under a
    /// flat crowding correction, the middling splat fell from 0.0032 with
    /// the galaxy seen whole to 0.00016 at twenty light years out, a
    /// nineteenfold fade to one level off black on an eight-bit display,
    /// and the faintest splat underflowed to zero — while the brightest sat
    /// at a third of a unit throughout and said nothing was wrong. Spending
    /// the correction as a cell resolves ([`splat`]) lays the same four
    /// zooms at 0.00078, 0.00078, 0.00080 and 0.0040.
    ///
    /// Both ends of the exposure have been wrong and neither showed up as an
    /// error. Spending `blend * count` rather than `blend * total` took the
    /// peak to four thousandths and the field was invisible; spending it
    /// without a ceiling took the peak past what `Rgba16Float` holds, and the
    /// `inf` became a `NaN` in the bloom chain that blackened the whole frame
    /// — the chrome and the grid with it.
    #[test]
    fn the_field_is_exposed() {
        let Some(dir) = measured() else { return };
        let mut middling: Vec<f32> = Vec::new();
        for away in [20., 200., 2_000., 30_000.] {
            let (laid, splats) = laid_at(&dir, away, Set::open());
            middling.push(laid.typical);
            println!(
                "{away:>8} ly out: {splats:>5} splats, {:>5} colonies, \
                 {:>5} backdrop, peak {:.5}|{:.5}|{:.3}, {:>5} clipped, \
                 radius {:.2}|{:.2}|{:.2}|{:.2}|{:.2} px, {} floored, \
                 {} separated",
                laid.colonies,
                laid.backdrop,
                laid.faintest,
                laid.typical,
                laid.peak,
                laid.clipped,
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
            assert!(
                laid.peak <= CEILING,
                "the field ran past the ceiling at {away} ly: peak {}",
                laid.peak
            );
            // A field whose every splat clips is a white sheet. Measured: one
            // quad of some three and a half thousand clips from inside the
            // bubble, and eighty of nearly four thousand with the galaxy seen
            // whole — the dense core, which is the part of a luminance field
            // that is meant to.
            let laid_quads = laid.colonies + laid.backdrop;
            assert!(
                laid.clipped * 20 < laid_quads,
                "the field is over-exposed at {away} ly: {} of {laid_quads} \
                 quads clipped",
                laid.clipped
            );
            // The field resolves to the frame and not to the split. A
            // splat's kernel is its cell's own spread ([`FLATTENED`]),
            // floored on half the cell ([`COVERAGE`]) and then on half a
            // pixel ([`FINEST`]) — and with [`galos_index::walk::SPLIT_PX`]
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

        // And the fade itself, which is a ratio and not a level: what went
        // wrong was the field thinning as the camera came in, and how
        // bright any one splat is depends on how finely the directory is
        // cut — a 204,466-cell galaxy lays a tenth of what a 4,072-cell one
        // does per quad and the same total. Under a flat correction the
        // middling splat came in at a nineteenth of its galaxy-wide value.
        // Measured now: a sixth over `.galos_index` and a fifth over
        // `.index/full`, the gap between the two being that a coarse
        // directory's galaxy-zoom cells are packed enough to sit under the
        // roll-off.
        let closest = middling[0];
        let widest = middling[middling.len() - 1];
        assert!(
            closest * 8. > widest,
            "the field faded as it was resolved: {closest} against {widest} \
             with the galaxy seen whole"
        );
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
            laid_at(&dir, 2_000., Set { reach: None, accounted: true });

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
            laid_at(&dir, 30_000., Set { reach: Some(200.), accounted: false });

        println!(
            "{splats} splats: {} + {} quads unbounded, {} + {} inside 200 ly \
             (peak {:.2}, {} clipped of {})",
            open.colonies,
            open.backdrop,
            held.colonies,
            held.backdrop,
            held.peak,
            held.clipped,
            held.colonies + held.backdrop,
        );
        assert!(
            held.backdrop < open.backdrop / 10,
            "the field drew past the reach: {} of {} quads",
            held.backdrop,
            open.backdrop
        );
        // And no quad reaches out of the bubble. A splat's own footprint is
        // a cell wide and the widest of them runs to two thousand pixels
        // unbounded, which is a wash over the whole frame laid from a
        // centre well inside the reach; cut to the room the reach leaves,
        // the field ends where the marks do.
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
            use galos_index::Source as _;
            (
                source.index().await.expect("the index should read"),
                source.populated().await.unwrap_or_default(),
            )
        });
        let settled = Inhabitance::of(&index, populated.iter());
        let root = galos_index::CellId::ROOT;
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
