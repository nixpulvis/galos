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
//! # What draws the lines
//!
//! [`crate::ruled`], which is a fullscreen pass that meets the view ray with
//! the plane per pixel and rules it there, counting in cells rather than in
//! the metres the map is drawn in. It is antialiased by the screen space
//! derivative, writes depth so that stars occlude it, and costs one draw call
//! however much of the galaxy is on screen.
//!
//! What it does not do is decide anything. How wide a cell is, where the plane
//! hangs, how strongly it is drawn and what any of it is called are worked out
//! here and written onto it every frame.
//!
//! # Two planes, and what stands between them
//!
//! One hangs in the galaxy's grid and one inside whatever system the camera
//! has descended into, because how finely a plane may be ruled follows the
//! grid it hangs in — see [`ruled::finest`]. The galaxy's cells are `2^53`
//! metres and bottom out around a light second; a system's are a metre and
//! bottom out well below anything worth drawing.
//!
//! The two are never on screen together. A light year is `3.15576e7` light
//! seconds, which is no power of ten, so the two ladders share no cell size at
//! any zoom: ruled at once they beat against each other. So the one is spent
//! before the other begins, with a moment of unruled sky between them. That
//! moment is the honest reading. At that zoom neither unit rules truthfully —
//! the galaxy's grid has run out of places to put a line, and the system's has
//! not yet been reached.
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::space::{self, Map};
use crate::systems::System;
use crate::systems::bodies::spawn::{Apparent, Body};
use crate::systems::labels::screen_offset;
use crate::systems::selection::Selected;
use crate::ruled::{
    self, BARE, Family, LETTERS, Lettering, NONE, NUMBERED, Numbered, Painted,
    Plane, RuledPlugin, Word,
};
use ab_glyph::{Font, FontRef, PxScale};
use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat,
};
use bevy::camera::visibility::VisibilitySystems;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::{
    EguiContexts, EguiPostUpdateSet, EguiPrimaryContextPass, egui,
};
use big_space::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_plugins(RuledPlugin);
    app.insert_resource(ShowGrid(true));
    app.insert_resource(ShowMiddle(true));
    app.insert_resource(ShowPicked(true));
    app.init_resource::<Said>();
    app.init_resource::<Reading>();
    app.init_resource::<Descended>();
    app.init_resource::<Dropped>();
    // After the map itself, which is what the galaxy's planes hang from. The
    // resource naming it is inserted through a command, so it is not there to
    // be read until the schedule that queued it has ended.
    app.add_systems(Startup, cut_lettering);
    app.add_systems(PostStartup, spawn_planes);
    // In `Present`, which runs after `Camera` has settled where the camera is
    // standing. Everything here is worked out from that and nothing else.
    app.add_systems(Update, rule.in_set(MapSet::Present));
    // The lines dropped to the plane read where the plane ended up rather than
    // deciding it, and a `GlobalTransform` under a floating origin is not
    // computed until `PostUpdate`. Reading it any earlier drops a line to
    // where the plane was last frame, which at these speeds is a line that
    // misses. Same reason [`crate::systems::labels::leaders`] runs here.
    // After the plane has been told where it stands, which is what turns the
    // middle of the view into a crossing.
    app.add_systems(PostUpdate, stand_clear.after(ruled::Placing));
    app.add_systems(
        PostUpdate,
        drop_lines
            .after(TransformSystems::Propagate)
            .after(VisibilitySystems::MarkNewlyHiddenEntitiesInvisible)
            // And before egui's pass, which is run from a system in this same
            // schedule with no ordering of its own against the transforms.
            // [`numbers`] writes what this leaves behind, so left unordered it
            // writes it a frame late, and a number a frame behind the line it
            // is about slides around while the camera moves and only lands
            // once it stops.
            .before(EguiPostUpdateSet::EndPass),
    );
    // The numbers are egui's, and egui draws in a pass of its own after
    // `Update`, so what they are drawn from is this frame's answer rather than
    // last frame's.
    app.add_systems(
        EguiPrimaryContextPass,
        numbers.after(crate::ui::lettering),
    );
}

/// Whether the ruled plane is drawn
#[derive(Resource)]
pub struct ShowGrid(pub bool);

/// Whether the place the camera is looking at is marked at the middle of the
/// view
///
/// The plane's own numbers say where its lines are; this says where the view is,
/// which is the one of the three a line cannot carry.
#[derive(Resource)]
pub struct ShowMiddle(pub bool);

/// And whether the places of the things picked out are
///
/// Each marked where its line meets the plane, with a line standing off it
/// saying how far off it is. A separate switch from the middle's: the middle is
/// one mark wherever the camera goes, and this is one for everything selected,
/// which is as busy as the selection is.
#[derive(Resource)]
pub struct ShowPicked(pub bool);

/// How many cells the finer of the two planes lays across the view
///
/// The ladder is decades, so what is actually on screen runs from this up to
/// ten times it before the next decade takes over. Eight at the sparse end is
/// eighty at the dense end, which is about as fine as ruling gets before the
/// lines stop reading as lines and start reading as shading.
const CELLS_ACROSS: f64 = 8.;

/// How many numbers to aim for across the view
///
/// Far fewer than there are cells, because the numbers are laid over the whole
/// plane rather than along one line through the middle of it: they land on a
/// lattice, and a lattice this wide is what keeps two of them from being
/// written on top of each other where the plane is nearest.
///
/// The cells are there to be counted between the numbers.
const TICKS_ACROSS: f64 = 2.;

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

/// How sharply the plane goes as it is turned edge on
///
/// The shader's own term, weighing how square the plane is to the eye. Left at
/// the default it ships with, which loses the plane as the camera comes level
/// with it.
const FADE_EDGE_ON: f32 = 0.25;

/// What the ruling is drawn in
///
/// Cold and unsaturated, so that it reads as chrome laid over the sky rather
/// than as more of the sky. Every star on the map is warmer than this.
const LINE: Color = Color::srgb(0.55, 0.66, 0.82);

/// How strongly a cell's lines and its tenth lines are drawn
///
/// Faint, both of them, and fainter than what is drawn in the sky rather than
/// under it. An orbit is laid down at a quarter, so a ruling that reached that
/// far would have the two competing, and the ruling would win by being
/// everywhere. It crosses the whole map and is meant to be glanced at rather
/// than looked at.
///
/// The numbers are not dropped with them. They are the part of the ruling that
/// is actually read, and they are already as small as a drawn face will go.
const MINOR: f32 = 0.05;
const MAJOR: f32 = 0.13;

/// How strongly the numbers and the lines dropped to the plane are drawn
///
/// Above the ruling, being the part of it that is actually read.
const INK: f32 = 0.75;

/// How many digits tall the view is
///
/// The lettering is painted on the plane, so its size has to be asked for in
/// cells — but a cell is a decade and the view runs from eight of them across
/// to eighty before the next decade takes over. Sized in cells a digit would
/// be ten times too large at one end of every decade and ten times too small
/// at the other. Sized as a share of the view it is the same on screen at any
/// zoom, which is what a number wants to be.
///
/// Thirty four is about eleven pixels of digit on a tall window, which is
/// where a real face still reads as letters rather than as grey. A cut bitmap
/// will take half that and stay crisp; a drawn one will not.
const FIGURES_ACROSS: f64 = 34.;

/// How long each arm of the cross at the middle of the view is, in pixels
///
/// The numbers at the middle are about one point on the plane, and a number
/// written over a plane with nothing under it is a number floating loose. So
/// the point is marked, along the plane's own axes, and they stand beside it.
const CROSS: f32 = 11.;

/// How far off the plane the middle's numbers are hung, in pixels
///
/// The two rulers lie in the plane and their numbers run along them. The third
/// is about the plane itself, so it is hung along the one direction on screen
/// that neither ruler runs in. Drawn where they cross it reads as one more
/// number in the row.
///
/// Under the plane rather than over it, which is the opposite side from the one
/// a pair on the plane is written on. The two are then on either side of the
/// lines they are both about, and the middle is read against a clear row rather
/// than into a number.
const LIFT: f32 = 16.;

/// And how far to the side of a dropped line its own number stands, in pixels
///
/// Beside the line rather than over it, for the same reason a pair on the plane
/// stands beside its crossing: a number with a rule through it is a number to
/// be worked out rather than read.
const ASIDE: f32 = 6.;

/// How far from the edge of the view a number is dropped
///
/// A number half off the screen reads as a different number.
const MARGIN: f32 = 30.;

/// How far a row of numbers reaches around the point it is about, in pixels
///
/// About the row the map writes there: three numbers each with its own power, a
/// unit and two commas comes to some forty characters of an eight pixel
/// monospaced face, centred on the point, so it runs about ninety five either
/// side. Across it the [`LIFT`] that hangs it off the plane and half its own
/// height.
///
/// In pixels rather than in the plane's own units because the row is drawn in
/// pixels and the plane is not. A unit of plane covers most of a digit's width
/// on screen with the camera overhead and a fraction of one with the camera
/// down near the plane, so a reach fixed in units is a reach that means
/// something different at every pitch. [`stand_clear`] converts.
const CROWDS: Vec2 = Vec2::new(96., 22.);

/// One of the two ruled planes
///
/// One per space, each carrying both cells of its own decade pair. The pair is
/// what makes cells subdivide rather than step as the camera comes in, and
/// written onto the one plane the two share an origin and an altitude by
/// construction rather than by two placements agreeing.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
struct Ruler {
    /// Ruled in a system's own grid rather than in the galaxy's
    inside: bool,
}

/// Which system the planes ruled in light seconds are hanging in
///
/// They are children of the system they rule, which is the only way to be
/// placed by its grid, and that system is despawned whenever the camera leaves
/// it or the spyglass sweeps it away — taking them with it. So which system
/// they were made for is remembered here, and they are made afresh whenever
/// the answer changes.
#[derive(Resource, Default)]
struct Descended(Option<Entity>);

/// What a plane's numbers are said in
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Unit {
    /// Out among the systems
    LightYears,
    /// Once the camera has descended into one
    LightSeconds,
}

impl Unit {
    /// What the numbers are marked with
    fn mark(self) -> &'static str {
        match self {
            Unit::LightYears => "Ly",
            Unit::LightSeconds => "Ls",
        }
    }

    /// How many metres one of these comes to
    ///
    /// The map is drawn in metres wherever it is drawn — that is what
    /// [`crate::space`] settles — so this is what turns a number said out loud
    /// into a distance across the plane.
    fn metres(self) -> f64 {
        match self {
            Unit::LightYears => space::LIGHT_YEAR,
            Unit::LightSeconds => space::LIGHT_SECOND,
        }
    }

    /// The finest cell a plane hanging in `grid` may be ruled in, said in this
    ///
    /// Where the ladder stops. For the galaxy that is arithmetic — the grid
    /// runs out of places to put a line, [`ruled::finest`] — and for a system
    /// it is taste, the grid having room to spare.
    fn finest(self, grid: &Grid) -> f64 {
        let placed = ruled::finest(grid) * STEADY / self.metres();
        match self {
            Unit::LightYears => placed,
            Unit::LightSeconds => placed.max(FINEST_SYSTEM_CELL),
        }
    }
}

/// What the numbers are asked to be said in
///
/// Left to the map by default, which turns the ruler over as it descends into
/// a system. Pinned either way from the bar, for reading a system's distances
/// in light years or a neighbourhood's in light seconds.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Said {
    #[default]
    Whichever,
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
fn said_in(own: Unit, asked: Said) -> Unit {
    match asked {
        Said::LightYears => Unit::LightYears,
        Said::LightSeconds => Unit::LightSeconds,
        Said::Whichever => own,
    }
}

/// How much of the galaxy's ruling is drawn, and how much of a system's, as
/// the map hands one to the other
///
/// Disjoint. The one is spent before the other begins, so that two rulings
/// which share no cell size are never on screen together, and between them is
/// a moment with nothing ruled at all.
///
/// `held` is how much of the mark standing for the system is left, which is
/// what the map fades its contents in against. Following it means the ruler
/// changes hands on the same figure the sky does.
fn handover(held: f32) -> (f32, f32) {
    (((held - 0.5) * 2.).clamp(0., 1.), ((0.5 - held) * 2.).clamp(0., 1.))
}

/// The decade a view `across` wide is ruled in, and how far past it it has got
///
/// The exponent of the finer plane's cell, and a fraction from nothing to one
/// saying how far the view has zoomed out towards the decade above it. The
/// coarser plane is that decade.
fn rung(across: f64) -> (f64, f32) {
    let wanted = (across / CELLS_ACROSS).max(f64::MIN_POSITIVE);
    let ladder = wanted.log10();
    let decade = ladder.floor();
    (decade, (ladder - decade) as f32)
}

/// Which cells the two planes of a space are ruled in, and how strongly each
/// is drawn
#[derive(Clone, Copy, PartialEq, Debug)]
struct Ruling {
    /// The finer plane's cell, in whatever unit it was asked in
    fine: f64,
    fine_strength: f32,
    /// The coarser plane's cell, a decade above [`Ruling::fine`]
    coarse: f64,
    coarse_strength: f32,
    /// How much of this ruling is drawn at all
    ///
    /// Not the two strengths above put together. Those are a crossfade, and
    /// through the middle of a decade both sit at half while what is on screen
    /// is one whole ruling handing over to another. This is whether there is a
    /// ruling there to hand over, which is a different question and the only
    /// one anything outside the plane should be asking: chrome that dimmed
    /// every time a cell subdivided would be pulsing about nothing.
    drawn: f32,
}

impl Ruling {
    /// The rows of lines this comes to, in cells of [`Ruling::fine`]
    ///
    /// Three rows rather than four: the finer cell's tenth lines and the
    /// coarser cell's own lines fall in the same places, so they are the one
    /// row, drawn at both strengths laid over each other. Which is what the
    /// two planes did by being blended over each other, and is now arithmetic.
    ///
    /// Widest first, [`ruled`] drawing each row into what the wider ones have
    /// left so that a line two rows fall on is drawn once.
    fn rows(&self, handed: f32) -> [Family; ruled::FAMILIES] {
        let over = |a: f32, b: f32| a + b - a * b;
        let fine = self.fine_strength * handed;
        let coarse = self.coarse_strength * handed;
        [
            Family { apart: 100., strength: MAJOR * coarse },
            Family { apart: 10., strength: over(MAJOR * fine, MINOR * coarse) },
            Family { apart: 1., strength: MINOR * fine },
            Family::default(),
        ]
    }

}

/// How to rule a space with `across` of it on screen
///
/// The two planes are a decade apart, so that the finer one's tenth lines fall
/// exactly on the coarser one's own lines, and they are crossfaded across the
/// decade. A cell therefore subdivides into ten as the camera comes in, rather
/// than the whole ruling stepping from one size to the next.
///
/// The handoff at the end of a decade is exact. As the crossfade completes the
/// coarse plane is drawn alone, ruling its cell and its tenth lines; the
/// decade then turns over and those same two rows of lines are the fine
/// plane's, at the same two strengths. Nothing on screen changes at the moment
/// it happens.
///
/// `finest` is the smallest cell the space can be ruled in. The ladder stops
/// there rather than going on, a plane whose lines cannot be placed within a
/// cell being a plane whose lines swim. Held at the floor the fine plane is
/// drawn alone, which costs nothing: its own tenth lines are what the coarse
/// plane would have been drawing.
///
/// Where the view has come in nearer than a single cell, both go. A ruling
/// with no lines left in it is chrome standing in front of the map for
/// nothing.
fn ruling(across: f64, finest: f64) -> Ruling {
    let (decade, through) = rung(across);
    let floor = finest.log10().round();
    let held = decade < floor;
    let fine = 10f64.powf(decade.max(floor));

    // A cell wider than the view leaves at most one line on screen. Faded over
    // the last of it rather than switched off, so that a camera coming down
    // onto a body loses the plane rather than having it vanish.
    let showing = ((across / fine) - 1.).clamp(0., 1.) as f32;

    Ruling {
        drawn: showing,
        fine,
        // Held at the floor there is no decade left to cross to, so the fine
        // plane is simply what is drawn.
        fine_strength: if held { showing } else { (1. - through) * showing },
        coarse: fine * 10.,
        coarse_strength: if held { 0. } else { through * showing },
    }
}

/// The next step up the one, two, five ladder
fn wider(step: f64) -> f64 {
    let decade = 10f64.powf(step.log10().floor());
    let rung = step / decade;
    if rung < 1.5 {
        decade * 2.
    } else if rung < 3.5 {
        decade * 5.
    } else {
        decade * 10.
    }
}

/// How far apart to number the crossings, for a view `across` wide
///
/// [`tick_step`] if it will do, and the next step up the ladder for as long as
/// it will not. The numbers are painted along their own lines at a size fixed
/// against the view, so a step chosen only for how many fit runs them into
/// each other wherever the ladder lands short — and a pair that runs into the
/// next is two pairs neither of which can be read.
///
/// Stepped rather than squeezed. The size cannot give: the lettering is
/// already as small as a drawn face reads at. So what gives is how many are
/// written, which is a thing the eye follows and the ladder keeps round.
fn numbering(across: f64) -> f64 {
    // The widest a pair can be, in the same terms as the step.
    let widest = ruled::SPAN as f64 * across / (5. * FIGURES_ACROSS);
    let mut step = tick_step(across);
    // The ladder climbs by at least two a rung, so this is a handful of turns
    // at the very most. Bounded all the same, a step that has come out as
    // nothing being a step that never reaches anything.
    for _ in 0..8 {
        if step >= widest {
            break;
        }
        step = wider(step);
    }
    step
}

/// The roundest step to put between two numbers, for a view `across` wide
///
/// One, two or five times a power of ten, which is the ladder a scale is read
/// in wherever scales are read. Always a whole multiple of the fine plane's
/// cell, so that every number written falls on a line rather than between two.
fn tick_step(across: f64) -> f64 {
    let wanted = (across / TICKS_ACROSS).max(f64::MIN_POSITIVE);
    let decade = 10f64.powf(wanted.log10().floor());
    let rung = wanted / decade;
    decade
        * if rung >= 5. {
            5.
        } else if rung >= 2. {
            2.
        } else {
            1.
        }
}

/// Round `value` onto the nearest multiple of `step`
///
/// In `f64`, which holds a position out at the rim to far better than the
/// smallest cell the map rules. Landing on a multiple is the whole point of
/// moving the plane, so this is the one piece of the arithmetic that cannot be
/// done in the float the shader works in.
fn snapped(value: f64, step: f64) -> f64 {
    if step > 0. && step.is_finite() {
        (value / step).round() * step
    } else {
        value
    }
}

/// [`snapped`] on all three axes at once
fn snapped_to(place: DVec3, step: f64) -> DVec3 {
    DVec3::new(
        snapped(place.x, step),
        snapped(place.y, step),
        snapped(place.z, step),
    )
}

/// The power of ten a number is said in
///
/// Its own, so that whatever is written before the point is the figure that
/// matters and everything after it is a place. The same rule for the numbers
/// painted on the plane and for the reading at the middle of the view, which
/// are read against each other and so have to be read the same way.
///
/// Nothing at all for numbers a person reads without help. `5.6e1` is a worse
/// way of writing `56`, and most of the map is looked at from scales that have
/// words.
///
/// Asked of the larger of the number and the step it is written to. A ruler
/// counting by tenths never says anything finer than a tenth, so a coordinate a
/// hair below the origin is a nought on it rather than a billionth.
fn power(value: f64) -> i32 {
    if value == 0. || !value.is_finite() {
        return 0;
    }
    let power = value.abs().log10().floor() as i32;
    if power.abs() >= 3 { power } else { 0 }
}

/// How a number is written along a ruler stepping by `step`
///
/// As a figure and a power past a thousand: `1e3` rather than `1000`, and
/// `1.2e6` rather than `1200000`. The noughts carry no more than the power does
/// and take four times the room, and a plane of them is a plane of noughts.
///
/// As many places as the step has once that is taken off it, and no more: a
/// ruler counting by hundreds that writes three decimals is three columns of
/// noughts.
///
/// Zero is written as zero however it was arrived at. Rounding a coordinate a
/// hair below the origin otherwise gives `-0`, which reads as somewhere else.
fn ticked(value: f64, step: f64) -> String {
    let power = power(value.abs().max(step));
    let under = 10f64.powi(power);
    let places = -(step / under).log10().floor();
    if !places.is_finite() {
        return format!("{value}");
    }
    let places = places.clamp(0., 6.) as usize;
    let said = format!("{:.places$}", value / under);
    // Zero is written as zero however it was arrived at. Rounding a coordinate
    // a hair below the origin otherwise gives `-0`, which reads as somewhere
    // else.
    if said.trim_start_matches('-').trim_matches(['0', '.']).is_empty() {
        format!("{:.places$}", 0.)
    } else if power == 0 {
        said
    } else {
        format!("{said}e{power}")
    }
}

/// How the three numbers about one place are said
///
/// Each in its own power, written onto the number itself. Shared, the smaller
/// of them are written at the largest's scale and come out as a row of noughts
/// — a view sixty light years out reads `0.0690, 2.0910, -2.2090 e9`, where the
/// first says nothing and says it at a scale that is not its own.
///
/// To the same places as the plane's own numbers, by the same [`ticked`] and
/// the same step. A position and the numbers on the ruler beside it are read
/// against each other, so they are written the same way; and a power carries
/// the magnitude, which is what a ruler used a column of figures for.
fn told(at: DVec3, step: f64) -> String {
    format!(
        "{}, {}, {}",
        ticked(at.x, step),
        ticked(at.y, step),
        ticked(at.z, step)
    )
}

/// How far off the plane something standing off it is, said out loud
///
/// The third number, and the one a ruler lying in the plane cannot carry. Which
/// way it went is said with a sign rather than left to the line to show: a line
/// dropped from above the plane and one dropped from below are drawn the same
/// way round on a screen, and which of the two it is, is half the answer.
///
/// Nothing at all for something standing on the plane, or near enough that the
/// number would read as nought. The line is already as short as it can be
/// there, and a `+0.0` beside it says less than the line does.
fn off_plane(high: f64, step: f64, unit: Unit) -> Option<String> {
    let said = ticked(high, step);
    if said == ticked(0., step) {
        return None;
    }
    // Marked with its unit, unlike the numbers on the plane. Those are read
    // against the rulers they are painted on; this one stands wherever the
    // thing it is about stands, with nothing beside it to say what it counts.
    //
    // The offset alone. Where the thing stands is said in full under the mark
    // at the line's foot, and a number said twice on one screen is a number to
    // be checked against itself.
    let sign = if high > 0. { "+" } else { "" };
    Some(format!("{sign}{said} {}", unit.mark()))
}

/// What the lines dropped to the plane are about
///
/// One entry for each thing picked out.
///
/// Held rather than worked out twice because the two halves run in different
/// passes. [`drop_lines`] draws the lines in `PostUpdate`, where a
/// `GlobalTransform` under a floating origin has settled; [`numbers`] writes
/// them in egui's own pass, which is run from `PostUpdate` with no ordering
/// against the transforms at all, so reading one there would be reading it
/// whenever.
#[derive(Resource, Default)]
struct Dropped(Vec<Drop>);

/// One line dropped to the plane, and what it is about
struct Drop {
    /// Where its foot stands on the plane, as an offset from the camera's eye,
    /// in metres
    ///
    /// Where the thing is marked and where the two numbers the plane can locate
    /// it by are written.
    foot: DVec3,
    /// And where its middle stands, likewise, which is where the third is
    middle: DVec3,
    /// Where the thing stands, in whatever the numbers are being said in
    at: DVec3,
}

/// What the numbers along the rulers say
///
/// Settled by [`rule`], where the planes are placed, and read by [`numbers`]
/// and [`drop_lines`], which letter them and draw to them. Held rather than
/// worked out three times over, the three running in three different passes,
/// one of which is egui's.
///
/// Nothing while the grid is switched off, or while there is no camera to have
/// worked any of it out from, or while nothing is drawn strongly enough to be
/// worth a number.
#[derive(Resource, Default)]
struct Reading(Option<Ruled>);

/// Where the rulers are, what they step in and what they are called
struct Ruled {
    /// Where the two rulers cross, as an offset from the camera's eye, in
    /// metres
    ///
    /// Only ever projected, and a place on screen is a length over a length,
    /// so what it is measured in cancels. Metres because that is what the map
    /// draws in, and so the one unit that serves both spaces.
    from_eye: DVec3,
    /// Where the camera is looking, as an offset from the eye, in metres
    ///
    /// The middle of the view, which is where the plane hangs and where the
    /// three numbers of the place being looked at are said. Not snapped, so it
    /// sits still while the plane slides under it.
    middle_from_eye: DVec3,
    /// And where that is, in [`Ruled::unit`]
    ///
    /// Absolute galactic coordinates out among the systems, and a distance
    /// from the star once the camera is inside one.
    at: DVec3,
    /// How far apart two numbers are, in [`Ruled::unit`]
    step: f64,
    unit: Unit,
    /// How much of the ruling is drawn, which the numbers follow
    ///
    /// A number standing over a plane that has faded out is a number about
    /// nothing.
    strength: f32,
}

/// How wide and tall a glyph's cell in the lettering strip is, in pixels
///
/// Three by five, the shape the plane lays a character out in, times sixteen.
/// Enough that a number drawn larger than this reads as a letter rather than
/// as a mosaic, and small enough that the whole strip is a few tens of
/// kilobytes.
const CELL_WIDE: u32 = 48;
const CELL_TALL: u32 = 80;

/// How much of a cell's height a digit fills
///
/// Short of the whole, so that a comma has somewhere below the line to hang
/// and the card reading one glyph cannot pick up the one above.
const FILLS: f32 = 0.78;

/// And how much of the rest sits above it rather than below
const AIR: f32 = 0.06;

/// Cut the strip of glyphs the plane's numbers are painted from
///
/// Rasterised here rather than in [`ruled`], which reads no fonts and carries
/// no assets, so that lifting it out is a move rather than a rewrite. Cut from
/// the face egui draws the bar in, which is the same face every name on the
/// map is drawn in: a number on the plane and a number in the bar are then the
/// one typeface, and stay so when egui moves.
///
/// Monospaced, which is what makes a strip of equal cells the right shape for
/// it. The plane counts characters rather than measuring them.
fn cut_lettering(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let Ok(face) = FontRef::try_from_slice(epaint_default_fonts::HACK_REGULAR)
    else {
        return;
    };
    let wide = CELL_WIDE as usize * LETTERS.len();
    let mut strip = vec![0u8; wide * CELL_TALL as usize];

    // How tall a digit comes out at a trial size, so that the real one can be
    // chosen to fill the cell rather than guessed at from the face's ascent.
    // A face's ascent leaves room for accents no digit has, and a glyph cut to
    // it fills half the cell and is read as nothing at all.
    let measure = |scale: PxScale| {
        face.outline_glyph(face.glyph_id('0').with_scale(scale))
            .map(|it| it.px_bounds())
    };
    let Some(trial) = measure(PxScale::from(CELL_TALL as f32)) else { return };
    let scale = PxScale::from(
        CELL_TALL as f32 * CELL_TALL as f32 * FILLS / trial.height(),
    );
    let Some(digit) = measure(scale) else { return };
    // Where the line the letters stand on falls in the cell: a little air
    // above the digits, and what a comma needs under them.
    let base = CELL_TALL as f32 * AIR - digit.min.y;

    for (nth, letter) in LETTERS.iter().enumerate() {
        let glyph = face.glyph_id(*letter).with_scale(scale);
        let Some(cut) = face.outline_glyph(glyph) else { continue };
        let bounds = cut.px_bounds();
        // Middle of its own cell across, on the line down.
        let left = nth as f32 * CELL_WIDE as f32
            + (CELL_WIDE as f32 - bounds.width()) / 2.;
        cut.draw(|x, y, covered| {
            let at = (
                (left + x as f32).round() as i32,
                (base + bounds.min.y + y as f32).round() as i32,
            );
            if at.0 < 0 || at.1 < 0 || at.0 as usize >= wide {
                return;
            }
            if at.1 as u32 >= CELL_TALL {
                return;
            }
            let ink = &mut strip[at.1 as usize * wide + at.0 as usize];
            *ink = (*ink).max((covered * 255.) as u8);
        });
    }

    let mut image = Image::new(
        Extent3d {
            width: wide as u32,
            height: CELL_TALL,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        strip,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    // Read smoothly, and never off the end of the strip.
    image.sampler = ImageSampler::linear();

    commands.insert_resource(Lettering(images.add(image)));
}

/// Create the two planes ruled in light years
///
/// Under the map rather than under the galaxy, which is thrown away and
/// replaced whenever the map is cleared. These are chrome and survive that,
/// the same as the camera does and for the same reason.
///
/// The two ruled in light seconds are not made here. They hang inside whatever
/// system the camera has descended into, and there is none at startup — see
/// [`Descended`].
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
    unit: Unit,
    /// The grid the planes of this space hang in, which splits a position into
    /// a cell and a remainder
    grid: &'a Grid,
    ruling: Ruling,
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
    /// Where the camera is, in [`Placement::unit`] and measured from whatever
    /// the space is measured from
    eye: DVec3,
    /// How much of this space's ruling is drawn, as the descent hands the map
    /// from one space to the other
    handed: f32,
}

impl Placement<'_> {
    /// Where something in this space lies from the camera's eye, in metres
    fn seen_from_eye(&self, place: DVec3) -> DVec3 {
        (place - self.eye) * self.unit.metres()
    }

    /// How much of this space is drawn at all
    fn showing(&self) -> f32 {
        self.ruling.drawn * self.handed
    }

    /// What the numbers over this space say
    fn reading(&self) -> Ruled {
        Ruled {
            from_eye: self.seen_from_eye(self.crossing),
            middle_from_eye: self.seen_from_eye(self.at),
            at: self.at,
            step: self.step,
            unit: self.unit,
            strength: self.showing(),
        }
    }
}

/// Work out how to rule one space
///
/// `from` is where the space is measured from in absolute galactic light
/// years, which is the galactic centre for the galaxy and the star for a
/// system. `across` is how much of the sky is on screen, in light years, which
/// is the one figure the whole ruling follows.
fn placed<'a>(
    unit: Unit,
    grid: &'a Grid,
    from: DVec3,
    across: f64,
    orbit: &OrbitCamera,
    handed: f32,
) -> Placement<'a> {
    // Everything from here is in `unit`. The view is measured in light years
    // whatever is being looked at, so it is spoken into the space's own unit
    // once, here, and not thought about again.
    let spoken =
        |place: DVec3| (place - from) * space::LIGHT_YEAR / unit.metres();
    let across = across * space::LIGHT_YEAR / unit.metres();

    let ruling = ruling(across, unit.finest(grid));
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
        ruling,
        at,
        crossing,
        across,
        step,
        eye: spoken(orbit.eye),
        handed,
    }
}

/// Rule the planes, place them under the camera, and say what they are called
///
/// Runs every frame. All of it follows the zoom, the zoom is eased rather than
/// stepped, and so there is no frame on which none of it has moved.
#[allow(clippy::too_many_arguments)]
fn rule(
    showing: Res<ShowGrid>,
    cameras: Query<(&OrbitCamera, Option<&Projection>)>,
    held: Res<Apparent>,
    // The system the camera has descended into, if it has. It is the one
    // carrying a grid of its own, which it does only while its contents are
    // drawn. Its cells are a metre, which is what lets a plane be ruled in
    // light seconds at all.
    inside: Query<(Entity, &System, &Grid), Without<BigSpace>>,
    outside: Query<&Grid, With<BigSpace>>,
    mut planes: Query<(
        Entity,
        &Ruler,
        &mut Transform,
        &mut CellCoord,
        &mut Plane,
        &mut Numbered,
        &mut Visibility,
    )>,
    said: Res<Said>,
    mut descended: ResMut<Descended>,
    mut reading: ResMut<Reading>,
    mut commands: Commands,
) {
    // Which system the planes ruled in light seconds should be hanging in.
    // Made and unmade here rather than placed, since a plane is placed by the
    // grid on its parent and there is no way to change that but to be a child
    // of something else.
    let wanted = inside.iter().next().map(|(entity, _, _)| entity);
    let settling = descended.0 != wanted;
    if settling {
        for (entity, plane, ..) in &planes {
            if plane.inside {
                commands.entity(entity).despawn();
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

    // How far the descent has got. The map fades a system's contents in over
    // this same stretch, so ruling the two spaces by it hands the ruler over
    // exactly as the sky changes hands.
    let out_among_them = held.held();

    // The one is spent before the other begins, so that two ladders which
    // share no cell size are never on screen together.
    let (out_there, down_here) = handover(out_among_them);

    let galaxy = lit
        .then(|| outside.single().ok())
        .flatten()
        .zip(orbit)
        .filter(|_| out_there > 0.)
        .map(|(grid, orbit)| {
            placed(
                said_in(Unit::LightYears, *said),
                grid,
                DVec3::ZERO,
                across,
                orbit,
                out_there,
            )
        });
    let within = lit
        .then(|| inside.iter().next())
        .flatten()
        .zip(orbit)
        .filter(|_| down_here > 0.)
        .map(|((_, system, grid), orbit)| {
            placed(
                said_in(Unit::LightSeconds, *said),
                grid,
                system.position(),
                across,
                orbit,
                down_here,
            )
        });

    // Whichever of the two is drawn is the one the numbers are read off. Only
    // ever one of them, the handover having no overlap in it, so this is a
    // choice rather than a contest.
    let louder = match (galaxy.as_ref(), within.as_ref()) {
        (Some(out), Some(down)) if down.showing() > out.showing() => Some(down),
        (Some(out), _) => Some(out),
        (_, down) => down,
    };
    reading.0 = louder.filter(|it| it.showing() > 0.).map(Placement::reading);

    let radius = orbit.map_or(0., |orbit| orbit.radius) as f64;
    for (
        _,
        ruler,
        mut transform,
        mut cell,
        mut plane,
        mut spoken,
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
            continue;
        };
        visible.set_if_neq(Visibility::Inherited);

        // Only the altitude. Where the ruling is measured from along the plane
        // is worked out under the camera by [`ruled::place`], so the plane
        // itself has no reason to move sideways and every reason not to.
        let (at_cell, at) = space.grid.translation_to_grid(DVec3::new(
            0.,
            space.at.y * space.unit.metres(),
            0.,
        ));
        cell.set_if_neq(at_cell);
        transform.translation = at;

        *plane = Plane {
            cell: space.ruling.fine * space.unit.metres(),
            families: space.ruling.rows(space.handed),
            numbers: Painted {
                // The crossings that carry a number are the ones the numbers
                // were already stepped by, so a number falls on a line and
                // there are about as many across the view as fit.
                apart: (space.step / space.ruling.fine) as f32,
                tall: (space.across / space.ruling.fine / FIGURES_ACROSS)
                    as f32,
                strength: INK * space.showing(),
                // Written by `ruled::place`, which settles where the ruling is
                // measured from and which way the camera is standing.
                from: plane.numbers.from,
                upright: plane.numbers.upright,
                downward: plane.numbers.downward,
                // Written by `stand_clear`, which runs later in the frame,
                // once the names have settled which of them are drawn.
                bare: plane.numbers.bare,
            },
            // In metres, being a distance out through the world rather than a
            // distance across the plane. Past the far side of the view, so
            // that what fades is the horizon rather than what is looked at.
            reach: radius * space::LIGHT_YEAR * FADE_BEYOND,
            edge_on: FADE_EDGE_ON,
            color: LINE,
            // Written by [`ruled::place`], which runs later in the frame.
            eye: plane.eye,
            facing: plane.facing,
        };

        // And what each of those crossings says, written out here rather than
        // worked out on the card. What a crossing is worth, which thousand it
        // is called and how many places it is said to are questions about the
        // map's own units, and the answers are the same ones [`numbers`] writes
        // at the middle of the view.
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
            spoken.along[into] = Word::say(&ticked(along, space.step));
            spoken.across[into] = Word::say(&ticked(across, space.step));
        }
    }
}

/// What wears a mark, of the two kinds of thing that can
type Marked = (With<Selected>, Or<(With<System>, With<Body>)>);

/// Give up the crossing the middle of the view is written over
///
/// The three numbers said at the middle are the same two the crossing beneath
/// them would be, and better: they carry the third, and they are not rounded
/// to a crossing. So where the two would land on each other the crossing gives
/// way.
///
/// The one it is written over, and only while it is. Each crossing owns a block
/// of the plane and the blocks tile it, so the middle stands in one of them and
/// that one gives way. Away from a row of lettering it stands in none and the
/// plane is left whole.
///
/// The names the map draws over the same sky are not asked about. A plane that
/// gave up a crossing for every name would be a plane pocked with holes
/// wherever the sky is busy, which is where its numbers are most wanted, and
/// what a name needs is to stand out rather than for everything else to move.
fn stand_clear(
    mut planes: Query<(&mut Plane, &Numbered)>,
    middle: Res<ShowMiddle>,
    reading: Res<Reading>,
    cameras: Query<(&OrbitCamera, &Camera)>,
) {
    let seen = reading.0.as_ref().zip(cameras.single().ok());

    for (mut plane, spoken) in &mut planes {
        let mut bare = [NONE; BARE];

        if middle.0
            && let Some((ruled, (orbit, camera))) = seen
            && let Some(room) = reaches(plane.as_ref(), ruled, orbit, camera)
            && let Some(crossing) = plane.crossing_near(
                spoken,
                ruled.middle_from_eye.as_vec3(),
                room,
            )
        {
            bare[0] = crossing;
        }

        if plane.numbers.bare != bare {
            plane.numbers.bare = bare;
        }
    }
}

/// [`CROWDS`] in the units `plane`'s lettering is laid out in
///
/// Measured at the middle of the view, by stepping a whole spacing along each
/// of the plane's own axes and seeing how far that carries on screen. Which
/// takes the pitch with it: the axis running away towards the horizon is
/// squashed to nothing as the camera comes down level with the plane, and a
/// row of pixels there covers a great many units of plane.
///
/// Nothing while the plane has no lettering to measure, or while the middle is
/// somewhere neither axis can be projected from.
fn reaches(
    plane: &Plane,
    ruled: &Ruled,
    orbit: &OrbitCamera,
    camera: &Camera,
) -> Option<Vec2> {
    let viewport = camera.logical_viewport_size()?;
    let cot_half_fov = camera.clip_from_view().y_axis.y;
    let at = screen_offset(orbit, cot_half_fov, viewport, ruled.middle_from_eye)?;

    // One numbered spacing, in metres, which is the length the axes are stepped
    // by. Long enough that the two ends do not land on the same float out at
    // the rim, and short enough to stay inside the view.
    let spacing = plane.numbers.apart as f64 * plane.cell;
    let unit = plane.numbers.tall / 5.;
    if !spacing.is_finite() || spacing <= 0. || unit <= 0. {
        return None;
    }
    let across = |axis: Vec2| -> Option<f32> {
        let along = plane.facing * Vec3::new(axis.x, 0., axis.y);
        let to = screen_offset(
            orbit,
            cot_half_fov,
            viewport,
            ruled.middle_from_eye + along.as_dvec3() * spacing,
        )?;
        // Pixels to a spacing, and a spacing is `apart / unit` of them.
        let pixels = (to - at).length() * unit / plane.numbers.apart;
        (pixels > 0.).then_some(pixels)
    };

    Some(Vec2::new(
        CROWDS.x / across(plane.numbers.upright)?,
        CROWDS.y / across(plane.numbers.downward)?,
    ))
}

/// Drop a line to the plane from whatever is worth locating
///
/// The two rulers give a place on the plane, and this gives the height above
/// it — the third of the three numbers, and the one a plane on its own cannot
/// say. Dropped from the point the camera is looking at, and from everything
/// picked out.
///
/// Gizmos rather than meshes, both ends of every line moving every frame.
#[allow(clippy::too_many_arguments)]
fn drop_lines(
    showing: Res<ShowGrid>,
    picked_out: Res<ShowPicked>,
    reading: Res<Reading>,
    mut gizmos: Gizmos,
    mut dropped: ResMut<Dropped>,
    cameras: Query<&GlobalTransform, With<Camera>>,
    // Whatever is picked out, which out among the systems is a system and
    // inside one is a body. Both stand off the plane and both are asked the
    // same question by it, so both are answered.
    //
    // Whether it is drawn as well as picked out. A system the map has hidden,
    // because a filter excluded it or because the camera has come down into it
    // and its bodies have taken over, is not there to be located; a line
    // dropped from where it would have stood is a line about nothing.
    picked: Query<
        (Entity, &GlobalTransform, &CellCoord, &Transform, &ViewVisibility),
        Marked,
    >,
    grids: Grids,
) {
    dropped.0.clear();
    if !showing.0 || !picked_out.0 {
        return;
    }
    let Some(ruled) = &reading.0 else { return };
    let Ok(eye) = cameras.single() else { return };

    // How high the plane the numbers are about stands, which is the one thing
    // a line dropped to it has to know. Taken from the same reading the
    // numbers come from rather than off whichever plane is drawn: the rulers
    // cross on the plane, so how far the crossing stands from the eye is how
    // far the plane does.
    let altitude = eye.translation().y + ruled.from_eye.y as f32;

    let color = LINE.with_alpha(INK * ruled.strength);

    // Only what is picked out. The plane runs through what the camera is
    // looking at, so a line dropped from there would have no length; how far
    // off it something else stands is the question a plane cannot answer by
    // being ruled.
    for (entity, at, cell, transform, shown) in &picked {
        if !shown.get() {
            continue;
        }
        let Some(grid) = grids.parent_grid(entity) else { continue };
        let from = at.translation();
        gizmos.line(from, Vec3::new(from.x, altitude, from.z), color);

        // Where it stands, asked of the grid that places it rather than
        // measured out from the camera. A cell is an `i64` count and a
        // transform is the offset inside it, so this is the position itself
        // and has nothing of the camera in it.
        //
        // Measured out from the camera it would have: the camera is at one
        // float's remove and the thing at another, and neither remove cancels
        // the other. It comes to a ten thousandth of the last place written,
        // which is nothing at all until the number sits on a rounding
        // boundary — and a coordinate stored in thirty seconds of a light year
        // sits on one about a third of the time. Then it turns over and back
        // as the camera swings.
        let place = (grid.cell_to_float(cell)
            + transform.translation.as_dvec3())
            / ruled.unit.metres();

        // Only where it goes on screen is measured from the eye, where a
        // float's worth of slack is a fraction of a pixel.
        let seen = (from - eye.translation()).as_dvec3();
        dropped.0.push(Drop {
            foot: DVec3::new(seen.x, ruled.from_eye.y, seen.z),
            middle: DVec3::new(seen.x, (seen.y + ruled.from_eye.y) / 2., seen.z),
            at: place,
        });
    }
}

/// Paint the plane's lines with their own numbers, and say where the view is
///
/// Painted onto egui's background layer, which takes no pointer input: these
/// are numbers written over the map rather than chrome standing in front of
/// it, and a wheel turned over one belongs to the map underneath.
fn numbers(
    mut contexts: EguiContexts,
    showing: Res<ShowGrid>,
    middle: Res<ShowMiddle>,
    reading: Res<Reading>,
    dropped: Res<Dropped>,
    cameras: Query<(&OrbitCamera, &Camera)>,
) -> Result {
    if !showing.0 {
        return Ok(());
    }
    let Some(ruled) = &reading.0 else { return Ok(()) };
    let Ok((orbit, camera)) = cameras.single() else { return Ok(()) };
    let Some(viewport) = camera.logical_viewport_size() else { return Ok(()) };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    let ctx = contexts.ctx_mut()?;
    let font = egui::TextStyle::Small.resolve(&ctx.global_style());
    let painter = ctx.layer_painter(egui::LayerId::background());
    // The colour the plane is ruled in, read from the one place it is said.
    // Egui is handed colours rather than asked for them, so this is where the
    // map's own is spoken into its.
    let hue = LINE.to_srgba();
    let ink = egui::Color32::from_rgba_unmultiplied(
        (hue.red * 255.) as u8,
        (hue.green * 255.) as u8,
        (hue.blue * 255.) as u8,
        (255. * INK * ruled.strength.clamp(0., 1.)) as u8,
    );

    // Where a number may be written without half of it falling off the edge.
    let room = egui::Rect::from_min_max(
        egui::pos2(MARGIN, MARGIN),
        egui::pos2(viewport.x - MARGIN, viewport.y - MARGIN),
    );

    // Which way the plane's own axes and its normal run on screen. The two
    // axes are what the cross at the middle is drawn along; the normal is the
    // one direction neither of them runs in, and so where anything written
    // about the plane goes rather than on it.
    //
    // The plane's own numbers are not written here at all. They are painted
    // into the ruling by [`ruled`], lying in the plane and turning, shrinking
    // and going with it, which is what a number on a ruler does.
    let metres = ruled.unit.metres();
    let toward = |from: DVec3, axis: DVec3| -> Vec2 {
        let (Some(at), Some(along)) = (
            screen_offset(orbit, cot_half_fov, viewport, from),
            screen_offset(orbit, cot_half_fov, viewport, from + axis * ruled.step * metres),
        ) else {
            return Vec2::ZERO;
        };
        (along - at).normalize_or_zero()
    };
    // A point on the plane, marked where its own two axes cross and with what
    // the plane can say about it hung underneath. The middle of the view and
    // the foot of every dropped line are the same kind of thing — a place on
    // the plane worth locating — so they are marked the same way.
    let mark = |at: DVec3, said: String| {
        let Some(seen) = screen_offset(orbit, cot_half_fov, viewport, at)
        else {
            return;
        };
        let seen = egui::pos2(seen.x, seen.y);
        if room.contains(seen) {
            for axis in [DVec3::X, DVec3::Z] {
                let arm = toward(at, axis) * CROSS;
                let arm = egui::vec2(arm.x, arm.y);
                painter.line_segment(
                    [seen - arm, seen + arm],
                    egui::Stroke::new(1_f32, ink),
                );
            }
        }

        // Hung off the plane along the one direction on screen that neither
        // ruler runs in, and under it rather than over it, which is the
        // opposite side from the one a pair on the plane is written on.
        let hung = -toward(at, DVec3::Y) * LIFT;
        let place = egui::pos2(seen.x + hung.x, seen.y + hung.y);
        if room.contains(place) {
            painter.text(
                place,
                egui::Align2::CENTER_CENTER,
                said,
                font.clone(),
                ink,
            );
        }
    };

    // The place the camera is looking at, all three of it, held at the middle
    // of the view. Not snapped to anything, so it sits still while the plane
    // slides under it, and rounded to the same step the plane is numbered in
    // so that it reads against those numbers.
    //
    // Drawn at full strength rather than faded like the plane's own: the
    // others fade with how far off they are because that is what they are
    // about, and this one is about where the view is, which is never far off.
    if middle.0 {
        mark(
            ruled.middle_from_eye,
            format!("{} {}", told(ruled.at, ruled.step), ruled.unit.mark()),
        );
    }

    // And every line dropped to the plane, which is the same three numbers
    // about something that is not at the middle. They are said in full under
    // the mark at the line's foot, where the two rulers can be read against
    // them; the line itself carries only how far off the plane it went, which
    // is the one thing about it neither ruler nor mark can show.
    for drop in &dropped.0 {
        mark(
            drop.foot,
            format!("{} {}", told(drop.at, ruled.step), ruled.unit.mark()),
        );

        let Some(said) =
            off_plane(drop.at.y - ruled.at.y, ruled.step, ruled.unit)
        else {
            continue;
        };
        let Some(place) =
            screen_offset(orbit, cot_half_fov, viewport, drop.middle)
        else {
            continue;
        };
        let place = egui::pos2(place.x + ASIDE, place.y);
        if room.contains(place) {
            painter.text(
                place,
                egui::Align2::LEFT_CENTER,
                said,
                font.clone(),
                ink,
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole zoom the map allows, in light years
    ///
    /// From a metre, which is as near as the camera may be pulled to what it
    /// looks at, out past the far rim of the galaxy. Every property below is
    /// swept across all of it: the ladder has to hold at both ends and at
    /// every decade between, and it is the seams between decades that go
    /// wrong rather than the middles.
    fn zooms() -> impl Iterator<Item = f64> {
        (-17..=6).flat_map(|decade| {
            [1., 1.7, 2.5, 4.2, 7.9]
                .into_iter()
                .map(move |through| through * 10f64.powi(decade))
        })
    }

    /// The fine plane's cell is always a decade, and the coarse one the decade
    /// above it
    #[test]
    fn the_two_planes_are_a_decade_apart() {
        for across in zooms() {
            let ruled = ruling(across, 0.);
            let decades = (ruled.coarse / ruled.fine).log10();
            assert!(
                (decades - 1.).abs() < 1e-9,
                "{across} ruled {} and {}, {decades} decades apart",
                ruled.fine,
                ruled.coarse
            );
            let exponent = ruled.fine.log10();
            assert!(
                (exponent - exponent.round()).abs() < 1e-9,
                "{across} ruled a cell of {}, not a decade",
                ruled.fine
            );
        }
    }

    /// However far the camera zooms, what is on screen is a countable number
    /// of cells
    ///
    /// The whole point of the ladder. A ruling that came out at two cells
    /// across at one zoom and two thousand at another is not a scale, and the
    /// seams between decades are where that would happen.
    #[test]
    fn the_view_always_holds_a_countable_number_of_cells() {
        for across in zooms() {
            let ruled = ruling(across, 0.);
            // Whichever plane is the more strongly drawn is the one being
            // counted, the other having faded towards nothing.
            let cell = if ruled.fine_strength >= ruled.coarse_strength {
                ruled.fine
            } else {
                ruled.coarse
            };
            let cells = across / cell;
            assert!(
                (2. ..=90.).contains(&cells),
                "{across} across came out {cells} cells wide"
            );
        }
    }

    /// The crossfade between the two planes never leaves the sky unruled
    ///
    /// One of them is always drawn at most of its strength. Both fading at
    /// once is a zoom that passes through a moment with no ruling on screen.
    #[test]
    fn something_is_always_drawn() {
        for across in zooms() {
            let ruled = ruling(across, 0.);
            let loudest = ruled.fine_strength.max(ruled.coarse_strength);
            assert!(
                loudest > 0.49,
                "{across} across left the strongest cell at {loudest}"
            );
        }
    }

    /// A decade turning over changes nothing on screen
    ///
    /// The seam the crossfade exists to hide. Just below the turn the coarse
    /// plane is drawn alone; just above it the fine plane is, ruled in the
    /// same cell and at the same strength. Anything else is a visible step in
    /// the middle of a smooth zoom.
    #[test]
    fn a_decade_turns_over_without_a_step() {
        for decade in -6..=4 {
            let turn = CELLS_ACROSS * 10f64.powi(decade);
            let under = ruling(turn * (1. - 1e-9), 0.);
            let over = ruling(turn * (1. + 1e-9), 0.);

            assert!(
                (under.coarse - over.fine).abs() < over.fine * 1e-6,
                "at {turn} the coarse plane ruled {} and the fine one {}",
                under.coarse,
                over.fine
            );
            assert!(
                (under.coarse_strength - over.fine_strength).abs() < 1e-3,
                "at {turn} the ruling stepped from {} to {}",
                under.coarse_strength,
                over.fine_strength
            );
        }
    }

    /// The two spaces are never ruled at the same time
    ///
    /// A light year is `3.15576e7` light seconds, so the two ladders share no
    /// cell size at any zoom and two rulings at once are two rulings that
    /// disagree. The handover is disjoint rather than a crossfade, which is
    /// what this says and the only thing that makes it so.
    #[test]
    fn only_one_space_is_ever_ruled() {
        for step in 0..=200 {
            let held = step as f32 / 200.;
            let (out, down) = handover(held);
            assert!(
                out == 0. || down == 0.,
                "at {held} the galaxy was drawn at {out} and a system at {down}"
            );
        }
    }

    /// And between them the sky is unruled, which is the price of that
    #[test]
    fn the_handover_passes_through_nothing() {
        let (out, down) = handover(0.5);
        assert_eq!((out, down), (0., 0.));
        // Either side of it, one of them has the sky.
        assert_eq!(handover(1.).0, 1.);
        assert_eq!(handover(0.).1, 1.);
    }

    /// A decade turning over moves no line and changes no strength
    ///
    /// [`a_decade_turns_over_without_a_step`] one level down, on the rows of
    /// lines actually drawn rather than on the two cells they are worked out
    /// from. Just under the turn the wider cell draws its own lines and its
    /// tenths; just over, the same two rows are the finer cell's, at the same
    /// two strengths and the same two spacings.
    #[test]
    fn a_decade_turns_over_without_a_line_moving() {
        // What is drawn, as how far apart the lines really are and how
        // strongly, faintest rows dropped as being nothing on screen.
        let drawn = |ruled: &Ruling| {
            let mut rows: Vec<(f64, f32)> = ruled
                .rows(1.)
                .into_iter()
                .filter(|row| row.apart > 0. && row.strength > 1e-4)
                .map(|row| (ruled.fine * row.apart as f64, row.strength))
                .collect();
            rows.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("a spacing"));
            rows
        };

        for decade in -4..=4 {
            let turn = CELLS_ACROSS * 10f64.powi(decade);
            let under = drawn(&ruling(turn * (1. - 1e-9), 0.));
            let over = drawn(&ruling(turn * (1. + 1e-9), 0.));

            assert_eq!(
                under.len(),
                over.len(),
                "at {turn} the ruling went from {under:?} to {over:?}"
            );
            for (before, after) in under.iter().zip(&over) {
                assert!(
                    (before.0 - after.0).abs() < before.0 * 1e-6,
                    "at {turn} a row of lines moved from {} apart to {}",
                    before.0,
                    after.0
                );
                assert!(
                    (before.1 - after.1).abs() < 1e-3,
                    "at {turn} a row of lines went from {} to {}",
                    before.1,
                    after.1
                );
            }
        }
    }

    /// Below the floor the ladder stops rather than going on
    ///
    /// A plane ruled finer than its own grid can place it is a plane whose
    /// lines swim as the camera moves, which is worse than one that has
    /// stopped subdividing.
    #[test]
    fn the_ladder_stops_at_the_finest_cell() {
        let finest = Unit::LightYears.finest(&crate::space::galaxy_grid());
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
        let finest = Unit::LightYears.finest(&crate::space::galaxy_grid());
        let ruled = ruling(finest / 100., finest);

        assert_eq!(ruled.drawn, 0.);
    }

    /// Every number written falls on a line of the fine plane
    ///
    /// The numbers and the ruling are chosen separately — one for how many
    /// will fit, the other for how dense the lines may be — so nothing but
    /// this says they agree. A number standing between two lines is a number
    /// about a place the ruling does not mark.
    #[test]
    fn every_number_falls_on_a_line() {
        for across in zooms() {
            let step = tick_step(across);
            let cell = ruling(across, 0.).fine;
            let cells = step / cell;
            assert!(
                (cells - cells.round()).abs() < 1e-6,
                "{across} across steps {step} over cells of {cell}"
            );
        }
    }

    /// No pair of numbers can run into the next
    ///
    /// The lettering is sized against the view and painted along its own
    /// lines, so what keeps two of them apart is the spacing and nothing else.
    /// It has to clear the widest a pair can ever be, at every rung of the
    /// ladder and every zoom — not on average, and not usually.
    #[test]
    fn no_pair_can_run_into_the_next() {
        for across in zooms() {
            let step = numbering(across);
            let widest = ruled::SPAN as f64 * across / (5. * FIGURES_ACROSS);
            assert!(
                step >= widest,
                "{across} across numbered every {step}, which a pair {widest} \
                 wide runs straight through"
            );
        }
    }

    /// And it is still a round number, however far it had to climb
    #[test]
    fn a_stepped_up_number_is_still_round() {
        for across in zooms() {
            let step = numbering(across);
            let decade = 10f64.powf(step.log10().floor());
            let rung = step / decade;
            assert!(
                [1., 2., 5.].iter().any(|it| (rung - it).abs() < 1e-6),
                "{across} across steps {step}, which is {rung} of a decade"
            );
        }
    }

    /// The numbers are stepped one, two or five times a power of ten
    ///
    /// The ladder a scale is read in everywhere scales are read. Three, seven
    /// or eleven is arithmetic the reader has to do.
    #[test]
    fn numbers_step_by_one_two_or_five() {
        for across in zooms() {
            let step = tick_step(across);
            let decade = 10f64.powf(step.log10().floor());
            let rung = step / decade;
            assert!(
                [1., 2., 5.].iter().any(|it| (rung - it).abs() < 1e-6),
                "{across} across steps {step}, which is {rung} of a decade"
            );
        }
    }

    /// And there are enough of them to read a scale off, without a wall of
    /// them
    ///
    /// A lattice rather than a row, so this is counted across the view in one
    /// direction and squared for what actually lands on screen. Two either way
    /// is a handful; five either way is twenty five numbers over the sky.
    #[test]
    fn the_plane_holds_a_handful_of_numbers() {
        for across in zooms() {
            let ticks = across / tick_step(across);
            assert!(
                (1. ..=5.).contains(&ticks),
                "{across} across wanted {ticks} numbers either way"
            );
        }
    }

    /// Snapping lands on a multiple, which is what keeps the ruling still
    ///
    /// The plane is moved under the camera every frame and its lines must not
    /// move with it. They do not, so long as every place it is moved to is a
    /// whole number of cells from every other.
    #[test]
    fn snapping_lands_on_a_multiple() {
        // Out at the rim, in light years, which is where a float would have
        // given up long ago and the `f64` this is done in must not.
        let step = 1e-3;
        for out in [0., 1., 1234.5678, 20_000.371, 68_272.94] {
            let landed = snapped(out, step);
            let cells = landed / step;
            assert!(
                (cells - cells.round()).abs() < 1e-6,
                "{out} snapped to {landed}, which is {cells} cells"
            );
            assert!(
                (landed - out).abs() <= step / 2. + f64::EPSILON * out.abs(),
                "{out} snapped to {landed}, more than half a cell away"
            );
        }
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

    /// Zero is a multiple of every step, so a crossing snapped anywhere near
    /// the middle lands exactly on it
    ///
    /// Which is what puts a `0` at the middle of a ruler rather than a number
    /// that happens to be small. The rulers are read against the crossing, so
    /// where the crossing is off by a hair every number along them is.
    #[test]
    fn a_crossing_near_the_middle_lands_on_it() {
        let step = 100.;
        for along in [-49., -0.4, 0., 12., 49.9] {
            assert_eq!(snapped(along, step), 0.);
        }
    }

    /// The middle turns over in its last figure as the view moves
    ///
    /// The whole of what the reading at the middle is for. Said to the places
    /// its own step carries, as the plane's numbers are, it would stand still
    /// until the view had crossed the gap between two of them, which at the
    /// rim is a hundred light years of dragging for one figure.
    ///
    /// Swept over the zoom, since a count of figures holds at one scale and
    /// fails at the next, which is what [`RESOLVES`] being a share of the step
    /// is about.
    #[test]
    fn the_middle_moves_when_the_view_does() {
        for across in steps() {
            let step = numbering(across);
            for at in near(across) {
                assert_ne!(
                    told(at, step),
                    told(at + DVec3::X * step, step),
                    "{across} across reads the same at {at} and one step along"
                );
            }
        }
    }

    /// And no finer than the numbers on the ruler beside it
    ///
    /// A position and a label are read against each other. One written to two
    /// more places than the other reads as a different kind of number, and the
    /// places past the label's own say nothing a power has not already said.
    ///
    /// Held on the last place written rather than on what a nudge does to it: a
    /// number sitting on a rounding boundary turns over for a hair whatever it
    /// is written to.
    #[test]
    fn the_middle_is_said_no_finer_than_the_ruler() {
        for across in steps() {
            let step = numbering(across);
            for at in wheres(across) {
                for said in told(at, step).split(", ") {
                    let (figures, power) =
                        said.split_once('e').unwrap_or((said, "0"));
                    let places = figures
                        .split_once('.')
                        .map_or(0, |(_, places)| places.len());
                    let last = 10f64
                        .powi(power.parse::<i32>().unwrap() - places as i32);
                    // A step of one is written to its own last place and a
                    // step of five to a fifth of it, the places being whole.
                    // Or to no places at all, which is where [`ticked`] stops
                    // however coarse the step is.
                    assert!(
                        places == 0 || last >= step / 10.,
                        "{across} across says {said} at {at}, to {last} \
                         against a step of {step}"
                    );
                }
            }
        }
    }

    /// And says it in a handful of figures
    ///
    /// The other half of it. A reading fine enough to move is a reading that
    /// can run to noughts, and three of them stand in one row at the middle of
    /// the view with a unit after them.
    #[test]
    fn the_middle_stays_short() {
        for across in steps() {
            let step = numbering(across);
            for at in wheres(across) {
                let said = told(at, step);
                assert!(
                    said.len() <= 32,
                    "{across} across says {said} at {at}, {} characters",
                    said.len()
                );
            }
        }
    }

    /// And every number in its own scale, written onto the number itself
    ///
    /// Shared, the smaller of them are written at the largest's scale and come
    /// out as a row of noughts, which is a number that says nothing and says it
    /// at a scale that is not its own.
    #[test]
    fn every_number_carries_its_own_scale() {
        // Sixty light years out, ruled in light seconds, where one axis stands
        // decades under the others.
        let at = DVec3::new(6.9e7, 2.091e9, -2.209e9);
        assert_eq!(told(at, 5e7), "7e7, 2.09e9, -2.21e9");

        // And no scale at all on numbers a person reads unaided.
        assert_eq!(told(DVec3::new(2.19, 6.62, -7.), 0.02), "2.19, 6.62, -7.00");
    }

    /// Somewhere a few views out from where the map is measured from, for a
    /// view `across` wide
    ///
    /// With the axes decades apart, which is the reading that goes wrong: a
    /// position at the rim is tens of thousands of light years along one axis
    /// and tens above the plane.
    fn near(across: f64) -> [DVec3; 2] {
        [
            DVec3::ZERO,
            DVec3::new(across * 7., across * 0.31, -across * 3.),
        ]
    }

    /// And somewhere far enough out to run the reading out of figures
    fn far(across: f64) -> [DVec3; 1] {
        [DVec3::new(across * 1234., across * 0.02, -across * 87.)]
    }

    fn wheres(across: f64) -> impl Iterator<Item = DVec3> {
        near(across).into_iter().chain(far(across))
    }

    /// The zooms whose step a number can be said to the whole of
    ///
    /// [`ticked`] stops at six places, so a number a million times its own step
    /// away from the origin is said as finely as it can be rather than as
    /// finely as the step asks. The ladder does not reach there unless the bar
    /// pins a unit the space is not measured in.
    fn steps() -> impl Iterator<Item = f64> {
        zooms().filter(|across| numbering(*across) >= 1e-6)
    }

    /// A number is written to as many places as its step has
    #[test]
    fn numbers_are_written_to_the_step() {
        assert_eq!(ticked(-20.5, 0.5), "-20.5");
        assert_eq!(ticked(0.25, 0.05), "0.25");
        assert_eq!(ticked(213., 100.), "213");
    }

    /// And past a thousand it is written in thousands
    ///
    /// A ruler counting in thousands writes `1K`. The noughts carry no more
    /// than the letter does and take four times the room, and a plane of them
    /// is a plane of noughts.
    #[test]
    fn a_thousand_is_written_as_one_and_a_power() {
        assert_eq!(ticked(1000., 1000.), "1e3");
        assert_eq!(ticked(1234., 100.), "1.2e3");
        assert_eq!(ticked(1234., 10.), "1.23e3");
        assert_eq!(ticked(-20_000., 10_000.), "-2e4");
        assert_eq!(ticked(1_200_000., 100_000.), "1.2e6");
        // And under a thousand it is said as it is.
        assert_eq!(ticked(999., 1.), "999");
    }

    /// Zero is written as zero, however it was arrived at
    ///
    /// Rounding a coordinate a hair below the origin gives `-0`, which reads
    /// as a place on the other side of the middle rather than as the middle.
    #[test]
    fn zero_is_never_written_as_minus_zero() {
        assert_eq!(ticked(-0.0, 1.), "0");
        assert_eq!(ticked(-1e-9, 0.1), "0.0");
    }

    /// A line dropped to the plane says which way it went
    ///
    /// A line dropped from above the plane and one dropped from below are drawn
    /// the same way round on a screen, so the sign carries the half of the
    /// answer the line cannot.
    #[test]
    fn a_dropped_line_says_which_way_it_went() {
        assert_eq!(
            off_plane(7., 2., Unit::LightYears).as_deref(),
            Some("+7 Ly")
        );
        assert_eq!(
            off_plane(-7., 2., Unit::LightYears).as_deref(),
            Some("-7 Ly")
        );
        // And in whatever the numbers are being said in.
        assert_eq!(
            off_plane(-1500., 500., Unit::LightSeconds).as_deref(),
            Some("-1.5e3 Ls")
        );
    }

    /// And says nothing at all where it has no length to speak of
    ///
    /// The line is already as short as it can be there, and a `+0.0` beside it
    /// says less than the line does.
    #[test]
    fn a_line_dropped_nowhere_says_nothing() {
        assert_eq!(off_plane(0., 2., Unit::LightYears), None);
        // Under half of the last place it is written to, which reads as nought.
        assert_eq!(off_plane(0.4, 2., Unit::LightYears), None);
        assert!(off_plane(0.6, 2., Unit::LightYears).is_some());
    }

    /// Left to the map, a space is said in its own unit at every zoom
    #[test]
    fn a_space_is_said_in_its_own_unit() {
        assert_eq!(
            said_in(Unit::LightYears, Said::Whichever),
            Unit::LightYears
        );
        assert_eq!(
            said_in(Unit::LightSeconds, Said::Whichever),
            Unit::LightSeconds
        );
        // And either may be pinned from the bar.
        assert_eq!(said_in(Unit::LightSeconds, Said::LightYears), Unit::LightYears);
        assert_eq!(said_in(Unit::LightYears, Said::LightSeconds), Unit::LightSeconds);
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
        for space in [Unit::LightYears, Unit::LightSeconds] {
            for across in zooms() {
                let unit = said_in(space, Said::Whichever);
                let seen = across * space::LIGHT_YEAR / unit.metres();
                let cell = ruling(seen, 0.).fine * unit.metres();
                // In the space's own unit, whatever it was said in.
                let decades = (cell / space.metres()).log10();
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
        app.init_resource::<Apparent>();
        app.init_resource::<Reading>();
        app.init_resource::<Descended>();

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
        app.init_resource::<Said>();
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
        assert_eq!(visible, Visibility::Inherited, "the plane was not drawn");
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
        let app = looking(100.);

        let reading = app.world().resource::<Reading>();
        let ruled = reading.0.as_ref().expect("nothing was left to read");
        assert_eq!(ruled.unit, Unit::LightYears);
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
            let app = looking_at(100., DVec3::new(0., up, 0.));

            let reading = app.world().resource::<Reading>();
            let ruled = reading.0.as_ref().expect("nothing was left to read");
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
        let app = looking_at(100., DVec3::new(120., 0., -40.));

        let reading = app.world().resource::<Reading>();
        let ruled = reading.0.as_ref().expect("nothing was left to read");
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
            let app = looking(back);

            let reading = app.world().resource::<Reading>();
            let ruled = reading.0.as_ref().expect("nothing was left to read");
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
        assert!(app.world().resource::<Reading>().0.is_none());
    }

    /// The two units are marked apart
    #[test]
    fn the_units_are_marked() {
        assert_eq!(Unit::LightYears.mark(), "Ly");
        assert_eq!(Unit::LightSeconds.mark(), "Ls");
        assert!(Unit::LightSeconds.metres() < Unit::LightYears.metres());
    }

    /// A system's plane is ruled far finer than the galaxy's can be
    ///
    /// The reason there are two spaces at all. The galaxy's grid cannot place
    /// a plane inside a star system, and the ruling has to go on getting finer
    /// after it has stopped being able to.
    #[test]
    fn a_system_rules_finer_than_the_galaxy() {
        let galaxy = Unit::LightYears.finest(&crate::space::galaxy_grid())
            * Unit::LightYears.metres();
        let system = Unit::LightSeconds.finest(&crate::space::system_grid())
            * Unit::LightSeconds.metres();

        assert!(
            system < galaxy / 1e3,
            "the galaxy stops at {galaxy}m and a system at {system}m"
        );
    }
}






