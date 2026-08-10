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
//! [`crate::ruled`] draws all of it: the lines, the numbers painted along
//! them, the crosses that mark a place worth locating, the lines dropped to
//! the plane and the three numbers about each of those places. It also works
//! out how wide a cell is for a view of a given width, how far apart to put
//! the numbers, and what each of them is called. None of that names a length.
//!
//! What is here is which unit a space is measured in, where the ruler changes
//! hands as the camera descends into a system, how loudly the whole of it is
//! drawn, and what is worth locating. Questions about a galaxy rather than
//! about a ruler.
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
use crate::ruled::{
    self, Decade, DistanceUnit, EDGE_ON, FIGURES_ACROSS, Family, INK, Located,
    NUMBERED, Number, Numbered, Painted, Plane, Reading, RuledPlugin, drawn_at,
    numbering, ruling, snapped_to, ticked,
};
use crate::schedule::MapSet;
use crate::space::{self, Map};
use crate::systems::System;
use crate::systems::bodies::spawn::{ApparentSize, Body};
use crate::systems::selection::Selected;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;

pub fn plugin(app: &mut App) {
    // Cut from the face egui draws the bar in, which is the same face every
    // name on the map is drawn in: a number on the plane and a number in the
    // bar are then the one typeface.
    app.add_plugins(RuledPlugin {
        face: ruled::Face {
            bytes: epaint_default_fonts::HACK_REGULAR,
            family: "Hack",
        },
    });
    app.insert_resource(ShowGrid(true));
    app.insert_resource(ShowMiddle(true));
    app.insert_resource(ShowPicked(true));
    app.init_resource::<Bright>();
    app.init_resource::<RulerUnit>();
    app.init_resource::<RuledSystem>();
    // After the map itself, which is what the galaxy's planes hang from. The
    // resource naming it is inserted through a command, so it is not there to
    // be read until the schedule that queued it has ended.
    app.add_systems(PostStartup, spawn_planes);
    // In `Present`, which runs after `Camera` has settled where the camera is
    // standing. Everything here is worked out from that and nothing else.
    //
    // In `ruled::Ruling`, which is what everything the module draws over a
    // plane runs after: the reading written here is what it all reads.
    app.add_systems(
        Update,
        (rule, mark_out).in_set(MapSet::Present).in_set(ruled::Ruling),
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
pub enum RulerUnit {
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
/// Disjoint. The one is spent before the other begins, so that two rulings
/// which share no cell size are never on screen together, and between them is
/// a moment with nothing ruled at all.
///
/// `standing` is how much of the mark standing for the system is left, which
/// is what the map fades its contents in against. Following it means the ruler
/// changes hands on the same figure the sky does.
fn handover(standing: f32) -> (f32, f32) {
    (
        ((standing - 0.5) * 2.).clamp(0., 1.),
        ((0.5 - standing) * 2.).clamp(0., 1.),
    )
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
    /// Where the camera is, in [`Placement::unit`] and measured from whatever
    /// the space is measured from
    eye: DVec3,
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
            eye: self.eye,
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
            commands.entity(entity).insert(Located);
        }
    }
    for entity in &marked {
        if !wanted || picked.get(entity).is_err() {
            commands.entity(entity).remove::<Located>();
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
        eye: spoken(orbit.eye),
        // In metres, being a distance out through the world rather than a
        // distance across the plane. Past the far side of the view, so that
        // what fades is the horizon rather than what is looked at.
        reach: orbit.radius as f64 * space::LIGHT_YEAR * FADE_BEYOND,
        share,
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
    seen_as: Res<ApparentSize>,
    // The system the camera has descended into, if it has. It is the one
    // carrying a grid of its own, which it does only while its contents are
    // drawn. Its cells are a metre, which is what lets a plane be ruled in
    // light seconds at all.
    inside: Query<(Entity, &System, &Grid), Without<BigSpace>>,
    outside: Query<&Grid, With<BigSpace>>,
    mut planes: Query<PlaneParts>,
    asked: Res<RulerUnit>,
    middle: Res<ShowMiddle>,
    mut descended: ResMut<RuledSystem>,
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
    let out_among_them = seen_as.standing();

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
                unit_for(LIGHT_YEARS, *asked),
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
        visible.set_if_neq(Visibility::Inherited);
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

    /// The two spaces are never ruled at the same time
    ///
    /// A light year is `3.15576e7` light seconds, so the two ladders share no
    /// cell size at any zoom and two rulings at once are two rulings that
    /// disagree. The handover is disjoint rather than a crossfade, which is
    /// what this says and the only thing that makes it so.
    #[test]
    fn only_one_space_is_ever_ruled() {
        for step in 0..=200 {
            let standing = step as f32 / 200.;
            let (out, down) = handover(standing);
            assert!(
                out == 0. || down == 0.,
                "at {standing} the galaxy was drawn at {out} \
                 and a system at {down}"
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
        app.init_resource::<ApparentSize>();
        app.init_resource::<RuledSystem>();

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
