//! A ruled plane to read the map's scale off
//!
//! A sky of points carries no scale of its own. Stars ten light years apart
//! and stars ten thousand light years apart are the same picture, and nothing
//! on screen says which one is being looked at or where in the galaxy it
//! stands. What is missing is a ruler.
//!
//! So a plane is ruled into cells and laid through what the camera is looking
//! at. Its lines carry their own numbers, painted over and over along them so
//! that one can be read wherever it is being looked at, and the place the
//! camera is looking at is said at the middle of the view. A line is dropped
//! to the plane from whatever is picked out, which is the one thing a plane
//! cannot say by being ruled. The cells and the numbers follow the zoom: out
//! among the systems they are light years, and once the camera has descended
//! into a system they are light seconds.
//!
//! # What is here and what is not
//!
//! `crate::ruled` draws the plane itself: its lines, and the numbers
//! painted over and over along them, the whole of it in one shader pass. It
//! also works out how wide a cell is for a view of a given width, how far
//! apart to put the numbers, and what each of them is called. None of that
//! names a length.
//!
//! Everything standing over the plane is painted here, by `draw_readouts`:
//! the crosses that mark a place worth locating, the lines dropped to the
//! plane, and the three numbers about each of those places. Flat on the
//! screen, projected on the processor in `f64`, because a text mesh out at a
//! system's galaxy coordinate jitters where a projected point holds steady —
//! see `draw_readouts` below and `docs/night-sky.md`.
//!
//! And here too are the questions about a galaxy rather than about a ruler:
//! which unit a space is measured in, where the ruler changes hands as the
//! camera descends into a system, how loudly the whole of it is drawn, and
//! what is worth locating.
//!
//! # Two planes, and what stands between them
//!
//! One hangs in the galaxy's grid and one inside whatever system the camera
//! has descended into, because how finely a plane may be ruled follows the
//! grid it hangs in — see `ruled::finest`. The galaxy's cells are `2^53`
//! metres and bottom out around a light second; a system's are a metre and
//! bottom out well below anything worth drawing.
//!
//! The two are never on screen together. A light year is `3.15576e7` light
//! seconds, which is no power of ten, so the two ladders share no cell size at
//! any zoom: ruled at once they beat against each other. So the one is spent
//! before the other begins, with a moment of unruled sky between them. That
//! moment is the honest reading. At that zoom neither unit rules truthfully.
//!
//! They change hands as the mark standing for the held system goes out, on the
//! very figure that fade is drawn from. So the sky and the ruler under it say
//! the same thing at the same moment: the shell gives way to the system it
//! stood for, and light years give way to light seconds. It falls at a
//! different distance for every system, the exchange running from eighty of
//! its own reaches out to twenty, and asks the same question of each.
//!
use crate::camera::OrbitCamera;
use crate::ruled::{
    self, Decade, DistanceUnit, EDGE_ON, FIGURES_ACROSS, Family, INK, Located,
    NUMBERED, Number, Numbered, Painted, Plane, Reading, RuledPlugin, drawn_at,
    faded, numbering, off_plane, ruling, snapped_to, ticked, told,
};
use crate::schedule::MapSet;
use crate::space::{self, Map};
use crate::systems::System;
use crate::systems::bodies::spawn::{Body, HeldSystem, Places, Strength};
use crate::systems::labels::{annotations_layer, color32, screen_offset};
use crate::systems::selection::Selected;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use big_space::prelude::*;

pub fn plugin(app: &mut App) {
    // Cut from the face egui draws the bar in, which is the same face every
    // name on the map is drawn in: a number on the plane and a number in the
    // bar are then the one typeface.
    app.add_plugins(RuledPlugin {
        face: ruled::Face { bytes: epaint_default_fonts::HACK_REGULAR },
    });
    app.insert_resource(ShowGrid(true));
    app.insert_resource(ShowMiddle(true));
    app.insert_resource(ShowPicked(true));
    app.init_resource::<Bright>();
    app.init_resource::<RulerUnit>();
    app.init_resource::<RuledSystem>();
    app.init_resource::<Handover>();
    // After the map itself, which is what the galaxy's planes hang from. The
    // resource naming it is inserted through a command, so it is not there to
    // be read until the schedule that queued it has ended.
    app.add_systems(PostStartup, spawn_planes);
    // In `Present`, which runs after `Camera` has settled where the camera is
    // standing. Everything here is worked out from that and nothing else.
    //
    // Nothing else in `Update` orders against it. The one thing that reads
    // the reading written here is [`draw_readouts`], and that runs in
    // `EguiPrimaryContextPass`, a schedule later in the same frame.
    app.add_systems(Update, (rule, mark_out).in_set(MapSet::Present));
    // Flat on the screen, in egui's own pass, under the chrome and over the
    // map — the same layer and the same reason the names and rings are drawn
    // there.
    //
    // First of the four painters writing into that one shared layer
    // ([`crate::systems::labels::annotations_layer`]), where paint order is
    // stacking order and so run order is what decides which mark ends up on
    // top. The readouts are the plane's own ruling, the substrate the map is
    // read against rather than anything picked out on it, so they go under
    // the marks and under the names: a dropline or a readout row painted
    // over a name's ground would cross the words the map is read by. Left to
    // the executor the stacking would be whichever painter it happened to
    // reach first, which is why the whole of it is spelled out here.
    //
    // Before the lettering as well, which is what registers the annotation
    // layer beneath the panes: the lettering leads the chrome (see
    // [`crate::ui`]), so before it is before all of that.
    app.add_systems(
        EguiPrimaryContextPass,
        draw_readouts
            .before(crate::systems::pointing::ring)
            .before(crate::systems::selection::ring)
            .before(crate::systems::labels::draw_names)
            .before(crate::ui::lettering),
    );
}

/// Whether the ruled plane is drawn
#[derive(Resource)]
pub(crate) struct ShowGrid(pub(crate) bool);

/// Whether the place the camera is looking at is marked at the middle of the
/// view
///
/// The plane's own numbers say where its lines are; this says where the view is,
/// which is the one of the three a line cannot carry.
#[derive(Resource)]
pub(crate) struct ShowMiddle(pub(crate) bool);

/// How strongly the ruling is drawn, against what the map settles on for it
///
/// One for the lines, which is what the map was tuned at. Under one for a
/// ruling that stays out of the way of a busy sky, and over one for one that
/// has to be read off a bright field or a screen in daylight.
///
/// Everything the ruling draws follows it together: the lines, the numbers
/// painted along them, the lines dropped to the plane and the numbers written
/// over it. They are one thing seen at once, and a ruler whose lines dimmed
/// while its numbers did not would read as two.
#[derive(Resource)]
pub(crate) struct Bright(pub(crate) f32);

impl Default for Bright {
    /// Half of the brightest it goes
    ///
    /// The ruling crosses the whole map and is meant to be glanced at rather
    /// than looked at, so it opens well short of its own ceiling and leaves the
    /// top of the range for a sky it has to be read off.
    fn default() -> Self {
        Bright(0.5)
    }
}

/// And whether the places of the things picked out are
///
/// Each marked where its line meets the plane, with a line standing off it
/// saying how far off it is. A separate switch from the middle's: the middle is
/// one mark wherever the camera goes, and this is one for everything selected,
/// which is as busy as the selection is.
#[derive(Resource)]
pub(crate) struct ShowPicked(pub(crate) bool);

/// How much coarser than a grid can place it a plane is actually ruled
///
/// [`ruled::finest`] is where the lines would begin to swim outright. A ruling
/// wants to be steady rather than barely standing, so the ladder stops some
/// way above it.
const STEADY: f64 = 1e3;

/// The finest a system's planes are ruled, in light seconds
///
/// A matter of taste rather than of arithmetic. A system's grid has cells of a
/// metre and could carry a ruling millions of times finer than this. But a
/// light second is three hundred thousand kilometres, a thousandth of one is
/// already smaller than the body being looked at, and past there the numbers
/// have stopped being light seconds in any useful sense.
const FINEST_SYSTEM_CELL: f64 = 1e-3;

/// How far the plane is drawn before it has faded out, as a multiple of how
/// far back the camera is standing
///
/// The plane is unbounded and the far end of it is always edge on, where any
/// ruling turns to moire. Fading it out is what the shader offers instead, and
/// this is the distance handed to it. Past the far side of the view, so that
/// what fades is the horizon rather than anything being looked at.
const FADE_BEYOND: f64 = 6.;

/// What the ruling is drawn in
///
/// Cold and unsaturated, so that it reads as chrome laid over the sky rather
/// than as more of the sky. Every star on the map is warmer than this.
const LINE: Color = Color::srgb(0.55, 0.66, 0.82);

/// How tall a readout's numbers are drawn, in logical pixels
///
/// A touch under the names ([`crate::systems::labels::NAME_HEIGHT`]) and the
/// chrome's smallest lettering, so a coordinate sits nearer the size of the
/// numbers painted along the grid it is read against rather than standing over
/// them.
const READS: f32 = 10.5;

/// How far below a mark its three numbers hang, in pixels
///
/// Below rather than above, so the number is read against clear screen rather
/// than into the mark it is about.
const LIFT: f32 = 32.;

/// How far beside a dropped line its offset stands, in pixels
const ASIDE: f32 = 6.;

/// How long each arm of a cross marking a place is, in pixels
const CROSS: f32 = 6.;

/// How wide a dropped line is painted, in logical pixels
///
/// Bolder than a hairline so it reads as one of the plane's ruling lines rather
/// than a scratch. Painted flat, so it holds that weight at every zoom.
const LINE_STROKE: f32 = 2.;

/// And how wide a cross marking a place is
const MARK_STROKE: f32 = 1.5;

/// How near a dropped line's numbers may come to the middle's before they give
/// way, in pixels
///
/// A row is three numbers with a power, a unit and two commas — some forty
/// characters of a [`READS`] tall monospaced face centred on its place, running
/// about ninety five either side and half a line above and below. Two rows
/// nearer than that are written through each other and neither can be read, and
/// of the pair the middle is the one kept.
const CROWDS: Vec2 = Vec2::new(96., 10.);

/// Everything one of the map's planes is written through
///
/// Where it hangs, how it is ruled, what its crossings are called, what can be
/// read off it and whether it is drawn at all.
type PlaneParts = (
    Entity,
    &'static Ruler,
    &'static mut Transform,
    &'static mut CellCoord,
    &'static mut Plane,
    &'static mut Numbered,
    &'static mut Reading,
    &'static mut Visibility,
);

/// One of the two ruled planes
///
/// One per space, each carrying both cells of its own decade pair. The pair is
/// what makes cells subdivide rather than step as the camera comes in, and
/// written onto the one plane the two share an origin and an altitude by
/// construction rather than by two placements agreeing.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Ruler {
    /// Ruled in a system's own grid rather than in the galaxy's
    pub(crate) inside: bool,
}

/// Which system the planes ruled in light seconds are hanging in
///
/// They are children of the system they rule, which is the only way to be
/// placed by its grid, and that system is despawned whenever the camera leaves
/// it or the spyglass sweeps it away — taking them with it. So which system
/// they were made for is remembered here, and they are made afresh whenever
/// the answer changes.
#[derive(Resource, Default)]
struct RuledSystem(Option<Entity>);

/// What a plane's numbers are said in, out among the systems
const LIGHT_YEARS: DistanceUnit =
    DistanceUnit { metres: space::LIGHT_YEAR, mark: "Ly" };

/// And once the camera has descended into one
const LIGHT_SECONDS: DistanceUnit =
    DistanceUnit { metres: space::LIGHT_SECOND, mark: "Ls" };

/// The finest cell a plane hanging in `grid` may be ruled in, said in `unit`
///
/// Where the ladder stops. Out among the systems that is arithmetic, the grid
/// running out of places to put a line, [`ruled::finest`]. Inside one it is
/// taste, the grid having room to spare.
fn finest(unit: DistanceUnit, grid: &Grid) -> f64 {
    let placed = ruled::finest(grid) * STEADY / unit.metres;
    if unit == LIGHT_SECONDS { placed.max(FINEST_SYSTEM_CELL) } else { placed }
}

/// The unit a plane's numbers are read in
///
/// Left to the map by default, which turns the ruler over as it descends into
/// a system. Pinned either way from the bar, for reading a system's distances
/// in light years or a neighbourhood's in light seconds.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RulerUnit {
    #[default]
    Automatic,
    LightYears,
    LightSeconds,
}

/// Which unit the numbers come out in, for a space measured in `own`
///
/// One function, so that moving where the ruler turns over is a line to change
/// rather than a rule spread through the module.
///
/// Left to the map, a space is said in its own unit and never in the other
/// one's. Which is not a matter of taste: the cell ladder is decades of
/// whatever the space is said in, so saying it in something else re-founds the
/// ladder, and a light year is 3.156e7 light seconds rather than a power of
/// ten. Turning the unit over under a camera therefore moves every line on the
/// plane by a factor of about three, in one frame, with nothing faded through.
/// The two spaces hand over to each other instead, which [`handover`] already
/// carries through nothing.
///
/// What made a zoom-led turnover tempting was that a decimal of a light year is
/// not a number anybody reads. Written as a figure and a power it is: `5e-5`
/// says as much as `1.6e3` light seconds does, and says it on a ladder that has
/// not moved. Out among the systems it is also the truer of the two, a plane in
/// the galaxy's grid having no way to be ruled to a light second at all —
/// [`ruled::finest`] there is some thousands of them.
///
/// Either may still be pinned from the bar, which moves the lines once, when
/// asked.
fn unit_for(own: DistanceUnit, asked: RulerUnit) -> DistanceUnit {
    match asked {
        RulerUnit::LightYears => LIGHT_YEARS,
        RulerUnit::LightSeconds => LIGHT_SECONDS,
        RulerUnit::Automatic => own,
    }
}

/// How much of the galaxy's ruling is drawn, and how much of a system's, as
/// the map hands one to the other
///
/// Never both. The two share no cell size — a light year is `3.15576e7` light
/// seconds, so nothing in one lattice falls on anything in the other — and two
/// of them on screen together is two rulings that disagree, with two sets of
/// numbers written over each other.
///
/// Never nothing either. What changes hands is the *contrast* rather than
/// which of them is there: over [`CUTOVER`] either side of the middle the
/// ruling that has the sky dims toward [`DIMMEST`], and the other picks up
/// from there. So the swap happens where a ruling is faint enough that
/// swapping it does not read as one, and the sky is never bare.
///
/// Three ways round this were tried against the map. Spending the whole band
/// on a disjoint fade left the sky ruled at a fraction of nothing for several
/// light years of panning out of a wide system — reported as the grid
/// disappearing. Spending a twentieth of it left the two rulings swapping in a
/// blink — reported as no fade at all. Crossfading them complementary put both
/// lattices and both readouts on the sky at once — reported, fairly, as worse
/// than either.
///
/// `standing` is how much of the mark standing for the system is left: whole
/// for the galaxy's ruling, nothing for the system's. The middle is where it
/// changes hands, and the tie goes to the galaxy, so there is no `standing` at
/// which both are drawn.
fn handover(standing: f32) -> (f32, f32) {
    let through = ((standing - 0.5).abs() / CUTOVER).clamp(0., 1.);
    let strength = DIMMEST + (1. - DIMMEST) * through;

    if standing >= 0.5 { (strength, 0.) } else { (0., strength) }
}

/// How much of the handover either side of the middle is spent dimming
///
/// A tenth, so the ruling comes down to [`DIMMEST`] over a fifth of the band
/// and stands whole over the rest of the one it is the right ruling for. The
/// band is the mark's, which is an angle: a fifth of it is about a light year
/// and three quarters of panning for the widest system on record, and some
/// eighty astronomical units for one with nothing on record. Both are about a
/// second of moving at the pace the thing is looked at.
const CUTOVER: f32 = 0.1;

/// How faint a ruling is where it changes hands
///
/// The one figure the swap is hidden behind. Low enough that a lattice
/// replaced by another lattice at this strength reads as the ruling changing
/// rather than as the sky flicking over — the lines are a suggestion here, not
/// a grid to read a number off — and high enough that the sky is never bare,
/// which is what a fade to nothing and back looked like.
const DIMMEST: f32 = 0.3;

/// How much of the galaxy's ruling the map wants, from one to nothing
///
/// `standing` is how much of the mark standing for the system is left
/// ([`Strength`]) and `carried` how much of a ruling the galaxy's own ladder
/// can lay at this zoom ([`rulable`]).
///
/// The mark decides it. A mark stands for a system while the system is too
/// small to be drawn as itself and gives way as it is: it is gone exactly
/// where the thing it stood in for has taken over the sky, and that is where
/// the sky becomes the system's to rule. Measured with the diagnostics panel
/// open, the old boundary — a fixed sphere the width of the narrowest
/// system's own shell — left Alpha Centauri reading `mark 0.00 left` beside
/// `ruler 1.00 out`: the mark wholly given way to nineteen drawn bodies, and
/// still galactic coordinates over them.
///
/// Only the system the map is holding ever has a mark that goes out (see
/// `fade` in `systems::bodies::spawn`), so a wide neighbour whose own
/// measure would have its mark nearly gone from four light years off cannot
/// take the ruler while the camera stands somewhere else.
///
/// Held back, while the camera is inside the system, by what the galaxy can
/// actually draw: where its ladder is under its floor there is nothing to hand
/// back to, so the system keeps the sky however whole its mark has become.
///
/// `stood_in` is what bounds that, and it has to. A camera panning outward at
/// close zoom leaves the galaxy's ladder under its floor the whole way, so the
/// clamp held the sky at the system's ruling however far the camera went —
/// numbers in light seconds about a point light years behind it, and no way
/// back short of zooming out. Inside the system its plane is the better of the
/// two whatever the galaxy can manage; outside it, it is not a plane the
/// camera is anywhere near, and the mark alone decides. See [`rule`].
fn wants_ruling(standing: f32, carried: f32, stood_in: bool) -> f32 {
    let wants = standing.clamp(0., 1.);

    if stood_in { wants.min(carried) } else { wants }
}

/// How long the ruler takes to change hands at the least, in seconds
///
/// The mark goes out over a band of distance, so a camera crossing that band
/// at a pace fades at that pace and needs nothing here. What this is for is a
/// camera that does not cross it at all: a search
/// flown to lands the eye inside a system in one frame, and the grid a plane
/// is ruled in comes and goes with the rows, so the share can still be asked
/// to step. Eased, a step becomes a fade of this long however it was arrived
/// at.
///
/// The same half second the mark standing for a system is bounded to
/// ([`crate::systems::bodies::spawn::GOES_OUT_IN`]), so where the two are both
/// moving they move together.
const HANDS_OVER_IN: f32 = 0.5;

/// How much of the way the ruler has changed hands
///
/// Eased toward what [`wants_ruling`] asks by [`rule`], at [`HANDS_OVER_IN`].
/// Whole is the galaxy's ruling, as a map with no system descended into is.
///
/// Read by [`crate::dev`], which says what the ruler is doing.
#[derive(Resource)]
pub(crate) struct Handover(pub(crate) f32);

impl Default for Handover {
    fn default() -> Self {
        Handover(1.)
    }
}

/// The share, a step nearer what is asked of it
///
/// Bounded, so however far the answer has jumped — or gone away with the
/// system it was about, leaving the galaxy's ruling asked for outright — the
/// ruler changes hands at a pace rather than at once.
fn handed_over(standing: f32, asked: f32, step: f32) -> f32 {
    standing + (asked - standing).clamp(-step, step)
}

/// Create the two planes ruled in light years
///
/// Under the map rather than under the galaxy, which is thrown away and
/// replaced whenever the map is cleared. These are chrome and survive that,
/// the same as the camera does and for the same reason.
///
/// The two ruled in light seconds are not made here. They hang inside whatever
/// system the camera has descended into, and there is none at startup — see
/// [`RuledSystem`].
fn spawn_planes(mut commands: Commands, map: Res<Map>) {
    commands.spawn((
        ruled::Ruled,
        Ruler { inside: false },
        // Placed by the map's own grid, which is the galaxy's. What [`rule`]
        // writes here every frame is an altitude; where the ruling is measured
        // from is worked out under the camera by [`ruled`] and the plane
        // itself stands still.
        CellCoord::default(),
        // Nothing is ruled until [`rule`] has looked at the camera.
        Visibility::Hidden,
        ChildOf(map.0),
    ));
}

/// One space's worth of what it takes to rule a plane
///
/// Built for the galaxy, and for the system the camera has descended into if
/// it has descended into one. Both at once through the middle of a descent,
/// which is what carries the ruling from light years to light seconds without
/// either of them appearing out of nowhere.
struct Placement<'a> {
    unit: DistanceUnit,
    /// The grid the planes of this space hang in, which splits a position into
    /// a cell and a remainder
    grid: &'a Grid,
    decade: Decade,
    /// Where the planes sit, in [`Placement::unit`], measured from whatever
    /// the space is measured from
    at: DVec3,
    /// Where the rulers cross, likewise
    ///
    /// A multiple of the tick step rather than of the cell, so that every
    /// number is a whole number of steps out from the middle. The step is
    /// itself a multiple of the fine cell, so the crossing still falls on a
    /// line.
    crossing: DVec3,
    /// How far apart two numbers are, in [`Placement::unit`]
    step: f64,
    /// How much of this space is on screen, in [`Placement::unit`]
    across: f64,
    /// How far its ruling reaches before it has faded out, in metres
    reach: f64,
    /// How much of this space's ruling is drawn, as the descent hands the map
    /// from one space to the other
    share: f32,
}

impl Placement<'_> {
    /// How much of this space is drawn at all
    fn showing(&self) -> f32 {
        self.decade.drawn * self.share
    }

    /// What the numbers over this space say
    fn reading(&self, middle: bool, bright: f32) -> Reading {
        Reading {
            at: self.at,
            step: self.step,
            unit: self.unit,
            strength: self.showing(),
            bright,
            middle,
        }
    }
}

/// What wears a mark, of the two kinds of thing that can
type Marked = (With<Selected>, Or<(With<System>, With<Body>)>);

/// Hand the ruler whatever is worth locating
///
/// Everything picked out, and only while the bar asks for it. The plane runs
/// through what the camera is looking at, so a line dropped from there would
/// have no length; how far off it something else stands is the question a plane
/// cannot answer by being ruled.
///
/// A mark rather than a list, so that whoever picks a thing out says nothing
/// about rulers and the ruler is never handed an entity that has gone.
fn mark_out(
    showing: Res<ShowGrid>,
    picked_out: Res<ShowPicked>,
    // What wants marking and is not marked yet, and what is marked. Only the
    // difference between the two is written: a mark put on every frame is a
    // command apiece every frame, and a component said to have changed when
    // nothing about it has.
    fresh: Query<Entity, (Marked, Without<Located>)>,
    marked: Query<Entity, With<Located>>,
    // And whether something already marked is still worth marking.
    picked: Query<(), Marked>,
    mut commands: Commands,
) {
    let wanted = showing.0 && picked_out.0;
    if wanted {
        for entity in &fresh {
            // `try_` because the evictor may despawn a marked system between
            // this query and the command landing: the mark is then moot, not
            // an error worth a warning per frame.
            commands.entity(entity).try_insert(Located);
        }
    }
    for entity in &marked {
        if !wanted || picked.get(entity).is_err() {
            commands.entity(entity).try_remove::<Located>();
        }
    }
}

/// Paint the numbers standing over the planes, flat on the screen
///
/// The place the camera is looking at, and each thing picked out: the three
/// numbers the plane says about it, a cross where it meets the plane, and — for
/// a thing off the plane — a line dropped to it with how far off it went beside
/// it.
///
/// Painted in screen space, projected on the processor in `f64`, rather than
/// drawn as text meshes out at a system's galaxy coordinate. A mesh there is
/// transformed by an f32 clip transform that resolves only to the grid's cell
/// over `2^24` — some light seconds — so it jitters by many pixels as the
/// floating origin recenters, once the camera is zoomed inside that step and
/// before it has descended onto a system's own metre grid. A single point
/// projected in `f64` from the camera and the thing's true position is steady
/// to well under a pixel at any zoom. The same reason the names, rings and
/// leaders are drawn flat; see [`crate::systems::labels::draw_names`] and
/// `docs/night-sky.md`.
///
/// What it gives up by leaving the scene is being tonemapped with the plane and
/// hidden behind whatever galaxy stands in front of it. A readout is chrome
/// laid over the map, the same as a name, and reads as one.
pub(crate) fn draw_readouts(
    mut contexts: EguiContexts,
    camera: Query<(&OrbitCamera, &Camera)>,
    planes: Query<(&Ruler, &Plane, &Reading)>,
    // Picked-out systems, placed against the galaxy plane by their true `f64`
    // position rather than an f32 grid remainder, which is the whole of the
    // fix. Their `Visibility` is the one [`crate::systems::visibility`] wrote
    // this frame, which is the component
    // [`crate::systems::selection::ring`] asks as well: one question, one
    // answer, and the two agree on the frame a system is put away.
    systems: Query<(&System, &Visibility), (With<Selected>, With<Located>)>,
    // And picked-out bodies, against a system's own plane. Read off the grid
    // holding them, which is near the origin and so already exact.
    //
    // Asked nothing beyond existing, as [`crate::systems::pointing::hits`]
    // asks nothing more. A body exists exactly while its system's contents
    // are on the map, which is the same ground its name and its ring are
    // granted on; being off the frame is answered by the projection giving
    // nothing for what the camera cannot see. Gated on anything the render
    // settles instead, a body picked out on the frame its system's contents
    // are drawn gets its name, its ring and its clickable area but no
    // readout, and the numbers arrive a frame after everything else.
    bodies: Query<Entity, (With<Body>, With<Selected>, With<Located>)>,
    places: Places,
) -> Result {
    let Ok((orbit, camera)) = camera.single() else { return Ok(()) };
    let Some(viewport) = camera.logical_viewport_size() else { return Ok(()) };
    let cot = camera.clip_from_view().y_axis.y;

    let ctx = contexts.ctx_mut()?;
    let painter = ctx.layer_painter(annotations_layer());
    let font = egui::FontId::new(READS, egui::FontFamily::Monospace);
    let hue = Srgba::from(LINE);
    // The plane hangs through what the camera looks at, so its altitude is the
    // middle's own, in absolute light years.
    let plane_y = orbit.center.y;

    let project =
        |place: DVec3| screen_offset(orbit, cot, viewport, place - orbit.eye);
    let seg = |a: Vec2, b: Vec2, width: f32, color: egui::Color32| {
        painter.line_segment(
            [egui::pos2(a.x, a.y), egui::pos2(b.x, b.y)],
            egui::Stroke::new(width, color),
        );
    };
    // A cross laid in the plane along its own axes, so it lies on the grid and
    // foreshortens with it rather than floating flat over the view. Its arms are
    // sized in the world to draw about [`CROSS`] pixels at the depth the place
    // lies at — measured into the view, not along the plane's own normal, or a
    // plane seen face on would size its cross to nothing.
    let cross = |at: DVec3, facing: Quat, color: egui::Color32| {
        let ahead =
            (at - orbit.eye).dot((orbit.rotation * Vec3::NEG_Z).as_dvec3());
        if ahead <= 0. {
            return;
        }
        let per_pixel = 2. * ahead / (cot as f64 * viewport.y as f64);
        let arm = CROSS as f64 * per_pixel;
        for axis in [Vec3::X, Vec3::Z] {
            let along = (facing * axis).as_dvec3() * arm;
            if let (Some(a), Some(b)) =
                (project(at - along), project(at + along))
            {
                seg(a, b, MARK_STROKE, color);
            }
        }
    };
    let row = |at: Vec2, align, said: String, color: egui::Color32| {
        painter.text(egui::pos2(at.x, at.y), align, said, font.clone(), color);
    };

    for (ruler, plane, reading) in &planes {
        if reading.strength <= 0. {
            continue;
        }
        let unit = reading.unit;
        // How far a readout stands before it has faded into the plane's
        // horizon. Read off the plane it is drawn over rather than worked out
        // again from the zoom, so a dropline and the ruling under it fade on
        // the one figure and cannot drift apart. [`Plane::reach`] is in
        // metres, being a distance out through the world; everything here is
        // in light years, so it is spoken into those once.
        let reach = plane.reach / space::LIGHT_YEAR;
        // Where this space is measured from, in absolute light years. The
        // middle is said in the plane's unit out from here, so undoing that on
        // the middle recovers it: nought for the galaxy, the star for a system.
        let from =
            orbit.center - reading.at * (unit.metres / space::LIGHT_YEAR);

        // What a mark or a number `base` strong is drawn in, faded by how far
        // toward the plane's horizon its place lies.
        let inked = |place: DVec3, base: f32| {
            drawn_at(base * faded(place - orbit.eye, reach), reading.bright)
                * reading.strength
        };
        let tint = |place: DVec3, base: f32| {
            color32(hue.with_alpha(inked(place, base).clamp(0., 1.)))
        };
        // A number stands over the lines to be read outright, so it is drawn
        // half again the ink the plane paints its own numbers in.
        let lettered = |place: DVec3, base: f32| {
            color32(hue.with_alpha((inked(place, base) * 1.5).clamp(0., 1.)))
        };

        // The middle of the view: the three numbers of the place looked at,
        // marked and hung below. Kept for the crowding test below either way.
        let middle = reading.middle.then(|| project(orbit.center)).flatten();
        if let Some(at) = middle {
            cross(orbit.center, plane.facing, tint(orbit.center, INK));
            row(
                at + Vec2::new(0., LIFT),
                egui::Align2::CENTER_CENTER,
                format!("{} {}", told(reading.at, reading.step), unit.mark),
                lettered(orbit.center, INK),
            );
        }

        // Everything picked out in this plane's space: a line dropped to the
        // plane, the three numbers under its foot, and how far off it went
        // beside the line. Systems out in the galaxy; bodies inside a system.
        // A system the map has put away is not there to be located, and a line
        // dropped from where it would have stood is a line about nothing; a
        // body is there to be located for as long as it is there at all.
        let located: Vec<DVec3> = if ruler.inside {
            bodies.iter().filter_map(|body| places.of(body)).collect()
        } else {
            systems
                .iter()
                .filter(|(_, shown)| **shown != Visibility::Hidden)
                .map(|(system, _)| system.position())
                .collect()
        };
        for place in located {
            let foot_at = DVec3::new(place.x, plane_y, place.z);
            let (Some(top), Some(foot)) = (project(place), project(foot_at))
            else {
                continue;
            };
            // The line, kept clearly visible as the connector to what is picked
            // out, and a cross where it meets the plane.
            seg(top, foot, LINE_STROKE, tint(foot_at, INK));
            cross(foot_at, plane.facing, tint(foot_at, INK));

            // Its three numbers under the foot, and how far off the plane it
            // stands beside the line — unless they would be written through the
            // middle's, of which the middle is the one kept.
            let below = foot + Vec2::new(0., LIFT);
            let crowds =
                middle.is_some_and(|m| (below - m).abs().cmplt(CROWDS).all());
            if !crowds {
                let at_unit = (place - from) * space::LIGHT_YEAR / unit.metres;
                row(
                    below,
                    egui::Align2::CENTER_CENTER,
                    format!("{} {}", told(at_unit, reading.step), unit.mark),
                    lettered(foot_at, INK),
                );
                if let Some(said) =
                    off_plane(at_unit.y - reading.at.y, reading.step, unit)
                {
                    let mid = (top + foot) / 2. + Vec2::new(ASIDE, 0.);
                    row(
                        mid,
                        egui::Align2::LEFT_CENTER,
                        said,
                        lettered(place, INK),
                    );
                }
            }
        }
    }

    Ok(())
}

/// Work out how to rule one space
///
/// `from` is where the space is measured from in absolute galactic light
/// years, which is the galactic centre for the galaxy and the star for a
/// system. `across` is how much of the sky is on screen, in light years, which
/// is the one figure the whole ruling follows.
fn placed<'a>(
    unit: DistanceUnit,
    grid: &'a Grid,
    from: DVec3,
    across: f64,
    orbit: &OrbitCamera,
    share: f32,
) -> Placement<'a> {
    // Everything from here is in `unit`. The view is measured in light years
    // whatever is being looked at, so it is spoken into the space's own unit
    // once, here, and not thought about again.
    let spoken =
        |place: DVec3| (place - from) * space::LIGHT_YEAR / unit.metres;
    let across = across * space::LIGHT_YEAR / unit.metres;

    let decade = ruling(across, finest(unit, grid));
    let looking = spoken(orbit.center);
    let step = numbering(across);

    // The plane hangs through exactly what the camera is looking at, and its
    // height is said out loud at the crossing.
    //
    // Not snapped. A plane laid on the nearest cell jumps a whole cell every
    // time the view climbs past one, which is a thing to fight rather than to
    // read a height off, and it leaves the number over it a height above a
    // plane that is itself somewhere unsaid. Said outright it needs no cell to
    // stand on, and a camera on the galactic plane reads zero, which is what
    // laying the plane there was for.
    //
    // Sideways is another matter. The crossing carries the numbers and they
    // have to fall on lines, so it is snapped, and every number is a whole
    // number of steps out from a middle that sits where a line does.
    let at = looking;
    let mut crossing = snapped_to(looking, step);
    crossing.y = at.y;

    Placement {
        unit,
        grid,
        decade,
        at,
        crossing,
        across,
        step,
        // In metres, being a distance out through the world rather than a
        // distance across the plane. Past the far side of the view, so that
        // what fades is the horizon rather than what is looked at.
        reach: orbit.radius as f64 * space::LIGHT_YEAR * FADE_BEYOND,
        share,
    }
}

/// How much of a ruling `unit` can lay in `grid` with `across` light years of
/// it on screen, from one to nothing
///
/// A ladder has a floor: a plane cannot be ruled finer than the grid holding
/// it can place it (see [`finest`]), and below that the ruling fades out
/// rather than going on swimming. So the galaxy's own grid, whose cells are
/// light years, has nothing to rule with once the view is a few astronomical
/// units across — the zoom the camera reaches inside a system, where only the
/// system's own metre-fine grid can carry a ruling.
///
/// Which is what bounds the handover: see [`rule`].
fn rulable(unit: DistanceUnit, grid: &Grid, across: f64) -> f32 {
    ruling(across * space::LIGHT_YEAR / unit.metres, finest(unit, grid)).drawn
}

/// Rule the planes, place them under the camera, and say what they are called
///
/// Runs every frame. All of it follows the zoom, the zoom is eased rather than
/// stepped, and so there is no frame on which none of it has moved.
#[allow(clippy::too_many_arguments)]
fn rule(
    showing: Res<ShowGrid>,
    bright: Res<Bright>,
    cameras: Query<(&OrbitCamera, Option<&Projection>)>,
    // The system the map is standing in, if there is one. It is the one
    // carrying a grid of its own, which it does only while its contents are
    // drawn. Its cells are a metre, which is what lets a plane be ruled in
    // light seconds at all, and its `Strength` — how much of the mark
    // standing for it is left — is what the two planes change hands on.
    inside: Query<(Entity, &System, &Grid, &Strength), Without<BigSpace>>,
    // Which of those the map is holding, which is the only one whose mark is
    // still being stepped: [`crate::systems::bodies::spawn::fade`] leaves
    // every other mark where it stands, so a system the camera has left
    // behind keeps whatever share it had at the moment it was let go of —
    // nothing, in the case of one the camera was inside — and would hold the
    // ruler on a plane nobody is standing in.
    holding: Res<HeldSystem>,
    outside: Query<&Grid, With<BigSpace>>,
    mut planes: Query<PlaneParts>,
    asked: Res<RulerUnit>,
    middle: Res<ShowMiddle>,
    time: Res<Time<Real>>,
    mut changing: ResMut<Handover>,
    mut descended: ResMut<RuledSystem>,
    mut commands: Commands,
) {
    // Which system the planes ruled in light seconds should be hanging in.
    // Made and unmade here rather than placed, since a plane is placed by the
    // grid on its parent and there is no way to change that but to be a child
    // of something else.
    let wanted = inside
        .iter()
        .find(|(entity, ..)| Some(*entity) == holding.of())
        .map(|(entity, ..)| entity);
    let settling = descended.0 != wanted;
    if settling {
        for (entity, plane, ..) in &planes {
            if plane.inside {
                // Tried rather than told. A plane ruled inside a system is a
                // child of it, so the frame the spyglass sweeps that system
                // away takes the plane with it, and this would be the second
                // despawn of an entity whose index has since been handed out
                // again.
                commands.entity(entity).try_despawn();
            }
        }
        if let Some(parent) = wanted {
            commands.spawn((
                ruled::Ruled,
                Ruler { inside: true },
                CellCoord::default(),
                Visibility::Hidden,
                ChildOf(parent),
            ));
        }
        descended.0 = wanted;
    }

    let camera = cameras.single().ok();
    let lit = showing.0 && camera.is_some();

    // How much of the sky is on screen, which is the one thing the whole
    // ruling is worked out from.
    let (across, orbit) = match camera {
        Some((orbit, lens)) => {
            (crate::camera::framed(orbit.radius, lens) as f64, Some(orbit))
        }
        None => (0., None),
    };

    // How much of the mark standing for the system is left is what the two
    // planes change hands on: as the mark gives way to the system drawn in its
    // place the galaxy plane gives way to the one ruled in the system's own
    // grid, and as the mark comes back so does the galaxy's. The mark and the
    // ruler are one thing happening — a mark is the system stood in for, and a
    // sky ruled about the system is the system stood in — which is what
    // [`crate::systems::bodies::spawn::fade`] says the fade is for.
    //
    // Only the one system the map is holding ever has a mark that goes out
    // (see [`crate::systems::bodies::spawn::fade`]), so a wide neighbour whose
    // own measure would have its mark nearly gone from four light years away
    // cannot take the ruler while the camera stands somewhere else.
    //
    // Whole where there is no system to be inside of, which is a map out among
    // the stars — and also the frame the poll hands the rows to a nearer
    // system and the grid goes with them. That last can be a step, so the
    // share is eased rather than taken; see [`HANDS_OVER_IN`].
    //
    // And held back, while the camera is inside the system, by what the galaxy
    // can rule at this zoom. Its ladder has a floor — a plane ruled finer than
    // the galaxy grid can place it swims, so below that it fades out (see
    // [`rulable`]) — and the camera reaches zooms inside a system that are far
    // under it. Handing over there rules the sky in nothing at all: the
    // system's plane is switched off by the share and the galaxy's has no
    // lines to draw. So inside the system the handover goes no further than
    // the galaxy's own ladder can carry it, and a camera zooming out keeps the
    // ruling that can be drawn until the zoom can carry the other.
    //
    // Only inside. Panning outward at close zoom leaves that ladder under its
    // floor the whole way, and the clamp then held the sky at the system's
    // ruling however far the camera went. See [`wants_ruling`].
    let carried = outside.single().ok().map_or(0., |grid| {
        rulable(unit_for(LIGHT_YEARS, *asked), grid, across)
    });
    let wants = wanted
        .and_then(|entity| inside.get(entity).ok())
        .map(|(_, system, _, standing)| {
            let away = orbit.map_or(f32::INFINITY, |orbit| {
                space::metres(orbit.eye - system.position()).length() as f32
            });
            let stood_in =
                away <= crate::systems::scale::drawn_shell(system.reach());

            wants_ruling(standing.0, carried, stood_in)
        })
        .unwrap_or(1.);
    let step = time.delta_secs() / HANDS_OVER_IN;
    let standing = handed_over(changing.0, wants, step);
    if changing.0 != standing {
        changing.0 = standing;
    }
    let (out_there, down_here) = handover(standing);

    let galaxy = lit
        .then(|| outside.single().ok())
        .flatten()
        .zip(orbit)
        .filter(|_| out_there > 0.)
        .map(|(grid, orbit)| {
            placed(
                unit_for(LIGHT_YEARS, *asked),
                grid,
                DVec3::ZERO,
                across,
                orbit,
                out_there,
            )
        });
    let within = lit
        .then(|| wanted.and_then(|entity| inside.get(entity).ok()))
        .flatten()
        .zip(orbit)
        .filter(|_| down_here > 0.)
        .map(|((_, system, grid, _), orbit)| {
            placed(
                unit_for(LIGHT_SECONDS, *asked),
                grid,
                system.position(),
                across,
                orbit,
                down_here,
            )
        });

    for (
        _,
        ruler,
        mut transform,
        mut cell,
        mut plane,
        mut spoken,
        mut reading,
        mut visible,
    ) in &mut planes
    {
        // Planes made this frame are not in the world yet, and the ones being
        // unmade are still in it. Either way this is not the frame to place
        // them: one has no parent to be placed by, and the other is about to
        // stop existing.
        let space = match (ruler.inside, settling) {
            (true, true) => None,
            (true, false) => within.as_ref(),
            (false, _) => galaxy.as_ref(),
        };
        let Some(space) = space.filter(|it| it.showing() > 0.) else {
            visible.set_if_neq(Visibility::Hidden);
            reading.strength = 0.;
            continue;
        };
        // Said outright rather than inherited. A plane ruled inside a system
        // is a child of that system so its grid places it, and nothing more:
        // left to inherit, the ruling went dark whenever the spyglass stopped
        // reaching the star it hangs from, which is a pan away from a system
        // the camera is still standing near.
        visible.set_if_neq(Visibility::Visible);
        // What the ruling comes to, for everything drawn over it to read. Only
        // ever one plane at a time carries one worth anything, the handover
        // having no overlap in it.
        *reading = space.reading(middle.0, bright.0);

        // Only the altitude. Where the ruling is measured from along the plane
        // is worked out under the camera by [`ruled::place`], so the plane
        // itself has no reason to move sideways and every reason not to.
        let (at_cell, at) = space.grid.translation_to_grid(DVec3::new(
            0.,
            space.at.y * space.unit.metres,
            0.,
        ));
        cell.set_if_neq(at_cell);
        transform.translation = at;

        *plane = Plane {
            cell: space.decade.fine * space.unit.metres,
            families: space.decade.rows(space.share).map(|row| Family {
                strength: drawn_at(row.strength, bright.0),
                ..row
            }),
            numbers: Painted {
                // The crossings that carry a number are the ones the numbers
                // were already stepped by, so a number falls on a line and
                // there are about as many across the view as fit.
                apart: (space.step / space.decade.fine) as f32,
                tall: (space.across / space.decade.fine / FIGURES_ACROSS)
                    as f32,
                strength: drawn_at(INK * space.showing(), bright.0),
                // Written by `ruled::place`, which settles where the ruling is
                // measured from and which way the camera is standing.
                from: plane.numbers.from,
                upright: plane.numbers.upright,
                downward: plane.numbers.downward,
            },
            reach: space.reach,
            edge_on: EDGE_ON,
            color: LINE,
            // Written by [`ruled::place`], which runs later in the frame.
            eye: plane.eye,
            facing: plane.facing,
        };

        // And what each of those crossings says, written out here rather than
        // worked out on the card. What a crossing is worth, which thousand it
        // is called and how many places it is said to are questions about the
        // map's own units, and the answers are the same ones the module
        // writes at the middle of the view.
        //
        // About the crossing the view is centred on, that being where the
        // numbers worth reading are. The window reaches further than the ruling
        // does at any zoom, the two both following how far the camera stands
        // back, so it running out is not a thing the map can be zoomed into.
        let middle = IVec2::new(
            (space.crossing.x / space.step).round() as i32,
            (space.crossing.z / space.step).round() as i32,
        );
        let base = middle - IVec2::splat(NUMBERED as i32 / 2);
        spoken.base = base;
        for into in 0..NUMBERED {
            let along = f64::from(base.x + into as i32) * space.step;
            let across = f64::from(base.y + into as i32) * space.step;
            spoken.along[into] = Number::say(&ticked(along, space.step));
            spoken.across[into] = Number::say(&ticked(across, space.step));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ruled::ladder::tests::zooms;
    use crate::systems::bodies::STAND_IN;

    /// Only one of the two spaces is ever ruled, and one of them always is
    ///
    /// Both halves reported, from three tries at the same crossing. The two
    /// ladders share no cell size at any zoom, so two rulings at once are two
    /// rulings that disagree with two sets of numbers over each other — and a
    /// handover that fades one out before the other comes in leaves the sky
    /// bare in the middle. What changes hands is the contrast: whichever has
    /// the sky dims toward [`DIMMEST`] and the other picks up from there.
    #[test]
    fn one_space_is_ruled_and_only_one() {
        for step in 0..=200 {
            let through = step as f32 / 200.;
            let (out, down) = handover(through);

            assert!(
                out == 0. || down == 0.,
                "{through} of the way out drew the galaxy at {out} \
                 and a system at {down}"
            );
            assert!(
                out.max(down) >= DIMMEST,
                "{through} of the way out left the sky ruled at \
                 {}",
                out.max(down)
            );
        }
    }

    /// And the swap happens where the ruling is faint, not where it is whole
    ///
    /// Which is the whole of how a lattice is replaced by one that shares no
    /// line with it without the sky reading as flicking over. Whole a cutover
    /// out on either side, [`DIMMEST`] at the middle, and one way about all
    /// the way.
    #[test]
    fn the_ruler_changes_hands_at_its_faintest() {
        assert_eq!(handover(1.), (1., 0.));
        assert_eq!(handover(0.), (0., 1.));
        // A hair inside the cutover, the arithmetic of `0.5 - CUTOVER` not
        // landing on it exactly in a float.
        let whole = 0.5 + CUTOVER * 1.01;
        assert_eq!(handover(whole), (1., 0.), "the galaxy came short");
        assert_eq!(handover(1. - whole), (0., 1.), "the system came short");

        // Either side of the middle, both at their faintest and one of them
        // dark: the swap costs the difference between `DIMMEST` and nothing.
        assert_eq!(handover(0.5), (DIMMEST, 0.));
        let (out, down) = handover(0.5 - f32::EPSILON);
        assert_eq!(out, 0., "both were drawn a hair under the middle");
        assert!(
            (down - DIMMEST).abs() < 1e-4,
            "the swap was at {down} rather than at {DIMMEST}"
        );

        assert!(
            DIMMEST > 0. && DIMMEST < 0.5,
            "{DIMMEST} is not a faint ruling"
        );
        assert!(
            CUTOVER <= 0.25,
            "the dimming takes {CUTOVER} of the band either side, which is \
             most of it"
        );

        // Whichever has the sky only ever brightens away from the middle, so
        // a camera crossing at a pace sees one ruling give way to the other
        // rather than the two trading.
        for step in 0..100 {
            let nearer = 0.5 + step as f32 / 400.;
            let further = 0.5 + (step + 1) as f32 / 400.;
            assert!(
                handover(further).0 >= handover(nearer).0,
                "the galaxy dimmed on its way out at {further}"
            );
            assert!(
                handover(1. - further).1 >= handover(1. - nearer).1,
                "the system dimmed on its way in at {further}"
            );
        }
    }

    /// A system the camera has panned away from does not keep the ruler
    ///
    /// The other half of the same report: the grid did not come back. Panning
    /// outward at close zoom leaves the galaxy's ladder under its floor the
    /// whole way, so the clamp that keeps a system's ruling where the galaxy
    /// has none held the sky at that system's plane however far the camera
    /// went — light seconds counted about a point light years behind it, with
    /// no way back short of zooming out. The clamp holds only while the camera
    /// is inside the system it belongs to.
    #[test]
    fn a_system_panned_away_from_does_not_keep_a_ruling() {
        // The galaxy can rule nothing at this zoom, and the mark standing for
        // the system has come whole back.
        assert_eq!(
            wants_ruling(1., 0., true),
            0.,
            "the system gave up a ruling the galaxy could not take"
        );
        assert_eq!(
            wants_ruling(1., 0., false),
            1.,
            "a system the camera had left behind kept the ruler"
        );
        // And inside it the mark still decides, the clamp only ever holding
        // the galaxy off.
        assert_eq!(wants_ruling(0., 1., true), 0.);
        assert_eq!(wants_ruling(1., 1., true), 1.);
    }

    /// The ruler changes hands as the mark standing for the system goes out
    ///
    /// The reported trouble, measured with the diagnostics panel open: inside
    /// Alpha Centauri, nineteen of its bodies drawn, the panel read `mark 0.00
    /// left` beside `ruler 1.00 out` — the mark wholly given way to the system
    /// drawn in its place, and galactic coordinates still written over it.
    ///
    /// The share was a fixed sphere about a thousand astronomical units wide,
    /// which is `STAND_IN / WORTH_DRAWING`: where the *narrowest* system's
    /// insides are drawn. Every wider system therefore kept the galaxy's
    /// ruling well inside itself, Alpha Centauri for another two and a half
    /// decades of zoom. A mark is an angle scaled by how far the system
    /// reaches, so read off the mark the two agree for every system however
    /// wide it is.
    #[test]
    fn the_ruler_changes_hands_as_the_mark_goes_out() {
        use crate::systems::bodies::spawn::{
            WORTH_HIDING, WORTH_KEEPING, standing_for,
        };

        // A fifth of a light year, and a system with nothing on record: the
        // widest and the narrowest the map draws.
        for reach in [2.1e15_f32, STAND_IN] {
            let system = crate::systems::tests::reaching(1, 0., reach);
            // The eye stood off where the system subtends `seen` radians.
            let at = |seen: f32| {
                let away = f64::from(reach / seen) / space::LIGHT_YEAR;
                let eye = system.position() + DVec3::new(away, 0., 0.);
                let away =
                    space::metres(eye - system.position()).length() as f32;
                let stood_in =
                    away <= crate::systems::scale::drawn_shell(reach);
                handover(wants_ruling(standing_for(&system, eye), 1., stood_in))
            };

            // Under the band the mark fades over, so the mark stands whole and
            // the galaxy has the whole sky.
            assert_eq!(
                at(WORTH_KEEPING),
                (1., 0.),
                "a system reaching {reach} m took the sky while its mark \
                 still stood whole"
            );
            // And past the far end of it, where there is no mark left: the
            // reported case, `mark 0.00 left` with the ruler still out.
            assert_eq!(
                at(WORTH_HIDING * 2.),
                (0., 1.),
                "a system reaching {reach} m left the galaxy's ruling over \
                 its own drawn bodies"
            );
        }
    }

    /// And the ruler is not handed over past what the galaxy can rule
    ///
    /// The reported trouble with the mark alone: panning out of a system at
    /// close zoom takes the mark back while the galaxy's own ladder is below
    /// its floor, so the system's plane would be switched off by the share and
    /// the galaxy's would have no lines to draw — the grid vanishing outright
    /// rather than changing hands. The handover goes no further than the
    /// galaxy's ladder can carry it, so what is drawn is always something.
    #[test]
    fn the_ruler_is_not_handed_over_past_what_the_galaxy_can_rule() {
        let grid = crate::space::galaxy_grid();
        let au = 1.495978707e11 / space::LIGHT_YEAR;

        // A view a few astronomical units across, which is the zoom the camera
        // reaches among a system's planets.
        let close = 3. * au;
        assert_eq!(
            rulable(LIGHT_YEARS, &grid, close),
            0.,
            "the galaxy ruled a view {close} light years across"
        );
        assert_eq!(
            wants_ruling(1., rulable(LIGHT_YEARS, &grid, close), true),
            0.,
            "a mark standing whole handed the ruler back anyway"
        );

        // And a view a hundredth of a light year across, which it can rule, so
        // there the mark is left to decide.
        let far = rulable(LIGHT_YEARS, &grid, 1e-2);
        assert!(far > 0.);
        assert_eq!(
            wants_ruling(1., far, true),
            1.,
            "the mark was not left to it"
        );
    }

    /// And it changes hands at a pace even where it is asked for at once
    ///
    /// The mark goes out over a band of distance, so a camera moving through
    /// that band fades as it moves. What this is for is the camera that does
    /// not move through it: a search flown to, or the frame the rows pass to a
    /// nearer system and the grid goes with them, where the share can still be
    /// asked to step.
    #[test]
    fn the_ruler_changes_hands_at_a_pace() {
        let frames = 60.;
        let step = (1. / frames) / HANDS_OVER_IN;
        let after = handed_over(0., 1., step);

        assert!(
            handover(after).0 <= 0.,
            "the galaxy's ruling arrived in one frame, at {after}"
        );

        // And it gets the whole way there, inside the time it is given.
        let mut share = after;
        for _ in 0..(frames * HANDS_OVER_IN) as usize {
            share = handed_over(share, 1., step);
        }
        assert_eq!(share, 1., "the handover never finished");
    }

    /// Below the floor the ladder stops rather than going on
    ///
    /// A plane ruled finer than its own grid can place it is a plane whose
    /// lines swim as the camera moves, which is worse than one that has
    /// stopped subdividing.
    #[test]
    fn the_ladder_stops_at_the_finest_cell() {
        let finest = finest(LIGHT_YEARS, &crate::space::galaxy_grid());
        for across in zooms().filter(|across| *across < finest) {
            let ruled = ruling(across, finest);
            assert!(
                ruled.fine >= finest * (1. - 1e-9),
                "{across} across ruled {}, finer than the {finest} floor",
                ruled.fine
            );
        }
    }

    /// And having stopped, it fades out rather than standing there empty
    #[test]
    fn a_cell_wider_than_the_view_is_not_drawn() {
        let finest = finest(LIGHT_YEARS, &crate::space::galaxy_grid());
        let ruled = ruling(finest / 100., finest);

        assert_eq!(ruled.drawn, 0.);
    }

    /// Every crossing the ruling reaches has a number written for it
    ///
    /// [`NUMBERED`] is a fixed window and the map zooms over twenty decades, so
    /// the whole design rests on how many numbered crossings fall inside the
    /// ruling not growing with the zoom. It does not: the ruling reaches
    /// [`FADE_BEYOND`] times how far the camera is standing back, and its
    /// numbers are spaced by a share of what that much standing back takes in,
    /// so the count is a ratio of two things that move together.
    ///
    /// What it does follow is the shape of the window. The view is cut to the
    /// narrower of its two angles, so a tall thin window takes in less sky from
    /// the same distance back and the same reach covers more crossings. Held
    /// down to a window four times taller than it is wide, which is narrower
    /// than one gets dragged.
    #[test]
    fn the_window_reaches_past_the_ruling() {
        for shape in [0.25, 0.5, 1., 2.] {
            let lens = Projection::Perspective(PerspectiveProjection {
                aspect_ratio: shape,
                ..default()
            });
            // How much of the sky one light year of standing back takes in,
            // which is what turns a reach into a count of crossings.
            let framed = crate::camera::framed(1., Some(&lens)) as f64;
            for across in zooms() {
                let reach = across * FADE_BEYOND / framed;
                let crossings = reach / numbering(across);
                assert!(
                    crossings < (NUMBERED / 2) as f64,
                    "{across} across on a window {shape} as wide as it is tall \
                     reaches {crossings} crossings, past the {} the window \
                     holds either way",
                    NUMBERED / 2
                );
            }
        }
    }

    /// Everything the ruling draws follows the one knob
    ///
    /// The lines and the numbers along them are one thing seen at once, and a
    /// ruler whose lines dimmed while its numbers did not would read as two.
    #[test]
    fn the_whole_ruling_dims_together() {
        let mut app = looking(100.);
        app.insert_resource(Bright(1.));
        app.update();
        let (lines, numbers) = drawn(&mut app);

        app.insert_resource(Bright(0.5));
        app.update();
        let (dimmer, fainter) = drawn(&mut app);

        assert!((dimmer - lines / 2.).abs() < 1e-6, "lines came out {dimmer}");
        assert!(
            (fainter - numbers / 2.).abs() < 1e-6,
            "numbers came out {fainter}"
        );
    }

    /// And none of it past whole, an alpha having nowhere above one to go
    #[test]
    fn the_ruling_does_not_brighten_past_whole() {
        let mut app = looking(100.);

        app.insert_resource(Bright(1e3));
        app.update();

        let (lines, numbers) = drawn(&mut app);
        assert_eq!(lines, 1.);
        assert_eq!(numbers, 1.);
    }

    /// What the plane that is drawn can be read off, if any of them is
    ///
    /// Only ever one at a time, the handover having no overlap in it.
    fn read(app: &mut App) -> Option<Reading> {
        let mut planes = app.world_mut().query::<&Reading>();
        planes.iter(app.world()).find(|it| it.strength > 0.).copied()
    }

    /// How strongly the plane's widest drawn row and its numbers come out
    fn drawn(app: &mut App) -> (f32, f32) {
        let mut planes = app.world_mut().query::<&Plane>();
        let plane = planes.iter(app.world()).next().expect("the plane");
        let lines =
            plane.families.iter().map(|row| row.strength).fold(0., f32::max);
        (lines, plane.numbers.strength)
    }

    /// The map opens on a view the ruled plane can be seen in
    ///
    /// Level with the plane the camera looks along it rather than at it, and
    /// the ruling is faded out entirely below [`EDGE_ON`] of square on. A
    /// map that opens there opens with no ruler on it, and a ruler that has to
    /// be found by dragging is a ruler nobody knows is there.
    #[test]
    fn the_map_opens_looking_at_the_plane() {
        // What the shader fades on: how much of the view ray runs across the
        // plane rather than along it, which for the ray down the middle of the
        // view is the pitch alone.
        let square = OrbitCamera::default().pitch.sin().abs();
        assert!(
            square > EDGE_ON,
            "opens {square} from square on, faded out under {EDGE_ON}"
        );
    }

    /// Left to the map, a space is said in its own unit at every zoom
    #[test]
    fn a_space_is_said_in_its_own_unit() {
        assert_eq!(unit_for(LIGHT_YEARS, RulerUnit::Automatic), LIGHT_YEARS);
        assert_eq!(
            unit_for(LIGHT_SECONDS, RulerUnit::Automatic),
            LIGHT_SECONDS
        );
        // And either may be pinned from the bar.
        assert_eq!(unit_for(LIGHT_SECONDS, RulerUnit::LightYears), LIGHT_YEARS);
        assert_eq!(
            unit_for(LIGHT_YEARS, RulerUnit::LightSeconds),
            LIGHT_SECONDS
        );
    }

    /// So the cells never change size but by a decade
    ///
    /// The whole of what a ruled plane is for. Its ladder is decades of
    /// whatever the space is said in, so a unit that turns over under the
    /// camera re-founds the ladder — and the two units are 3.156e7 apart, which
    /// is not a power of ten. Every line on the plane then moves by a factor of
    /// about three, in one frame, and the ruler stops being a ruler.
    #[test]
    fn the_cells_never_change_size_but_by_a_decade() {
        for space in [LIGHT_YEARS, LIGHT_SECONDS] {
            for across in zooms() {
                let unit = unit_for(space, RulerUnit::Automatic);
                let seen = across * space::LIGHT_YEAR / unit.metres;
                let cell = ruling(seen, 0.).fine * unit.metres;
                // In the space's own unit, whatever it was said in.
                let decades = (cell / space.metres).log10();
                assert!(
                    (decades - decades.round()).abs() < 1e-9,
                    "{across} across in {space:?} rules cells of {cell}m, \
                     {decades} decades of what the space is measured in"
                );
            }
        }
    }

    /// A world with a galaxy, a camera `back` light years out from the middle
    /// of it, and the two planes waiting to be ruled
    ///
    /// Everything [`rule`] reads and nothing else. The planes are spawned by
    /// hand rather than through [`spawn_planes`], which wants a startup
    /// schedule and a map that has already flushed its commands.
    fn looking(back: f32) -> App {
        looking_at(back, DVec3::ZERO)
    }

    fn looking_at(back: f32, center: DVec3) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(ShowGrid(true));
        app.insert_resource(ShowMiddle(true));
        app.init_resource::<Bright>();
        app.init_resource::<RuledSystem>();
        app.init_resource::<Handover>();
        app.init_resource::<HeldSystem>();

        let map = app
            .world_mut()
            .spawn((BigSpace::default(), crate::space::galaxy_grid()))
            .id();
        app.insert_resource(Map(map));
        app.world_mut().spawn((
            OrbitCamera {
                radius: back,
                target_radius: back,
                center,
                ..default()
            },
            CellCoord::default(),
            Transform::default(),
        ));
        app.init_resource::<RulerUnit>();
        app.world_mut().spawn((
            ruled::Ruled,
            Ruler { inside: false },
            CellCoord::default(),
            Visibility::Hidden,
            ChildOf(map),
        ));

        app.add_systems(Update, rule);
        app.update();
        app
    }

    /// How the galaxy's plane came out: whether it is drawn, the cell it is
    /// ruled in, in light years, and how high it hangs
    fn ruled(app: &mut App) -> (Visibility, f64, f32) {
        let mut planes =
            app.world_mut().query::<(&Visibility, &Plane, &Transform)>();
        let (visible, plane, transform) =
            planes.iter(app.world()).next().expect("the plane was spawned");
        (*visible, plane.cell / space::LIGHT_YEAR, transform.translation.y)
    }

    /// How strongly the plane's family of lines `apart` cells apart is drawn
    fn family(app: &mut App, apart: f32) -> f32 {
        let mut planes = app.world_mut().query::<&Plane>();
        let plane = planes.iter(app.world()).next().expect("the plane");
        plane
            .families
            .iter()
            .find(|it| it.apart == apart)
            .map_or(0., |it| it.strength)
    }

    /// A camera looking at the galaxy is given a ruled plane
    ///
    /// The end of the whole thing. Every property above holds of arithmetic
    /// that nothing has yet been asked to run, and a ruling that is never
    /// made visible passes all of them.
    #[test]
    fn looking_at_the_galaxy_rules_a_plane() {
        let mut app = looking(100.);

        let (visible, cell, _) = ruled(&mut app);
        assert_eq!(visible, Visibility::Visible, "the plane was not drawn");
        // A hundred light years back takes in about thirty eight of them, so
        // the ladder lands on cells of one.
        assert!((cell - 1.).abs() < 1e-6, "ruled cells of {cell} light years");
    }

    /// One plane carries the whole decade
    ///
    /// Three rows of lines from the two cells, on the one plane, so that they
    /// share an origin and an altitude rather than two placements having to
    /// agree about where they are.
    #[test]
    fn one_plane_carries_the_whole_decade() {
        let mut app = looking(100.);

        for apart in [1., 10., 100.] {
            assert!(
                family(&mut app, apart) > 0.,
                "nothing was drawn {apart} cells apart"
            );
        }
    }

    /// And numbers to read off it, in light years
    #[test]
    fn looking_at_the_galaxy_gives_numbers_to_read() {
        let mut app = looking(100.);

        let ruled = read(&mut app).expect("nothing was left to read");
        assert_eq!(ruled.unit, LIGHT_YEARS);
        assert!(ruled.strength > 0.);
        // A hundred back takes in about thirty eight light years. Ten apart
        // is what the ladder alone would give, and the widest a pair can be is
        // twenty two of them, so it steps up twice rather than let two run
        // into each other.
        assert!(
            (ruled.step - 50.).abs() < 1e-6,
            "numbered every {} light years",
            ruled.step
        );
    }

    /// Looking at the galactic plane lays the ruled plane exactly on it
    ///
    /// [`a_plane_near_the_galactic_plane_lands_on_it`] the whole way through,
    /// from a camera to a transform: the plane is placed through the grid,
    /// and a snap that survived the arithmetic but not the cell split would
    /// still leave the map with a floor a little off from the galaxy's.
    #[test]
    fn a_plane_over_the_galactic_plane_sits_on_it() {
        let mut app = looking(100.);

        let (_, _, altitude) = ruled(&mut app);
        assert_eq!(altitude, 0., "the plane sat {altitude}m off the galaxy");
    }

    /// The plane hangs exactly where the camera looks, at any height
    ///
    /// Rather than on the nearest cell. Laid on a cell it stepped a whole one
    /// every time the view climbed past a boundary, which is a thing to fight
    /// rather than a thing to read a height off, and it left the number over
    /// it a height above a plane that was itself somewhere unsaid.
    #[test]
    fn the_plane_follows_the_view_without_stepping() {
        // Heights a hundredth of a light year apart, at a zoom whose cells are
        // whole light years. Laid on a cell these would all be the one answer.
        for up in [0., 0.01, 0.02, 12.34, -7.5] {
            let mut app = looking_at(100., DVec3::new(0., up, 0.));

            let ruled = read(&mut app).expect("nothing was left to read");
            assert_eq!(
                ruled.at.y, up,
                "the camera looked at {up} and the plane hung at {}",
                ruled.at.y
            );
        }
    }

    /// And a camera on the galactic plane still reads zero
    ///
    /// Which is what laying the plane on the nearest cell was for. Said
    /// outright it needs no cell to stand on.
    #[test]
    fn a_view_on_the_galactic_plane_reads_zero() {
        let mut app = looking_at(100., DVec3::new(120., 0., -40.));

        let ruled = read(&mut app).expect("nothing was left to read");
        assert_eq!(ticked(ruled.at.y, ruled.step), "0");
    }

    /// What is drawn over the plane holds its strength through a decade
    ///
    /// The two cells crossfade, and through the middle of a decade both sit at
    /// half. That is one ruling handing over to itself rather than a ruling
    /// going away, so anything drawn over the plane rather than on it — the
    /// numbers, the mark at the middle of the view — must not follow the two
    /// of them. It would pulse once a decade, about nothing.
    #[test]
    fn what_is_drawn_over_the_plane_holds_through_a_decade() {
        // A decade of zoom, the middle of it included: eight cells across at
        // one end and eighty at the other, and both cells at half in between.
        for back in [30., 45., 66., 100., 150., 220., 300.] {
            let mut app = looking(back);

            let ruled = read(&mut app).expect("nothing was left to read");
            assert!(
                ruled.strength > 0.99,
                "{back} ly back drew what stands over the plane at {}",
                ruled.strength
            );
        }
    }

    /// The ruling holds at both ends of the zoom
    ///
    /// A scale that came out infinite, negative or nothing at all is one the
    /// shader rules with `fract` of a number that is not a number.
    #[test]
    fn the_ruling_holds_across_the_whole_zoom() {
        for back in [1e-6, 1., 1e3, 1e5] {
            let mut app = looking(back);
            let (_, cell, _) = ruled(&mut app);
            assert!(
                cell.is_finite() && cell > 0.,
                "{back} ly back ruled cells of {cell}"
            );
        }
    }

    /// Switched off, nothing is ruled and nothing is left to read
    #[test]
    fn a_grid_switched_off_draws_nothing() {
        let mut app = looking(100.);
        app.world_mut().resource_mut::<ShowGrid>().0 = false;
        app.update();

        let (visible, ..) = ruled(&mut app);
        assert_eq!(visible, Visibility::Hidden);
        assert!(read(&mut app).is_none());
    }

    /// The two units are marked apart
    ///
    /// Both are written beside the numbers they belong to, so a pair that read
    /// the same would say nothing about which space is being looked at.
    #[test]
    fn the_units_are_marked() {
        assert_eq!(LIGHT_YEARS.mark, "Ly");
        assert_eq!(LIGHT_SECONDS.mark, "Ls");
    }

    /// A system's plane is ruled far finer than the galaxy's can be
    ///
    /// The reason there are two spaces at all. The galaxy's grid cannot place
    /// a plane inside a star system, and the ruling has to go on getting finer
    /// after it has stopped being able to.
    #[test]
    fn a_system_rules_finer_than_the_galaxy() {
        let galaxy = finest(LIGHT_YEARS, &crate::space::galaxy_grid())
            * LIGHT_YEARS.metres;
        let system = finest(LIGHT_SECONDS, &crate::space::system_grid())
            * LIGHT_SECONDS.metres;

        assert!(
            system < galaxy / 1e3,
            "the galaxy stops at {galaxy}m and a system at {system}m"
        );
    }
}
