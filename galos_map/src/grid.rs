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
//! # What draws the numbers standing over it
//!
//! The three numbers about a place are text meshes hung in the world, and the
//! crosses that mark the place are gizmos. Both go through the same pass the
//! plane does, so an ink asked for here comes out the same whether it is
//! painted into the ruling or stood over it, and a star in front of a number
//! hides it the way it hides a line.
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
use crate::systems::labels::{
    FONT, MIN_DEPTH, SIZE, depth_of, screen_offset, world_per_pixel,
};
use crate::systems::selection::Selected;
use crate::ruled::{
    self, BARE, FIGURES_ACROSS, Family, INK, MAJOR, NONE, NUMBERED, Numbered,
    ASIDE, CROSS, CROWDS, EDGE_ON, LIFT, Painted, Plane, READS, Ruling,
    RuledPlugin, Unit, Word, drawn_at, faded, numbering, off_plane, ruling,
    snapped_to, ticked, told,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_rich_text3d::{
    Text3d, Text3dSegment, Text3dStyling, TextAnchor, TextAtlas,
};
use big_space::prelude::*;

pub fn plugin(app: &mut App) {
    // Cut from the face egui draws the bar in, which is the same face every
    // name on the map is drawn in: a number on the plane and a number in the
    // bar are then the one typeface.
    app.add_plugins(RuledPlugin { face: epaint_default_fonts::HACK_REGULAR });
    app.insert_resource(ShowGrid(true));
    app.insert_resource(ShowMiddle(true));
    app.insert_resource(ShowPicked(true));
    app.init_resource::<Bright>();
    app.init_resource::<Said>();
    app.init_resource::<Reading>();
    app.init_resource::<Descended>();
    app.init_resource::<Dropped>();
    app.init_resource::<Readouts>();
    // After the map itself, which is what the galaxy's planes hang from. The
    // resource naming it is inserted through a command, so it is not there to
    // be read until the schedule that queued it has ended.
    app.add_systems(PostStartup, spawn_planes);
    // In `Present`, which runs after `Camera` has settled where the camera is
    // standing. Everything here is worked out from that and nothing else.
    //
    // All three in `Update` rather than later, because the readouts are text
    // meshes: their meshes are built and their transforms propagated in
    // `PostUpdate`, so a transform written after that is a readout a frame
    // behind the plane it stands on, which slides around while the camera
    // moves and only lands once it stops.
    app.add_systems(
        Update,
        (rule, locate, readouts).chain().in_set(MapSet::Present),
    );
    // After the plane has been told where it stands, which is what turns the
    // middle of the view into a crossing.
    app.add_systems(PostUpdate, stand_clear.after(ruled::Placing));
    // The gizmos are drawn where the world is, and a `GlobalTransform` under a
    // floating origin is not computed until `PostUpdate`. Read any earlier the
    // camera's is last frame's, and a line drawn from this frame's offset to
    // last frame's eye is a line that misses. Same reason
    // [`crate::systems::labels::leaders`] runs here.
    app.add_systems(PostUpdate, marks.after(TransformSystems::Propagate));
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
pub struct Bright(pub f32);

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
pub struct ShowPicked(pub bool);

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

/// What a plane's numbers are said in, out among the systems
const LIGHT_YEARS: Unit = Unit { metres: space::LIGHT_YEAR, mark: "Ly" };

/// And once the camera has descended into one
const LIGHT_SECONDS: Unit = Unit { metres: space::LIGHT_SECOND, mark: "Ls" };

/// The finest cell a plane hanging in `grid` may be ruled in, said in `unit`
///
/// Where the ladder stops. Out among the systems that is arithmetic, the grid
/// running out of places to put a line, [`ruled::finest`]. Inside one it is
/// taste, the grid having room to spare.
fn finest(unit: Unit, grid: &Grid) -> f64 {
    let placed = ruled::finest(grid) * STEADY / unit.metres;
    if unit == LIGHT_SECONDS { placed.max(FINEST_SYSTEM_CELL) } else { placed }
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
        Said::LightYears => LIGHT_YEARS,
        Said::LightSeconds => LIGHT_SECONDS,
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

/// What the lines dropped to the plane are about
///
/// One entry for each thing picked out.
///
/// Worked out by [`locate`] in `Update`, where the reading it is measured
/// against has just been settled, and read twice afterwards: by [`readouts`],
/// which writes the numbers, and by [`marks`], which draws the lines
/// themselves once the world has finished moving.
#[derive(Resource, Default)]
struct Dropped(Vec<Drop>);

/// One line dropped to the plane, and what it is about
struct Drop {
    /// Where the thing itself stands, as an offset from the camera's eye, in
    /// metres
    ///
    /// The head of the line, the other end being straight below it on the
    /// plane.
    top: DVec3,
    /// Where its foot stands on the plane, likewise
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
/// Settled by [`rule`], where the planes are placed, and read by everything
/// that locates a place on the plane, letters it or draws to it. Held rather
/// than worked out once per reader, some of them running in a later schedule
/// than the one that decides it.
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
    /// What those are measured from, in absolute galactic light years
    ///
    /// The galactic centre or the star. Anything else on the map is somewhere
    /// in absolute galactic light years too, and this is what says it in the
    /// same terms the rulers do.
    from: DVec3,
    /// Where the camera's eye is, in [`Ruled::unit`] and measured from
    /// [`Ruled::from`]
    ///
    /// What turns a place in this space into an offset from the eye, which is
    /// where everything the map draws about the plane is hung.
    eye: DVec3,
    /// How far apart two numbers are, in [`Ruled::unit`]
    step: f64,
    unit: Unit,
    /// How much of the ruling is drawn, which the numbers follow
    ///
    /// A number standing over a plane that has faded out is a number about
    /// nothing.
    strength: f32,
    /// How far the ruling reaches before it has faded out, in metres
    ///
    /// What [`faded`] measures a point against, and the plane's own
    /// [`Plane::reach`] said in the unit a reading is held in.
    reach: f64,
}

impl Ruled {
    /// Where something in this space lies from the camera's eye, in metres
    fn seen_from_eye(&self, place: DVec3) -> DVec3 {
        (place - self.eye) * self.unit.metres
    }
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
    /// And what that is, in absolute galactic light years
    ///
    /// The galactic centre out among the systems and the star once the camera
    /// has descended into one. What turns anything else on the map into a
    /// position this space can say.
    from: DVec3,
    /// How far its ruling reaches before it has faded out, in metres
    reach: f64,
    /// How much of this space's ruling is drawn, as the descent hands the map
    /// from one space to the other
    handed: f32,
}

impl Placement<'_> {
    /// Where something in this space lies from the camera's eye, in metres
    fn seen_from_eye(&self, place: DVec3) -> DVec3 {
        (place - self.eye) * self.unit.metres
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
            from: self.from,
            eye: self.eye,
            step: self.step,
            unit: self.unit,
            strength: self.showing(),
            reach: self.reach,
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
        |place: DVec3| (place - from) * space::LIGHT_YEAR / unit.metres;
    let across = across * space::LIGHT_YEAR / unit.metres;

    let ruling = ruling(across, finest(unit, grid));
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
        from,
        // In metres, being a distance out through the world rather than a
        // distance across the plane. Past the far side of the view, so that
        // what fades is the horizon rather than what is looked at.
        reach: orbit.radius as f64 * space::LIGHT_YEAR * FADE_BEYOND,
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
    bright: Res<Bright>,
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
                said_in(LIGHT_YEARS, *said),
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
                said_in(LIGHT_SECONDS, *said),
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
            space.at.y * space.unit.metres,
            0.,
        ));
        cell.set_if_neq(at_cell);
        transform.translation = at;

        *plane = Plane {
            cell: space.ruling.fine * space.unit.metres,
            families: space.ruling.rows(space.handed).map(|row| Family {
                strength: drawn_at(row.strength, bright.0),
                ..row
            }),
            numbers: Painted {
                // The crossings that carry a number are the ones the numbers
                // were already stepped by, so a number falls on a line and
                // there are about as many across the view as fit.
                apart: (space.step / space.ruling.fine) as f32,
                tall: (space.across / space.ruling.fine / FIGURES_ACROSS)
                    as f32,
                strength: drawn_at(INK * space.showing(), bright.0),
                // Written by `ruled::place`, which settles where the ruling is
                // measured from and which way the camera is standing.
                from: plane.numbers.from,
                upright: plane.numbers.upright,
                downward: plane.numbers.downward,
                // Written by `stand_clear`, which runs later in the frame,
                // once the names have settled which of them are drawn.
                bare: plane.numbers.bare,
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
        // map's own units, and the answers are the same ones [`readouts`]
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
            spoken.along[into] = Word::say(&ticked(along, space.step));
            spoken.across[into] = Word::say(&ticked(across, space.step));
        }
    }
}

/// What wears a mark, of the two kinds of thing that can
type Marked = (With<Selected>, Or<(With<System>, With<Body>)>);

/// Everything a marked thing is asked for
///
/// Where it is placed, whether it is drawn at all, and what its position is
/// measured from. A system carries its own and a body carries its star's.
type Mark = (
    Entity,
    &'static CellCoord,
    &'static Transform,
    &'static ViewVisibility,
    Option<&'static System>,
    Option<&'static ChildOf>,
);

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

/// Work out what the plane is worth marking, and where those places stand
///
/// The two rulers give a place on the plane, and a line dropped to it gives
/// the height above it. That is the third of the three numbers and the one a
/// plane on its own cannot say. Dropped from everything picked out; the point
/// the camera is looking at needs none, the plane running through it.
///
/// Where a thing stands is asked of the thing itself rather than measured out
/// from the camera. A cell is an `i64` count and a transform is the offset
/// inside it, so this is the position and has nothing of the camera in it.
///
/// Measured out from the camera it would have: the camera is at one float's
/// remove and the thing at another, and neither remove cancels the other. It
/// comes to a ten thousandth of the last place written, which is nothing at all
/// until the number sits on a rounding boundary — and a coordinate stored in
/// thirty seconds of a light year sits on one about a third of the time. Then
/// it turns over and back as the camera swings.
fn locate(
    showing: Res<ShowGrid>,
    picked_out: Res<ShowPicked>,
    reading: Res<Reading>,
    mut dropped: ResMut<Dropped>,
    // Whatever is picked out, which out among the systems is a system and
    // inside one is a body. Both stand off the plane and both are asked the
    // same question by it, so both are answered.
    //
    // Whether it is drawn as well as picked out. A system the map has hidden,
    // because a filter excluded it or because the camera has come down into it
    // and its bodies have taken over, is not there to be located; a line
    // dropped from where it would have stood is a line about nothing. Settled
    // in `PostUpdate`, so this is last frame's answer, which is a frame of a
    // line about something that has only just gone.
    picked: Query<Mark, Marked>,
    // Whatever a body is hanging in, for the star it is measured from.
    stars: Query<&System>,
    grids: Grids,
) {
    dropped.0.clear();
    if !showing.0 || !picked_out.0 {
        return;
    }
    let Some(ruled) = &reading.0 else { return };

    for (entity, cell, transform, shown, own, under) in &picked {
        if !shown.get() {
            continue;
        }
        let Some(grid) = grids.parent_grid(entity) else { continue };

        // In absolute galactic light years first, because what a grid places a
        // thing from is not what the numbers are measured from. A system hangs
        // in the galaxy's grid and is placed from the galactic centre, and the
        // rulers around it are measured from the star the camera has descended
        // into. Read straight out of the grid, a marked system says where it
        // stands from the middle of the galaxy in light seconds.
        let absolute = match own {
            Some(system) => system.position(),
            // A body hangs in the grid of the system it is in, so what it
            // carries is already its offset from that star.
            None => {
                let Some(star) =
                    under.and_then(|under| stars.get(under.parent()).ok())
                else {
                    continue;
                };
                star.position()
                    + (grid.cell_to_float(cell)
                        + transform.translation.as_dvec3())
                        / space::LIGHT_YEAR
            }
        };
        let at =
            (absolute - ruled.from) * space::LIGHT_YEAR / ruled.unit.metres;

        let top = ruled.seen_from_eye(at);
        let foot = DVec3::new(top.x, ruled.from_eye.y, top.z);
        dropped.0.push(Drop { top, foot, middle: (top + foot) / 2., at });
    }
}

/// One of the numbers the map stands over the plane
///
/// Text in the world rather than painted on the screen afterwards, so that a
/// number is drawn into the same pass the plane is, goes through the same
/// tonemapping, and comes out at the strength it was asked for. It is also
/// what lets a star stand in front of one.
#[derive(Component)]
struct Readout;

/// The readouts there are, in the order they are handed work
///
/// An ordered list rather than a query, so that the same readout goes on
/// saying the same thing from one frame to the next. Walked in query order the
/// numbers would swap between entities whenever an archetype moved, and a
/// number that changes which mesh it is drawn from is a number that flickers.
///
/// Pooled rather than made and unmade with the selection. A readout with
/// nothing to say is hidden, and the pool only grows.
#[derive(Resource, Default)]
struct Readouts(Vec<Entity>);

/// One number standing over the plane, and where it stands
struct Says {
    /// The place it is about, as an offset from the camera's eye, in metres
    from_eye: DVec3,
    /// Which way it is hung off that place, and how far, in pixels
    ///
    /// A direction through the world and a length on screen. What the length
    /// comes to in the world is worked out where the readout is placed, from
    /// how deep into the view the place lies.
    hung: Vec3,
    /// Which side of the place it stands on
    anchor: TextAnchor,
    said: String,
    /// The ink it is written in, before [`Bright`] and the ruling's own
    /// strength have had their say
    ink: f32,
}

/// Everything the map has to say about the plane this frame
///
/// The middle of the view first, then each thing picked out: where it stands,
/// and how far off the plane it went. In a settled order, so that a readout
/// goes on saying what it said last frame while nothing has changed.
///
/// `sideways` is which way the right of the view runs through the world.
fn spoken(
    ruled: &Ruled,
    dropped: &Dropped,
    middle: bool,
    sideways: Vec3,
) -> Vec<Says> {
    // What the plane can say about one place on it, hung under the mark there.
    // The middle of the view and the foot of every dropped line are the same
    // kind of thing, a place on the plane worth locating, so they are said the
    // same way.
    let placed = |from_eye: DVec3, at: DVec3| Says {
        from_eye,
        // Along the one direction neither ruler runs in, and under the plane
        // rather than over it, which is the opposite side from the one a pair
        // on the plane is written on. The two are then on either side of the
        // lines they are both about. Squared up on the plane it comes to
        // nothing on screen and the row sits on its own mark, which is a view
        // with no room for a third number in it anyway.
        hung: Vec3::NEG_Y * LIFT,
        anchor: TextAnchor::CENTER,
        said: format!("{} {}", told(at, ruled.step), ruled.unit.mark),
        ink: INK * faded(from_eye, ruled.reach, EDGE_ON),
    };

    let mut says = Vec::new();

    // The place the camera is looking at, all three of it, held at the middle
    // of the view. Not snapped to anything, so it sits still while the plane
    // slides under it, and said to the same step the plane is numbered in so
    // that it reads against those numbers.
    if middle {
        says.push(placed(ruled.middle_from_eye, ruled.at));
    }

    // And every line dropped to the plane, which is the same three numbers
    // about something that is not at the middle. They are said in full under
    // the mark at the line's foot, where the two rulers can be read against
    // them; the line itself carries only how far off the plane it went, which
    // is the one thing about it neither ruler nor mark can show.
    for drop in &dropped.0 {
        says.push(placed(drop.foot, drop.at));

        let Some(said) =
            off_plane(drop.at.y - ruled.at.y, ruled.step, ruled.unit)
        else {
            continue;
        };
        says.push(Says {
            from_eye: drop.middle,
            // Beside the line rather than over it, for the same reason a pair
            // on the plane stands beside its crossing: a number with a rule
            // through it is a number to be worked out rather than read.
            hung: sideways * ASIDE,
            anchor: TextAnchor::CENTER_RIGHT,
            said,
            ink: INK * faded(drop.foot, ruled.reach, EDGE_ON),
        });
    }

    says
}

/// Everything one readout is written through
///
/// Where it stands, what it says, which side of its place it says it on,
/// whether it says anything at all, and what it is drawn in.
type Written = (
    &'static mut Transform,
    &'static mut Text3d,
    &'static mut Text3dStyling,
    &'static mut Visibility,
    &'static MeshMaterial3d<StandardMaterial>,
);

/// Stand the numbers the plane is worth over the places they are about
///
/// Each is a child of the camera turned no further, which is what makes it
/// face the camera however the camera swings, and held at [`READS`] pixels
/// tall whatever it is standing over.
#[allow(clippy::too_many_arguments)]
fn readouts(
    showing: Res<ShowGrid>,
    middle: Res<ShowMiddle>,
    bright: Res<Bright>,
    reading: Res<Reading>,
    dropped: Res<Dropped>,
    mut pool: ResMut<Readouts>,
    cameras: Query<(Entity, &OrbitCamera, &Camera)>,
    mut written: Query<Written, With<Readout>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let seen = cameras.single().ok().and_then(|(eye, orbit, camera)| {
        let viewport = camera.logical_viewport_size()?;
        Some((eye, orbit, viewport, camera.clip_from_view().y_axis.y))
    });

    let mut says = Vec::new();
    let mut strength = 0.;
    if let (true, Some(ruled), Some((_, orbit, ..))) =
        (showing.0, reading.0.as_ref(), seen)
    {
        strength = ruled.strength;
        says = spoken(ruled, &dropped, middle.0, orbit.rotation * Vec3::X);
    }

    // Room for everything there is to say. One made now is not in the world
    // until the commands are flushed, so it says nothing until the next frame;
    // what is picked out changes far more slowly than the map is drawn.
    if let Some((eye, ..)) = seen {
        while pool.0.len() < says.len() {
            pool.0.push(commands.spawn(readout(&mut materials, eye)).id());
        }
    }

    for (nth, entity) in pool.0.iter().enumerate() {
        let Ok((mut place, mut text, mut styling, mut visible, painted)) =
            written.get_mut(*entity)
        else {
            continue;
        };
        let seen = says.get(nth).zip(seen);
        // Nothing to say, or a plane so far faded where it would have stood
        // that saying it would be saying it about nothing.
        let ink = seen
            .map_or(0., |(says, _)| drawn_at(says.ink * strength, bright.0));
        let Some((says, (_, orbit, viewport, cot_half_fov))) =
            seen.filter(|_| ink > 0.)
        else {
            visible.set_if_neq(Visibility::Hidden);
            continue;
        };
        visible.set_if_neq(Visibility::Inherited);

        let into_view = depth_of(orbit, says.from_eye).max(MIN_DEPTH);
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, into_view);
        // Onto the camera's own axes, the readout hanging off it. Turned no
        // further than its parent it faces the camera, and a length written
        // here is a length across the view.
        place.translation = orbit.rotation.inverse()
            * (says.from_eye.as_vec3() + says.hung * per_pixel);
        place.rotation = Quat::IDENTITY;
        // The line box is exactly `SIZE` tall, so this is the height the row
        // draws at, in pixels, whatever the camera is doing.
        place.scale = Vec3::splat(READS * per_pixel / SIZE);

        // Both of these are read before they are written, so that a readout
        // saying what it said last frame does not have its mesh rebuilt for
        // it. Most frames say what the last one did.
        if lettered(&text) != Some(says.said.as_str()) {
            *text = Text3d::new(says.said.clone());
        }
        if styling.anchor.0 != says.anchor.0 {
            styling.anchor = says.anchor;
        }

        if let Some(mut painted) = materials.get_mut(&painted.0) {
            painted.base_color = LINE.with_alpha(ink);
        }
    }
}

/// What one readout is made of
///
/// A material apiece rather than a handful shared out. What a readout is drawn
/// at follows how far the plane has faded where it stands, so no two of them
/// are alike and a shared one would come out however the last to write it left
/// it.
fn readout(
    materials: &mut Assets<StandardMaterial>,
    eye: Entity,
) -> impl Bundle {
    (
        Readout,
        Text3d::new(String::new()),
        Text3dStyling {
            size: SIZE,
            font: FONT.into(),
            color: Srgba::WHITE,
            anchor: TextAnchor::CENTER,
            ..default()
        },
        Mesh3d::default(),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: LINE.with_alpha(0.),
            // The glyphs are drawn white and unlit, so the base color
            // multiplies straight through them and is what a readout comes
            // out.
            base_color_texture: Some(TextAtlas::DEFAULT_IMAGE.clone()),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        })),
        // Nothing is said until [`readouts`] has been round.
        Visibility::Hidden,
        Transform::default(),
        ChildOf(eye),
    )
}

/// What a readout says, if it says anything this wrote
///
/// A readout is one run of text, [`readout`] having set it that way, and
/// anything else is not a readout this put up.
fn lettered(text: &Text3d) -> Option<&str> {
    match text.segments.as_slice() {
        [(Text3dSegment::String(said), _)] => Some(said),
        _ => None,
    }
}

/// Scratch the plane where a place on it is worth locating
///
/// A cross at the middle of the view and at the foot of every dropped line,
/// laid along the plane's own axes, and the dropped lines themselves. A number
/// written over a plane with nothing under it is a number floating loose.
///
/// Gizmos rather than meshes, every one of them moving every frame. Which puts
/// them in the same pass as the plane, so a line dropped to the ruling is
/// drawn exactly as the ruling's own lines are.
fn marks(
    showing: Res<ShowGrid>,
    middle: Res<ShowMiddle>,
    bright: Res<Bright>,
    reading: Res<Reading>,
    dropped: Res<Dropped>,
    mut gizmos: Gizmos,
    cameras: Query<(&OrbitCamera, &Camera, &GlobalTransform)>,
) {
    if !showing.0 {
        return;
    }
    let Some(ruled) = &reading.0 else { return };
    let Ok((orbit, camera, at)) = cameras.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;
    let eye = at.translation();

    // Where a place on the plane is in the world, how long an arm of its cross
    // comes to there, and what the two are drawn in.
    let scratched = |from_eye: DVec3, ink: f32| {
        let into_view = depth_of(orbit, from_eye).max(MIN_DEPTH);
        (
            eye + from_eye.as_vec3(),
            CROSS * world_per_pixel(cot_half_fov, viewport.y, into_view),
            LINE.with_alpha(drawn_at(ink * ruled.strength, bright.0)),
        )
    };

    if middle.0 {
        let ink = INK * faded(ruled.middle_from_eye, ruled.reach, EDGE_ON);
        let (at, arm, color) = scratched(ruled.middle_from_eye, ink);
        cross(&mut gizmos, at, arm, color);
    }

    for drop in &dropped.0 {
        let left = faded(drop.foot, ruled.reach, EDGE_ON);
        let (foot, arm, color) = scratched(drop.foot, INK * left);
        cross(&mut gizmos, foot, arm, color);
        // The line at the ink the ruling's widest lines are drawn in, so that
        // it reads as one of the plane's rather than as something laid over
        // it.
        gizmos.line(
            eye + drop.top.as_vec3(),
            foot,
            LINE.with_alpha(drawn_at(MAJOR * ruled.strength * left, bright.0)),
        );
    }
}

/// Two arms along the plane's own axes, crossing at `at`
///
/// Laid in the plane rather than across the screen, so a cross out towards the
/// horizon is foreshortened the way the cells around it are.
fn cross(gizmos: &mut Gizmos, at: Vec3, arm: f32, color: Color) {
    for axis in [Vec3::X, Vec3::Z] {
        gizmos.line(at - axis * arm, at + axis * arm, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ruled::ladder::tests::zooms;
    use crate::ruled::snapped;

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

    /// What is drawn over the plane fades the way the plane fades
    ///
    /// The same arithmetic `ruled.wgsl` does per fragment, worked out here for
    /// one point, so that what is drawn over the plane by hand goes as what is
    /// drawn into it goes.
    #[test]
    fn what_is_written_fades_with_what_it_is_written_over() {
        let reach = 60.;
        // Straight down onto the plane, which is where nothing fades: the
        // distance term is softened away entirely as the view squares up.
        assert_eq!(faded(DVec3::new(0., -10., 0.), reach, EDGE_ON), 1.);
        // And level with it, where the plane is a line across the sky.
        assert_eq!(faded(DVec3::new(10., 0., 0.), reach, EDGE_ON), 0.);
        // Between the two it carries both terms. Half of `EDGE_ON` from
        // the plane, ten out of sixty along, comes to five sixths of the
        // distance left and half of that for being edge on.
        let square = f64::from(EDGE_ON) / 2.;
        let low = DVec3::new((1. - square * square).sqrt(), -square, 0.) * 10.;
        assert!(
            (faded(low, reach, EDGE_ON) - 0.427_08).abs() < 1e-4,
            "came out {}",
            faded(low, reach, EDGE_ON)
        );
        // Out at the reach the distance term has run out, and what is left is
        // what squaring up put back.
        let out = DVec3::new(0., -reach, 0.);
        assert_eq!(faded(out, reach, EDGE_ON), 1.);
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

    /// A number standing over the plane is the ink the plane paints its own in
    ///
    /// The same numbers about the same plane, and now drawn into the same pass
    /// as it, so an equal ink reaches the eye equally. What is left between
    /// them is where each stands: the plane's are painted all over it and fade
    /// wherever they lie, and one standing over it takes the fade at its own
    /// place. Nothing else.
    #[test]
    fn what_stands_over_the_plane_is_the_ink_painted_on_it() {
        let mut app = looking(100.);
        // Standing back and up from what it is looking at, so that the plane
        // is neither square on nor edge on and there is a fade in it to tell
        // the two apart by.
        let mut cameras = app.world_mut().query::<&mut OrbitCamera>();
        for mut orbit in cameras.iter_mut(app.world_mut()) {
            orbit.eye = DVec3::new(0., 50., 75_f64.sqrt() * 10.);
        }
        app.update();
        let (_, painted) = drawn(&mut app);

        let bright = app.world().resource::<Bright>().0;
        let reading = app.world().resource::<Reading>();
        let ruled = reading.0.as_ref().expect("a reading to write");
        let says = spoken(ruled, &Dropped::default(), true, Vec3::X);
        let middle = says.first().expect("the middle is said");

        let stood = drawn_at(middle.ink * ruled.strength, bright);
        let left = faded(ruled.middle_from_eye, ruled.reach, EDGE_ON);
        assert!(left > 0. && left < 1., "nothing to tell apart at {left}");
        assert!(
            (stood - painted * left).abs() < 1e-6,
            "stood at {stood}, painted {painted} with {left} of the plane left"
        );
    }

    /// A thing off the plane is said at its foot and again on its line
    ///
    /// Three numbers under the mark where the line meets the plane, which is
    /// where the two rulers can be read against them, and how far off the
    /// plane it went beside the line itself. That last is the one thing about
    /// it neither ruler nor mark can show.
    #[test]
    fn a_dropped_line_is_said_at_both_ends() {
        let mut app = looking(100.);
        app.update();
        let reading = app.world().resource::<Reading>();
        let ruled = reading.0.as_ref().expect("a reading to write");

        // A step off the middle both ways, so that neither number reads as
        // nought and the offset is worth saying out loud.
        let at = ruled.at + DVec3::new(ruled.step, ruled.step, 0.);
        let top = ruled.seen_from_eye(at);
        let foot = DVec3::new(top.x, ruled.from_eye.y, top.z);
        let dropped =
            Dropped(vec![Drop { top, foot, middle: (top + foot) / 2., at }]);

        let says = spoken(ruled, &dropped, true, Vec3::X);
        assert_eq!(says.len(), 3, "the middle, the foot and the offset");
        // The foot says all three, and it says them where the foot stands.
        assert_eq!(says[1].from_eye, foot);
        assert_eq!(says[1].said.matches(',').count(), 2);
        // The line says only how far off the plane it went, and says it
        // halfway up itself where there is a line to stand beside.
        assert_eq!(says[2].from_eye, (top + foot) / 2.);
        assert!(
            says[2].said.starts_with('+'),
            "a step above the plane came out {}",
            says[2].said
        );
        assert!(says[2].said.ends_with(ruled.unit.mark));
    }

    /// And nothing is said about the middle when the middle is switched off
    #[test]
    fn the_middle_goes_quiet_when_it_is_not_asked_for() {
        let mut app = looking(100.);
        app.update();
        let reading = app.world().resource::<Reading>();
        let ruled = reading.0.as_ref().expect("a reading to write");

        assert!(spoken(ruled, &Dropped::default(), false, Vec3::X).is_empty());
        assert_eq!(spoken(ruled, &Dropped::default(), true, Vec3::X).len(), 1);
    }

    /// How strongly the plane's widest drawn row and its numbers come out
    fn drawn(app: &mut App) -> (f32, f32) {
        let mut planes = app.world_mut().query::<&Plane>();
        let plane = planes.iter(app.world()).next().expect("the plane");
        let lines = plane
            .families
            .iter()
            .map(|row| row.strength)
            .fold(0., f32::max);
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
        assert_eq!(
            said_in(LIGHT_YEARS, Said::Whichever),
            LIGHT_YEARS
        );
        assert_eq!(
            said_in(LIGHT_SECONDS, Said::Whichever),
            LIGHT_SECONDS
        );
        // And either may be pinned from the bar.
        assert_eq!(said_in(LIGHT_SECONDS, Said::LightYears), LIGHT_YEARS);
        assert_eq!(said_in(LIGHT_YEARS, Said::LightSeconds), LIGHT_SECONDS);
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
                let unit = said_in(space, Said::Whichever);
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
        assert_eq!(LIGHT_YEARS.mark, "Ly");
        assert_eq!(LIGHT_SECONDS.mark, "Ls");
        assert!(LIGHT_SECONDS.metres < LIGHT_YEARS.metres);
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






