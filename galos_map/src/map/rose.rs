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
//! Which half of the card is nearer the eye is drawn stronger, and the needle
//! is painted over the card or under it by the same test, so the rose reads
//! as a solid thing rather than as a flat drawing that could be either way up.
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

use crate::map::camera::OrbitCamera;
use crate::map::grid::{Handover, LINE, READS, RulerUnit, said_in};
use crate::map::labels::GROUND;
use crate::map::ruled::{INK, roundest, ticked};
use crate::map::schedule::PaintSet;
use crate::map::screen::{annotations_layer, world_per_pixel};
use crate::map::space;
use crate::style::color32;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

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

/// Paint the rose in the bottom right corner, flat on the screen
///
/// In the annotations layer with the readouts, under the chrome: a panel
/// dragged into the corner covers it, as it covers the rest of the map.
pub(crate) fn draw_rose(
    mut contexts: EguiContexts,
    showing: Res<ShowRose>,
    camera: Query<(&OrbitCamera, &Camera)>,
    changing: Res<Handover>,
    asked: Res<RulerUnit>,
) -> Result {
    if !showing.0 {
        return Ok(());
    }
    let Ok((orbit, camera)) = camera.single() else { return Ok(()) };
    let Some(viewport) = camera.logical_viewport_size() else { return Ok(()) };
    let unit = said_in(&changing, *asked);
    // The ink the grid's own numbers are drawn in, whole: the rose is not
    // dimmed with the ruling as it changes hands, having no lattice to hide a
    // swap behind.
    let ink = INK;

    // What a pixel covers where the plane hangs, which is the look-at point:
    // straight ahead of the eye, the orbit's radius into the view.
    let cot = camera.clip_from_view().y_axis.y;
    let per_pixel = f64::from(world_per_pixel(cot, viewport.y, orbit.radius))
        * space::LIGHT_YEAR
        / unit.metres;
    let (length, across) = ring(per_pixel);

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
    let hub = viewport - HUB_FROM;
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

    // The needle: north standing over the card, south hanging under it. Which
    // end is nearer the eye is painted over the card and the other under it.
    let north = Vec3::Y * CARD * NEEDLE;
    let south = Vec3::NEG_Y * CARD * NEEDLE;
    let north_near = toward.y >= 0.;
    let needle = |end: Vec3| {
        seg(Vec3::ZERO, end, BOLD, depth(end));
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
            inked(depth(end)),
            egui::Stroke::NONE,
        ));
    };
    let (under, over) =
        if north_near { (south, north) } else { (north, south) };
    needle(under);

    // The ring the bearings are marked on, every fifteen degrees and longer
    // every forty five.
    const ROUND: usize = 96;
    let around = |turn: f32, radius: f32| {
        let (sin, cos) = (turn * std::f32::consts::TAU).sin_cos();
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
    // turning the corner. Under the card, which stands over them as it stands
    // over the far end of the needle.
    let bar = hub.y + BAR_BELOW;
    let dash = std::f32::consts::TAU * across / DASHES as f32;
    for x in [hub.x - across, hub.x + across] {
        painter.extend(egui::Shape::dashed_line(
            &[egui::pos2(x, bar - END), egui::pos2(x, hub.y + CLEAR)],
            egui::Stroke::new(STROKE, inked(EXTENDED)),
            dash,
            dash,
        ));
    }

    // The points, each a bare line from the hub out to its tip, the short
    // ones under the long ones and the further of each under the nearer.
    let point = |way: Vec3, reach: f32| {
        seg(Vec3::ZERO, way * CARD * reach, STROKE, depth(way));
    };
    let by_depth = |mut ways: [Vec3; 4]| {
        ways.sort_by(|a, b| a.dot(toward).total_cmp(&b.dot(toward)));
        ways
    };
    let ways = WAYS.map(|(way, _)| way);
    for way in by_depth(ways.map(|way| (way + Vec3::Y.cross(way)).normalize()))
    {
        point(way, SHORT);
    }
    for way in by_depth(ways) {
        point(way, TIP);
    }

    // The hub, and what it says of the needle seen end on: a dot where north
    // comes at the eye, a cross where it goes away from it.
    let middle = at(Vec3::ZERO);
    painter.circle_filled(middle, HUB, grounded(1.));
    painter.circle_stroke(middle, HUB, egui::Stroke::new(STROKE, inked(1.)));
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

    needle(over);

    let [pz, nx, nz, px] =
        WAYS.map(|(way, said)| way_name(way * CARD * TIP, said));
    let names =
        [pz, nx, nz, px, needle_name(north, "+Y"), needle_name(south, "-Y")];
    let kept =
        given_way(&names.each_ref().map(|(_, rect, _, rank)| (*rect, *rank)));
    // Each on a ground, as the map's own names are, so a line of the rose
    // passing under a name is cut there rather than struck through it. All
    // the grounds first, so none is laid over a name already lettered.
    for ((_, rect, share, _), kept) in names.iter().zip(kept) {
        let share = share * kept;
        if share > 0. {
            painter.rect_filled(
                rect.expand(GROUND_PAD),
                GROUND_PAD,
                grounded(share),
            );
        }
    }
    for ((galley, rect, share, _), kept) in names.into_iter().zip(kept) {
        let share = share * kept;
        if share > 0. {
            painter.galley(rect.min, galley, lettered(share));
        }
    }

    // The scale bar, spanning the ring, a solid mark standing up at either
    // end as on a chart's scale — and carrying on dashed, above, to the ring
    // it measures. The length it spans is written under its middle, which is
    // under the hub, and so stands still however the ring grows.
    let stroke = egui::Stroke::new(BOLD, inked(1.));
    let (from, to) =
        (egui::pos2(hub.x - across, bar), egui::pos2(hub.x + across, bar));
    painter.line_segment([from, to], stroke);
    for end in [from, to] {
        painter.line_segment([end, end - egui::vec2(0., END)], stroke);
    }
    painter.text(
        egui::pos2(hub.x, bar + GAP),
        egui::Align2::CENTER_TOP,
        format!("{} {}", ticked(length, length), unit.mark),
        font,
        lettered(1.),
    );

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
}
