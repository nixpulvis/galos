//! Drawing what is inside a system
//!
//! A body is drawn at the size it is, in the metres the map is laid out in,
//! placed where its orbit puts it. Nothing here is exaggerated — the shell
//! standing for the system is what carries the exaggeration, and it is drawn
//! elsewhere.
//!
//! # Light
//!
//! Real, and physical, which is what re-basing the map to metres bought. A
//! star is given a [`PointLight`] of however many lumens its size and
//! temperature come to, so a planet at an Earth's distance from a Sun sees an
//! Earth's daylight and the far side of it is dark. In light years none of
//! that could be said: bevy's lighting is in lumens and lux, and a unit that
//! large sends every one of them out of range.
//!
//! Bodies keep a little emission of their own regardless, so that a system
//! whose star is not on record is dim rather than invisible.
//!
//! # The ladder
//!
//! Four figures decide what is drawn for a system as the camera comes in, and
//! they are four points on one axis: how much of the sky the system takes up,
//! which is its own reach over how far off it is. Held in that order, and
//! declared in it, so a figure out of place is a number out of order:
//!
//! | | |
//! |---|---|
//! | [`WORTH_KEEPING`] | 0.008, what is inside is taken away again |
//! | [`WORTH_DRAWING`] | 0.01, what is inside is drawn |
//! | [`WORTH_MARKING`] | 0.0125, the mark standing for it starts to go |
//! | [`WORTH_HIDING`] | 0.05, there is nothing of the mark left |
//!
//! So the contents arrive and leave behind a mark at full strength, and what
//! is watched is one thing becoming another, which
//! `the_contents_come_and_go_before_the_mark_gives_way` holds them to.

use super::{
    Clock, Contents,
    orbit::{Orbits, Spacing},
};
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::space;
use crate::systems::System;
use crate::systems::pointing::Indicator;
use crate::systems::roundness::Roundness;
use crate::systems::route::{LineList, LineStrip};
use bevy::ecs::system::SystemParam;
use bevy::light::NotShadowCaster;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use galos_db::bodies::Body as DbBody;
use galos_db::stars::Star as DbStar;
use std::collections::HashSet;
use std::f64::consts::PI;

pub fn plugin(app: &mut App) {
    app.init_resource::<DrawnContents>();
    app.init_resource::<HeldSystem>();
    app.insert_resource(ShowOrbits(true));
    app.add_systems(Startup, init_materials);
    // After the rows have been taken in, so that a system's contents can be
    // drawn on the frame they land rather than the one after.
    app.add_systems(
        Update,
        draw.in_set(MapSet::Populate).after(super::fetch::collect),
    );
    // After the lines are spawned, so one drawn this frame is hidden on this
    // frame rather than being shown once and taken away.
    app.add_systems(Update, show_orbits.in_set(MapSet::Present));
    // After whatever it moves exists. A body spawned this frame is already
    // standing where the clock says, `draw` having read the same clock.
    app.add_systems(Update, wind.in_set(MapSet::Populate).after(draw));
    // After the camera has settled where it is standing, that being what says
    // how much of a ring to lay out.
    app.add_systems(
        Update,
        redash.in_set(MapSet::Present).after(crate::camera::orbit_camera),
    );
    // Reads where the camera came to rest, which `Camera` settles, and is read
    // by everything drawn in `Present`.
    app.add_systems(
        Update,
        fade.in_set(MapSet::Camera).after(crate::camera::orbit_camera),
    );
}

/// Whether the lines a system's contents trace are drawn
#[derive(Resource)]
pub struct ShowOrbits(pub bool);

/// The line one thing traces about whatever it goes round
///
/// Its own component rather than [`Inside`], which the bodies wear too. What is
/// asked of these is asked of the lines alone.
#[derive(Component)]
pub struct OrbitLine {
    /// Which body the ellipse is drawn about, where it is not the middle
    ///
    /// An ellipse sits on whatever its thing goes round, so a moon's is drawn
    /// at its planet and moves when the planet does. Carried because winding
    /// the clock on has to put the line back where its anchor has gone, and the
    /// line is a flat child of the system like everything else: nothing moves
    /// it by inheritance.
    pub about: Option<i16>,
    /// Where the ring's own points are measured from, about whatever it goes
    /// round
    ///
    /// Which is where the thing riding the line stood when it was drawn. The
    /// points are held as offsets from there so that the ones near what rides
    /// the line are small numbers: a mesh is `f32`, and the ring Pluto and
    /// Charon go round reaches 5.9e12 metres, where one float stands 524
    /// kilometres from the next. Measured from the parent instead, the whole
    /// line snaps between those as the camera moves, a quarter of Pluto's own
    /// orbit at a step, and rotating close in on the pair sweeps it through a
    /// couple of hundred of them.
    pub pin: DVec3,
    /// Which ring this is, where it is drawn in dashes
    ///
    /// A dashed ring is laid closest where the camera stands, so it has to be
    /// laid again as the camera moves and has to say which ring to ask about.
    /// [`None`] for a line drawn whole, which is laid about the thing riding it
    /// and left alone.
    pub dashed: Option<i16>,
    /// How its points were laid, when they last were
    ///
    /// What [`redash`] compares against to know whether the camera has moved far
    /// enough to be worth laying them again.
    pub spacing: Spacing,
}

/// Draw the orbit lines, or do not, as the view asks
///
/// Every line every frame rather than only when the answer changes, since a
/// line spawned while they are turned off has to be caught as well. There is
/// one system's worth of them, and nothing is written where nothing moved.
fn show_orbits(
    showing: Res<ShowOrbits>,
    mut lines: Query<&mut Visibility, With<OrbitLine>>,
) {
    let wanted =
        if showing.0 { Visibility::Inherited } else { Visibility::Hidden };

    for mut visibility in &mut lines {
        visibility.set_if_neq(wanted);
    }
}

/// How small a system may look before what is in it is taken away again, in
/// radians
///
/// Under [`WORTH_DRAWING`], so a camera sitting on the line does not spawn and
/// despawn a system's insides every frame. Under [`WORTH_MARKING`] as well, so
/// that the taking away happens behind a whole mark as the drawing did.
const WORTH_KEEPING: f32 = 0.008;

/// And how large it has to look before what is in it is drawn
///
/// About half a degree, at which the whole system is some twenty pixels across
/// and everything in it is a speck. Which is the point: the mark standing for
/// the system is still whole at this size and the contents arrive behind it,
/// so the one thing nobody should watch happen is not watched.
///
/// An angle rather than a distance, so that a system reaching light hours and
/// one reaching light seconds are both drawn when they are equally worth
/// looking at. The rows arrive far earlier — that is [`super::fetch`]'s
/// business — so nothing waits on the database at this range.
const WORTH_DRAWING: f32 = 0.01;

/// And how large before the mark standing for it starts to go
///
/// Past [`WORTH_DRAWING`], so what the mark stands in for is already there
/// when the mark begins to give way to it. The two bands do not overlap and
/// that is the whole of what they are for: everything arrives and leaves
/// behind a mark at full strength, and what is watched is one thing fading
/// into another.
const WORTH_MARKING: f32 = 0.0125;

/// And how large before there is nothing of the mark left
///
/// About three degrees, by which point the system fills a good part of the
/// view and a mark standing in for it would be standing over the thing itself.
/// Four times [`WORTH_MARKING`], which is a quarter of the distance, and long
/// enough that the fade reads as one thing becoming another.
///
/// The far end of the ladder, and the one figure outside this module reads:
/// [`crate::systems::scale::MARGIN`] is held under its inverse, so a mark is
/// gone before the camera can reach the shell it stood for.
pub(crate) const WORTH_HIDING: f32 = 0.05;

/// The system whose insides the map is drawing, if any
///
/// [`Contents`] says which by address, and outlives the entity: the spyglass
/// may drag a system off the map while the camera is still standing in it.
/// This is the entity, for whoever has to reach the system itself rather than
/// its rows, and it names the one system whose mark may go out.
#[derive(Resource, Default)]
pub struct HeldSystem(Option<Entity>);

impl HeldSystem {
    /// Which system that is, if the map is drawing one
    pub fn of(&self) -> Option<Entity> {
        self.0
    }
}

/// How strongly the mark standing for a system is drawn, from one to nothing
///
/// What is drawn, which follows what the distance asks at a bounded rate
/// rather than exactly. Read by everything that goes out with a system: its
/// shell, the rings around it, its name, the routes reaching it and the ruled
/// plane under it, so that the whole of it goes together.
#[derive(Component)]
pub struct Strength(pub f32);

impl Default for Strength {
    /// Whole, as a system too far off to be closed on is
    fn default() -> Self {
        Strength(1.)
    }
}

/// How long a mark takes to go out, in seconds
///
/// How much of a mark should be left is a question about how far off the
/// system is, and the camera is free to cross the whole band in a frame or
/// two. A flight covers four light years in ninety frames and spends three of
/// them inside the band; a drag flicked across the view is not far behind.
/// Only a scroll crosses it slowly, being multiplicative and asked for a click
/// at a time, and it is the one path that ever looked like a fade.
///
/// So the distance says what a mark should come to and this says how fast what
/// is drawn may follow it. Half a second: long enough to read as one thing
/// becoming another, short enough that a mark is not left standing over the
/// system it stands for.
///
/// A bound rather than a pace, so raising it slows only the paths that cross
/// the band faster than this. A camera coming in on the wheel is slower than
/// this the whole way and never touches it.
pub(crate) const GOES_OUT_IN: f32 = 0.5;

/// Draw every mark a step nearer what is wanted of it
///
/// Every system every frame, and written only where it moved, which past the
/// one being closed on is none of them.
fn fade(
    time: Res<Time<Real>>,
    camera: Query<&OrbitCamera>,
    holding: Res<HeldSystem>,
    mut systems: Query<(Entity, &System, &mut Strength)>,
) {
    let Ok(eye) = camera.single().map(|camera| camera.eye) else { return };
    let drawing = holding.of();
    let step = time.delta_secs() / GOES_OUT_IN;

    for (entity, system, mut standing) in &mut systems {
        // Only the one system whose insides the map is holding may give way to
        // them. Every other mark stands whole however near the camera comes:
        // there is nothing drawn behind it, so a mark going out there is a
        // system going out altogether. Alpha Centauri reaches a fifth of a
        // light year, and by its own measure its mark is nearly gone from Sol.
        let wanted = if Some(entity) == drawing {
            standing_for(system, eye)
        } else {
            1.
        };
        // A system arriving is drawn at whatever its distance asks rather than
        // fading in from whole. It is one the map has just been told about,
        // not one the camera has come up on.
        let drawn = if standing.is_added() {
            wanted
        } else {
            standing.0 + (wanted - standing.0).clamp(-step, step)
        };

        if standing.0 != drawn {
            standing.0 = drawn;
        }
    }
}

/// How much of the mark standing for `system` is left, from one to nothing,
/// seen from `eye`
///
/// The whole of it while the system is a speck, none of it once the system is
/// large enough to stand for itself, and the way between over
/// [`WORTH_MARKING`] to [`WORTH_HIDING`].
///
/// What the distance asks for rather than what is drawn, which is
/// [`Strength`]. Nothing outside [`fade`] wants this one, and it is asked only
/// of the system the map is holding: a mark may only go out where what it
/// stands for is drawn in its place.
pub(crate) fn standing_for(system: &System, eye: DVec3) -> f32 {
    let away = space::metres(eye - system.position()).length() as f32;

    fading(system.reach() / away.max(1.))
}

/// How much of a mark is left for something looking `seen` radians across
fn fading(seen: f32) -> f32 {
    let through = (seen - WORTH_MARKING) / (WORTH_HIDING - WORTH_MARKING);

    1. - through.clamp(0., 1.)
}

/// How far past the system a star's light is allowed to reach
///
/// As a multiple of how far the system reaches. Bevy eases the last of a
/// light away as its range is approached, so a range set at the outermost
/// orbit would draw the body standing there darker than it is; four times over
/// leaves that worth well under a percent anywhere anything stands.
///
/// A range at all, rather than the `f32::MAX` this was, because the range is
/// also the bounding sphere the renderer sorts lights into clusters by. That
/// sphere is projected to find which clusters it covers, and a radius of
/// `f32::MAX` puts infinities through the projection: which clusters the light
/// lands in then depends on where the camera happens to be pointing, and the
/// light comes and goes as it moves.
const LIGHT_REACH: f32 = 4.;

/// How many points an orbit is drawn with
///
/// Enough that the roundest orbit does not read as a polygon at the size an
/// orbit is ever looked at: laid evenly, a circle drawn five hundred pixels
/// across is off by a hundredth of one between its points.
///
/// Few enough that a system of a hundred bodies is some tens of thousands of
/// vertices rather than a mesh worth thinking about. Unlike a body's sphere
/// these are a mesh apiece, every orbit being its own ellipse, so the count is
/// paid per line. It is the whole budget for a ring however wide the ring is
/// against the view: what changes with the view is where along the ring the
/// points fall, not how many there are. See [`Spacing`].
const ORBIT_POINTS: usize = 512;

/// Which system's insides are drawn, and which answer about it they were drawn
/// from
///
/// The answer as well as the system, because the poll keeps asking after the
/// one being stood in and a scan running in the game fills it in as it goes.
/// What is on screen is the rows as they stood at one moment, so the moment is
/// recorded beside the system: without it a system flown into half scanned
/// stays half drawn for as long as the camera is in it.
#[derive(Resource, Default)]
struct DrawnContents(Option<(i64, u32)>);

/// Anything drawn because it is inside a system
///
/// One marker over stars, bodies and orbit lines alike, since what they have
/// in common is when they go away: all of them at once, when the camera
/// leaves.
#[derive(Component)]
pub struct Inside;

/// A star, a planet or a moon, drawn where its orbit puts it
///
/// Carries the row it was drawn from, so that whatever the pointer lands on
/// can say what it is without going back to the database.
#[derive(Component)]
pub struct Body {
    /// Which system it is in
    ///
    /// A body is a child of its system on the map, so this says nothing the
    /// tree does not. It is here because what picks a body out holds it as a
    /// value and has to find its way back: an id alone names a different body
    /// in every system, and one system's contents are drawn at a time.
    pub address: i64,
    /// What it is called
    pub name: String,
    /// Which of the system's numbering it is
    pub id: i16,
    /// What kind of thing it is, as the journal spells it
    pub class: String,
    /// How far across it is, in metres
    pub radius: f32,
    /// How many ancestors the scan named it under
    ///
    /// Nothing for the star a system arrives at, one for what goes round it,
    /// and one more for each step further down. What a parent names is a
    /// suffix of what its children name, so a parent always counts fewer than
    /// they do, and the smaller of any two on one chain is the one the other
    /// goes round.
    ///
    /// Which is most of what settles who answers when two of them are under
    /// the pointer at once, and which of them is named where two names would
    /// overlap. A moon crossing in front of its planet is not what was being
    /// aimed at.
    pub ancestors: u8,
    /// Whether it is the star the system arrives at
    ///
    /// The one the system is named for and the one everything else in it is
    /// measured from, so its name is the one worth keeping where two would
    /// overlap. A star rather than a body always, and the nearest to arrival
    /// where a system has several, which [`Contents::primary`] settles.
    pub primary: bool,
    /// Whether it is a star rather than something going round one
    ///
    /// What settles the rest of it. A system's stars and the planets that go
    /// round the pair of them are all children of the point at the middle and
    /// count the same ancestors, so the depth cannot tell them apart; a star
    /// is what the system is named for and what everything in it is lit by.
    pub star: bool,
}

/// What a star is drawn in, by the colour its class comes to
#[derive(Resource)]
struct StarMaterials(Vec<Handle<StandardMaterial>>);

/// What a body is drawn in, by the surface its class comes to
#[derive(Resource)]
struct BodyMaterials(Vec<Handle<StandardMaterial>>);

/// What an orbit's line is drawn in
#[derive(Resource)]
struct OrbitMaterial(Handle<StandardMaterial>);

/// The colours a star is drawn in
///
/// By temperature, which is what a star's class is a shorthand for. The
/// remnants and the oddities share one colour rather than being guessed at.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Glow {
    Blue,
    White,
    Yellow,
    Orange,
    Red,
    Odd,
}

impl Glow {
    const ALL: [Glow; 6] = [
        Glow::Blue,
        Glow::White,
        Glow::Yellow,
        Glow::Orange,
        Glow::Red,
        Glow::Odd,
    ];

    /// What the glow is painted in
    const fn color(self) -> Color {
        match self {
            Glow::Blue => Color::srgb(0.7, 0.8, 1.),
            Glow::White => Color::srgb(0.95, 0.95, 1.),
            Glow::Yellow => Color::srgb(1., 0.95, 0.75),
            Glow::Orange => Color::srgb(1., 0.75, 0.45),
            Glow::Red => Color::srgb(1., 0.5, 0.35),
            Glow::Odd => Color::srgb(0.6, 0.55, 0.7),
        }
    }

    /// Which colour a class of star comes to
    ///
    /// The first letter carries it for the ordinary sequence, hottest to
    /// coolest. What is left — dwarfs, neutron stars, holes, the carbon and
    /// Wolf-Rayet families — is not on that sequence at all, and is drawn as
    /// itself rather than as whatever letter it happens to start with.
    fn of(class: &str) -> Glow {
        let class = class.trim().to_uppercase();
        match class.as_str() {
            _ if class.starts_with("D") => Glow::White,
            _ if class.starts_with("N") || class.starts_with("H") => Glow::Odd,
            _ if class.starts_with("W") || class.starts_with("C") => Glow::Odd,
            _ if class.starts_with("SUPERMASSIVE") => Glow::Odd,
            _ if class.starts_with("O") || class.starts_with("B") => Glow::Blue,
            _ if class.starts_with("A") || class.starts_with("F") => {
                Glow::White
            }
            _ if class.starts_with("G") => Glow::Yellow,
            _ if class.starts_with("K") => Glow::Orange,
            _ if class.starts_with("M")
                || class.starts_with("L")
                || class.starts_with("T")
                || class.starts_with("Y") =>
            {
                Glow::Red
            }
            _ => Glow::Odd,
        }
    }
}

/// The colours a body is drawn in
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SurfaceLook {
    Earthlike,
    Water,
    Ammonia,
    Gas,
    Icy,
    Rocky,
    Metal,
    Unknown,
}

impl SurfaceLook {
    const ALL: [SurfaceLook; 8] = [
        SurfaceLook::Earthlike,
        SurfaceLook::Water,
        SurfaceLook::Ammonia,
        SurfaceLook::Gas,
        SurfaceLook::Icy,
        SurfaceLook::Rocky,
        SurfaceLook::Metal,
        SurfaceLook::Unknown,
    ];

    /// What the surface is painted in
    const fn color(self) -> Color {
        match self {
            SurfaceLook::Earthlike => Color::srgb(0.25, 0.5, 0.3),
            SurfaceLook::Water => Color::srgb(0.2, 0.4, 0.7),
            SurfaceLook::Ammonia => Color::srgb(0.7, 0.6, 0.35),
            SurfaceLook::Gas => Color::srgb(0.75, 0.65, 0.5),
            SurfaceLook::Icy => Color::srgb(0.8, 0.85, 0.9),
            SurfaceLook::Rocky => Color::srgb(0.45, 0.4, 0.35),
            SurfaceLook::Metal => Color::srgb(0.5, 0.45, 0.4),
            SurfaceLook::Unknown => Color::srgb(0.35, 0.35, 0.35),
        }
    }

    /// Which surface a class of body comes to
    ///
    /// Matched on what the phrase contains rather than on the whole of it. The
    /// journal spells a class out in words — "Sudarsky class III gas giant",
    /// "High metal content body" — and there are more of them than are worth
    /// listing, most differing in ways nothing here draws.
    ///
    /// Order matters, and in two places. A gas giant is named for what lives
    /// in it — "Gas giant with ammonia based life" — so it has to be caught
    /// before ammonia is, or every one of them comes out an ammonia world. And
    /// a "rocky ice body" contains both its materials, of which the ice is the
    /// one that shows.
    fn of(class: &str) -> SurfaceLook {
        let class = class.to_lowercase();
        if class.contains("earthlike") {
            SurfaceLook::Earthlike
        } else if class.contains("gas giant") {
            SurfaceLook::Gas
        } else if class.contains("water world") || class.contains("water giant")
        {
            SurfaceLook::Water
        } else if class.contains("ammonia") {
            SurfaceLook::Ammonia
        } else if class.contains("rocky ice") {
            SurfaceLook::Icy
        } else if class.contains("icy") || class.contains("ice") {
            SurfaceLook::Icy
        } else if class.contains("metal") {
            SurfaceLook::Metal
        } else if class.contains("rocky") {
            SurfaceLook::Rocky
        } else {
            SurfaceLook::Unknown
        }
    }
}

/// The Stefan-Boltzmann constant, in watts per square metre per kelvin to the
/// fourth
const STEFAN_BOLTZMANN: f64 = 5.670374419e-8;

/// How many lumens a watt of starlight is worth
///
/// Sunlight comes to about this. A hotter star puts more of itself where the
/// eye cannot see and a cooler one likewise, so taking one figure for all of
/// them overstates the ends of the range — by enough to matter to a
/// photometrist and not to a map.
const EFFICACY: f64 = 93.;

/// How much light a star of this size and heat gives off, in lumens
///
/// Stefan-Boltzmann: the power leaving a sphere goes as its area and as the
/// fourth power of its temperature. Every term is on record — `radius` in
/// metres and `temperature` in kelvin — so this is the star's real
/// output rather than anything chosen to look right.
fn lumens(radius: f32, temperature: f32) -> f32 {
    let (r, t) = (radius.max(0.) as f64, temperature.max(0.) as f64);
    let watts =
        4. * std::f64::consts::PI * r * r * STEFAN_BOLTZMANN * t.powi(4);
    (watts * EFFICACY) as f32
}

fn init_materials(
    mut assets: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    commands.insert_resource(StarMaterials(
        Glow::ALL
            .into_iter()
            .map(|glow| {
                assets.add(StandardMaterial {
                    base_color: glow.color(),
                    // A star is the light rather than a thing lit by it.
                    emissive: LinearRgba::from(glow.color()) * 4000.,
                    ..default()
                })
            })
            .collect(),
    ));

    commands.insert_resource(BodyMaterials(
        SurfaceLook::ALL
            .into_iter()
            .map(|surface| {
                assets.add(StandardMaterial {
                    base_color: surface.color(),
                    perceptual_roughness: 0.9,
                    // A trace of its own, so that a body whose star is not on
                    // record is dim rather than invisible.
                    emissive: LinearRgba::from(surface.color()) * 0.02,
                    ..default()
                })
            })
            .collect(),
    ));

    commands.insert_resource(OrbitMaterial(assets.add(StandardMaterial {
        base_color: Color::srgba(0.5, 0.6, 0.75, 0.25),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    })));
}

/// Put a system's insides on the map, and take them off again
///
/// One system at a time, which is however many [`Contents`] holds. The `Grid`
/// a system wears goes on with its contents and comes off with them: every
/// grid in the world is walked every frame, so one that holds nothing is a
/// cost with nothing to show for it.
#[allow(clippy::too_many_arguments)]
fn draw(
    camera: Query<(Entity, &OrbitCamera, Option<&Projection>)>,
    map: Res<crate::space::Map>,
    systems: Query<(Entity, &System)>,
    inside: Query<Entity, With<Inside>>,
    contents: Res<Contents>,
    clock: Res<Clock>,
    roundness: Res<Roundness>,
    stars: Res<StarMaterials>,
    bodies: Res<BodyMaterials>,
    orbit_material: Res<OrbitMaterial>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut drawn: ResMut<DrawnContents>,
    mut holding: ResMut<HeldSystem>,
    mut commands: Commands,
) {
    let Ok((eye_entity, eye, across)) =
        camera.single().map(|(e, c, lens)| (e, c.eye, seen_across(c, lens)))
    else {
        holding.0 = None;
        return;
    };

    // How large the system being held looks from here, which is the one
    // question deciding whether any of this is worth drawing.
    //
    // The system's own reach rather than what its rows come to, so this is the
    // very figure the marks standing for it fade on and the two happen
    // together. What the rows come to is still what says whether there are any:
    // nothing is drawn until they are in.
    let apparent = contents
        .of()
        .filter(|_| contents.extent().is_some())
        .and_then(|address| {
            let (entity, system) =
                systems.iter().find(|(_, s)| s.address == address)?;
            let away = space::metres(eye - system.position()).length() as f32;
            Some((address, entity, system.reach() / away.max(1.)))
        });

    // Whether an answer about somewhere new is on the wire while what is drawn
    // is the answer before it. It stands until there is something to put in
    // its place: taking a system down for the few frames a query takes and
    // putting it back is a system blinking as the camera moves, with the plane
    // ruled inside it going too.
    let holding_over =
        drawn.0.is_some() && apparent.is_none() && contents.of().is_some();

    // Said before anything is decided from it, since the marks standing for
    // this system answer it as well and one of the ways below is to leave
    // everything as it is. The two go together: a mark may only go out where
    // what it stands for is drawn, so whichever system is named here is the
    // one drawn below.
    if !holding_over {
        holding.0 = apparent.map(|(_, entity, _)| entity);
    }

    let wanted = match (drawn.0, apparent) {
        _ if holding_over => return,
        // Nothing held, or too small to bother with.
        (_, None) => None,
        // Already drawn, from the rows as they stand, and still worth keeping.
        (Some((shown, from)), Some((address, _, seen)))
            if shown == address
                && from == contents.revision()
                && seen >= WORTH_KEEPING =>
        {
            return;
        }
        // Drawn from rows the poll has since found more behind. Drawn again at
        // the size that keeps a system rather than the larger size it takes to
        // start one: this system is already on screen, and taking it away for
        // being too small to have begun is taking away what is being looked at.
        (Some((shown, _)), Some((address, entity, seen)))
            if shown == address && seen >= WORTH_KEEPING =>
        {
            Some((address, entity))
        }
        (_, Some((address, entity, seen))) if seen >= WORTH_DRAWING => {
            Some((address, entity))
        }
        (Some(_), _) => None,
        _ => return,
    };

    // The camera first, then the contents, then the grid they were all
    // placed in: a `CellCoord` under an entity that is no longer a grid has
    // nothing to be measured against.
    if let Some((shown, _)) = drawn.0.take() {
        commands.entity(eye_entity).insert(ChildOf(map.0));
        for entity in &inside {
            commands.entity(entity).despawn();
        }
        if let Some((entity, _)) =
            systems.iter().find(|(_, s)| s.address == shown)
        {
            commands.entity(entity).remove::<Grid>();
        }
    }

    let Some((address, entity)) = wanted else { return };

    let grid = space::system_grid();
    let orbits = contents.orbits();
    // Where the camera stands, in the terms the orbits are worked out in,
    // which is what says which piece of a ring is worth laying out. Those are
    // measured from the point the system's stars go round and what is drawn is
    // measured from the star it arrives at, so the middle stands between them.
    let standing = systems.get(entity).map_or(DVec3::ZERO, |(_, system)| {
        space::metres(eye - system.position())
    });
    let mut commands = commands.entity(entity);
    commands.insert(grid.clone());
    // Down into the system with them.
    //
    // Everything is drawn relative to the cell the floating origin stands in
    // rather than to the origin itself, so what a float has left over inside
    // that cell is the precision everything near the camera is drawn with. A
    // galaxy cell is `2^53` metres and the camera can stand anywhere in one,
    // which leaves about five hundred thousand kilometres: enough to shred an
    // orbit into a polygon and a name into scribble. A system's cells are a
    // metre, so descending leaves nothing over and everything inside is drawn
    // as exactly as a float can hold it.
    commands.add_child(eye_entity);

    // Everything is placed short of the arrival star, so the star lands at the
    // middle of the grid and the system's own position is where its star is.
    // The camera is sent to that position and zooms towards it, and in a
    // system whose stars go round a point between them it was arriving at the
    // point: empty sky with the star it came for ten billion kilometres off to
    // one side.
    let middle = contents.middle(&orbits, clock.at);

    // How far a star has to light, which is out to the far side of the
    // outermost thing going round it.
    let reach = contents.extent().unwrap_or_default();
    let primary = contents.primary();
    for star in contents.stars() {
        let place = orbits.place(star.id, clock.at) - middle;
        commands.with_child(drawn_star(
            star,
            primary == Some(star.id),
            place,
            reach,
            &grid,
            &roundness,
            &stars,
        ));
    }
    for body in contents.bodies() {
        let place = orbits.place(body.id, clock.at) - middle;
        commands
            .with_child(drawn_body(body, place, &grid, &roundness, &bodies));
    }

    // One line per thing that goes round something, hung off whatever it goes
    // round so its own vertices carry only the size of the orbit.
    //
    // Read off the orbits rather than assembled a second time from the rows,
    // so that what is drawn and what is placed cannot come to disagree about
    // which things there are. A barycenter is one of them: nothing is drawn at
    // one, but a close pair rides its ellipse and that is the whole of how far
    // out the pair sits.
    //
    // Which is also what its line is drawn in dashes for. Every other ellipse
    // has the thing it belongs to standing on it, and a solid ring with
    // nothing anywhere on it reads as a body the map failed to draw.
    let bare: HashSet<i16> =
        contents.barycenters().iter().map(|center| center.id).collect();
    for (id, parent) in orbits.circling() {
        if let Some(line) = drawn_orbit(
            id,
            parent,
            bare.contains(&id),
            middle,
            clock.at,
            standing + middle,
            across,
            &orbits,
            &grid,
            &mut meshes,
            &orbit_material,
        ) {
            commands.with_child(line);
        }
    }

    debug!("drew what is inside {address}");
    drawn.0 = Some((address, contents.revision()));
}

/// Where something inside a system sits, as that system's grid wants it
fn placed(place: DVec3, grid: &Grid) -> (CellCoord, Vec3) {
    grid.translation_to_grid(place)
}

/// Where the things inside a system stand, out in the galaxy
///
/// A body is placed in its system's own grid, in metres from that system's
/// centre, so saying where one is in the galaxy means going up to the system
/// holding it. Two places ask it, the row the bar draws for a body picked out
/// and the double click that flies to one, so it is answered in one.
///
/// Read from the grid rather than from a body's [`GlobalTransform`], which is
/// measured from the camera in a float and has tens of kilometres of slack out
/// at the edge of a wide system.
#[derive(SystemParam)]
pub struct Placed<'w, 's> {
    inside: Query<
        'w,
        's,
        (&'static ChildOf, &'static CellCoord, &'static Transform),
        With<Body>,
    >,
    systems: Query<'w, 's, (&'static System, &'static Grid)>,
}

impl Placed<'_, '_> {
    /// Where `body` stands, in light years
    ///
    /// Nothing for anything that is not a body drawn inside a system on the
    /// map, which is the only thing this can answer about.
    pub fn of(&self, body: Entity) -> Option<DVec3> {
        let (child_of, cell, at) = self.inside.get(body).ok()?;
        let (system, grid) = self.systems.get(child_of.parent()).ok()?;
        let metres = cell.as_dvec3(grid) + at.translation.as_dvec3();

        Some(system.position() + space::light_years(metres))
    }
}

/// A star, drawn at its own size and lighting what is around it
fn drawn_star(
    star: &DbStar,
    primary: bool,
    place: DVec3,
    reach: f32,
    grid: &Grid,
    roundness: &Roundness,
    materials: &StarMaterials,
) -> impl Bundle {
    let (cell, offset) = placed(place, grid);
    (
        Body {
            address: star.system_address,
            name: star.name.clone(),
            id: star.id,
            class: star.star_class.clone(),
            radius: star.radius,
            ancestors: star.parents.len() as u8,
            primary,
            star: true,
        },
        Inside,
        // Both fitted by `size_inside` and `pointing::size_bodies` before the
        // first draw.
        Indicator::default(),
        cell,
        Transform::from_translation(offset)
            .with_scale(Vec3::splat(star.radius.max(1.))),
        Mesh3d(roundness.coarsest()),
        MeshMaterial3d(
            materials.0[Glow::of(&star.star_class) as usize].clone(),
        ),
        NotShadowCaster,
        // A body does not block what lies behind it, so everything under the
        // pointer is reported and `pointing` weighs them by which is nearer.
        Pickable { should_block_lower: false, is_hoverable: true },
        // Placed on the star rather than beside it, and given the star's real
        // output. Shadows are off: a shadow map spanning a system would be
        // all of one texel, and a star lights from inside its own mesh.
        children![(
            PointLight {
                intensity: lumens(star.radius, star.temperature),
                // Out past everything in the system. Never shorter than the
                // star itself, so a system nobody has recorded anything else
                // about still has a light with a size to it.
                range: reach.max(star.radius) * LIGHT_REACH,
                shadow_maps_enabled: false,
                ..default()
            },
            // Cancels the scale the mesh is drawn at, so the light sits at a
            // point rather than being stretched with the sphere.
            Transform::default(),
        )],
    )
}

/// A planet or a moon, drawn at its own size
fn drawn_body(
    body: &DbBody,
    place: DVec3,
    grid: &Grid,
    roundness: &Roundness,
    materials: &BodyMaterials,
) -> impl Bundle {
    let (cell, offset) = placed(place, grid);
    (
        Body {
            address: body.system_address,
            name: body.name.clone(),
            id: body.id,
            class: body.planet_class.clone(),
            radius: body.radius,
            ancestors: body.parents.len() as u8,
            primary: false,
            star: false,
        },
        Inside,
        // Both fitted by `size_inside` and `pointing::size_bodies` before the
        // first draw.
        Indicator::default(),
        cell,
        Transform::from_translation(offset)
            .with_scale(Vec3::splat(body.radius.max(1.))),
        Mesh3d(roundness.coarsest()),
        MeshMaterial3d(
            materials.0[SurfaceLook::of(&body.planet_class) as usize].clone(),
        ),
        // As a star: what lies behind is reported too, and settled by depth.
        Pickable { should_block_lower: false, is_hoverable: true },
    )
}

/// The line one thing traces about whatever it goes round
///
/// Hung off the parent's own place, so its vertices hold the size of the orbit
/// rather than the distance out to it — the same reason a route's line is hung
/// off its midpoint.
///
/// Nothing for something that does not go round anything, which is what a
/// primary star and a barycenter at the root of a multi-star system both come
/// back as.
///
/// `bare` for a ring with nothing standing anywhere on it, which is drawn in
/// dashes. See [`DASHES`].
#[allow(clippy::too_many_arguments)]
fn drawn_orbit(
    id: i16,
    parent: Option<i16>,
    bare: bool,
    middle: DVec3,
    clock: f64,
    eye: DVec3,
    across: f64,
    orbits: &Orbits,
    grid: &Grid,
    meshes: &mut Assets<Mesh>,
    material: &OrbitMaterial,
) -> Option<impl Bundle> {
    let about =
        parent.map_or(DVec3::ZERO, |parent| orbits.place(parent, clock));
    // A ring drawn in dashes is laid closest where the camera stands, so that
    // its dashes can be cut close enough together to be seen there. One with
    // something standing on it is laid closest about that thing, being a line
    // that has to pass through it rather than a run of marks.
    let spacing = if bare {
        laid(orbits, id, orbits.nearest(id, eye - about), across)
    } else {
        Spacing::even(orbits.anomaly(id, clock), ORBIT_POINTS)
    };
    let path = orbits.path(id, &spacing)?;
    // Measured from where the line is pinned rather than from what it goes
    // round, so that the points near what rides it are small numbers. The
    // ring is put back where it belongs by hanging it there.
    let pin = path[path.len() / 2];
    let (cell, offset) = placed(about + pin - middle, grid);

    let points: Vec<Vec3> =
        path.into_iter().map(|p| (p - pin).as_vec3()).collect();
    let mesh = if bare {
        meshes.add(LineList { points: dashed(&points, spacing.run) })
    } else {
        meshes.add(LineStrip { points })
    };
    Some((
        Inside,
        OrbitLine { about: parent, pin, dashed: bare.then_some(id), spacing },
        cell,
        Transform::from_translation(offset),
        Mesh3d(mesh),
        MeshMaterial3d(material.0.clone()),
    ))
}

/// How a dashed ring's points are laid, for a camera standing `at` round it
fn laid(orbits: &Orbits, id: i16, at: f64, across: f64) -> Spacing {
    Spacing::round(at, orbits.finest(id, across), ORBIT_POINTS)
}

/// How far off its dashes may drift before a ring is laid again
///
/// As a ratio of the dash the view asks for where the camera stands. Zooming
/// out asks for a longer dash and panning walks off the close-laid part of the
/// ring into the open, and both show up here as the drawn dash and the wanted
/// one parting company. A third is under what the eye reads as a change of
/// spacing.
const RELAID_AT: f64 = 1.33;

/// Whether a ring laid to `was` is worth laying again for a camera at `now`
///
/// The dash `now` asks for where it stands, against the dash `was` actually
/// carries there. A ring the view holds whole answers the same either way, its
/// points being laid evenly and its dashes counted round rather than measured
/// against the view, so it is laid once and left alone.
fn relaid(was: &Spacing, now: &Spacing) -> bool {
    let along = turn(now.at - was.at);
    let drawn = was.run as f64 * was.step(along);
    let wanted = now.run as f64 * now.finest;

    !(1. / RELAID_AT..=RELAID_AT).contains(&(wanted / drawn))
}

/// An angle taken the short way round, within half a turn of nothing
///
/// How far round a ring the camera has moved, which crossing the start of the
/// ring would otherwise read as a whole turn.
fn turn(angle: f64) -> f64 {
    let turn = angle.rem_euclid(std::f64::consts::TAU);
    if turn > PI { turn - std::f64::consts::TAU } else { turn }
}

/// Lay each dashed ring out again as the camera moves
///
/// The piece of a ring that is drawn is chosen from how much sky the camera
/// takes in and from what it is looking at, so it is a different piece at
/// every zoom and everywhere along the ring. Zooming out leaves its dashes too
/// far apart to read and panning runs off the end of it, so both are watched.
///
/// The points and where the line hangs both move, the piece being laid out
/// about somewhere new. The mesh is rebuilt under the handle the line already
/// holds, so nothing downstream has to be told.
///
/// Only the rings drawn in dashes. A line with something standing on it is
/// drawn whole and laid out once.
fn redash(
    camera: Query<(&OrbitCamera, Option<&Projection>)>,
    holding: Res<HeldSystem>,
    systems: Query<&System>,
    grids: Query<&Grid>,
    contents: Res<Contents>,
    clock: Res<Clock>,
    mut lines: Query<(
        &mut OrbitLine,
        &ChildOf,
        &Mesh3d,
        &mut CellCoord,
        &mut Transform,
    )>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if lines.is_empty() {
        return;
    }
    let Ok((orbit, lens)) = camera.single() else { return };
    let Some(system) = holding.of().and_then(|held| systems.get(held).ok())
    else {
        return;
    };

    let across = seen_across(orbit, lens);
    let orbits = contents.orbits();
    let middle = contents.middle(&orbits, clock.at);
    let standing = space::metres(orbit.eye - system.position()) + middle;

    for (mut line, of, mesh, mut cell, mut at) in &mut lines {
        let Some(id) = line.dashed else { continue };
        let Ok(grid) = grids.get(of.parent()) else { continue };

        let about = line
            .about
            .map_or(DVec3::ZERO, |parent| orbits.place(parent, clock.at));
        let spacing =
            laid(&orbits, id, orbits.nearest(id, standing - about), across);

        // Only where enough has moved to be worth the work, which past a
        // scroll click or two, or a drag of the same, is no ring at all.
        if !relaid(&line.spacing, &spacing) {
            continue;
        }

        let Some(path) = orbits.path(id, &spacing) else { continue };
        // The middle of the run, which is where its points are laid closest.
        let pin = path[path.len() / 2];
        let points: Vec<Vec3> =
            path.into_iter().map(|p| (p - pin).as_vec3()).collect();

        // Nothing to write to where the mesh has already gone. How it was laid
        // is left as it was, so it is tried again rather than taken as done.
        let cut = LineList { points: dashed(&points, spacing.run) };
        if meshes.insert(&mesh.0, cut.into()).is_err() {
            continue;
        }

        let (into, offset) = placed(about + pin - middle, grid);
        cell.set_if_neq(into);
        at.translation = offset;
        line.pin = pin;
        line.spacing = spacing;
    }
}

/// How much sky the camera takes in, top to bottom, in metres
///
/// What a ring's own size is weighed against to know how much of it to lay
/// out. Twice what the camera answers, which is measured about what it is
/// looking at, and in metres because a system is laid out in them while the
/// camera answers in light years.
fn seen_across(camera: &OrbitCamera, lens: Option<&Projection>) -> f64 {
    2. * crate::camera::framed(camera.radius, lens) as f64 * space::LIGHT_YEAR
}

/// A closed path, cut into dashes of `run` steps, as a [`LineList`]'s pairs
///
/// The dashes are cut out of the points the ring was laid with rather than
/// measured along it afresh, so a dash bends exactly as the line it came from
/// does, and a run laid closest beside the camera puts its shortest dashes
/// there. A dash and the gap after it are the same length, which is what makes
/// the run read as a dashed line rather than as marks left by one.
fn dashed(path: &[Vec3], run: usize) -> Vec<Vec3> {
    let segments = path.len().saturating_sub(1);
    // Never nothing, so a path too short to cut is drawn every other segment
    // rather than not at all.
    let run = run.max(1);

    let mut points = Vec::with_capacity(segments);
    for segment in 0..segments {
        if (segment / run).is_multiple_of(2) {
            points.push(path[segment]);
            points.push(path[segment + 1]);
        }
    }
    points
}

/// Put everything inside a system where the clock says it stands
///
/// The bodies are spawned once and moved after, rather than drawn again from
/// scratch every time the clock moves. Redrawing means despawning a system's
/// whole insides and building the meshes back, which is work enough to be seen
/// as a stutter on a control the user drags.
///
/// The lines move too. An ellipse is drawn about whatever its thing goes round,
/// so a moon's sits on its planet, and everything here is a flat child of the
/// system: nothing is carried along by its parent moving.
///
/// The paths themselves are left alone. Winding the clock on moves a thing
/// along its orbit and does not change the orbit, so the mesh a line was built
/// from is still the right shape wherever it has to be put.
fn wind(
    clock: Res<Clock>,
    contents: Res<Contents>,
    grids: Query<&Grid>,
    mut placed_bodies: Query<
        (&Body, &ChildOf, &mut CellCoord, &mut Transform),
        Without<OrbitLine>,
    >,
    mut lines: Query<
        (&OrbitLine, &ChildOf, &mut CellCoord, &mut Transform),
        Without<Body>,
    >,
) {
    if !clock.is_changed() {
        return;
    }

    let orbits = contents.orbits();
    let middle = contents.middle(&orbits, clock.at);

    let put = |grid: &Grid,
               place: DVec3,
               cell: &mut CellCoord,
               at: &mut Transform| {
        let (into, offset) = placed(place - middle, grid);
        *cell = into;
        at.translation = offset;
    };

    for (body, of, mut cell, mut at) in &mut placed_bodies {
        let Ok(grid) = grids.get(of.parent()) else { continue };
        put(grid, orbits.place(body.id, clock.at), &mut cell, &mut at);
    }

    for (line, of, mut cell, mut at) in &mut lines {
        let Ok(grid) = grids.get(of.parent()) else { continue };
        let about = line
            .about
            .map_or(DVec3::ZERO, |parent| orbits.place(parent, clock.at));
        put(grid, about + line.pin, &mut cell, &mut at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ring laid to a view a hundred thousandth of it across
    fn laid_at(at: f64) -> Spacing {
        Spacing::round(at, 1e-6, ORBIT_POINTS)
    }

    /// A ring standing still is not laid again
    #[test]
    fn a_ring_the_camera_is_still_beside_is_left_alone() {
        assert!(!relaid(&laid_at(2.), &laid_at(2.)));
        // Nor for a nudge of the zoom.
        assert!(!relaid(
            &laid_at(2.),
            &Spacing::round(2., 1.1e-6, ORBIT_POINTS)
        ));
    }

    /// Zooming past what its dashes can carry lays it again
    #[test]
    fn zooming_lays_a_ring_again() {
        assert!(relaid(&laid_at(2.), &Spacing::round(2., 2e-6, ORBIT_POINTS)));
        assert!(relaid(&laid_at(2.), &Spacing::round(2., 5e-7, ORBIT_POINTS)));
    }

    /// And so does panning along it, before the dashes there are unreadable
    ///
    /// The points open out from where the camera stood, so panning along a ring
    /// walks off the close-laid part of it into the open. Left to the zoom
    /// alone, the dashes ahead of a pan stretch out without bound.
    #[test]
    fn panning_along_a_ring_lays_it_again() {
        let was = laid_at(2.);
        // How far round its own dashes open out by what is worth redrawing for.
        let along = (RELAID_AT - 1.) * was.finest / was.flare;

        assert!(relaid(&was, &laid_at(2. + 1.1 * along)));
        assert!(relaid(&was, &laid_at(2. - 1.1 * along)));
        assert!(!relaid(&was, &laid_at(2. + 0.5 * along)));
    }

    /// A ring the view holds whole is laid once and left alone
    ///
    /// Its points are laid evenly and its dashes counted round it rather than
    /// measured against the view, so nothing the camera does asks for another.
    #[test]
    fn a_ring_smaller_than_the_view_is_never_laid_again() {
        let was = Spacing::round(0., 1., ORBIT_POINTS);

        assert!(!relaid(&was, &Spacing::round(1., 100., ORBIT_POINTS)));
    }

    /// A ring drawn with `points` points, closing on itself
    fn ring(points: usize) -> Vec<Vec3> {
        (0..=points)
            .map(|step| {
                let turn = std::f32::consts::TAU * step as f32 / points as f32;
                Vec3::new(turn.cos(), 0., turn.sin())
            })
            .collect()
    }

    /// How many dashes a run of pairs comes to
    ///
    /// A dash is however many pairs run end to end. Where one pair does not
    /// start where the last ended, a gap has been left and a new dash begun.
    fn counted(drawn: &[Vec3]) -> usize {
        let pairs: Vec<_> = drawn.chunks(2).collect();
        1 + pairs.windows(2).filter(|two| two[0][1] != two[1][0]).count()
    }

    /// An orbit with nothing on it comes out in dashes
    ///
    /// However many steps make one: eight of them cut a ring of five hundred
    /// and twelve into thirty two dashes, and one apiece cuts it into as many
    /// as it has room for.
    #[test]
    fn a_bare_orbit_is_cut_into_dashes() {
        for (run, dashes) in [(8, 32), (4, 64), (1, ORBIT_POINTS / 2)] {
            let drawn = dashed(&ring(ORBIT_POINTS), run);

            assert_eq!(drawn.len() % 2, 0, "a line list is drawn from pairs");
            assert_eq!(counted(&drawn), dashes, "a run of {run} steps");
        }
    }

    /// A dash and the gap after it are the same length
    ///
    /// Otherwise the run reads as marks left by a line rather than as a dashed
    /// one. Half the ring drawn is what says the two are equal.
    #[test]
    fn a_dash_is_as_long_as_the_gap_after_it() {
        let whole = ring(ORBIT_POINTS);
        let drawn = dashed(&whole, 8);

        assert_eq!(
            drawn.len() / 2,
            ORBIT_POINTS / 2,
            "{} of the ring's {ORBIT_POINTS} segments were drawn",
            drawn.len() / 2
        );
    }

    /// Every dash is cut from the points the whole ring was drawn with
    ///
    /// So that a dash bends the way the line it came from does, rather than
    /// cutting the corner between two places on it. It is also what puts the
    /// shortest dashes where the ring's points are laid closest, which is
    /// beside the camera.
    #[test]
    fn the_dashes_are_cut_from_the_line_itself() {
        let whole = ring(ORBIT_POINTS);

        for point in dashed(&whole, 8) {
            assert!(
                whole.contains(&point),
                "{point} is not on the ring it was cut from"
            );
        }
    }

    /// A run of no steps at all is still a run of one
    ///
    /// Nothing offers one today, a [`Spacing`] holding its run at one or more.
    /// The floor is so that a ring which did comes back as a dashed line rather
    /// than as no line at all.
    #[test]
    fn a_run_of_no_steps_is_still_dashed() {
        let drawn = dashed(&ring(ORBIT_POINTS), 0);

        assert_eq!(counted(&drawn), ORBIT_POINTS / 2);
    }

    /// Turning the orbit lines off hides them, and on brings them back
    #[test]
    fn the_orbit_lines_are_drawn_or_not_as_asked() {
        let mut app = App::new();
        app.insert_resource(ShowOrbits(true));
        app.add_systems(Update, show_orbits);
        let line = app
            .world_mut()
            .spawn((
                OrbitLine {
                    about: None,
                    pin: DVec3::ZERO,
                    dashed: None,
                    spacing: Spacing::even(0., ORBIT_POINTS),
                },
                Visibility::Inherited,
            ))
            .id();

        app.world_mut().resource_mut::<ShowOrbits>().0 = false;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(line),
            Some(&Visibility::Hidden),
            "the line was left drawn"
        );

        app.world_mut().resource_mut::<ShowOrbits>().0 = true;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(line),
            Some(&Visibility::Inherited),
            "the line did not come back"
        );
    }

    /// Where the one body in `app` says it stands
    #[derive(Resource, Default)]
    struct BodyPosition(Option<DVec3>);

    fn read_position(
        placed: Placed,
        bodies: Query<Entity, With<Body>>,
        mut position: ResMut<BodyPosition>,
    ) {
        position.0 = bodies.single().ok().and_then(|body| placed.of(body));
    }

    /// Where a body `out` metres from the middle of a system `away` light
    /// years off is found to stand
    ///
    /// Placed by the same call that places one on the map, so this measures
    /// the round trip rather than a rearrangement of the same arithmetic.
    fn stands(away: f64, out: DVec3) -> DVec3 {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<BodyPosition>();
        app.add_systems(Update, read_position);

        let grid = space::system_grid();
        let (cell, offset) = placed(out, &grid);
        let system = app
            .world_mut()
            .spawn((crate::systems::tests::at(1, away), grid))
            .id();
        app.world_mut().spawn((
            Body {
                address: 1,
                name: String::new(),
                id: 1,
                class: String::new(),
                radius: 1e6,
                ancestors: 0,
                primary: false,
                star: false,
            },
            cell,
            Transform::from_translation(offset),
            ChildOf(system),
        ));

        app.update();
        app.world()
            .resource::<BodyPosition>()
            .0
            .expect("the body stands somewhere")
    }

    /// A system holding one body, on a circle `out` metres across
    ///
    /// Stood `through` of the way through the system's year, with the body and
    /// the line drawn about it both put where that leaves them. Answers the two,
    /// so a test can say whether they moved together.
    ///
    /// The one body's period is the system's year, it being the only thing here
    /// that goes round anything. `pin` is where the line's own points are
    /// measured from, which its anchor has to carry as well.
    fn wound(out: f64, through: f64, pin: DVec3) -> (DVec3, DVec3) {
        use super::super::{Clock, Contents, FetchState};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(Clock {
            at: through * 400. * crate::systems::info::DAY,
            ..default()
        });
        app.insert_resource(Contents {
            of: Some(1),
            revision: 0,
            state: FetchState::Known {
                stars: vec![],
                bodies: vec![{
                    // A period of its own. The shared row carries none, and a
                    // body with nothing to come round in has nowhere to be
                    // wound to.
                    let mut row = super::super::tests::body(out as f32);
                    row.orbit.orbital_period =
                        (400. * crate::systems::info::DAY) as f32;
                    row
                }],
                centers: vec![],
            },
        });
        app.add_systems(Update, wind);

        let grid = space::system_grid();
        let system = app
            .world_mut()
            .spawn((crate::systems::tests::at(1, 0.), grid.clone()))
            .id();
        let (cell, offset) = placed(DVec3::ZERO, &grid);
        let body = app
            .world_mut()
            .spawn((
                Body {
                    address: 1,
                    name: String::new(),
                    id: 1,
                    class: String::new(),
                    radius: 1e6,
                    ancestors: 1,
                    primary: false,
                    star: false,
                },
                cell,
                Transform::from_translation(offset),
                ChildOf(system),
            ))
            .id();
        // The line drawn about that body, which is where a moon's would sit.
        let line = app
            .world_mut()
            .spawn((
                OrbitLine {
                    about: Some(1),
                    pin,
                    dashed: None,
                    spacing: Spacing::even(0., ORBIT_POINTS),
                },
                cell,
                Transform::from_translation(offset),
                ChildOf(system),
            ))
            .id();

        app.update();

        let read = |of: Entity| {
            let cell = *app.world().get::<CellCoord>(of).expect("a cell");
            let at = app.world().get::<Transform>(of).expect("a transform");
            cell.as_dvec3(&grid) + at.translation.as_dvec3()
        };
        (read(body), read(line))
    }

    /// Standing further through the year carries a body along its orbit
    #[test]
    fn moving_through_the_year_moves_a_body() {
        let (still, _) = wound(1e11, 0., DVec3::ZERO);
        let (later, _) = wound(1e11, 0.5, DVec3::ZERO);

        assert!(
            still.distance(later) > 1e10,
            "the body stayed at {still} half a year on",
        );
    }

    /// A line is hung where its points are measured from
    ///
    /// The ring is held as offsets from where it is pinned, so anything
    /// placing it has to put that back. Left out, every line whose points are
    /// measured from anywhere but its parent is drawn a whole pin away from
    /// where it belongs.
    #[test]
    fn a_line_is_hung_where_its_points_are_measured_from() {
        let pin = DVec3::new(3e9, -1e9, 7e8);

        let (_, about) = wound(1e11, 0.25, DVec3::ZERO);
        let (_, hung) = wound(1e11, 0.25, pin);

        assert!(
            (hung - about - pin).length() < 1.,
            "a line pinned at {pin} hung {}m off",
            (hung - about - pin).length()
        );
    }

    /// And carries the line drawn about it to the same place
    ///
    /// An ellipse sits on whatever its thing goes round, so a moon's is drawn at
    /// its planet. Everything inside a system is a flat child of it, so nothing
    /// moves the line by inheritance: left behind, a moon would orbit the empty
    /// point its planet set out from.
    #[test]
    fn moving_through_the_year_moves_a_line_with_its_anchor() {
        let (body, line) = wound(1e11, 0.5, DVec3::ZERO);

        assert_eq!(body, line, "the line was left behind at {line}");
    }

    /// How much of the mark for a system looking `seen` radians across is left
    fn standing(seen: f32) -> f32 {
        fading(seen)
    }

    /// A world holding a camera `away` light years from one system
    ///
    /// The marks are kept up in it, and the camera is stood wherever a test
    /// wants it rather than flown there: what is being asked is how fast a
    /// mark follows the camera, not how the camera moves.
    fn approaching(away: f64) -> App {
        let mut app = App::new();
        app.init_resource::<Time<Real>>();
        app.world_mut()
            .spawn(OrbitCamera { eye: DVec3::new(away, 0., 0.), ..default() });
        // A system of the middling sort, at the origin. Its mark goes out
        // between 0.0127 light years and 0.0032.
        let held = app
            .world_mut()
            .spawn(crate::systems::tests::reaching(1, 0., 1.5e12))
            .id();
        // Held, since a mark only goes out where the map is drawing what it
        // stands for.
        app.insert_resource(HeldSystem(Some(held)));
        app.add_systems(Update, fade);
        app.update();
        app
    }

    /// And one the map is not holding
    fn beside(away: f64) -> App {
        let mut app = approaching(away);
        app.insert_resource(HeldSystem::default());
        app
    }

    /// A mark over a system the map is not holding stands whole
    ///
    /// However near the camera comes. Only one system's insides are drawn at a
    /// time, so a mark going out anywhere else is a system going out with
    /// nothing put in its place: Alpha Centauri reaches a fifth of a light
    /// year, and by its own measure its mark is all but gone from Sol.
    #[test]
    fn a_mark_the_map_is_not_holding_stands_whole() {
        let mut app = beside(0.001);

        for _ in 0..30 {
            stepped(&mut app, 0.001);
        }
        assert_eq!(drawn(&mut app), 1., "a system went out with nothing drawn");
    }

    /// How much of that system's mark is drawn
    fn drawn(app: &mut App) -> f32 {
        app.world_mut().query::<&Strength>().single(app.world()).unwrap().0
    }

    /// Stand the camera `away` light years off and draw a frame
    fn stepped(app: &mut App, away: f64) -> f32 {
        {
            let world = app.world_mut();
            let mut cameras = world.query::<&mut OrbitCamera>();
            cameras.single_mut(world).unwrap().eye = DVec3::new(away, 0., 0.);
        }
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_secs_f64(1. / 60.));
        app.update();

        drawn(app)
    }

    /// A mark goes out at its own pace, however the camera got there
    ///
    /// The distance says what a mark should come to, and the camera is free to
    /// cross the whole band between two frames: a flight spends three frames
    /// of its ninety in there, and a drag flicked across the view is not far
    /// behind. Only a scroll crosses it slowly, and it was the one path that
    /// ever looked like a fade.
    #[test]
    fn a_mark_goes_out_at_its_own_pace() {
        let mut app = approaching(0.02);
        assert_eq!(drawn(&mut app), 1.);

        // The camera arrives the whole way in, as a flight lands.
        let after = stepped(&mut app, 0.001);
        assert!(after > 0.95, "a mark went out in one frame, to {after}");

        // And is gone half a second later.
        for _ in 0..30 {
            stepped(&mut app, 0.001);
        }
        assert_eq!(drawn(&mut app), 0.);
    }

    /// And comes back at the same pace
    #[test]
    fn a_mark_comes_back_at_its_own_pace() {
        let mut app = approaching(0.001);

        let after = stepped(&mut app, 0.02);
        assert!(after < 0.05, "a mark came back in one frame, to {after}");
    }

    /// A system arriving under the camera is drawn where it stands
    ///
    /// Rather than fading in from whole. It is a system the map has just been
    /// told about, not one the camera has come up on, and a mark fading in
    /// over what it stands for is the one thing the whole band is arranged to
    /// avoid.
    #[test]
    fn a_system_arriving_is_drawn_where_it_stands() {
        let mut app = approaching(0.001);

        assert_eq!(drawn(&mut app), 0., "a mark faded in over its own system");
    }

    /// A mark goes out on the approach to the system it stands for
    ///
    /// Every one of them, on its own reach and its own distance. Read off the
    /// one system the map is holding instead, every other mark in the sky
    /// stands whole until the map changes hands and then drops to wherever the
    /// camera has already got to, which is what panning onto a system while
    /// zoomed in did.
    #[test]
    fn a_mark_goes_out_on_the_approach_to_its_own_system() {
        // A system of the middling sort, at the origin.
        let system = crate::systems::tests::reaching(1, 0., 1.5e12);
        let from = |ly: f64| standing_for(&system, DVec3::new(ly, 0., 0.));

        assert_eq!(from(0.02), 1., "a mark had begun to go from 0.02 ly off");
        assert_eq!(from(0.001), 0., "a mark was still standing 0.001 ly off");

        let mut before = 1.;
        for ly in [0.012, 0.008, 0.006, 0.005, 0.004, 0.0035] {
            let left = from(ly);
            assert!(
                left < before,
                "{ly} ly off left {left} of a mark, against {before} for one \
                 further out"
            );
            before = left;
        }
    }

    /// And a wider system's goes out from further away
    ///
    /// Which is what makes the fade a question about the system rather than
    /// about the camera. Alpha Centauri reaches a fifth of a light year and is
    /// drawn as itself from light years off; its neighbours are marks until
    /// the camera is a hundredth of one away.
    #[test]
    fn a_wider_system_goes_out_from_further_off() {
        let wide = crate::systems::tests::reaching(1, 0., 2.1e15);
        let ordinary = crate::systems::tests::reaching(2, 0., 1.5e12);
        let eye = DVec3::new(2., 0., 0.);

        assert_eq!(standing_for(&wide, eye), 0.);
        assert_eq!(standing_for(&ordinary, eye), 1.);
    }

    /// A mark is gone by the time the camera is inside the system
    ///
    /// Half the exchange this is for. A shell drawn over the bodies it stood
    /// in for is a lit sphere around the camera.
    #[test]
    fn a_mark_is_gone_once_the_camera_is_inside() {
        assert_eq!(standing(WORTH_HIDING), 0.);
        assert_eq!(standing(WORTH_HIDING * 2.), 0.);
    }

    /// And whole while the system's contents come and go
    ///
    /// The other half, and the one that is easy to get backwards. The mark
    /// begins to give way only once what it stands in for is already there,
    /// and it is whole again before that is taken away, so neither the
    /// arriving nor the leaving is ever watched.
    #[test]
    fn a_mark_is_whole_while_the_contents_come_and_go() {
        assert_eq!(standing(WORTH_DRAWING), 1.);
        assert_eq!(standing(WORTH_KEEPING), 1.);
    }

    /// The two bands stand in the one order that hides both exchanges
    ///
    /// Let go of, drawn, begins to fade, gone. Anything else has the contents
    /// arriving or leaving in front of a mark the viewer can see through.
    #[test]
    fn the_contents_come_and_go_before_the_mark_gives_way() {
        assert!(WORTH_KEEPING < WORTH_DRAWING);
        assert!(WORTH_DRAWING <= WORTH_MARKING);
        assert!(WORTH_MARKING < WORTH_HIDING);
    }

    /// And whole until well before then
    #[test]
    fn a_mark_stands_whole_until_the_camera_is_close() {
        assert_eq!(standing(WORTH_MARKING), 1.);
        assert_eq!(standing(WORTH_MARKING / 2.), 1.);
        assert_eq!(standing(0.), 1.);
    }

    /// Going out the whole way in between, without stepping
    #[test]
    fn a_mark_goes_out_over_the_way_between() {
        let mut before = 1.;
        for step in 1..=100 {
            let seen = WORTH_MARKING
                + (WORTH_HIDING - WORTH_MARKING) * step as f32 / 100.;
            let left = standing(seen);

            assert!(left < before, "{seen} left {left}, against {before}");
            assert!((0. ..1.).contains(&left), "{seen} left {left}");
            before = left;
        }
    }

    /// A body is found where the system holding it put it
    ///
    /// What the double click that flies to one asks, and what the bar's row
    /// for one asks. A body carries no galactic position of its own: it is
    /// placed in metres from the middle of its system, and this is that
    /// spoken back into the light years everything outside a system talks in.
    #[test]
    fn a_body_stands_where_its_system_put_it() {
        // A light second out, at a system ten light years off.
        let second = 2.99792458e8;
        let at = stands(10., DVec3::new(second, 0., 0.));

        let expected = DVec3::new(10. + second / space::LIGHT_YEAR, 0., 0.);
        assert!(
            at.distance(expected) * space::LIGHT_YEAR < 1.,
            "the body stood at {at}, not {expected}"
        );
    }

    /// And out at the rim, where a light second is nothing beside the distance
    ///
    /// The whole reason a body is measured from its system rather than from
    /// the galactic centre. Forty thousand light years leaves a double eleven
    /// digits to its right, and a light second asks for eight of them.
    #[test]
    fn a_body_stands_apart_from_its_system_out_at_the_rim() {
        let second = 2.99792458e8;
        let middle = stands(40_000., DVec3::ZERO);
        let out = stands(40_000., DVec3::new(0., second, 0.));

        let apart = middle.distance(out) * space::LIGHT_YEAR;
        assert!(
            (apart - second).abs() < second * 1e-3,
            "a body a light second out stood {apart}m from the middle"
        );
    }

    /// The sun comes out at the light the sun gives off
    ///
    /// A little over three and a half times ten to the twenty-eighth lumens,
    /// which is the figure this is checked against everywhere else. Getting it
    /// from a radius and a temperature rather than choosing it is the whole
    /// point: every star on record then lights its own system correctly
    /// without anything being tuned.
    #[test]
    fn a_sun_gives_off_a_suns_worth_of_light() {
        let sol = lumens(6.957e8, 5772.);

        assert!(
            (sol - 3.6e28).abs() < 0.2e28,
            "the sun came to {sol} lumens, not about 3.6e28"
        );
    }

    /// A cooler star of the same size gives off far less
    ///
    /// The fourth power is most of what decides a star's output, so halving
    /// the temperature should take all but a sixteenth of the light.
    #[test]
    fn heat_counts_far_more_than_size() {
        let hot = lumens(1e9, 6000.);
        let cool = lumens(1e9, 3000.);

        assert!(
            (hot / cool - 16.).abs() < 0.1,
            "halving the heat left {} of the light, not a sixteenth",
            cool / hot
        );
    }

    /// A star with nothing on record does not light anything, and does not
    /// come out as a nonsense
    #[test]
    fn a_star_with_nothing_recorded_gives_off_nothing() {
        assert_eq!(lumens(0., 0.), 0.);
        assert_eq!(lumens(-1., -1.), 0.);
    }

    /// The ordinary sequence runs blue to red the way it should
    #[test]
    fn the_star_sequence_runs_hot_to_cold() {
        assert_eq!(Glow::of("O"), Glow::Blue);
        assert_eq!(Glow::of("B"), Glow::Blue);
        assert_eq!(Glow::of("A"), Glow::White);
        assert_eq!(Glow::of("G"), Glow::Yellow);
        assert_eq!(Glow::of("K"), Glow::Orange);
        assert_eq!(Glow::of("M"), Glow::Red);
    }

    /// What is not on the sequence is drawn as itself
    ///
    /// A neutron star begins with the same letter as nothing in particular,
    /// and a Wolf-Rayet with the same one as a white dwarf would if the
    /// dwarfs were not caught first. None of them is a main sequence star and
    /// none should be painted as one.
    #[test]
    fn what_is_not_a_main_sequence_star_is_not_drawn_as_one() {
        assert_eq!(Glow::of("N"), Glow::Odd);
        assert_eq!(Glow::of("H"), Glow::Odd);
        assert_eq!(Glow::of("WC"), Glow::Odd);
        assert_eq!(Glow::of("CJ"), Glow::Odd);
        assert_eq!(Glow::of("SupermassiveBlackHole"), Glow::Odd);
    }

    /// A white dwarf is white, whatever letter follows the D
    #[test]
    fn every_white_dwarf_is_a_white_dwarf() {
        for class in ["D", "DA", "DAB", "DQ", "DCV"] {
            assert_eq!(Glow::of(class), Glow::White, "{class} came out wrong");
        }
    }

    /// A class nobody has written down is still drawn
    #[test]
    fn an_unheard_of_star_is_still_given_a_colour() {
        assert_eq!(Glow::of(""), Glow::Odd);
        assert_eq!(Glow::of("Quite unlike anything"), Glow::Odd);
    }

    /// The classes the journal actually writes come out as themselves
    #[test]
    fn a_body_is_drawn_as_what_it_is() {
        assert_eq!(SurfaceLook::of("Earthlike body"), SurfaceLook::Earthlike);
        assert_eq!(SurfaceLook::of("Water world"), SurfaceLook::Water);
        assert_eq!(SurfaceLook::of("Ammonia world"), SurfaceLook::Ammonia);
        assert_eq!(
            SurfaceLook::of("Sudarsky class III gas giant"),
            SurfaceLook::Gas
        );
        assert_eq!(SurfaceLook::of("Icy body"), SurfaceLook::Icy);
        assert_eq!(SurfaceLook::of("Rocky body"), SurfaceLook::Rocky);
        assert_eq!(
            SurfaceLook::of("High metal content body"),
            SurfaceLook::Metal
        );
    }

    /// A rocky ice body is ice rather than rock
    ///
    /// The one place the order of the matching shows: both words are in the
    /// phrase, and the ice is what it looks like.
    #[test]
    fn a_rocky_ice_body_reads_as_ice() {
        assert_eq!(SurfaceLook::of("Rocky ice body"), SurfaceLook::Icy);
    }

    /// A gas giant with something living in it is still a gas giant
    #[test]
    fn a_gas_giant_with_life_is_still_a_gas_giant() {
        assert_eq!(
            SurfaceLook::of("Gas giant with water based life"),
            SurfaceLook::Gas
        );
        assert_eq!(
            SurfaceLook::of("Gas giant with ammonia based life"),
            SurfaceLook::Gas
        );
    }

    /// A class nobody has written down is still drawn
    #[test]
    fn an_unheard_of_body_is_still_given_a_surface() {
        assert_eq!(SurfaceLook::of(""), SurfaceLook::Unknown);
        assert_eq!(
            SurfaceLook::of("Something else entirely"),
            SurfaceLook::Unknown
        );
    }

    /// It takes more to draw a system's insides than to keep them
    ///
    /// Which is what stops a camera sitting on the line from spawning and
    /// despawning everything in a system every frame.
    #[test]
    fn drawing_asks_more_than_keeping() {
        assert!(WORTH_DRAWING > WORTH_KEEPING);
    }

    /// A mark goes out over the whole of its band and no further
    #[test]
    fn a_mark_is_whole_below_its_band_and_gone_above_it() {
        assert_eq!(standing(WORTH_MARKING), 1.);
        assert!(standing(WORTH_MARKING * 1.001) < 1.);
        assert!(standing(WORTH_HIDING * 0.999) > 0.);
    }
}
