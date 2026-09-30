//! A compass rose in the corner, to read which way the map is turned and how
//! far across it is
//!
//! The ruled plane carries the scale and its numbers carry the axes, but both
//! are read off lines running away into perspective: which way `+z` runs is a
//! matter of watching a column of numbers climb, and how far a cell is, a
//! matter of finding two of them. A chart puts both in one place, as a rose
//! and a scale bar in the corner, and so does this.
//!
//! # A rose turned into three dimensions
//!
//! A chart's rose lies flat on the paper, and the one thing it singles out is
//! north. The galaxy has no paper, so this one is laid in the galactic plane
//! and turned with the camera, foreshortening exactly as the ruled plane
//! under the middle of the view does: drawn by the camera's own rotation,
//! without the perspective, it is that plane at the middle of the view,
//! brought into the corner.
//!
//! Its north is no longer on the card. The axis the galactic plane is laid
//! across is `y`, so the rose's needle stands up out of the card along `+y`,
//! with `-y` hanging below. What is left on the card is the plane's own
//! axes, `x` and `z`, both ways.
//!
//! Named as the axes they are, rather than in the coreward, rimward, spinward
//! and trailing of Traveller and of Elite's own explorers. Those are relative
//! to where they are said: `+z` is coreward only on Sol's side of the core,
//! and rimward on the far one. A rose in a fixed frame says what is fixed,
//! which is the axes the grid's numbers count along.
//!
//! Seen end on, as from straight over the plane, the needle is a point, and
//! the hub says which end is coming at the eye the way a drawing of a field
//! does: a dot for the head of an arrow, a cross for its tail.
//!
//! Which half of the card is nearer the eye is drawn stronger, and so is
//! whichever end of the needle is, painted over the hub where the other is
//! painted under it, so the rose reads as a solid thing rather than as a flat
//! drawing that could be either way up.
//!
//! # The scale
//!
//! A dashed ring on the card, as wide as the roundest length that fits on it,
//! and a chart's scale bar under the rose spanning exactly that width, with
//! the length written under it. The solid marks standing up at the bar's ends
//! carry on up to the ring dashed, as the ring is, so the bar is plainly the
//! ring's. Measured where the ruled plane hangs, at the middle of the view:
//! nearer the eye the plane is larger than that and further off it is
//! smaller, which the ring's own foreshortening is the reminder of.
//!
//! The camera never rolls, so the plane's horizontal on screen is always a
//! true length and only its depth is foreshortened. The ring's widest reach
//! across the screen is therefore always its diameter, and always on the
//! hub's own row, at every pitch and every turn: the two points the
//! extension lines drop from never move but to grow and shrink with the zoom.
//!
//! Said in whatever the grid is said in, and handed from light years to light
//! seconds when it is, whether or not the grid itself is drawn. A switch of
//! its own: a rose over an unruled sky is still a rose.
//!
//! # Bearings
//!
//! Whatever is picked out is marked on the rose in the color its star is
//! painted in. The needle is a diameter as long as the card's, so the rose
//! is a sphere drawn by three of its diameters, at the scale the ring says.
//! A thing near enough to lie inside that sphere is a circle where it is, to
//! scale. One beyond it is a bearing: a head pointing in at the hub where the
//! way to it meets the sphere, as a chart marks the bearing to a light — on
//! the ring for something level with the middle of the view, at the needle's
//! head for something straight over it, and between for everything between.
//! Either way it turns with the rose, and something ahead or behind lands on
//! the near or far half and is drawn as strongly as that half is. A bearing
//! turned end on is a ring on the hub, hollow, so it is not taken for a
//! circle standing in the middle. Most use where the thing is off the screen,
//! which is where nothing else on the map says which way it went.
//!
//! The star's color rather than the selection's, since every bearing is on
//! something selected and the selection's blue would say only that. The
//! star's says which of them each one is, in the terms the map is already
//! colored in: the key's hue on the map, the star's own heat in the
//! realistic view. A body takes the color of the star of its system.
//!
//! # Pointing at it
//!
//! Dragging the rose would only do what dragging the sky already does. What
//! the rose has that the sky does not is its fixed axes and its scale, so
//! that is what it answers with. Clicked, a way turns the camera to face
//! along it, keeping the pitch; a bearing turns it, pitch and all, to look
//! straight at what it bears on, which then stands in the middle of the
//! screen, beyond the middle of the view and still what the camera orbits;
//! the needle's ends look straight down from `+Y` and straight up from
//! `-Y`; and the hub looks straight down, and back to the pitch it was
//! clicked from. Every one of them writes the target the camera eases to, so
//! a click is a turn and not a jump.
//!
//! Pointed at, a piece says what it is for in the line under the bar, in
//! place of the bar's length: what a click will do, what a bearing is on and
//! how far off, how wide the whole view is for the bar itself, and for the
//! hub where the view is. That last was said at the middle of the view, over
//! the one place the user is looking; said here, it is there when it is
//! wanted and nowhere when it is not, and the middle's is off by default.

use crate::map::bodies::spawn::Entered;
use crate::map::camera::{OPENS_AT, OrbitCamera, PITCH_LIMIT, framed};
use crate::map::galaxy::Addresses;
use crate::map::galaxy::System;
use crate::map::galaxy::spawn::{ColorBy, Hue};
use crate::map::grid::{Handover, LINE, READS, RulerUnit, said_in};
use crate::map::labels::GROUND;
use crate::map::paint::sizing::View;
use crate::map::ruled::{DistanceUnit, INK, numbering, roundest, ticked, told};
use crate::map::schedule::PaintSet;
use crate::map::screen::{annotations_layer, world_per_pixel};
use crate::map::selection::{Picked, Selection, going};
use crate::map::space;
use crate::style::color32;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui::emath::GuiRounding;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use galos_photometry::Temperature;
use std::f32::consts::{PI, TAU};

pub fn plugin(app: &mut App) {
    app.insert_resource(ShowRose(true));
    // Into the annotations layer straight over the grid's readouts and under
    // every mark and name: a ring or a name drifting into the corner is
    // something picked out, and the rose is what it is read against. See
    // [`crate::map::screen::annotations_layer`].
    app.add_systems(
        EguiPrimaryContextPass,
        draw_rose
            .after(crate::map::grid::draw_readouts)
            .before(crate::map::pointing::ring)
            .before(crate::map::selection::ring)
            .before(crate::map::labels::draw_names)
            .in_set(PaintSet::Map),
    );
}

/// Whether the rose is drawn
///
/// Apart from the grid's switch. The rose is read against the grid when both
/// are up, but says which way the galaxy lies and how far across the view is
/// on its own, and is as much use over a sky with the grid hidden.
#[derive(Resource)]
pub(crate) struct ShowRose(pub(crate) bool);

/// The plane's four ways, in the order they go round the card
///
/// The same axes, and the same signs, the grid's numbers count along.
const WAYS: [(Vec3, &str); 4] = [
    (Vec3::Z, "+Z"),
    (Vec3::NEG_X, "-X"),
    (Vec3::NEG_Z, "-Z"),
    (Vec3::X, "+X"),
];

/// How far the rose's hub stands in from the viewport's bottom right corner,
/// in logical pixels
///
/// Room on the right for a way's name standing off the tip of a point, and
/// below for the scale bar under the name of the point nearest the eye.
const HUB_FROM: Vec2 = Vec2::new(112., 118.);

/// How wide the card is, in logical pixels
///
/// The radius of the ring its bearings are marked on. Everything else on the
/// rose is sized from it, and the scale ring is never wider.
const CARD: f32 = 48.;

/// How far past the ring the four ways reach, as a multiple of [`CARD`]
const TIP: f32 = 1.18;

/// How far the four short points between them reach, as a multiple of
/// [`CARD`]
///
/// Half the way out and a little more, as on a chart: they are there to
/// divide the quarters, and the eye goes to the long points first.
const SHORT: f32 = 0.62;

/// How far the needle stands over the card, and hangs under it, as a
/// multiple of [`CARD`]
///
/// The same both ways. The needle is the `y` axis as the card's points are
/// `x` and `z`, each running as far one way as the other; the head on `+y`
/// is what tells the two ends apart.
const NEEDLE: f32 = 1.0;

/// How long the needle's head is, and how wide either side of it, in pixels
const HEAD: Vec2 = Vec2::new(7., 3.5);

/// The hub's radius, in pixels
const HUB: f32 = 3.5;

/// How long the bearing ticks are, every fifteen degrees and every forty five,
/// in pixels
const TICK: f32 = 3.;
const LONG_TICK: f32 = 6.;

/// How far below the hub the scale bar stands, in pixels
///
/// Clear of the name of a point reaching straight down, which is where the
/// point nearest the eye lands when the camera looks straight down.
const BAR_BELOW: f32 = CARD * TIP + 30.;

/// How far an extension line stands off the ring it rises to, in pixels
///
/// As on a drawing: clear of the part, so the eye tells the two apart.
const CLEAR: f32 = 2.;

/// How tall the scale bar's end marks stand, solid, before they carry on
/// up to the ring dashed, in pixels
const END: f32 = 6.;

/// How much of the ink the extension lines are drawn in
///
/// Under the rest: they run down through the card, and are there to be
/// followed rather than read.
const EXTENDED: f32 = 0.6;

/// How far a name stands off what it names, in pixels
const GAP: f32 = 4.;

/// How long a bearing's mark is, out from the ring, and how wide either side
/// of its point, in pixels
///
/// A head like the needle's, a little broader: it is a thing picked out,
/// and it has to be found on the ring at a glance among the ticks.
const BEARING: Vec2 = Vec2::new(7., 4.);

/// How wide the square is that catches the pointer over the hub or a tip, in
/// pixels
///
/// Wider than either is drawn. A tip is the end of a hairline and the hub a
/// few pixels across, and something to click has to be something a hand can
/// land on.
const REACH: f32 = 14.;

/// How much of itself a name has to be drawn with to be pointed at
///
/// A name fading out as its point turns end on is sliding onto the hub, and
/// one giving way to another is under it; either way what is there to be
/// pointed at is the other thing.
const PICKABLE: f32 = 0.3;

/// How near the viewport's edge the line under the bar may come, in pixels
const EDGE: f32 = 8.;

/// How far a name's ground reaches past its letters, in pixels
const GROUND_PAD: f32 = 2.;

/// How wide the rose's lines are, in logical pixels
const STROKE: f32 = 1.;

/// And the needle's and the bar's, which are read first
const BOLD: f32 = 1.5;

/// How much of its ink the half of the rose away from the eye keeps
const FAR: f32 = 0.4;

/// And how much of it a name on that half keeps
///
/// More than a line does. A line's strength is what says which half is
/// nearer, and a name has to be read wherever it is.
const FAR_NAME: f32 = 0.7;

/// How much a name on the needle is ranked against one on the card
///
/// Under it. The needle's head already says which way `+y` is, and the
/// card's four ways are what the rose is looked at for.
const NEEDLE_RANK: f32 = 0.7;

/// How sharply one name outranks another; see [`given_way`]
const SHARP: i32 = 8;

/// How much of the smaller of two names has to be covered before it gives
/// way wholly
const OVERLAP: f32 = 0.4;

/// The roundest length that fits across the card, and the radius it draws at
///
/// `per_pixel` is how much of the map's unit one pixel covers at the middle
/// of the view. The length is the ring's diameter, rather than its radius,
/// so that the bar spanning it is exactly as long as what it says. One,
/// two or five of a power of ten, never wider than the card — and, the
/// ladder's rungs being two and a half apart at most, never narrower than two
/// fifths of it.
fn ring(per_pixel: f64) -> (f64, f32) {
    let length = roundest(2. * CARD as f64 * per_pixel);
    (length, (length / per_pixel / 2.) as f32)
}

/// Where along from nothing to whole `x` has got between `from` and `to`
fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// Where a name `size` wide and tall stands off a tip at `tip`, pointing
/// `outward` on screen
///
/// Its middle pushed out along the point by [`GAP`] and by as far as the
/// name's own box reaches that way — half its width pointing sideways, half
/// its height pointing up or down, and the share of each between. Which is
/// smooth in `outward`, so a name slides round its point as the camera turns.
/// Anchored instead by whichever corner or edge was nearest, a name leapt
/// half its width each time the point crossed from one to the next.
fn standing_off(
    tip: egui::Pos2,
    outward: Vec2,
    size: egui::Vec2,
) -> egui::Rect {
    let out =
        GAP + outward.x.abs() * size.x / 2. + outward.y.abs() * size.y / 2.;
    let middle = tip + egui::vec2(outward.x, outward.y) * out;
    egui::Rect::from_center_size(middle, size)
}

/// How much of each of a set of names is kept, where they land on each other
///
/// Each name is a box and a rank. Wherever two boxes overlap, the lower
/// ranked name gives way to the higher, by as much as they overlap: so a
/// point turned toward the eye keeps its name over one turned away, and a
/// way keeps its name over the needle's. The ranks are raised to [`SHARP`]
/// before they are weighed, so a name a little ahead of another all but has
/// the place to itself, and two dead level share it, half each. Which is
/// what makes it continuous: as the camera turns one name past another the
/// two cross fade, rather than one of them blinking out on the frame the
/// ranking changes hands.
///
/// Rather than moving either of them. A name stands where it does to say which
/// point it belongs to, and one pushed off to make room says it of the wrong
/// one.
fn given_way<const N: usize>(names: &[(egui::Rect, f32); N]) -> [f32; N] {
    let area = |rect: egui::Rect| rect.width().max(0.) * rect.height().max(0.);
    std::array::from_fn(|this| {
        let (rect, rank) = names[this];
        let rank = rank.max(0.).powi(SHARP);
        let lost: f32 = names
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != this)
            .map(|(_, (theirs, their_rank))| {
                let their_rank = their_rank.max(0.).powi(SHARP);
                let least = area(rect).min(area(*theirs));
                if least <= 0. || their_rank <= 0. {
                    return 0.;
                }
                let over = area(rect.intersect(*theirs)) / least;
                smoothstep(0., OVERLAP, over) * their_rank / (rank + their_rank)
            })
            .sum();
        (1. - lost).clamp(0., 1.)
    })
}

/// What on the rose answers the pointer
///
/// Each is something the rose already says. Pointed at, it says it in full
/// under the bar; clicked, it asks the camera for it outright.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Aim {
    /// One of the card's four ways, by its place in [`WAYS`]: turn to face
    /// along it
    Way(usize),
    /// The needle's `+Y` end, or its `-Y`: look straight down from over the
    /// plane, or straight up from under it
    Needle(bool),
    /// The hub: where the view is, and straight down onto it and back
    Hub,
    /// The scale bar: how wide the whole view is
    Scale,
    /// The mark bearing on the thing in this place of the [`Selection`]: what
    /// it is and how far off, and a turn to look straight at it
    Bearing(usize),
}

impl Aim {
    /// Whether a click on it does anything
    ///
    /// All but the bar, which is read and not worked.
    fn works(self) -> bool {
        self != Aim::Scale
    }
}

/// Which piece of the rose `at` is over, of `pieces` laid out first come
/// first served
///
/// The pieces overlap where the rose is turned so two of them land together:
/// a bearing on the tip of a way, the needle's name beside a way's. The one
/// laid out first takes the pointer, which is the order the rose builds them
/// in: the hub, the bearings, the needle, the ways, the bar.
fn aimed(pieces: &[(Aim, egui::Rect)], at: egui::Pos2) -> Option<Aim> {
    pieces.iter().find(|(_, rect)| rect.contains(at)).map(|(aim, _)| *aim)
}

/// What a bearing on `system` is painted in: what its star is painted in
///
/// The key's hue on the map, as the key's own swatch fills it, so a bearing
/// and the key read the same; the blackbody tint of the star's heat in the
/// realistic view, brought to full brightness for the display, the star's
/// own brightness being its flux and nothing a mark on the rose should say.
/// Grey where there is no system to ask, a body whose system the map has put
/// away, which is what the map paints a system it knows nothing of in.
fn star_color(system: Option<&System>, view: View, color_by: ColorBy) -> Srgba {
    let Some(system) = system else { return Hue::Grey.swatch(1.) };
    match view {
        View::Map => color_by.swatch(color_by.hue(system)),
        View::Realistic => {
            let tint = Temperature(system.temp_bucket().temperature()).color();
            let top = tint.0.into_iter().fold(f32::MIN_POSITIVE, f32::max);
            Srgba::from(LinearRgba::rgb(
                tint[0] / top,
                tint[1] / top,
                tint[2] / top,
            ))
        }
    }
}

/// Which piece of the rose is lit, and which was clicked, with the pointer
/// over `under` this frame
///
/// `held` is the piece a press went down on, which is kept from the press to
/// the release. `down` is whether any button is, and `pressed` and `released`
/// whether the primary one went down or came up this frame.
///
/// A click is a press and a release on the one piece. While a press begun on a
/// piece is down that piece stays lit wherever the pointer goes, it being
/// what the press is about; while one begun anywhere else is down nothing is,
/// a drag of the sky passing over the corner being a drag of the sky.
fn pointed(
    held: &mut Option<Aim>,
    under: Option<Aim>,
    down: bool,
    pressed: bool,
    released: bool,
) -> (Option<Aim>, Option<Aim>) {
    if pressed {
        *held = under;
    }
    let lit = if down || pressed { *held } else { under };
    let clicked = released
        .then(|| held.take())
        .flatten()
        .filter(|aim| Some(*aim) == under && aim.works());
    (lit, clicked)
}

/// The yaw that faces the camera along `way`, the short way round from
/// `from`
///
/// Along the part of `way` lying in the plane, which is the part a yaw can
/// turn to: the camera's heading is the way it faces laid flat (see
/// [`OrbitCamera::heading`]), and this is the yaw that heading is `way` at.
/// Nothing for a way with nothing in the plane, which no yaw faces.
///
/// The yaw is never wrapped where it is kept, so it is taken from wherever it
/// has wound round to and turned by less than half a turn either way. The
/// long way round is the camera swinging through most of the sky to land
/// where a short turn would have.
fn facing(way: Vec3, from: f32) -> Option<f32> {
    let flat = Vec2::new(way.x, way.z);
    if flat.length_squared() <= f32::EPSILON {
        return None;
    }
    let wanted = f32::atan2(-flat.x, -flat.y);
    let turn = (wanted - from + PI).rem_euclid(TAU) - PI;
    Some(from + turn)
}

/// The pitch that looks along `way`, a unit length
///
/// With [`facing`]'s yaw, the way into the screen is `way` itself: the
/// middle of the view stands between the eye and whatever lies that way
/// from it, so the thing is dead ahead, in the middle of the screen. No
/// steeper than [`PITCH_LIMIT`], which no pitch passes, so a thing more
/// nearly straight over or under the middle lands a hair off it.
fn pointing(way: Vec3) -> f32 {
    way.y.clamp(-1., 1.).asin().clamp(-PITCH_LIMIT, PITCH_LIMIT)
}

/// A way `off` in the galaxy, in light years, in the rose's own pixels
///
/// At the scale the dashed ring is drawn at, `per_light_year` being how many
/// light years a pixel covers at the middle of the view ([`ring`]'s
/// `per_pixel` in light years), so a thing on the ring is as far off as the
/// ring's radius says.
fn to_scale(off: DVec3, per_light_year: f64) -> Vec3 {
    (off / per_light_year).as_vec3()
}

/// How far inside the card's edge a thing picked out starts to become a
/// bearing, in pixels
///
/// Eased over these last few rather than switched at the edge, so a thing
/// drifting across it in a zoom does not flick from one mark to the other.
const TO_BEARING: f32 = 3.;

/// Where on the rose the thing `off` from the middle of the view is marked,
/// `off` in the rose's own pixels, and how much of it is a circle there
///
/// **Within the rose's reach, where it is, to scale.** The rose is drawn at
/// the scale the ring says, and the card is as wide as the ring or wider, so
/// a thing near enough to lie inside the sphere the card and the needle
/// draw has a place on the rose that is true: a circle there. Only past the
/// card's edge does the rose not reach it, and then it is a bearing: where
/// the way to it meets the sphere, a head pointing in along it. Anything else
/// said a thing a few light years off was as far as one across the galaxy.
///
/// So the circle is whole inside the edge, and gives way to the bearing over
/// the last [`TO_BEARING`] pixels to it, where the two stand in one place.
///
/// `off` is [`to_scale`]'s.
fn placed(off: Vec3) -> (Vec3, f32) {
    let reach = off.length();
    let circle = 1. - smoothstep(CARD - TO_BEARING, CARD, reach);
    (off.clamp_length_max(CARD), circle)
}

/// How near straight up or down a pitch is to count as there, in radians
///
/// A pitch is eased toward what is asked of it and never quite lands, and a
/// drag can leave it a hair short of either end.
const SQUARE: f32 = 1e-2;

/// Do what a click on `aim` asks of the camera
///
/// Only ever the target, which the camera eases toward as it does after a
/// drag, so a click is a turn rather than a jump. `before` is the pitch a
/// look straight down was taken from, which the hub goes back to; `bearings`
/// the way to each thing picked out, as [`Aim::Bearing`] counts them.
fn act(
    aim: Aim,
    orbit: &mut OrbitCamera,
    before: &mut Option<f32>,
    bearings: &[Option<Vec3>],
) {
    let pitch = orbit.target_pitch;
    let overhead = pitch <= -PITCH_LIMIT + SQUARE;
    // Only a pitch between the two ends is worth going back to. Straight up
    // is where a click on the needle's foot put it, and going back there from
    // straight down is not what the hub is for.
    let between = pitch.abs() < PITCH_LIMIT - SQUARE;
    match aim {
        Aim::Way(index) => {
            if let Some(yaw) = facing(WAYS[index].0, orbit.target_yaw) {
                orbit.target_yaw = yaw;
            }
        }
        Aim::Bearing(index) => {
            let Some(way) = bearings.get(index).copied().flatten() else {
                return;
            };
            if let Some(yaw) = facing(way, orbit.target_yaw) {
                orbit.target_yaw = yaw;
            }
            orbit.target_pitch = pointing(way);
        }
        Aim::Needle(north) => {
            if between {
                *before = Some(pitch);
            }
            orbit.target_pitch = if north { -PITCH_LIMIT } else { PITCH_LIMIT };
        }
        Aim::Hub if overhead => {
            orbit.target_pitch = before.take().unwrap_or(-OPENS_AT);
        }
        Aim::Hub => {
            if between {
                *before = Some(pitch);
            }
            orbit.target_pitch = -PITCH_LIMIT;
        }
        Aim::Scale => {}
    }
}

/// A length said the way the rose says it: three figures, and its unit
///
/// By the grid's own [`ticked`], so a figure and a power past a thousand
/// read the same under the rose as along the plane.
fn said(length: f64, unit: DistanceUnit) -> String {
    let fine = roundest(length.max(f64::MIN_POSITIVE)) / 100.;
    format!("{} {}", ticked(length, fine), unit.mark)
}

/// Paint the rose in the bottom right corner, flat on the screen, and answer
/// the pointer over it
///
/// In the annotations layer with the readouts, under the chrome: a panel
/// dragged into the corner covers it, as it covers the rest of the map.
///
/// Which leaves the pointer. The annotations layer is one painter list rather
/// than an area, and egui counts nothing in it as its own, so a press on the
/// rose would go on to turn or pick on the map behind it. So whichever piece
/// the pointer is over is covered by an area of its own the size of that
/// piece, which egui does count, and [`crate::input::PressOwner`] then gives
/// the press to the chrome. Only that piece, and only while it is pointed at:
/// the rest of the corner is sky, and a drag begun between the points turns
/// the map as it would anywhere.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_rose(
    mut contexts: EguiContexts,
    showing: Res<ShowRose>,
    mut cameras: Query<(&mut OrbitCamera, &Camera, Option<&Projection>)>,
    changing: Res<Handover>,
    asked: Res<RulerUnit>,
    selection: Res<Selection>,
    holding: Res<Entered>,
    systems: Query<&System>,
    // What a body picked out is looked up by, to be colored as its star.
    addresses: Res<Addresses>,
    view: Res<View>,
    color_by: Res<ColorBy>,
    // The piece a press went down on, which is the one it has to come up on
    // to be a click on it.
    mut held: Local<Option<Aim>>,
    // The pitch a look straight down was taken from.
    mut before: Local<Option<f32>>,
) -> Result {
    if !showing.0 {
        *held = None;
        return Ok(());
    }
    let Ok((mut orbit, camera, lens)) = cameras.single_mut() else {
        return Ok(());
    };
    let Some(viewport) = camera.logical_viewport_size() else { return Ok(()) };
    let unit = said_in(&changing, *asked);
    // The ink the grid's own numbers are drawn in, whole: the rose is not
    // dimmed with the ruling as it changes hands, having no lattice to hide a
    // swap behind.
    let ink = INK;

    // What a pixel covers where the plane hangs, which is the look-at point:
    // straight ahead of the eye, the orbit's radius into the view.
    let cot = camera.clip_from_view().y_axis.y;
    let per_light_year =
        f64::from(world_per_pixel(cot, viewport.y, orbit.radius));
    let per_pixel = per_light_year * space::LIGHT_YEAR / unit.metres;
    let (length, across) = ring(per_pixel);

    // Where the view is, said in the unit and from the place the ruling is:
    // the star of the system the map is standing in once the ruler has been
    // handed to it, and the galaxy's own origin otherwise. The tie goes to
    // the galaxy, as it does in [`crate::map::grid::said_in`].
    let from = (changing.0 < 0.5)
        .then(|| holding.of().and_then(|held| systems.get(held).ok()))
        .flatten()
        .map_or(DVec3::ZERO, System::position);
    let looking = orbit.center_from(from) * space::LIGHT_YEAR / unit.metres;
    let step = numbering(
        f64::from(framed(orbit.radius, lens)) * space::LIGHT_YEAR / unit.metres,
    );

    let ctx = contexts.ctx_mut()?;
    let painter = ctx.layer_painter(annotations_layer());
    let font = egui::FontId::new(READS, egui::FontFamily::Monospace);
    let hue = Srgba::from(LINE);
    let inked =
        |share: f32| color32(hue.with_alpha((ink * share).clamp(0., 1.)));
    // Names half again the ink, as the readouts are: they stand over lines to
    // be read outright.
    let lettered =
        |share: f32| color32(hue.with_alpha((ink * 1.5 * share).clamp(0., 1.)));
    let grounded =
        |share: f32| color32(GROUND.with_alpha((ink * share).clamp(0., 1.)));

    // The camera's own axes, which turn a way in the galaxy into a way on the
    // screen. No perspective: the rose is the plane at the middle of the view,
    // where a small enough piece of it is drawn as this.
    let right = orbit.right();
    let up = orbit.up();
    let toward = -orbit.forward();
    // On a pixel's middle, so the lines that run straight across or down the
    // screen — the needle, the bar, its end marks and extension lines — lie
    // on one row of pixels rather than smeared over two. Nothing a display
    // two pixels to the point shows, and the whole of how thin a line looks
    // on one of one.
    let pixel = painter.pixels_per_point();
    let centered = |x: f32| x.round_to_pixel_center(pixel);
    let hub = (viewport - HUB_FROM).map(centered);
    let flat = |v: Vec3| Vec2::new(v.dot(right), -v.dot(up));
    let at = |v: Vec3| {
        let it = hub + flat(v);
        egui::pos2(it.x, it.y)
    };
    // How strongly something on the rose is drawn for how near the eye it
    // lies: whole on the near side, [`FAR`] on the far one, and eased across
    // the rim between so turning the camera never flicks a half over.
    let nearness =
        |v: Vec3| smoothstep(-0.15, 0.15, v.normalize_or_zero().dot(toward));
    let depth = |v: Vec3| FAR + (1. - FAR) * nearness(v);
    let seg = |a: Vec3, b: Vec3, width: f32, share: f32| {
        painter.line_segment(
            [at(a), at(b)],
            egui::Stroke::new(width, inked(share)),
        );
    };
    // A name standing off the end of `v`, faded as `v` turns end on and the
    // name comes to stand on the hub over everything else. Laid out here and
    // painted once every name is known, so that names landing on each other
    // can give way first; see [`given_way`].
    let name = |v: Vec3,
                placed: &dyn Fn(egui::Vec2) -> egui::Rect,
                said: &str,
                rank: f32| {
        let seen = smoothstep(0.2, 0.45, flat(v.normalize_or_zero()).length());
        let share = seen * (FAR_NAME + (1. - FAR_NAME) * nearness(v));
        let galley = painter.layout_no_wrap(
            said.to_owned(),
            font.clone(),
            egui::Color32::PLACEHOLDER,
        );
        let rect = placed(galley.size());
        (galley, rect, share, share * rank)
    };
    // Off the tip of a point, running away from it.
    let way_name = |v: Vec3, said: &str| {
        let outward = flat(v).normalize_or_zero();
        name(v, &|size| standing_off(at(v), outward, size), said, 1.)
    };
    // Beside the end of the needle rather than past it. Past it is straight
    // up or straight down the screen, and that is where the point toward or
    // away from the eye lands whenever the camera faces along a way.
    let needle_name = |v: Vec3, said: &str| {
        let place = at(v) + egui::vec2(GAP + HEAD.y, 0.);
        name(
            v,
            &|size| egui::Align2::LEFT_CENTER.anchor_size(place, size),
            said,
            NEEDLE_RANK,
        )
    };

    // The needle: north standing over the card, south hanging under it.
    let north = Vec3::Y * CARD * NEEDLE;
    let south = Vec3::NEG_Y * CARD * NEEDLE;
    let north_near = toward.y >= 0.;

    // Every name, laid out before anything is painted: where they stand is
    // what the pointer is weighed against, and which is pointed at is what
    // they are then drawn by.
    let [pz, nx, nz, px] =
        WAYS.map(|(way, said)| way_name(way * CARD * TIP, said));
    let names =
        [pz, nx, nz, px, needle_name(north, "+Y"), needle_name(south, "-Y")];
    let kept =
        given_way(&names.each_ref().map(|(_, rect, _, rank)| (*rect, *rank)));
    let shown: [f32; 6] =
        std::array::from_fn(|index| names[index].2 * kept[index]);

    // Where each thing picked out is from what is being looked at, in the
    // rose's own pixels: at the scale the ring says, in all three of its
    // axes. The card and the needle are three diameters of one sphere, and
    // what lies inside it is marked where it is, a bearing on it where it
    // lies beyond; see [`placed`].
    let center = orbit.center();
    let bearings: Vec<Option<(Vec3, Srgba)>> = (0..selection.len())
        .map(|index| {
            let picked = selection.get(index)?;
            let off = to_scale(picked.position() - center, per_light_year);
            // The system itself, or the one a body is in, where the map
            // has it; see [`star_color`].
            let system = match picked {
                Picked::System(system) => Some(system),
                Picked::Body(_) => addresses
                    .get(picked.address())
                    .and_then(|entity| systems.get(entity).ok()),
            };
            Some((off, star_color(system, *view, *color_by)))
        })
        .collect();
    // The mark bearing on `way`: where on the sphere it lands, and a head
    // standing out past there, flat on the screen as the needle's is and
    // pointing in at the hub. Its three corners, and how much of it is
    // head: a point turned end on lands on the hub with no way in to point
    // along, and is a ring there instead, hollow so it is not taken for a
    // thing standing in the middle of the view.
    let bearing_mark = |way: Vec3| {
        let way = way.normalize_or(Vec3::NEG_Z);
        let on = at(way * CARD);
        let out = flat(way);
        let headed = smoothstep(0.15, 0.35, out.length());
        let out = out.normalize_or(Vec2::NEG_Y);
        let (out, side) =
            (egui::vec2(out.x, out.y), egui::vec2(-out.y, out.x) * BEARING.y);
        let tip = on + out * CLEAR;
        let back = tip + out * BEARING.x;
        (on, [tip, back + side, back - side], headed)
    };

    // What the pointer can be over, most wanted first; see [`aimed`]. A name
    // and its tip together, and only while the name is drawn strongly
    // enough to be what is being pointed at: a way turned end on has its tip
    // on the hub and its name gone, and the hub is what is there.
    let tip =
        |v: Vec3| egui::Rect::from_center_size(at(v), egui::Vec2::splat(REACH));
    let named = |index: usize, v: Vec3| {
        (shown[index] >= PICKABLE)
            .then(|| names[index].1.expand(GROUND_PAD).union(tip(v)))
    };
    let bar = centered(hub.y + BAR_BELOW);
    let ends = [hub.x - across, hub.x + across].map(centered);
    let mut pieces = vec![(
        Aim::Hub,
        egui::Rect::from_center_size(at(Vec3::ZERO), egui::Vec2::splat(REACH)),
    )];
    for (index, bearing) in bearings.iter().enumerate() {
        if let Some((off, _)) = bearing {
            let (on, circle) = placed(*off);
            let spot =
                egui::Rect::from_center_size(at(on), egui::Vec2::splat(REACH));
            let rect = match circle >= 0.5 {
                true => spot,
                false => egui::Rect::from_points(&bearing_mark(*off).1)
                    .union(spot)
                    .expand(CLEAR),
            };
            pieces.push((Aim::Bearing(index), rect));
        }
    }
    for (index, (up_end, end)) in
        [(true, north), (false, south)].into_iter().enumerate()
    {
        if let Some(rect) = named(4 + index, end) {
            pieces.push((Aim::Needle(up_end), rect));
        }
    }
    for (index, (way, _)) in WAYS.into_iter().enumerate() {
        if let Some(rect) = named(index, way * CARD * TIP) {
            pieces.push((Aim::Way(index), rect));
        }
    }
    pieces.push((
        Aim::Scale,
        egui::Rect::from_min_max(
            egui::pos2(ends[0], bar - END),
            egui::pos2(ends[1], bar + GAP + READS),
        )
        .expand(CLEAR),
    ));

    // Which of them the pointer is over, where it is the rose's to have.
    // Not while it is over some other piece of the chrome, which stands over
    // the rose; and not while a press begun somewhere else is still down, a
    // drag of the sky passing over the corner being a drag of the sky. While
    // one begun on a piece is down, that piece is the one lit, wherever the
    // pointer has wandered: it is still the one the press is about.
    let ours = egui::Id::new("rose");
    let (pointer, down, pressed, released) = ctx.input(|input| {
        let pointer = &input.pointer;
        (
            pointer.hover_pos(),
            pointer.any_down(),
            pointer.primary_pressed(),
            pointer.primary_released(),
        )
    });
    let clear = |at: egui::Pos2| {
        ctx.layer_id_at(at).is_none_or(|layer| {
            layer.id == ours || layer.order == egui::Order::Background
        })
    };
    let under =
        pointer.filter(|at| clear(*at)).and_then(|at| aimed(&pieces, at));
    let (lit, clicked) = pointed(&mut held, under, down, pressed, released);

    // The area that makes the lit piece egui's; see above.
    if let Some(rect) = lit
        .and_then(|aim| pieces.iter().find(|(piece, _)| *piece == aim))
        .map(|(_, rect)| *rect)
    {
        egui::Area::new(ours)
            .order(egui::Order::Middle)
            .constrain(false)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                ui.allocate_exact_size(rect.size(), egui::Sense::click());
            });
        if lit.is_some_and(Aim::works) {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    let is_lit = |aim: Aim| lit == Some(aim);

    // Which end of the needle is nearer the eye is painted over the hub and
    // the other under it.
    let needle = |end: Vec3| {
        let share =
            if is_lit(Aim::Needle(end.y > 0.)) { 1. } else { depth(end) };
        seg(Vec3::ZERO, end, BOLD, share);
        if end.y <= 0. {
            return;
        }
        // A head on the north end, laid flat on the screen along the way the
        // needle runs there. Nothing to lay it along once the needle is
        // shorter than the head, which is the hub's to say.
        let tip = at(end);
        let along = flat(end);
        if along.length() <= HEAD.x {
            return;
        }
        let along = along.normalize();
        let back = tip - egui::vec2(along.x, along.y) * HEAD.x;
        let side = egui::vec2(-along.y, along.x) * HEAD.y;
        painter.add(egui::Shape::convex_polygon(
            vec![tip, back + side, back - side],
            inked(share),
            egui::Stroke::NONE,
        ));
    };
    let (under_hub, over_hub) =
        if north_near { (south, north) } else { (north, south) };
    needle(under_hub);

    // The bearings, each in the color its star is painted in. Those on the
    // far half of the sphere go under the card, painted here, and those on
    // the near half over the hub, painted after it: which half a bearing is
    // on is half of what it says.
    let bearing = |index: usize, near: bool| {
        let Some((off, color)) = bearings[index] else { return };
        // How near the eye, off where the mark stands rather than the way to
        // it: the same on the sphere, and a circle crossing the hub is half
        // way between, not flicked from one half to the other.
        let (spot, circle) = placed(off);
        let nearness = smoothstep(-0.15, 0.15, spot.dot(toward) / CARD);
        if (nearness >= 0.5) != near {
            return;
        }
        let share = match is_lit(Aim::Bearing(index)) {
            true => 1.,
            false => FAR + (1. - FAR) * nearness,
        };
        let color = |share: f32| color32(going(color, share));
        if circle > 0. {
            painter.circle_filled(at(spot), BEARING.y, color(share * circle));
        }
        if circle >= 1. {
            return;
        }
        let share = share * (1. - circle);
        let (on, head, headed) = bearing_mark(off);
        if headed > 0. {
            painter.add(egui::Shape::convex_polygon(
                head.to_vec(),
                color(share * headed),
                egui::Stroke::NONE,
            ));
        }
        if headed < 1. {
            painter.circle_stroke(
                on,
                BEARING.y,
                egui::Stroke::new(BOLD, color(share * (1. - headed))),
            );
        }
    };
    for index in 0..bearings.len() {
        bearing(index, false);
    }

    // The ring the bearings are marked on, every fifteen degrees and longer
    // every forty five.
    const ROUND: usize = 96;
    let around = |turn: f32, radius: f32| {
        let (sin, cos) = (turn * TAU).sin_cos();
        Vec3::new(sin, 0., cos) * radius
    };
    for step in 0..ROUND {
        let (a, b) =
            (step as f32 / ROUND as f32, (step + 1) as f32 / ROUND as f32);
        let (a, b) = (around(a, CARD), around(b, CARD));
        seg(a, b, STROKE, depth(a + b));
    }
    for bearing in 0..24 {
        let reach = if bearing % 3 == 0 { LONG_TICK } else { TICK };
        let turn = bearing as f32 / 24.;
        let inner = around(turn, CARD);
        seg(inner, around(turn, CARD + reach), STROKE, depth(inner));
    }

    // The scale ring, dashed: every other of its steps drawn.
    const DASHES: usize = 64;
    for step in (0..DASHES).step_by(2) {
        let (a, b) =
            (step as f32 / DASHES as f32, (step + 1) as f32 / DASHES as f32);
        let (a, b) = (around(a, across), around(b, across));
        seg(a, b, STROKE, depth(a + b));
    }

    // Its extension lines, from the tops of the scale bar's end marks up to
    // the two ends of its widest reach, in the ring's own dash: one stroke,
    // turning the corner.
    let dash = TAU * across / DASHES as f32;
    for x in ends {
        painter.extend(egui::Shape::dashed_line(
            &[egui::pos2(x, bar - END), egui::pos2(x, hub.y + CLEAR)],
            egui::Stroke::new(STROKE, inked(EXTENDED)),
            dash,
            dash,
        ));
    }

    // The points, each a bare line from the hub out to its tip.
    let point = |way: Vec3, reach: f32, lit: bool| {
        let (width, share) =
            if lit { (BOLD, 1.) } else { (STROKE, depth(way)) };
        seg(Vec3::ZERO, way * CARD * reach, width, share);
    };
    for (index, (way, _)) in WAYS.into_iter().enumerate() {
        point(way, TIP, is_lit(Aim::Way(index)));
        point((way + Vec3::Y.cross(way)).normalize(), SHORT, false);
    }

    // The hub, and what it says of the needle seen end on: a dot where north
    // comes at the eye, a cross where it goes away from it.
    let middle = at(Vec3::ZERO);
    let rim = if is_lit(Aim::Hub) { BOLD } else { STROKE };
    painter.circle_filled(middle, HUB, grounded(1.));
    painter.circle_stroke(middle, HUB, egui::Stroke::new(rim, inked(1.)));
    let end_on = smoothstep(0.5, 0.9, toward.y.abs());
    if end_on > 0. {
        if north_near {
            painter.circle_filled(middle, 1.5, inked(end_on));
        } else {
            let arm = HUB * 0.6;
            for (x, y) in [(arm, arm), (arm, -arm)] {
                painter.line_segment(
                    [middle - egui::vec2(x, y), middle + egui::vec2(x, y)],
                    egui::Stroke::new(STROKE, inked(end_on)),
                );
            }
        }
    }

    for index in 0..bearings.len() {
        bearing(index, true);
    }

    needle(over_hub);

    // Each on a ground, as the map's own names are, so a line of the rose
    // passing under a name is cut there rather than struck through it. All
    // the grounds first, so none is laid over a name already lettered. A
    // name pointed at is drawn whole, wherever it stands.
    let aims = [
        Aim::Way(0),
        Aim::Way(1),
        Aim::Way(2),
        Aim::Way(3),
        Aim::Needle(true),
        Aim::Needle(false),
    ];
    let share = |index: usize| {
        if is_lit(aims[index]) { 1. } else { shown[index] }
    };
    for (index, (_, rect, ..)) in names.iter().enumerate() {
        if share(index) > 0. {
            painter.rect_filled(
                rect.expand(GROUND_PAD),
                GROUND_PAD,
                grounded(share(index)),
            );
        }
    }
    for (index, (galley, rect, ..)) in names.into_iter().enumerate() {
        if share(index) > 0. {
            painter.galley(rect.min, galley, lettered(share(index)));
        }
    }

    // The scale bar, spanning the ring, a solid mark standing up at either
    // end as on a chart's scale — and carrying on dashed, above, to the ring
    // it measures.
    let stroke = egui::Stroke::new(BOLD, inked(1.));
    let (from_end, to_end) =
        (egui::pos2(ends[0], bar), egui::pos2(ends[1], bar));
    painter.line_segment([from_end, to_end], stroke);
    for end in [from_end, to_end] {
        painter.line_segment([end, end - egui::vec2(0., END)], stroke);
    }

    // Under the bar, the length it spans — or, while a piece of the rose is
    // pointed at, what that piece has to say: where the view is, how wide it
    // is, what a bearing is on, or what a click will do. Under its middle,
    // which is under the hub, so it stands still however the ring grows;
    // and kept on the screen, a place in the galaxy being wider than the
    // room the corner leaves to the right of the hub.
    let reading = match lit {
        Some(Aim::Hub) => format!("{} {}", told(looking, step), unit.mark),
        Some(Aim::Scale) => {
            format!("{} across", said(f64::from(viewport.x) * per_pixel, unit))
        }
        Some(Aim::Way(index)) => format!("Face {}", WAYS[index].1),
        Some(Aim::Needle(true)) => "Look down from +Y".to_owned(),
        Some(Aim::Needle(false)) => "Look up from -Y".to_owned(),
        Some(Aim::Bearing(index)) => selection
            .get(index)
            .map(|picked| {
                let off = picked.position() - center;
                let away = off.length() * space::LIGHT_YEAR / unit.metres;
                format!("{} {}", picked.name(), said(away, unit))
            })
            .unwrap_or_default(),
        None => format!("{} {}", ticked(length, length), unit.mark),
    };
    let galley = painter.layout_no_wrap(reading, font, lettered(1.));
    let size = galley.size();
    let left = (hub.x - size.x / 2.).min(viewport.x - EDGE - size.x).max(EDGE);
    painter.galley(egui::pos2(left, bar + GAP), galley, lettered(1.));

    if let Some(aim) = clicked {
        let ways: Vec<Option<Vec3>> = bearings
            .iter()
            .map(|bearing| bearing.and_then(|(off, _)| off.try_normalize()))
            .collect();
        act(aim, &mut orbit, &mut before, &ways);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::ruled::ladder::tests::zooms;

    /// The scale ring stays on the card at every zoom, and never shrinks to a
    /// point on it
    ///
    /// Wider than the card and it runs out through the points and the names;
    /// much narrower and it is a smudge on the hub with a number beside it.
    #[test]
    fn the_scale_ring_fits_the_card() {
        for across in zooms() {
            // A view `across` wide on a viewport some thousand pixels tall.
            let per_pixel = across / 1000.;
            let (length, drawn) = ring(per_pixel);
            assert!(
                drawn <= CARD * (1. + 1e-5),
                "{length} over {across} draws {drawn} wide",
            );
            assert!(
                drawn > CARD * 0.4 * (1. - 1e-5),
                "{length} over {across} draws {drawn} wide",
            );
        }
    }

    /// A way's name slides round its point as the camera turns, rather than
    /// jumping
    ///
    /// Swept all the way round in fifths of a degree, a name the size of the
    /// widest on the card moves under a pixel a step. Anchored by its nearest
    /// corner or edge instead, it leapt half its width at each change.
    #[test]
    fn a_name_slides_round_its_point() {
        let tip = egui::pos2(100., 100.);
        let size = egui::vec2(20., 13.);
        let steps = 1800;
        let place = |step: usize| {
            let turn = step as f32 / steps as f32 * std::f32::consts::TAU;
            standing_off(tip, Vec2::new(turn.cos(), turn.sin()), size).center()
        };
        for step in 0..steps {
            let moved = (place(step + 1) - place(step)).length();
            assert!(moved < 1., "moved {moved} pixels at step {step}");
        }
    }

    /// A way clicked turns the camera to face along it
    ///
    /// Read back off the camera's own heading, so the yaw is the one the
    /// camera means by facing that way and not one this module made up.
    #[test]
    fn a_way_is_faced_along() {
        for (way, said) in WAYS {
            let mut orbit = OrbitCamera::default();
            act(Aim::Way(index_of(way)), &mut orbit, &mut None, &[]);
            orbit.yaw = orbit.target_yaw;
            assert!(
                orbit.heading().distance(way) < 1e-5,
                "facing {said} heads {}",
                orbit.heading(),
            );
        }
    }

    fn index_of(way: Vec3) -> usize {
        WAYS.iter().position(|(it, _)| *it == way).unwrap()
    }

    /// A turn to face a way is never more than half a turn
    ///
    /// The yaw is kept unwrapped, so a camera swung round a few times has
    /// wound it far from nought, and a turn worked out from nought would
    /// swing it back through all of them.
    #[test]
    fn a_turn_goes_the_short_way_round() {
        for wound in [-40., -7., -0.1, 0., 3., 6.3, 25.] {
            for (way, said) in WAYS {
                let yaw = facing(way, wound).unwrap();
                assert!(
                    (yaw - wound).abs() <= PI + 1e-4,
                    "from {wound}, facing {said} turned to {yaw}",
                );
            }
        }
    }

    /// Nothing faces straight up
    #[test]
    fn no_yaw_faces_along_the_needle() {
        assert_eq!(facing(Vec3::Y, 1.), None);
        assert_eq!(facing(Vec3::NEG_Y, 1.), None);
    }

    /// The hub looks straight down, and back to where it looked from
    #[test]
    fn the_hub_looks_down_and_back() {
        let mut orbit = OrbitCamera::default();
        let mut before = None;
        orbit.target_pitch = -0.7;

        act(Aim::Hub, &mut orbit, &mut before, &[]);
        assert_eq!(orbit.target_pitch, -PITCH_LIMIT);

        act(Aim::Hub, &mut orbit, &mut before, &[]);
        assert_eq!(orbit.target_pitch, -0.7);
    }

    /// Straight down from the hub with nothing to go back to goes back to
    /// the pitch the map opens at
    #[test]
    fn the_hub_goes_back_to_the_opening_pitch() {
        let mut orbit = OrbitCamera::default();
        orbit.target_pitch = -PITCH_LIMIT;

        act(Aim::Hub, &mut orbit, &mut None, &[]);
        assert_eq!(orbit.target_pitch, -OPENS_AT);
    }

    /// The needle's ends look from over the plane and from under it, and the
    /// hub still goes back to where the first of them was clicked from
    #[test]
    fn the_needle_looks_from_either_end() {
        let mut orbit = OrbitCamera::default();
        let mut before = None;
        orbit.target_pitch = -0.4;

        act(Aim::Needle(false), &mut orbit, &mut before, &[]);
        assert_eq!(orbit.target_pitch, PITCH_LIMIT);
        act(Aim::Needle(true), &mut orbit, &mut before, &[]);
        assert_eq!(orbit.target_pitch, -PITCH_LIMIT);
        act(Aim::Hub, &mut orbit, &mut before, &[]);
        assert_eq!(orbit.target_pitch, -0.4);
    }

    /// A thing picked out within the rose's reach is a circle where it is,
    /// to the ring's scale, and only one beyond the card is a bearing on it
    ///
    /// Reported as the selected systems inside the rose's reach not showing
    /// as circles: everything was a head on the card's edge, so a system a
    /// few light years off read as far as one across the galaxy.
    #[test]
    fn what_the_rose_reaches_is_a_circle_to_scale() {
        // A light year a pixel at the middle of the view, the ring's length
        // said in light years.
        let per_light_year = 1.;
        let (length, across) = ring(per_light_year);
        // On the dashed ring, at the ring's radius, whichever way it lies.
        for way in [DVec3::X, DVec3::new(1., 2., -2.).normalize(), DVec3::NEG_Y]
        {
            let off = to_scale(way * length / 2., per_light_year);
            let (spot, circle) = placed(off);
            assert_eq!(circle, 1., "{way} on the ring was not a circle");
            assert!((spot.length() - across).abs() < 1e-3, "{way}: {spot}");
            assert!(spot.normalize().distance(way.as_vec3()) < 1e-5);
        }
        // Right at the middle, on the hub.
        assert_eq!(placed(Vec3::ZERO), (Vec3::ZERO, 1.));
        // Past the card's edge, a bearing where the way meets the sphere.
        let way = Vec3::new(3., -1., 2.).normalize();
        let (spot, circle) = placed(way * CARD * 5.);
        assert_eq!(circle, 0., "a thing past the card was drawn as a circle");
        assert!(spot.distance(way * CARD) < 1e-3, "{spot}");
        // And across the edge the one gives way to the other in one place.
        let (inside, _) = placed(way * (CARD - 0.01));
        let (beyond, _) = placed(way * (CARD + 0.01));
        assert!(inside.distance(beyond) < 0.05);
    }

    /// A bearing clicked turns the camera to look straight at what it bears
    /// on, above the plane or below it, and one that has gone turns nothing
    ///
    /// Reported as a click on a bearing leaving the thing off the middle of
    /// the screen: the turn faced it along the plane and kept the pitch, so
    /// whatever lay above or below the middle of the view stayed there.
    #[test]
    fn a_bearing_is_looked_straight_at() {
        use crate::map::camera::turned;

        for way in [
            Vec3::new(3., 0., -4.),
            Vec3::new(-2., 3., 1.),
            Vec3::new(1., -4., 2.),
            Vec3::new(0., -1., 0.),
        ] {
            let way = way.normalize();
            let mut orbit = OrbitCamera::default();
            act(Aim::Bearing(1), &mut orbit, &mut None, &[None, Some(way)]);
            let ahead =
                turned(orbit.target_yaw, orbit.target_pitch) * Vec3::NEG_Z;
            // Straight down is as far as a pitch goes, a hair short of it.
            assert!(ahead.distance(way) < 2e-3, "{way} is not ahead: {ahead}");
        }

        let way = Vec3::new(-2., 3., 1.).normalize();
        let mut orbit = OrbitCamera::default();
        act(Aim::Bearing(1), &mut orbit, &mut None, &[None, Some(way)]);
        let (yaw, pitch) = (orbit.target_yaw, orbit.target_pitch);
        act(Aim::Bearing(0), &mut orbit, &mut None, &[None, Some(way)]);
        act(Aim::Bearing(5), &mut orbit, &mut None, &[None, Some(way)]);
        assert_eq!((orbit.target_yaw, orbit.target_pitch), (yaw, pitch));
    }

    /// Where two pieces overlap, the one laid out first has the pointer
    #[test]
    fn the_first_piece_laid_out_has_the_pointer() {
        let square = |x: f32| {
            egui::Rect::from_center_size(
                egui::pos2(x, 0.),
                egui::vec2(10., 10.),
            )
        };
        let pieces = [(Aim::Hub, square(0.)), (Aim::Way(0), square(4.))];

        assert_eq!(aimed(&pieces, egui::pos2(2., 0.)), Some(Aim::Hub));
        assert_eq!(aimed(&pieces, egui::pos2(8., 0.)), Some(Aim::Way(0)));
        assert_eq!(aimed(&pieces, egui::pos2(20., 0.)), None);
    }

    /// A press and a release on one piece is a click on it
    #[test]
    fn a_press_and_release_on_a_piece_clicks_it() {
        let mut held = None;
        let way = Some(Aim::Way(2));

        assert_eq!(pointed(&mut held, way, false, false, false), (way, None));
        assert_eq!(pointed(&mut held, way, true, true, false), (way, None));
        assert_eq!(pointed(&mut held, way, false, false, true), (way, way));
        assert_eq!(held, None);
    }

    /// And a whole click inside one frame is still one
    #[test]
    fn a_click_inside_one_frame_clicks() {
        let mut held = None;
        let hub = Some(Aim::Hub);
        assert_eq!(pointed(&mut held, hub, false, true, true), (hub, hub));
    }

    /// A press let go of somewhere else is not a click, and the piece it went
    /// down on stays lit while it is held
    #[test]
    fn a_press_dragged_off_does_not_click() {
        let mut held = None;
        let way = Some(Aim::Way(1));

        pointed(&mut held, way, true, true, false);
        assert_eq!(pointed(&mut held, None, true, false, false), (way, None));
        assert_eq!(pointed(&mut held, None, false, false, true), (None, None));
    }

    /// A drag begun on the sky lights nothing it passes over
    #[test]
    fn a_drag_of_the_sky_lights_nothing() {
        let mut held = None;

        pointed(&mut held, None, true, true, false);
        let over = Some(Aim::Way(3));
        assert_eq!(pointed(&mut held, over, true, false, false), (None, None));
        assert_eq!(pointed(&mut held, over, false, false, true), (over, None));
    }

    /// The bar is read and not worked
    #[test]
    fn the_bar_is_not_clicked() {
        let mut held = None;
        let bar = Some(Aim::Scale);
        assert_eq!(pointed(&mut held, bar, false, true, true), (bar, None));
    }

    /// A length is said to three figures, in its unit
    #[test]
    fn a_length_is_said_to_three_figures() {
        let years = DistanceUnit { metres: space::LIGHT_YEAR, mark: "Ly" };
        assert_eq!(said(1234., years), "1.23e3 Ly");
        assert_eq!(said(12.34, years), "12.3 Ly");
        assert_eq!(said(0.5, years), "0.500 Ly");
    }

    /// A bearing is painted in the key's color of the star it bears on, and
    /// in grey where there is no star to ask
    #[test]
    fn a_bearing_wears_its_star_s_color() {
        use crate::map::galaxy::tests::{politics, system};
        use elite_journal::Allegiance;

        let mut federal = system(1);
        politics(&mut federal).readings.allegiance =
            Some(Allegiance::Federation);
        assert_eq!(
            star_color(Some(&federal), View::Map, ColorBy::Allegiance),
            ColorBy::Allegiance.swatch(Hue::Red),
        );
        assert_eq!(
            star_color(None, View::Map, ColorBy::Allegiance),
            Hue::Grey.swatch(1.),
        );
    }
}
