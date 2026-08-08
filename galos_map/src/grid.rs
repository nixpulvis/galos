//! A ruled plane to read the map's scale off
//!
//! A sky of points carries no scale of its own. Stars ten light years apart
//! and stars ten thousand light years apart are the same picture, and nothing
//! on screen says which one is being looked at or where in the galaxy it
//! stands. What is missing is a ruler.
//!
//! So a plane is ruled into cells and laid through what the camera is looking
//! at, numbered along two axes, with a line dropped to it from whatever is
//! worth locating. The cells and the numbers follow the zoom: out among the
//! systems they are light years, and once the camera has descended into a
//! system they are light seconds.
//!
//! # What draws the lines
//!
//! [`bevy::dev_tools::infinite_grid`], which is a fullscreen pass that
//! intersects the view ray with the plane per pixel and rules it there. It is
//! antialiased by the screen space derivative, writes depth so that stars
//! occlude it, and costs one draw call however much of the galaxy is on
//! screen — all of which lines drawn as geometry would have to be made to do.
//!
//! What it does not do is decide anything. Which cell the plane is ruled in,
//! where it sits, how strongly it is drawn and what any of it is called are
//! worked out here and written onto it every frame.
//!
//! # Why the plane moves
//!
//! The shader rules the plane by taking `fract` of the distance from its
//! origin, in `f32`. A plane left at the galactic centre would be ruled by a
//! number some `1.9e20` metres large out at the rim, where a float steps in
//! units of about a thousandth of a light year: cells of a hundred light years
//! would survive that, and cells of a light second would be noise.
//!
//! So it is not left there. It is moved under the camera every frame, onto the
//! nearest multiple of its own cell — near enough that the numbers the shader
//! works in stay the size of the view, and on a multiple so that its lines
//! still fall exactly where they would have fallen. The origin moves; the
//! ruling does not.
//!
//! That is also why the shader's coloured axis lines are turned off here. They
//! are drawn at the plane's origin, and the origin is wherever the camera
//! happened to be standing, so they would mark nothing while looking exactly
//! like the one pair of lines on the map that means something.
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::space::{self, Map};
use crate::systems::System;
use crate::systems::bodies::spawn::Apparent;
use crate::systems::labels::screen_offset;
use crate::systems::selection::Selected;
use bevy::camera::visibility::VisibilitySystems;
use bevy::dev_tools::infinite_grid::{
    InfiniteGrid, InfiniteGridPlugin, InfiniteGridSettings,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use big_space::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_plugins(InfiniteGridPlugin);
    app.insert_resource(ShowGrid(true));
    app.init_resource::<Reading>();
    app.init_resource::<Descended>();
    // After the map itself, which is what the galaxy's planes hang from. The
    // resource naming it is inserted through a command, so it is not there to
    // be read until the schedule that queued it has ended.
    app.add_systems(PostStartup, spawn_planes);
    // In `Present`, which runs after `Camera` has settled where the camera is
    // standing. Everything here is worked out from that and nothing else.
    app.add_systems(Update, rule.in_set(MapSet::Present));
    // The lines dropped to the plane read where the plane ended up rather than
    // deciding it, and a `GlobalTransform` under a floating origin is not
    // computed until `PostUpdate`. Reading it any earlier drops a line to
    // where the plane was last frame, which at these speeds is a line that
    // misses. Same reason [`crate::systems::labels::leaders`] runs here.
    app.add_systems(
        PostUpdate,
        drop_lines
            .after(TransformSystems::Propagate)
            .after(VisibilitySystems::MarkNewlyHiddenEntitiesInvisible),
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

/// How many cells the finer of the two planes lays across the view
///
/// The ladder is decades, so what is actually on screen runs from this up to
/// ten times it before the next decade takes over. Eight at the sparse end is
/// eighty at the dense end, which is about as fine as ruling gets before the
/// lines stop reading as lines and start reading as shading.
const CELLS_ACROSS: f64 = 8.;

/// How many numbers to aim for along each ruler
///
/// Fewer than there are cells. Every cell numbered is a wall of digits, and
/// the cells are there to be counted between the numbers.
const TICKS_ACROSS: f64 = 6.;

/// The finest the galaxy's planes may be ruled, in light years
///
/// Not a matter of taste. A plane's origin is placed through the grid it hangs
/// in, and the galaxy's cells are `2^53` metres: the `f32` holding a remainder
/// inside one of those resolves about `3e-8` light years. Ruling finer than
/// this is ruling from an origin that cannot be placed within a cell of it,
/// and the whole ruling would swim as the camera moved.
///
/// Well before this the camera is descending into a system, whose own planes
/// are ruled in that system's grid and have no such floor. This is what holds
/// the ladder together for a camera zooming into empty space, where there is
/// no system to descend into and nothing else to say the plane should stop.
const FINEST_GALAXY_CELL: f64 = 1e-4;

/// The finest a system's planes may be ruled, in light seconds
///
/// A matter of taste, unlike [`FINEST_GALAXY_CELL`]: a system's grid has cells
/// of a metre and could carry a far finer ruling than this. But a light second
/// is three hundred thousand kilometres, a thousandth of one is already
/// smaller than the body being looked at, and past there the numbers have
/// stopped being light seconds in any useful sense.
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
/// Faint, both of them. The plane crosses everything on the map and is meant
/// to be glanced at rather than looked at; a ruling as bright as the stars is
/// a ruling that has become the subject.
const MINOR: f32 = 0.07;
const MAJOR: f32 = 0.20;

/// How strongly the numbers and the lines dropped to the plane are drawn
///
/// Above the ruling, being the part of it that is actually read.
const INK: f32 = 0.75;

/// How much of a step a rise has to be before it is worth saying
///
/// The plane is laid on the nearest cell to what is being looked at, so the
/// rise is whatever was left over by that rounding and is very often nothing
/// at all. A line of no length wants no number over it.
const WORTH_SAYING: f64 = 1e-3;

/// How far from the edge of the view a number is dropped
///
/// A number half off the screen reads as a different number.
const MARGIN: f32 = 30.;

/// How far out a ruler may be numbered
///
/// The step is chosen to put about [`TICKS_ACROSS`] numbers across the view,
/// so this is several screens in every direction and nothing ever reaches it.
/// It is here so that a step which has come out wrong asks for a few dozen
/// labels rather than a million.
const REACH: i32 = 24;

/// One of the four ruled planes
///
/// Two spaces, and two planes a decade apart in each. The decade pair is what
/// makes cells subdivide rather than step as the camera comes in; the two
/// spaces are what carries the ruling from light years to light seconds as it
/// descends into a system. Both are crossfades, and both are described where
/// they are worked out — [`ruling`] for the first, [`Placement`] for the
/// second.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
struct Ruler {
    /// The finer of the pair, ruled a decade below the other
    fine: bool,
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

    /// The finest cell a plane said in this may be ruled in
    fn finest(self) -> f64 {
        match self {
            Unit::LightYears => FINEST_GALAXY_CELL,
            Unit::LightSeconds => FINEST_SYSTEM_CELL,
        }
    }
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
}

impl Ruling {
    /// What one of the two planes is ruled in, and how strongly it is drawn
    fn of(&self, plane: &Ruler) -> (f64, f32) {
        if plane.fine {
            (self.fine, self.fine_strength)
        } else {
            (self.coarse, self.coarse_strength)
        }
    }

    /// How much of this ruling is drawn at all
    fn showing(&self) -> f32 {
        self.fine_strength.max(self.coarse_strength)
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
        fine,
        // Held at the floor there is no decade left to cross to, so the fine
        // plane is simply what is drawn.
        fine_strength: if held { showing } else { (1. - through) * showing },
        coarse: fine * 10.,
        coarse_strength: if held { 0. } else { through * showing },
    }
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

/// How a number is written along a ruler stepping by `step`
///
/// As many places as the step has and no more: a ruler counting by hundreds
/// that writes three decimals is three columns of zeroes. Past four places it
/// is written in exponent form instead, a long run of leading zeroes being
/// unreadable at a glance and most of the width of the label.
///
/// Zero is written as zero however it was arrived at. Rounding a coordinate a
/// hair below the origin otherwise gives `-0`, which reads as somewhere else.
fn ticked(value: f64, step: f64) -> String {
    let places = -step.log10().floor();
    if !places.is_finite() {
        return format!("{value}");
    }
    if places > 4. {
        return format!("{value:.0e}");
    }
    let places = places.max(0.) as usize;
    let said = format!("{value:.places$}");
    if said.trim_start_matches('-').trim_matches(['0', '.']).is_empty() {
        format!("{:.places$}", 0.)
    } else {
        said
    }
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
    /// What the crossing point is called, in [`Ruled::unit`]
    ///
    /// Absolute galactic coordinates out among the systems, and a distance
    /// from the star once the camera is inside one.
    at: DVec3,
    /// How far apart two numbers are, in [`Ruled::unit`]
    step: f64,
    unit: Unit,
    /// How far the point the camera is looking at stands above the plane, in
    /// [`Ruled::unit`]
    rise: f64,
    /// Where that point is, as an offset from the eye, in metres
    rise_from_eye: DVec3,
    /// How much of the ruling is drawn, which the numbers follow
    ///
    /// A number standing over a plane that has faded out is a number about
    /// nothing.
    strength: f32,
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
    for fine in [true, false] {
        commands.spawn((
            InfiniteGrid,
            Ruler { fine, inside: false },
            // Placed by the map's own grid, which is the galaxy's. What
            // [`rule`] writes here every frame is a remainder within one of
            // its cells.
            CellCoord::default(),
            // Nothing is ruled until [`rule`] has looked at the camera.
            Visibility::Hidden,
            ChildOf(map.0),
        ));
    }
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
    /// Where the camera is, in [`Placement::unit`] and measured from whatever
    /// the space is measured from
    eye: DVec3,
    /// Where the camera is looking, likewise
    looking: DVec3,
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
        self.ruling.showing() * self.handed
    }

    /// What the numbers over this space say
    fn reading(&self) -> Ruled {
        Ruled {
            from_eye: self.seen_from_eye(self.crossing),
            at: self.crossing,
            step: self.step,
            unit: self.unit,
            rise: self.looking.y - self.at.y,
            rise_from_eye: self.seen_from_eye(self.looking),
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

    let ruling = ruling(across, unit.finest());
    let looking = spoken(orbit.center);
    let step = tick_step(across);

    // Both planes of a space sit at the one altitude, snapped to the coarser
    // of the two cells. Snapped to their own cells they would be two planes at
    // two heights, which is one plane and its shadow.
    //
    // Snapping the altitude at all is also what lays the plane on the galactic
    // plane whenever it is anywhere near it. Zero is a multiple of every cell,
    // so a camera looking within half a cell of `y = 0` is given `y = 0`
    // exactly. Further off than that the plane comes up to meet whatever is
    // being looked at, and the line dropped to it says how far above the
    // galaxy the view has climbed.
    let at = snapped_to(looking, ruling.coarse);
    let mut crossing = snapped_to(looking, step);
    crossing.y = at.y;

    Placement {
        unit,
        grid,
        ruling,
        at,
        crossing,
        step,
        eye: spoken(orbit.eye),
        looking,
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
        &mut InfiniteGridSettings,
        &mut Visibility,
    )>,
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
            for fine in [true, false] {
                commands.spawn((
                    InfiniteGrid,
                    Ruler { fine, inside: true },
                    CellCoord::default(),
                    Visibility::Hidden,
                    ChildOf(parent),
                ));
            }
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

    let galaxy = lit
        .then(|| outside.single().ok())
        .flatten()
        .zip(orbit)
        .filter(|_| out_among_them > 0.)
        .map(|(grid, orbit)| {
            placed(
                Unit::LightYears,
                grid,
                DVec3::ZERO,
                across,
                orbit,
                out_among_them,
            )
        });
    let within = lit
        .then(|| inside.iter().next())
        .flatten()
        .zip(orbit)
        .filter(|_| out_among_them < 1.)
        .map(|((_, system, grid), orbit)| {
            placed(
                Unit::LightSeconds,
                grid,
                system.position(),
                across,
                orbit,
                1. - out_among_them,
            )
        });

    // Whichever of the two is the more strongly drawn is the one the numbers
    // are read off. Through the middle of a descent both are on screen, and a
    // second set of numbers over the first is unreadable however faint it is.
    let louder = match (galaxy.as_ref(), within.as_ref()) {
        (Some(out), Some(down)) if down.showing() > out.showing() => Some(down),
        (Some(out), _) => Some(out),
        (_, down) => down,
    };
    reading.0 = louder.filter(|it| it.showing() > 0.).map(Placement::reading);

    for (_, plane, mut transform, mut cell, mut settings, mut visible) in
        &mut planes
    {
        // Planes made this frame are not in the world yet, and the ones being
        // unmade are still in it. Either way this is not the frame to place
        // them: one has no parent to be placed by, and the other is about to
        // stop existing.
        let space = match (plane.inside, settling) {
            (true, true) => None,
            (true, false) => within.as_ref(),
            (false, _) => galaxy.as_ref(),
        };
        let Some(space) = space else {
            visible.set_if_neq(Visibility::Hidden);
            continue;
        };

        let (in_cells, strength) = space.ruling.of(plane);
        let strength = strength * space.handed;
        if strength <= 0. {
            visible.set_if_neq(Visibility::Hidden);
            continue;
        }
        visible.set_if_neq(Visibility::Inherited);

        let (at_cell, at) =
            space.grid.translation_to_grid(space.at * space.unit.metres());
        cell.set_if_neq(at_cell);
        transform.translation = at;

        let radius = orbit.map_or(0., |orbit| orbit.radius) as f64;
        *settings = InfiniteGridSettings {
            // The axis lines mark the plane's own origin, and the origin is
            // wherever the camera happened to be standing when it was snapped.
            // In colours of their own they would read as the galactic axes,
            // which they are not, while looking exactly like the one pair of
            // lines on this map that would mean something. So they are given
            // the colour of the lines they stand among, and disappear into
            // them.
            x_axis_color: LINE.with_alpha(MAJOR * strength),
            z_axis_color: LINE.with_alpha(MAJOR * strength),
            minor_line_color: LINE.with_alpha(MINOR * strength),
            major_line_color: LINE.with_alpha(MAJOR * strength),
            // In metres, being a distance out through the world rather than a
            // distance across the plane.
            fadeout_distance: (radius * space::LIGHT_YEAR * FADE_BEYOND) as f32,
            dot_fadeout_strength: FADE_EDGE_ON,
            // The shader rules by `fract` of the distance across the plane
            // times this, and that distance is in metres.
            scale: (1. / (in_cells * space.unit.metres())) as f32,
        };
    }
}

/// Drop a line to the plane from whatever is worth locating
///
/// The two rulers give a place on the plane, and this gives the height above
/// it — the third of the three numbers, and the one a plane on its own cannot
/// say. Dropped from the point the camera is looking at, and from everything
/// picked out.
///
/// Gizmos rather than meshes, both ends of every line moving every frame.
fn drop_lines(
    showing: Res<ShowGrid>,
    reading: Res<Reading>,
    mut gizmos: Gizmos,
    planes: Query<(&GlobalTransform, &Visibility), With<Ruler>>,
    cameras: Query<(&OrbitCamera, &GlobalTransform), With<Camera>>,
    picked: Query<&GlobalTransform, (With<Selected>, With<System>)>,
) {
    if !showing.0 {
        return;
    }
    let Some(ruled) = &reading.0 else { return };
    let Ok((orbit, eye)) = cameras.single() else { return };

    // Where the planes came out, which is the one thing a line dropped to them
    // has to know. Whichever is drawn answers for all of them: the planes of a
    // space are laid at the one altitude, and only one space at a time is ever
    // the one being read.
    let Some((plane, _)) =
        planes.iter().find(|(_, visible)| **visible != Visibility::Hidden)
    else {
        return;
    };
    let altitude = plane.translation().y;

    let color = LINE.with_alpha(INK * ruled.strength);
    let mut drop = |from: Vec3| {
        gizmos.line(from, Vec3::new(from.x, altitude, from.z), color);
    };

    // What the camera is looking at, which is where the rulers cross. Worked
    // out in the metres everything is drawn in and measured from the camera,
    // which is the floating origin and so the thing every other position on
    // the map is already measured from.
    let back = (orbit.radius as f64 * space::LIGHT_YEAR) as f32;
    drop(eye.translation() + (orbit.rotation * Vec3::NEG_Z) * back);

    for at in &picked {
        drop(at.translation());
    }
}

/// Letter the two rulers, and say how far above the plane the view stands
///
/// Painted onto egui's background layer, which takes no pointer input: these
/// are numbers written over the map rather than chrome standing in front of
/// it, and a wheel turned over one belongs to the map underneath.
fn numbers(
    mut contexts: EguiContexts,
    showing: Res<ShowGrid>,
    reading: Res<Reading>,
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

    let write = |offset: DVec3, said: String| -> bool {
        let Some(place) = screen_offset(orbit, cot_half_fov, viewport, offset)
        else {
            return false;
        };
        let place = egui::pos2(place.x, place.y);
        if !room.contains(place) {
            return false;
        }
        painter.text(
            place,
            egui::Align2::CENTER_CENTER,
            said,
            font.clone(),
            ink,
        );
        true
    };

    // One ruler along each of the two axes the plane is ruled in, crossing
    // under what the camera is looking at. The unit is written once per ruler,
    // on the first number of it that lands on screen: said on every number it
    // is a column of the same two letters, and said nowhere it is a column of
    // numbers that could be anything.
    for (axis, along) in [(DVec3::X, ruled.at.x), (DVec3::Z, ruled.at.z)] {
        let mut marked = false;
        for tick in -REACH..=REACH {
            // The crossing itself belongs to neither ruler. Written by both it
            // is two labels on top of each other.
            if tick == 0 {
                continue;
            }
            let out = tick as f64 * ruled.step;
            let mut said = ticked(along + out, ruled.step);
            if !marked {
                said = format!("{said} {}", ruled.unit.mark());
            }
            let written =
                write(ruled.from_eye + axis * out * ruled.unit.metres(), said);
            marked = marked || written;
        }
    }

    // And how far above the plane the view has climbed, written at the top of
    // the line dropped to it. Left off where the plane runs through what is
    // being looked at: a height of nothing is what the absence of a line
    // already says.
    //
    // Written to its own size rather than to the ruler's step. The plane is
    // laid on the nearest cell, so the rise is what that rounding left over
    // and is always under half a cell; a ruler counting in hundreds writes no
    // decimals, and every rise smaller than one would be written as zero.
    if ruled.rise.abs() > ruled.step * WORTH_SAYING {
        write(
            ruled.rise_from_eye,
            format!(
                "{}{} {}",
                if ruled.rise > 0. { "+" } else { "" },
                ticked(ruled.rise, tick_step(ruled.rise.abs())),
                ruled.unit.mark()
            ),
        );
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
            assert!(
                ruled.showing() > 0.49,
                "{across} across left the strongest plane at {}",
                ruled.showing()
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

    /// Below the floor the ladder stops rather than going on
    ///
    /// A plane ruled finer than its own grid can place it is a plane whose
    /// lines swim as the camera moves, which is worse than one that has
    /// stopped subdividing.
    #[test]
    fn the_ladder_stops_at_the_finest_cell() {
        let finest = FINEST_GALAXY_CELL;
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
        let finest = FINEST_GALAXY_CELL;
        let ruled = ruling(finest / 100., finest);

        assert_eq!(ruled.showing(), 0.);
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
    #[test]
    fn a_ruler_holds_a_handful_of_numbers() {
        for across in zooms() {
            let ticks = across / tick_step(across);
            assert!(
                (3. ..=20.).contains(&ticks),
                "{across} across wanted {ticks} numbers"
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

    /// Zero is a multiple of every cell, so a plane snapped anywhere near the
    /// galactic plane lands exactly on it
    ///
    /// Which is the whole reason the altitude is snapped rather than followed.
    /// A ruled plane sitting a hair off `y = 0` says the galaxy has a floor
    /// somewhere other than where it has one.
    #[test]
    fn a_plane_near_the_galactic_plane_lands_on_it() {
        let cell = 100.;
        for altitude in [-49., -0.4, 0., 12., 49.9] {
            assert_eq!(snapped(altitude, cell), 0.);
        }
    }

    /// A number is written to as many places as its step has
    #[test]
    fn numbers_are_written_to_the_step() {
        assert_eq!(ticked(1234., 100.), "1234");
        assert_eq!(ticked(-20.5, 0.5), "-20.5");
        assert_eq!(ticked(0.25, 0.05), "0.25");
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

    /// A step too fine to write out is written in exponent form
    ///
    /// The alternative is a label that is mostly leading zeroes, which is
    /// unreadable at a glance and several times the width of the number.
    #[test]
    fn a_very_fine_step_is_written_short() {
        let said = ticked(2e-6, 1e-6);
        assert!(said.contains('e'), "wrote {said}");
        assert!(said.len() < 8, "wrote {said}, which is no shorter");
    }

    /// A rise is written to its own size rather than to the ruler's step
    ///
    /// The plane is laid on the nearest cell to what is being looked at, so
    /// the rise is what that rounding left over and is always under half a
    /// cell. A ruler counting in hundreds writes no decimals at all, and
    /// every rise below one written to it would read as zero — which is the
    /// one thing the number is there to say is not the case.
    #[test]
    fn a_rise_is_written_to_its_own_size() {
        // Four tenths of a light year above a plane ruled in hundreds, which
        // is the sort of rise the rounding actually leaves.
        let rise = 0.4;
        assert_eq!(ticked(rise, 100.), "0", "the step is meant to lose it");

        assert_eq!(ticked(rise, tick_step(rise)), "0.40");
    }

    /// A world with a galaxy, a camera `back` light years out from the middle
    /// of it, and the two planes waiting to be ruled
    ///
    /// Everything [`rule`] reads and nothing else. The planes are spawned by
    /// hand rather than through [`spawn_planes`], which wants a startup
    /// schedule and a map that has already flushed its commands.
    fn looking(back: f32) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(ShowGrid(true));
        app.init_resource::<Apparent>();
        app.init_resource::<Reading>();
        app.init_resource::<Descended>();

        let map = app
            .world_mut()
            .spawn((BigSpace::default(), crate::space::galaxy_grid()))
            .id();
        app.insert_resource(Map(map));
        app.world_mut().spawn((
            OrbitCamera { radius: back, target_radius: back, ..default() },
            CellCoord::default(),
            Transform::default(),
        ));
        for fine in [true, false] {
            app.world_mut().spawn((
                InfiniteGrid,
                Ruler { fine, inside: false },
                CellCoord::default(),
                Visibility::Hidden,
                ChildOf(map),
            ));
        }

        app.add_systems(Update, rule);
        app.update();
        app
    }

    /// What one of the two planes came out as
    fn ruled(app: &mut App, fine: bool) -> (Visibility, f64, f32) {
        let mut planes = app.world_mut().query::<(
            &Ruler,
            &Visibility,
            &InfiniteGridSettings,
            &Transform,
        )>();
        let (_, visible, settings, transform) = planes
            .iter(app.world())
            .find(|(plane, ..)| plane.fine == fine)
            .expect("the plane was spawned");
        // Back out of the scale into the cell it was ruled in, in light
        // years, which is what everything here is actually about.
        (
            *visible,
            1. / settings.scale as f64 / space::LIGHT_YEAR,
            transform.translation.y,
        )
    }

    /// A camera looking at the galaxy is given a ruled plane
    ///
    /// The end of the whole thing. Every property above holds of arithmetic
    /// that nothing has yet been asked to run, and a ruling that is never
    /// made visible passes all of them.
    #[test]
    fn looking_at_the_galaxy_rules_a_plane() {
        let mut app = looking(100.);

        let (visible, cell, _) = ruled(&mut app, true);
        assert_eq!(visible, Visibility::Inherited, "the plane was not drawn");
        // A hundred light years back takes in about thirty eight of them, so
        // the ladder lands on cells of one.
        assert!((cell - 1.).abs() < 1e-6, "ruled cells of {cell} light years");
    }

    /// And numbers to read off it, in light years
    #[test]
    fn looking_at_the_galaxy_gives_numbers_to_read() {
        let app = looking(100.);

        let reading = app.world().resource::<Reading>();
        let ruled = reading.0.as_ref().expect("nothing was left to read");
        assert_eq!(ruled.unit, Unit::LightYears);
        assert!(ruled.strength > 0.);
        assert!(
            (ruled.step - 5.).abs() < 1e-6,
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

        let (_, _, altitude) = ruled(&mut app, true);
        assert_eq!(altitude, 0., "the plane sat {altitude}m off the galaxy");
    }

    /// The ruling holds at both ends of the zoom
    ///
    /// A scale that came out infinite, negative or nothing at all is one the
    /// shader rules with `fract` of a number that is not a number.
    #[test]
    fn the_ruling_holds_across_the_whole_zoom() {
        for back in [1e-6, 1., 1e3, 1e5] {
            let mut app = looking(back);
            for fine in [true, false] {
                let (_, cell, _) = ruled(&mut app, fine);
                assert!(
                    cell.is_finite() && cell > 0.,
                    "{back} ly back ruled cells of {cell}"
                );
            }
        }
    }

    /// Switched off, nothing is ruled and nothing is left to read
    #[test]
    fn a_grid_switched_off_draws_nothing() {
        let mut app = looking(100.);
        app.world_mut().resource_mut::<ShowGrid>().0 = false;
        app.update();

        let (visible, ..) = ruled(&mut app, true);
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
        let galaxy = Unit::LightYears.finest() * Unit::LightYears.metres();
        let system = Unit::LightSeconds.finest() * Unit::LightSeconds.metres();

        assert!(
            system < galaxy / 1e3,
            "the galaxy stops at {galaxy}m and a system at {system}m"
        );
    }
}
