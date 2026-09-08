//! How large a system is drawn
//!
//! Two sizings, one per [`View`]. Neither is a real size: a system drawn at
//! its own scale is invisible from the next one over, so what is drawn is
//! whatever keeps it on screen and tells the viewer something.
//!
//! [`View::Map`] draws whichever of two is wider: a mark that says a system is
//! there, held at a size in the world so the sky reads as depth, and the
//! system's own extent, which is phased in over how much of the sky the system
//! takes up and takes over once the camera is near enough for the mark to have
//! been squeezed down under it. [`View::Realistic`] draws a bare point for the
//! eye's bloom to spread into a star, sized to a pixel whatever the distance so
//! that a star's brightness is what reads and not its disc; see
//! [`super::field`], which paints it.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;

use super::System;
use super::bodies::spawn::{Body, Unscanned, WORTH_KEEPING, WORTH_SIZING};
use super::labels::{depth, depth_of, world_per_pixel};
use super::roundness::Roundness;
use super::spawn::{Shell, StarExposure};
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use galos_photometry::{Distance, Magnitude};

pub fn plugin(app: &mut App) {
    app.insert_resource(View::Map);
    app.insert_resource(ScalePopulation(false));
    // One view is drawn at a time, so these two never run in the same frame.
    // The scheduler cannot see that from the run conditions alone.
    app.add_systems(
        Update,
        size_by_distance
            .in_set(MapSet::Present)
            .ambiguous_with(size_photometrically)
            .run_if(resource_equals(View::Map)),
    );
    app.add_systems(
        Update,
        size_photometrically
            .in_set(MapSet::Present)
            .ambiguous_with(size_by_distance)
            .run_if(resource_equals(View::Realistic)),
    );
    // Reads where a body ended up rather than deciding it, and `big_space`
    // writes that during `PostUpdate`, so it waits as `pointing::size_bodies`
    // does. What it writes is read by the next frame's propagation, which is
    // a frame behind and nowhere near enough movement to see.
    app.add_systems(PostUpdate, size_inside.after(TransformSystems::Propagate));
    // In `Update` rather than beside `size_inside`, since it reads where a
    // mark stands off the grid rather than off a transform: a mark spawned
    // this frame has no propagated one yet, and it also decides whether the
    // mark is drawn at all, which has to be settled before the visibility it
    // writes is propagated.
    app.add_systems(Update, size_marks.in_set(MapSet::Present));
}

#[derive(Resource, Debug, PartialEq)]
pub enum View {
    // TODO: Settle this one by eye. The size a shell is drawn at now falls
    // to the system's own extent rather than to a fixed floor, and nothing
    // has looked at what that does across the whole range, a crowded sky
    // especially.
    // #[default]
    Map,
    // The photometric sky: every system drawn as the star it is, sized to a
    // point and emitted at its flux so the eye's bloom spreads it into the disc
    // a sky reads a star as. The far aggregate glow behind the resolved stars —
    // the Milky Way — is not drawn yet; `super::aggregate::Planned` carries
    // the splats it will be drawn from.
    Realistic,
}

#[derive(Resource, Debug)]
pub struct ScalePopulation(pub bool);

/// Whether the map is reading the sky as populations
///
/// The option and the view it means anything in, asked together wherever it
/// is asked. [`size_by_distance`] sizes marks by population only in
/// [`View::Map`] — it does not run in the other — and everything else that
/// answers to the option has to agree with it: what is drawn at all
/// ([`super::visibility`]), what the pointer prefers between two marks
/// ([`super::pointing`]), and whether a mark may be painted under a pixel
/// ([`super::field::floor`]). The realistic sky sizes stars by what they put
/// out, which is nothing to do with who lives under them, and the option is
/// not offered there.
pub(crate) fn by_population(view: &View, option: &ScalePopulation) -> bool {
    *view == View::Map && option.0
}

/// The population a system is drawn at its ordinary size for
///
/// The median of the index's own 62,788 populated systems. Measured, along
/// with the rest of the spread this scale is cut to: nobody lives in fewer
/// than ten, a quarter of systems hold under 200 thousand, nineteen in twenty
/// under 720 million, and the busiest on record holds 32 billion — ten
/// decades end to end, with four of them carrying nine tenths of the systems.
///
/// The anchor rather than the bottom of the scale, so turning the option on
/// leaves the middle of the sky drawn where it already was and spreads the
/// rest around it. Anchored at the bottom instead every mark grew, which is a
/// sky that has only got larger: the reading is which systems are bigger than
/// the ones around them, and that is a comparison the ordinary sky has to be
/// one end of.
const POP_TYPICAL: f32 = 1.6e6;

/// How much of a decade of population goes into the mark
///
/// The power the ratio to [`POP_TYPICAL`] is raised to, so a decade of people
/// is a fixed multiple of a mark wherever on the scale it falls: at this
/// figure ten times the population draws about three tenths wider and a
/// hundred times two thirds again. Over the real spread that leaves a hamlet
/// of ten at a quarter of an ordinary mark and the busiest system on record
/// at three times one.
///
/// Steeper reads more easily and crowds sooner, and the whole populated
/// bubble on screen is a few thousand marks. Held where the busiest are still
/// plainly the busiest and a crowd of them is still a crowd of marks rather
/// than one blob: at a fifth the top of the scale came out seven times an
/// ordinary mark and the near half of the sky overlapped into itself.
/// `the_population_scale_leaves_a_sky_rather_than_a_wall` holds the bounds.
///
/// The other lever on that crowding is how many marks are drawn at all rather
/// than how large each is: [`super::bounded`] spends a cell's budget on the
/// busiest systems in it, so holding marks further apart there
/// (`MARK_SEPARATION_PX`) thins the drawn set from the bottom of the scale up.
const POP_POWER: f32 = 0.11;

/// How much larger or smaller than its ordinary mark a system is drawn for
/// the people living in it
///
/// One at [`POP_TYPICAL`], under one below it, over one above, and applied to
/// the mark [`mark_at`] draws rather than replacing it. So the sky still reads
/// as depth — a mark still falls away with distance — and what population
/// adds is which marks stand out from the ones around them.
///
/// Not clamped at either end, and not measured against what happens to be
/// loaded. A clamp draws two different populations at one size, which is the
/// lie a floor tells; an average over the loaded systems moves as the map
/// fetches and evicts, so the same system would draw at different sizes on
/// different frames with nothing about it having changed. What anchors this is
/// a figure measured once off the whole index.
///
/// Nor is there a floor under it. A thinly populated system is drawn smaller
/// than an ordinary one, and out where an ordinary one is already the smallest
/// mark the map paints that means smaller than the floor: see
/// [`super::field::floor`], which the map does without while it is scaling
/// this way. Held up to the floor instead, every system with few enough people
/// came out the size of an ordinary one, which is the floor saying something
/// about population that is not true.
///
/// An empty system is not drawn at all rather than drawn at the bottom of the
/// scale: nobody living there is not a size, and [`super::visibility`] is what
/// leaves it off the sky.
fn population_factor(population: u64) -> f32 {
    (population.max(1) as f32 / POP_TYPICAL).powf(POP_POWER)
}

/// The size a system is marked at, in metres
///
/// About a twelfth of a light year. A size in the world rather than one on
/// screen, which is what makes the sky read as depth: a mark held at a fixed
/// angle draws every system the same however far off it is, so the near ones
/// never pull ahead of the far ones and a wide view is a flat field of equal
/// dots crowding into each other.
const MARK: f32 = (8.5e-2 * crate::space::LIGHT_YEAR) as f32;

/// The most of the sky a mark may take, as an angular radius in radians
///
/// Six pixels down a 1080 line window, which a mark of [`MARK`] comes to about
/// twenty light years out. Nearer than that a size in the world swamps the
/// sky: a twelfth of a light year seen from a tenth of one is fifty degrees
/// across, and a system whose mark the camera is already inside cannot be
/// flown into.
///
/// Which is what this and [`ANGULAR`] are held to: the two of them together
/// stay well under one, so a mark is always a smaller length than the
/// distance it is seen from. `a_mark_never_encloses_the_camera` is what holds
/// them.
const NEAREST: f32 = 4e-3;

/// How large a system is drawn from far off, in radians
///
/// What is left once distance has taken the rest away, which is past about two
/// hundred light years: by then every system in the sky is the same dot, and a
/// mark that went on shrinking would leave nothing to see at all. An angle, so
/// what it settles to is a size on screen rather than in the world — about
/// half a pixel of radius down a 1080 line window at the default lens.
///
/// Half a pixel is under [`super::field`]'s own floor, and that floor is the
/// last word: a mark is painted at `SMALLEST` (0.75 px of radius) or wider,
/// whatever this works out to. So the two share the far sky between them, and
/// which one decides is worth knowing:
///
/// - Out past roughly a thousand light years down an 1080 line window, this
///   comes to under three quarters of a pixel and the field's floor decides.
///   Measured: 0.52 px at the rim, 0.53 at ten thousand light years.
/// - Nearer than that it decides itself — 1.08 px at two hundred light years —
///   which is the band the "same dot" reading is really about.
/// - Taller windows move the crossing down: at 1600 lines and up this decides
///   the whole way out.
/// - `ScalePopulation` multiplies whichever of the two decided, rather than
///   standing in for either: [`size_by_distance`] floors the ordinary mark
///   first and scales that by [`population_factor`]. Which is the order the
///   reported trouble turned on. Scaled *before* the floor, the factor was
///   swallowed for everything it did not lift clear — and out past a thousand
///   light years the floor is what a mark comes to, so the far half of the
///   galaxy drew one size whatever lived in it. Taken after, an ordinary
///   system draws exactly as it does with the option off wherever it stands,
///   and the population is the whole of the difference.
///
/// The floor is what guarantees a system is drawn at all; this is what makes
/// the far field read as one depth rather than as a size falling away. Neither
/// is redundant, and neither is the whole answer.
const ANGULAR: f32 = 4e-4;

/// How wide the mark standing for a system is drawn, in metres
///
/// Written by whichever of [`size_by_distance`] and [`size_photometrically`]
/// the drawn view belongs to, and read by [`super::field`], which divides it
/// back out to pixels and paints there, and by
/// [`super::pointing::size_indicators`], which rings it and catches the
/// pointer over it.
///
/// Its own component rather than the shell's `Transform.scale`, which is
/// where it used to live. That worked only for as long as nothing else wanted
/// the transform, and something does: a system the camera descends into gains
/// a `Grid`, and from then on its transform is that sub-grid's placement,
/// which `big_space` reads to hang the camera and every body in the system.
/// One field, two meanings, and the two collided exactly where the mark
/// matters most — a size written there scaled the whole system by the width
/// of the shell around it, and a sizing system taught to stand down instead
/// left the mark frozen at a metre, so a shell snapped to a dot on the way in
/// rather than swelling and going out. Held apart, neither has to give way.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq)]
pub(crate) struct Drawn(pub(crate) f32);

/// How much larger than its system a shell is drawn
///
/// Enough that the outermost orbit sits inside rather than on the surface.
///
/// Held under the inverse of [`super::bodies::spawn::WORTH_HIDING`], which is
/// twenty reaches, so a mark is gone by the time the camera can reach the
/// shell it stood for. The only figure here held against one in another
/// module, and the two are not in the same units: that one is an angle and
/// this a multiple of a length, so the comparison takes an inversion.
/// `a_mark_is_gone_before_the_camera_reaches_the_shell` is what holds it.
pub(crate) const MARGIN: f32 = 1.2;

/// How wide the shell drawn around a system reaching `reach` is, in metres
///
/// The surface of the disc [`super::field`] paints once the system is near
/// enough to be drawn as itself rather than as a mark — [`shell`]'s own answer
/// with the mark taken out of it, which is what it settles to from
/// [`WORTH_KEEPING`] inward.
///
/// What it is for outside this module is the one boundary the map has for
/// being *inside* a system: cross it and the camera is within the thing the
/// shell stands for. [`crate::grid`] hands the ruled plane over across it, and
/// [`MARGIN`] being held where it is — under twenty reaches — is what keeps
/// two systems' shells from standing in each other.
pub(crate) fn drawn_shell(reach: f32) -> f32 {
    reach * MARGIN
}

/// How wide the mark saying a system is there is drawn from `away` metres, in
/// metres
///
/// [`MARK`] across the middle of the map, which is a size in the world: twice
/// as far off draws about half as large, and the sky reads as depth. It gives
/// way to an angle at either end, where a size in the world is too large to
/// get past or too small to see. Being an angle holds it still on screen as
/// the camera moves, and close in that is what lets the system's own extent
/// come up through it: the mark shrinks into the shell rather than the shell
/// arriving out of it.
///
/// The map's ordinary answer, and the one [`population_factor`] scales while
/// the sky is being read as populations.
fn mark_at(away: f32) -> f32 {
    MARK.min(NEAREST * away) + ANGULAR * away
}

/// How large a system is drawn, in metres
///
/// The wider of the system itself and `mark`, the mark saying one is there. A
/// system at its true size is invisible from the next one over, and a mark is
/// no use once the camera is inside the system, so each answers for the range
/// the other cannot.
///
/// The wider rather than the two added, so that how far a system reaches
/// cannot swell a mark that is still doing its job.
///
/// The extent is phased in over the sky the system takes up, from
/// [`WORTH_SIZING`] to [`WORTH_KEEPING`], and counts for nothing below that
/// band. A mark is an angle up to twenty light years out and near enough a
/// fixed size in the world past it, so an extent counted in full would beat it
/// from there to the rim: the widest systems on record draw as a ball among
/// their neighbours' dots from four hundred light years off, twenty times
/// further than anything in them is visible. Phased in, a system takes its own
/// size on about half again as far out as its contents are drawn, whatever
/// size it is.
///
/// Full by [`WORTH_KEEPING`] rather than by
/// [`super::bodies::spawn::WORTH_DRAWING`], which is where the contents of a
/// system are drawn. They are kept down to the lower of the two, so a shell
/// drawn to a part of the extent anywhere between them would stand inside the
/// orbits still being drawn in it.
///
/// Whichever mark it is given, the system's own extent is its own: a busy
/// system is worth a larger mark, it is not worth a larger volume, and how
/// far a system reaches is not a thing the people in it move.
fn shell(extent: f32, away: f32, mark: f32) -> f32 {
    let seen = extent / away.max(1.);
    let counting =
        ((seen - WORTH_SIZING) / (WORTH_KEEPING - WORTH_SIZING)).clamp(0., 1.);

    (extent * MARGIN * counting).max(mark)
}

/// Draw each system large enough to be seen from where the camera is
///
/// The size goes on the shell, which shares an entity with the [`System`] it
/// stands for. Nothing is drawn where it is: [`super::field`] reads the size
/// back off the transform, divides it by what a pixel covers out there, and
/// paints the mark flat in screen space, so this one number is what the
/// sizing and the drawing agree through.
///
/// How far a system reaches is read off the system itself, which every one of
/// them carries. Asking the system the map is holding the insides of instead
/// would draw one star in the sky by what it is and the rest by what they are
/// assumed to be, and hand that difference from star to star as the camera
/// moves.
///
/// Distance is measured between two absolute galactic positions, the
/// camera's [`OrbitCamera::eye`] and the system's. A system's `Transform`
/// holds only the remainder left over from its grid cell, and its
/// `GlobalTransform` is written after this runs, so neither answers where it
/// is. Both are in light years, and what is written is a size in metres, so
/// the two meet here.
///
/// Written onto the shell's own [`Drawn`] rather than its transform, so a
/// descended system — which wears a `Grid` and whose transform is that
/// sub-grid's placement — is sized like any other. The mark has to go on
/// being sized there of all places: giving way to the system's contents is
/// exactly what it does as the camera comes inside one.
pub(crate) fn size_by_distance(
    scale_population: Res<ScalePopulation>,
    camera: Query<(&OrbitCamera, &Camera)>,
    // A route's stop is drawn to be found rather than to say who lives there,
    // so the population scale leaves it alone; see below.
    mut shells: Query<
        (&mut Drawn, &System, &Visibility, Has<super::route::Hop>),
        With<Shell>,
    >,
) {
    if !shells.is_empty() {
        let Ok((orbit, camera)) = camera.single() else { return };
        let Some(viewport) = camera.logical_viewport_size() else { return };
        let cot_half_fov = camera.clip_from_view().y_axis.y;
        let eye = orbit.eye;

        // TODO(#46): We should still change rgba color/emmisivity as needed.
        for (mut drawn, system, visible, hop) in shells.iter_mut() {
            // Out of the spyglass is not drawn, so the size it would draw at
            // is not worked out. It is left where it last stood, which is
            // close enough for the frame it comes back on.
            if *visible == Visibility::Hidden {
                continue;
            }
            let away = crate::space::metres(eye - DVec3::from(system.position))
                .length() as f32;
            let extent = system.reach();

            // The mark the ordinary sky draws: [`mark_at`]'s size in the
            // world, held at the smallest the field paints. The floor is
            // taken here rather than left to the field because the population
            // scale multiplies it, and out past a thousand light years the
            // floor is what an ordinary mark comes to — scaled after it, an
            // ordinary system draws exactly as it does with the option off
            // wherever it stands, which is what the option has to leave
            // alone.
            let per_pixel =
                world_per_pixel(cot_half_fov, viewport.y, away.max(1.));
            let ordinary =
                mark_at(away).max(super::field::SMALLEST * per_pixel);

            // How much larger or smaller than that the people living there
            // make it. The size in the world still falls away with distance,
            // so the sky goes on reading as depth, and what the population
            // changes is which marks stand out from the ones beside them.
            //
            // A stop on a route is exempt. It is drawn where the spyglass and
            // the filters would both have dropped it, because it is what
            // answers where to go next, and a stop shrunk to a speck for
            // having nobody living on it is a stop that cannot be found.
            let prominence = if scale_population.0 && !hop {
                population_factor(system.population)
            } else {
                1.
            };

            let size = shell(extent, away, ordinary * prominence);
            // Only where it moved, as `size_inside` is: what reads it is
            // gated on the change, and every shell in the sky marked changed
            // every frame is every one of them re-read.
            if drawn.0 != size {
                drawn.0 = size;
            }
        }
    }
}

/// The smallest anything inside a system is drawn, as a radius in pixels
///
/// A sphere drawn under a pixel across falls between the samples that decide
/// which pixels it covers, so it comes and goes as the camera moves rather
/// than fading: a moon at the far side of a system sparkles, and so does a
/// star seen from the edge of one. Held at a pixel it is a point instead,
/// which is what a thing too small to have a shape looks like.
///
/// A radius, so this is two pixels across. Below [`super::pointing`]'s floor
/// for the same body's mark, which keeps what can be aimed at a little wider
/// than what is drawn.
///
/// TODO(#72): A pixel is the right idea and not far enough. How many of the
/// four samples in a pixel a hard edge catches is off its true area by about
/// `r^-3/2`, so a body held here covers eleven to seventeen of them over the
/// sub-pixel positions and swings half its brightness between the two. What
/// draws a fresh one of those each frame is movement across the screen: a
/// zoom covers a quarter of the camera's remaining distance a frame, so
/// anything more than a pixel off the middle of the view crosses a quarter of
/// a sample's spacing in that time. The star the camera orbits is the one
/// thing exempt, since it lands on the same spot however far the zoom goes.
/// Four pixels would hold the swing to a couple of percent, and a billboard
/// sampled by its own falloff rather than by an edge has no such floor.
const SMALLEST_DRAWN: f32 = 1.;

/// Draw everything inside a system at its own size, down to a point
///
/// A body is drawn at the size it is, which is the whole difference between
/// what fills a system and the shell standing in for the system itself. That
/// holds until its own size is less than the screen can carry, and from there
/// it is a point.
///
/// Measured from the body's own [`GlobalTransform`], which [`big_space`]
/// writes relative to the cell the floating origin stands in. Inside a system
/// those cells are a metre across, so a float holds that offset exactly, and
/// this runs in `PostUpdate` after the propagation that wrote it, so the
/// transform is the frame's own and not the frame before's.
///
/// Which is the other way round from [`super::pointing::size_bodies`], and
/// deliberately. That one sizes the mark a name is packed against during
/// `Update`, before anything has been propagated, so it has to ask the grid
/// where a body stands rather than read a transform. What comes out of this
/// is a scale and a mesh the renderer picks up later in the same `PostUpdate`,
/// so there is nothing here for the wait to cost.
pub fn size_inside(
    camera: Query<(&GlobalTransform, &OrbitCamera, &Camera)>,
    roundness: Res<Roundness>,
    mut bodies: Query<(&GlobalTransform, &Body, &mut Transform, &mut Mesh3d)>,
) {
    let Ok((eye, orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (at, body, mut drawn, mut mesh) in &mut bodies {
        let offset = (at.translation() - eye.translation()).as_dvec3();
        // A metre, which is as near as the camera may be pulled to anything.
        let into_view = depth_of(orbit, offset).max(1.);
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, into_view);

        // A metre at the floor, as it is where a body is spawned: a body with
        // no radius on record would otherwise be drawn at no size at all.
        let size = body.radius.max(SMALLEST_DRAWN * per_pixel).max(1.);
        // Only where it moved. A scale assigned every frame marks every body
        // changed every frame, and everything hung off one is walked again
        // for it.
        if drawn.scale.x != size {
            drawn.scale = Vec3::splat(size);
        }

        // The size it is drawn at rather than the size it is, so a body held
        // at the floor asks for the sphere a point wants.
        let wanted = roundness.at(&mesh.0, size / per_pixel);
        if mesh.0 != *wanted {
            mesh.0 = wanted.clone();
        }
    }
}

/// How long each arm of the cross standing for a barycentre is, in pixels
///
/// Small: nothing is there, and the mark is only saying where. Under
/// [`super::pointing`]'s floor for a body's mark, so a mark standing in for
/// nothing never draws wider than the smallest thing that is actually there.
const MARK_ARM: f32 = 3.;

/// How much of the ring around it a mark may take up
///
/// A mark stands at the middle of whatever goes round it, so the ring it sits
/// in is what it has to fit inside: at a quarter of that ring's near radius,
/// a mark reads as a place marked out and leaves the ring the eye's. Held to
/// this rather than to the sizes above alone, which fill a ring a few pixels
/// across and draw a mark over the very thing it stands beside — the
/// reported trouble.
const MARK_SHARE: f32 = 0.25;

/// How small a mark is not worth drawing at all, in pixels
///
/// Past here the ring is too tight to hold a mark and the mark is too small
/// to read, so it goes out rather than being drawn as a speck inside a speck.
/// What is lost is nothing: at this size the ring itself is a few pixels, and
/// the thing to do about a place that small is fly in, which brings the mark
/// straight back.
const MARK_LEAST: f32 = 1.5;

/// Hold each mark to a size on screen, and inside the ring it stands in
///
/// An unscanned place has no size of its own — that is what is missing about
/// it — so unlike a body there is nothing to draw it at and nothing to grow
/// into. What it wants is to stay legible at every zoom, which is a size in
/// pixels and so a scale that follows the distance, bounded by how much of
/// the ring around it that size would swallow.
///
/// Measured off the grid holding it rather than off its own
/// [`GlobalTransform`], which is the other way round from [`size_inside`] and
/// for the reason [`super::pointing::size_bodies`] is: `big_space` writes
/// that transform during `PostUpdate`, and a mark spawned this frame has not
/// got one yet. Read there, the frame a system's contents arrive sized every
/// mark as if it stood on the camera — a shape the width of the view,
/// flashing once and gone, which is the box that flickered on the way in.
pub fn size_marks(
    camera: Query<(&OrbitCamera, &Camera)>,
    systems: Query<(&System, &Grid)>,
    mut marks: Query<(
        &Unscanned,
        &ChildOf,
        &CellCoord,
        &mut Transform,
        &mut Visibility,
    )>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (mark, of, cell, mut drawn, mut shown) in &mut marks {
        let Ok((system, grid)) = systems.get(of.parent()) else { continue };
        let metres = cell.as_dvec3(grid) + drawn.translation.as_dvec3();
        let place = system.position() + crate::space::light_years(metres);
        // A metre, which is as near as the camera may be pulled to anything.
        let into_view = depth(orbit, place).max(1.);
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, into_view);

        // How near the ring around it comes, in pixels, which is what the
        // mark has to fit inside.
        let ring = mark.ridden / per_pixel;
        let across = MARK_ARM.min(ring * MARK_SHARE);
        if across < MARK_LEAST {
            shown.set_if_neq(Visibility::Hidden);
            continue;
        }
        // Inherited rather than visible: the system holding it is still
        // free to take the whole of its insides away.
        shown.set_if_neq(Visibility::Inherited);

        // Only where it moved, as every scale asked of everything drawn every
        // frame is: a scale assigned regardless marks the mark changed every
        // frame and has `big_space` walk it again for nothing.
        let size = across * per_pixel;
        if drawn.scale.x != size {
            drawn.scale = Vec3::splat(size);
        }
    }
}

/// How fast a star's drawn radius grows with brightness, in screen pixels per
/// e-fold of flux
///
/// A star is a point; what reaches the screen is the instrument's point spread,
/// the same shape ([`super::spawn::star_psf`]) for every star. A brighter star
/// is not drawn wider — it clears more of that one fixed shape above the eye's
/// floor. That cleared radius grows with the *logarithm* of brightness, the
/// law the eye reads by and the one that never runs away: each doubling of
/// flux adds a fixed step, so even the sky's most luminous stars stay a
/// bounded glint with no cap to impose. Tuned against a long exposure of a
/// real sky.
const PSF_GROWTH: f64 = 0.45;

/// The smallest a drawn star may be, as a radius in screen pixels
///
/// A star that clears the floor is drawn at least this large so it lands as a
/// stable dot rather than a sub-pixel speck that flickers as the camera moves:
/// under a pixel, the share of one a dot covers changes with where it falls
/// on the grid, so it twinkles from the camera's motion alone. The smallest
/// mark that draws stably is the display's to set rather than the sky's, which
/// is why the floor is a radius in pixels. Most of the sky sits here — a field
/// of tiny dots — with only the brighter stars grown past it by their point
/// spread. Not a cap: the floor is the pixel grid, and brightness above it
/// still grows the star.
const DOT_RADIUS: f32 = 0.6;

/// The size a star below the flux floor shrinks to, as a fraction of a pixel
///
/// Not zero, though no longer for the reason it was written for: a name is
/// painted flat in screen space now (see [`super::labels::draw_names`]) and
/// inherits nothing from the star, so a star of no size would still be named
/// and still be aimed at — [`super::pointing`] floors a system's mark at a
/// size for the hand whatever the field draws.
///
/// What reads it is the field. [`super::field::drawn_radius`] takes the size
/// back off the shell, divides it by what a pixel covers out there, halves it
/// for this view, and draws a star only where what is left is more than half
/// of this. So the sliver is the one thing saying "this star did not clear the
/// exposure floor", and the field drops it rather than flooring it up the way
/// the map does — the map's floor applied here would draw every star under the
/// floor as a point of light.
///
/// Not zero for the room a float wants, rather than because a zero would light
/// the sky. It would not: [`psf_radius`] returns exactly zero under the floor,
/// so a zero sliver leaves the field a `raw` of zero and `0. > 0.` is still
/// false, and the star is still undrawn. What a nonzero value buys is that the
/// sentinel comes out of the multiply and divide by `per_pixel` it makes the
/// trip through as a strictly positive number, instead of the test resting on
/// an exact zero surviving two float operations.
///
/// A thousandth of a pixel puts it three orders under the smallest star that
/// draws ([`DOT_RADIUS`]), which is the room the test wants on both sides:
/// nothing that survives the round trip lands near the threshold. What it may
/// not be is anything approaching twice [`DOT_RADIUS`] — a star that cleared
/// the floor is at least that wide, so at `1.2` the `UNSEEN * 0.5` test would
/// begin dropping stars that did clear it.
pub(crate) const UNSEEN: f32 = 1e-3;

/// The visible radius of a star's point spread, in screen pixels
///
/// A star's image is its exposed `energy` —
/// [`galos_photometry::Magnitude::exposure`] of its apparent magnitude, the
/// same law `galos_sky` sizes by — spread over a fixed point spread, and the
/// disc that shows is where that clears the eye's floor. The cleared radius is
/// [`PSF_GROWTH`]` · ln(energy)`, zero where the energy is under one (a star
/// fainter than the zero point), so a star too faint to see has no size and is
/// not drawn. The logarithm is the whole of the bound: brightness climbs it a
/// fixed step per e-fold, so the sky's most luminous stars — Elite's procedural
/// O and B supergiants run past a million suns — stay a glint a few pixels wide
/// rather than a disc, with no cap to impose.
//
// TODO(psf): the profile and its `β` are now [`galos_photometry::psf::Moffat`],
// baked to a texture by [`super::spawn::star_psf`] and stretched to this radius
// on a billboard. The stretch is the approximation left to remove: the plan is
// a custom billboard material that evaluates the Moffat per fragment at a fixed
// core width, integrated over each pixel's footprint so a star crossing a pixel
// boundary does not shimmer, its above-floor radius falling out of the profile
// itself. The shape is the instrument's — one profile for every star, and only
// the exposure between them — so stretching a baked texture to each star's
// radius makes the core width a per-star number instead: a bright star is
// drawn through a wider instrument rather than through more of the same one,
// and the light its mark lays down follows its drawn area rather than its
// flux. It reads well enough on screen to pass for finished, which is how it
// gets left. DO NOT FORGET THIS.
fn psf_radius(energy: f64) -> f32 {
    if energy <= 1. {
        return 0.;
    }
    ((PSF_GROWTH * energy.ln()) as f32).max(DOT_RADIUS)
}

/// Size each system by its point spread, for the realistic view
///
/// A star is sized to the radius its point spread (`super::spawn::star_psf`)
/// clears above the eye's floor, by [`psf_radius`]. A brighter star clears
/// more of the same profile, so it draws larger and a fainter one smaller,
/// both by the log of their brightness; opening the exposure grows them all
/// and draws fainter ones in; and a star whose peak is under the floor is cut
/// to the [`UNSEEN`] sliver and drawn by nobody. What is written is a world
/// size, which [`super::field`] takes back to pixels and paints the glint at.
///
/// Written onto the shell's own [`Drawn`], as [`size_by_distance`] is, so a
/// descended system is sized like any other: its transform belongs to the
/// sub-grid `big_space` hangs the system in and has nothing to do with how
/// wide the mark standing for it is drawn.
pub(crate) fn size_photometrically(
    camera: Query<(&OrbitCamera, &Camera)>,
    exposure: Res<StarExposure>,
    mut shells: Query<(&mut Drawn, &System, &Visibility), With<Shell>>,
) {
    let Ok((orbit, camera)) = camera.single() else {
        return;
    };
    let Some(viewport) = camera.logical_viewport_size() else {
        return;
    };
    let cot_half_fov = camera.clip_from_view().y_axis.y;
    let zero_point = exposure.zero_point();
    for (mut drawn, system, visible) in shells.iter_mut() {
        if *visible == Visibility::Hidden {
            continue;
        }
        let apparent = Magnitude(system.absolute_magnitude()).apparent(
            Distance::light_years(orbit.eye.distance(system.position())),
        );
        let energy = apparent.exposure(Magnitude(zero_point)).0;
        let radius = psf_radius(energy);
        let away =
            crate::space::metres(orbit.eye - system.position()).length() as f32;
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, away.max(1.));
        // The quad is a unit square, so twice the radius sets its half-width to
        // the cleared radius. The Moffat profile fades to nothing well inside
        // that edge, so a bright star is a cored glint, not the flat disc a
        // bare sphere gave.
        // Floored to a sliver of a pixel rather than nothing; see [`UNSEEN`].
        let size = (2. * radius * per_pixel).max(per_pixel * UNSEEN);
        if drawn.0 != size {
            drawn.0 = size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::bodies::STAND_IN;
    use crate::systems::tests::{at, reaching};

    /// How large a system reaching `extent` is drawn from `away`, the map
    /// drawing marks by distance rather than by population
    ///
    /// The ordinary sky, which is what everything about the exchange between
    /// a mark and a system's own extent is read at.
    fn plain(extent: f32, away: f32) -> f32 {
        shell(extent, away, mark_at(away))
    }

    /// A shell holds the system it stands around wherever the insides are drawn
    ///
    /// What the rest rests on. Out to [`WORTH_KEEPING`], which is as far off as
    /// what fills a system is kept: past that a system is a mark saying where
    /// it is, and a mark is smaller than the thing it stands for. Swept over
    /// extents from a compact system to the widest on record, since a
    /// floor-shaped mistake passes on a large system and fails on every
    /// smaller one.
    #[test]
    fn a_shell_holds_the_system_inside_it() {
        // A light second out to a fifth of a light year, which is compact to
        // the widest on record.
        for extent in [3e8f32, 1.5e12, 1.7e14, 2.1e15] {
            // A metre out to as far as the system's insides are still kept.
            let kept = extent / WORTH_KEEPING;
            for away in [1f32, kept * 1e-4, kept * 1e-2, kept * 0.5, kept] {
                // Both scales, since a mark of any size is only ever the
                // wider of the two answers: the distance mark, and a mark
                // wide enough to swamp it, which is what a busy system's
                // population mark is up close.
                for mark in [mark_at(away), mark_at(away) * 20.] {
                    let drawn = shell(extent, away, mark);

                    assert!(
                        drawn >= extent,
                        "a system {extent}m across was drawn {drawn}m \
                         from {away}m away"
                    );
                }
            }
        }
    }

    /// A shell settles onto its system rather than arriving at it
    ///
    /// The mark shrinks as the camera comes in and stops counting for anything
    /// once the system it stands for is the wider of the two. From there the
    /// shell holds still at the system's own extent, so there is no distance
    /// at which it steps.
    #[test]
    fn a_shell_settles_onto_its_system() {
        let extent = 1.7e14;
        let held = extent * MARGIN;

        assert!(plain(extent, 1e18) > held, "the mark had already gone");
        assert_eq!(plain(extent, 1e16), held);
        assert_eq!(plain(extent, 1e12), held);
    }

    /// A wide system is a mark from far off, as any other system is
    ///
    /// The whole of what phasing the extent in is for. The widest systems on
    /// record reach a fifth of a light year, and counted in full one of those
    /// is drawn at its own size from four hundred light years out, a ball
    /// sitting among its neighbours' dots for as long as it takes to fly the
    /// twenty times nearer that seeing anything in it takes.
    #[test]
    fn a_wide_system_is_a_mark_from_far_off() {
        let away = 100. * crate::space::LIGHT_YEAR as f32;
        // The widest on record, one of the ordinary sort, and a system of no
        // size at all, which is the mark and nothing else.
        let widest = plain(2.1e15, away);
        let ordinary = plain(1e14, away);
        let mark = plain(0., away);

        assert_eq!(
            widest, mark,
            "the widest system on record drew {widest}m against a mark of \
             {mark}m from a hundred light years"
        );
        assert_eq!(ordinary, mark);
    }

    /// And is drawn to its own size before what fills it arrives
    ///
    /// The other end of the same band, and what keeps the extent from being
    /// watched arriving: by the time there is anything inside a system to
    /// hold, the shell holding it has settled. Over the whole range of sizes,
    /// each read at the distance its own contents are drawn at.
    #[test]
    fn a_system_is_sized_before_its_insides_are_drawn() {
        use crate::systems::bodies::spawn::WORTH_DRAWING;

        for extent in [STAND_IN, 1e13, 1e14, 5e14, 1e15, 2.1e15] {
            let away = extent / WORTH_DRAWING;
            let drawn = plain(extent, away);

            assert_eq!(
                drawn,
                extent * MARGIN,
                "a system reaching {extent}m drew {drawn}m where its bodies \
                 were being drawn, from {away}m away"
            );
        }
    }

    /// A shell only ever grows on screen as the camera comes in
    ///
    /// What says the extent arrives without a step in it. The mark and the
    /// extent cross over as the camera closes, and in metres the shell dips
    /// through the crossing: the mark it is still drawn at is falling while
    /// the extent coming up under it has not caught up. On screen it does not
    /// dip, the mark holding its angle over exactly that stretch, and the
    /// screen is where it is watched.
    ///
    /// Swept from beyond the far rim in to a system's own surface, over every
    /// size of system. The tolerance is the last bit of an `f32`: a mark held
    /// at a fixed angle is a constant here, and it is arrived at by dividing
    /// two numbers that both move.
    #[test]
    fn a_shell_only_grows_on_screen_as_the_camera_comes_in() {
        for extent in [STAND_IN, 1e13, 1e14, 5e14, 1e15, 2.1e15] {
            let mut away = 2000. * crate::space::LIGHT_YEAR as f32;
            let mut before = plain(extent, away) / away;

            while away > extent {
                away *= 0.98;
                let seen = plain(extent, away) / away;

                assert!(
                    seen >= before - before * 1e-6,
                    "a system reaching {extent}m shrank from {before} to \
                     {seen} radians as the camera came in to {away}m"
                );
                before = seen;
            }
        }
    }

    /// A mark is gone well before the camera reaches the shell it stood for
    ///
    /// The whole exchange is ratios of a system's own reach, so this holds for
    /// a system of any size at once: a mark is gone twenty reaches out and the
    /// shell surface is at [`MARGIN`], sixteen times nearer. Reversed, a
    /// camera would fly into a lit sphere drawn over the very bodies the mark
    /// was standing in for.
    ///
    /// The two constants sit in different modules and nothing else holds them
    /// to each other. [`super::bodies::spawn::WORTH_HIDING`] is where the fade
    /// ends, and everything read off it is read in radians; `MARGIN` is a
    /// multiple of a length.
    #[test]
    fn a_mark_is_gone_before_the_camera_reaches_the_shell() {
        let gone_at = 1. / super::super::bodies::spawn::WORTH_HIDING;

        assert!(
            gone_at > MARGIN * 4.,
            "a mark lasts to {gone_at} reaches, against a shell at {MARGIN}"
        );
    }

    /// A shell only encloses the camera once it has settled onto its system
    ///
    /// What [`NEAREST`] is for. A mark is an angle, and an angle subtends a
    /// smaller length than the distance it is seen from, so a shell still
    /// drawn as one stands further off than it is wide and cannot be a sphere
    /// the camera is already inside.
    #[test]
    fn a_mark_never_encloses_the_camera() {
        // A metre out to the far rim of the galaxy.
        for away in [1f32, 1e9, 1e13, 1e17, 4.7e20] {
            // A system of no size at all, so the mark is the whole of what is
            // drawn.
            let drawn = plain(0., away);

            assert!(
                drawn < away,
                "a mark drawn {drawn}m wide was seen from {away}m"
            );
        }
    }

    /// The population scale leaves a sky rather than a wall
    ///
    /// The reported trouble, twice. A scale anchored at its bottom made every
    /// mark larger — turning the option on only swelled the sky — and one
    /// that added pixels per decade put the median system at an eight pixel
    /// radius, a galaxy of overlapping discs saying nothing about any of them.
    ///
    /// So the middle of the spread is drawn exactly as the ordinary sky draws
    /// it, the thin end under that, and the busy end a few times it. Read
    /// against the real spread: the median system holds 1.6 million, the
    /// thinnest ten, and the busiest on record thirty-two billion.
    #[test]
    fn the_population_scale_leaves_a_sky_rather_than_a_wall() {
        let ordinary = population_factor(POP_TYPICAL as u64);
        let thinnest = population_factor(10);
        let busiest = population_factor(32_000_000_000);

        assert!(
            (ordinary - 1.).abs() < 1e-3,
            "an ordinary system drew {ordinary} times its ordinary mark"
        );
        assert!(
            thinnest < 0.5,
            "a system of ten people drew {thinnest} times an ordinary mark"
        );
        assert!(
            (2.0..4.).contains(&busiest),
            "the busiest system on record drew {busiest} times one"
        );

        // And nine in twenty systems hold under 720 million, which is where
        // the crowding is: a few thousand marks of the bubble at once, so what
        // the sky is mostly made of has to stay near an ordinary mark rather
        // than swell into its neighbours. Reported as too much overlap at a
        // fifth of a decade, where this came to two and a half.
        let most = population_factor(720_000_000);
        assert!(most < 2., "the ninety-fifth percentile drew {most} times one");
    }

    /// Learning how far a system reaches leaves its mark alone
    ///
    /// The map draws every system at [`STAND_IN`] until it has asked what is
    /// inside one, which it does five light years out. All but the widest
    /// hundredth of systems reach under 1e14 metres, and from five light years
    /// the mark is wider than any of those, so the answer landing writes the
    /// size that was already there.
    #[test]
    fn learning_how_far_a_system_reaches_leaves_its_mark_alone() {
        let away = 5. * crate::space::LIGHT_YEAR as f32;
        let unknown = plain(STAND_IN, away);

        for extent in [STAND_IN, 1e13, 1e14] {
            let known = plain(extent, away);
            assert_eq!(
                known, unknown,
                "a system reaching {extent}m drew {known}m where the map \
                 had been drawing {unknown}m"
            );
        }
    }

    /// How much of the sky a system the map knows nothing about takes up from
    /// `ly` light years off, as an angular radius in radians
    fn seen(ly: f32) -> f32 {
        let away = ly * crate::space::LIGHT_YEAR as f32;
        plain(STAND_IN, away) / away
    }

    /// Twice as far off draws about half as large, across the middle of the map
    ///
    /// What makes a sky read as depth rather than as a field of equal dots.
    /// A mark held at a fixed angle draws the near systems and the far ones
    /// the same size, so nothing separates them and they crowd together as
    /// the camera pulls back.
    ///
    /// About half rather than half: the angle a mark never falls below is in
    /// there as well, and it is what a system holds on to whatever the
    /// distance.
    #[test]
    fn twice_as_far_off_is_about_half_as_large() {
        let near = seen(50.);
        let far = seen(100.);

        assert!(
            near / far > 1.6,
            "twice the distance drew {far} against {near}"
        );
    }

    /// And a shell goes on shrinking the whole way out
    ///
    /// Over the range the map is actually flown at: a spyglass of ten light
    /// years stands the camera some thirty back, and one of five hundred
    /// stands it fifteen hundred, so this is that end to end.
    #[test]
    fn a_shell_shrinks_as_the_camera_pulls_back() {
        let mut nearer = seen(20.);
        for out in [30., 50., 100., 200., 400., 800., 1500.] {
            let further = seen(out);
            assert!(
                further < nearer,
                "a system {out}ly off drew {further}, against {nearer} for \
                 one nearer in"
            );
            nearer = further;
        }

        assert!(
            seen(20.) > seen(1500.) * 5.,
            "the whole range only took a system from {} to {}",
            seen(20.),
            seen(1500.)
        );
    }

    /// The far sky holds one apparent size rather than falling away
    ///
    /// A size in the world comes to nothing at the far rim, which is most of
    /// what a map of the galaxy has on screen, so past a couple of hundred
    /// light years an angle takes over and the rim reads as one depth.
    ///
    /// Not what keeps the far sky lit — [`super::field`] floors the painted
    /// radius, so a mark would be drawn here even if this came back at
    /// nothing. What it holds is that the angle is the thing being held, which
    /// is what `ScalePopulation` multiplies and what a tall window draws
    /// wider.
    #[test]
    fn a_system_across_the_galaxy_is_still_a_mark() {
        assert!(seen(50_000.) >= ANGULAR);
    }

    /// A system the map knows nothing about is still drawn as a mark
    ///
    /// The stand-in is what keeps a system visible from light years off. Read
    /// at the spacing of the nearest stars, where the old floor drew a sphere
    /// tens of degrees across.
    #[test]
    fn a_neighbour_is_a_mark_rather_than_a_sky() {
        // A tenth of a light year, which is nearer than any real neighbour.
        let away = 0.1 * crate::space::LIGHT_YEAR as f32;
        let seen = plain(STAND_IN, away) / away;

        assert!(seen < 0.01, "a system {away}m off subtended {seen} radians");
    }

    /// A system is drawn to its own reach, whatever the map is looking into
    ///
    /// Every system carries how far it reaches, so a wide one is drawn as what
    /// it is from wherever it is looked at. Asked of the system the camera
    /// happens to be nearest instead, one star in the sky is drawn by what it
    /// is and the rest by what they are assumed to be, and the difference
    /// jumps from star to star as the crosshair moves.
    #[test]
    fn a_shell_is_drawn_to_its_own_systems_reach() {
        let mut app = sky();
        app.add_systems(Update, size_by_distance);
        // Alpha Centauri, which holds Proxima six million light seconds out,
        // and a neighbour of the middling sort beside it.
        shelled(&mut app, reaching(1, 5., 2.1e15));
        shelled(&mut app, at(2, 5.));
        app.update();

        let away = 5. * crate::space::LIGHT_YEAR as f32;
        assert_eq!(drawn(&mut app, 1), plain(2.1e15, away));
        assert_eq!(drawn(&mut app, 2), plain(STAND_IN, away));
    }

    /// A system with `population` living in it, `away` light years off
    fn peopled(address: i64, away: f64, population: u64) -> System {
        let mut system = at(address, away);
        system.population = population;
        system
    }

    /// The pixel radius the field paints a system's mark at
    ///
    /// What a reader actually sees, which is the world size the sizing left
    /// on the shell put through the same [`super::field::drawn_radius`] the
    /// field and the rings both read it by, floored as that mode floors.
    fn pixels(app: &mut App, address: i64, by_population: bool) -> f32 {
        let mut cameras = app.world_mut().query::<(&OrbitCamera, &Camera)>();
        let (orbit, camera) = cameras.single(app.world()).expect("a camera");
        let height = camera.logical_viewport_size().expect("a viewport").y;
        let cot_half_fov = camera.clip_from_view().y_axis.y;
        let eye = orbit.eye;

        let mut shells =
            app.world_mut().query_filtered::<(&Drawn, &System), With<Shell>>();
        let (drawn, system) = shells
            .iter(app.world())
            .find(|(_, system)| system.address == address)
            .expect("a shell for that system");
        let away =
            crate::space::metres(eye - system.position()).length() as f32;
        let per_pixel = world_per_pixel(cot_half_fov, height, away.max(1.));

        crate::systems::field::drawn_radius(
            &View::Map,
            drawn.0,
            per_pixel,
            crate::systems::field::floor(by_population),
        )
        .expect("a mark the map draws")
    }

    /// What a system holding `population` draws at from `away` light years,
    /// with the option on or off
    fn drawn_px(population: u64, away: f64, by_population: bool) -> f32 {
        let mut app = sky();
        app.insert_resource(ScalePopulation(by_population));
        app.add_systems(Update, size_by_distance);
        shelled(&mut app, peopled(1, away, population));
        app.update();

        pixels(&mut app, 1, by_population)
    }

    /// Population orders how large the marks are drawn
    ///
    /// Read at five thousand light years, where an ordinary mark comes to
    /// less than [`super::field::SMALLEST`] and the floor is what it is drawn
    /// at — so every rung of this ladder is a system the floor used to catch
    /// and draw at one size.
    ///
    /// A ladder rather than two ends, because what the option says is how
    /// many people live there: two systems drawn the same size at the same
    /// distance have to be two systems with the same population, and a floor
    /// or a clamp anywhere in the range breaks that for everything it
    /// catches.
    ///
    /// It starts at ten, the fewest anyone lives in. Nobody at all is not a
    /// smaller mark, it is no mark: [`super::visibility`] leaves an empty
    /// system off the sky, and
    /// `scaling_by_population_hides_an_empty_system` is where that is held.
    #[test]
    fn population_orders_how_large_the_marks_are_drawn() {
        let mut app = sky();
        app.insert_resource(ScalePopulation(true));
        app.add_systems(Update, size_by_distance);
        // The thinnest on record, an outpost, a town, the median system, a
        // world, and one of the busiest there is.
        let ladder =
            [10, 10_000, 200_000, 1_600_000, 1_000_000_000, 32_000_000_000];
        for (rung, population) in ladder.iter().enumerate() {
            shelled(&mut app, peopled(rung as i64, 5000., *population));
        }
        app.update();

        let mut last = 0.;
        for (rung, population) in ladder.iter().enumerate() {
            let drawn = pixels(&mut app, rung as i64, true);
            assert!(
                drawn > last,
                "{population} people drew at {drawn} px, no larger than the \
                 {last} px of the rung below"
            );
            last = drawn;
        }
    }

    /// An ordinary system is drawn as it is with the option off
    ///
    /// The reported trouble: turning the option on only made the sky larger.
    /// Every mark grew, so nothing stood out against the rest and the sky one
    /// remembers was gone.
    ///
    /// So [`POP_TYPICAL`] is the anchor rather than the bottom of the scale,
    /// and it holds at every distance — including out where an ordinary mark
    /// is the floor rather than the angle, which is where the floor has to be
    /// taken before the population rather than after it.
    #[test]
    fn an_ordinary_system_is_drawn_as_it_is_without_the_option() {
        for away in [20., 100., 1000., 5000.] {
            let off = drawn_px(POP_TYPICAL as u64, away, false);
            let on = drawn_px(POP_TYPICAL as u64, away, true);

            assert!(
                (on - off).abs() <= off * 1e-3,
                "an ordinary system {away} ly off drew {on} px with the \
                 option on against {off} px with it off"
            );
        }
    }

    /// A thinly populated one is drawn smaller than that, and a busy one
    /// larger
    ///
    /// The other half of the same reading. Smaller means smaller than the
    /// ordinary sky's mark, floor included: out at five thousand light years
    /// an ordinary mark is already the smallest the map paints, so a system
    /// of ten people is painted under a pixel — which is why
    /// [`super::field::floor`] stands down in this mode.
    #[test]
    fn a_thin_population_draws_smaller_and_a_busy_one_larger() {
        for away in [20., 100., 5000.] {
            let ordinary = drawn_px(POP_TYPICAL as u64, away, false);
            let thin = drawn_px(10, away, true);
            let busy = drawn_px(32_000_000_000, away, true);

            assert!(
                thin < ordinary * 0.5,
                "ten people {away} ly off drew {thin} px against an ordinary \
                 {ordinary} px"
            );
            assert!(
                busy > ordinary * 2.,
                "thirty-two billion {away} ly off drew {busy} px against an \
                 ordinary {ordinary} px"
            );
        }
    }

    /// How many shells were written to
    #[derive(Resource, Default)]
    struct Writes(usize);

    fn count_writes(
        mut writes: ResMut<Writes>,
        shells: Query<(), (Changed<Drawn>, With<Shell>)>,
    ) {
        writes.0 += shells.iter().count();
    }

    /// A world holding a camera and whatever shells are hung in it
    ///
    /// The camera carries a viewport of its own, answering nothing for its
    /// size otherwise, that being the render target's to say and nothing here
    /// bringing one up.
    fn sky() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Writes>();
        app.insert_resource(ScalePopulation(false));
        app.world_mut()
            .spawn((OrbitCamera::default(), crate::systems::tests::seeing()));
        app
    }

    /// A system with a shell standing around it
    fn shelled(app: &mut App, system: System) {
        app.world_mut().spawn((
            system,
            Shell,
            Transform::default(),
            Visibility::Visible,
        ));
    }

    /// How large the mark around the system at `address` was drawn
    fn drawn(app: &mut App, address: i64) -> f32 {
        let mut shells =
            app.world_mut().query_filtered::<(&Drawn, &System), With<Shell>>();
        shells
            .iter(app.world())
            .find(|(_, system)| system.address == address)
            .expect("a shell for that system")
            .0
            .0
    }

    /// And what its transform stands at, which is the grid's business and not
    /// the mark's
    fn placement(app: &mut App, address: i64) -> Vec3 {
        let mut shells = app
            .world_mut()
            .query_filtered::<(&Transform, &System), With<Shell>>();
        shells
            .iter(app.world())
            .find(|(_, system)| system.address == address)
            .expect("a shell for that system")
            .0
            .scale
    }

    /// What has been written to a shell so far
    fn writes(app: &App) -> usize {
        app.world().resource::<Writes>().0
    }

    /// A frame that moves nothing leaves a shell's size alone
    ///
    /// The field's mesh build and the pointer's indicator sizing both read a
    /// shell's size, and both look only at what changed since the last frame.
    /// Assigning it regardless hands them every star in the sky every frame,
    /// whether or not the camera has moved.
    #[test]
    fn a_resting_frame_leaves_a_shell_alone() {
        let mut app = sky();
        app.add_systems(Update, (size_by_distance, count_writes).chain());
        shelled(&mut app, at(1, 5.));

        // The shell arriving is itself a change, so the first frame is counted
        // whatever this system does. It is the second that says whether a
        // resting frame writes.
        app.update();
        let settled = writes(&app);

        app.update();
        assert_eq!(writes(&app), settled, "sized a shell that had not moved");
    }

    /// And a camera that has moved still resizes it
    ///
    /// Which is what the size is for. A guard that held through a zoom would
    /// leave every mark in the sky drawn at whatever it was when the camera
    /// last stood still.
    #[test]
    fn a_shell_is_sized_again_when_the_camera_moves() {
        let mut app = sky();
        app.add_systems(Update, (size_by_distance, count_writes).chain());
        shelled(&mut app, at(1, 5.));

        app.update();
        app.update();
        let settled = writes(&app);

        let mut cameras = app.world_mut().query::<&mut OrbitCamera>();
        cameras.single_mut(app.world_mut()).unwrap().eye =
            DVec3::new(2., 0., 0.);
        app.update();

        assert!(writes(&app) > settled, "left a shell at the size it was");
    }

    /// A descended shell's transform is left to its grid
    ///
    /// Down inside a system the shell wears a `Grid`, and its transform stops
    /// being the remainder left over from a galaxy cell: it is that sub-grid's
    /// own placement, which `big_space` reads to hang the camera and every
    /// body in the system. A mark size written there scales all of that
    /// instead of a mark — the map's size is the system's own extent, so the
    /// insides would be blown up by the width of the shell around them.
    ///
    /// Nothing writes it now, the mark having a [`Drawn`] of its own, so this
    /// holds by construction rather than by either sizing standing down. It
    /// is kept because the collision is easy to re-introduce: the transform is
    /// right there on the same entity.
    #[test]
    fn a_descended_shells_transform_is_left_to_its_grid() {
        let mut app = descended();
        app.update();

        assert_eq!(
            placement(&mut app, 1),
            Vec3::ONE,
            "wrote a mark size onto a descended system's sub-grid"
        );
    }

    /// And its mark is still sized, which is how it goes out
    ///
    /// The reported trouble. Giving way to the system's contents is the one
    /// thing a mark does as the camera comes inside one, so a descended shell
    /// is the last place sizing may stand down: held at whatever it was, the
    /// mark stopped swelling and fading and snapped to the pixel floor
    /// instead — a shell that went out in one frame rather than over half a
    /// second.
    ///
    /// Both views, since each has a sizing of its own and either may be the
    /// drawn one on the way in.
    #[test]
    fn a_descended_shell_is_still_sized() {
        let mut app = descended();
        app.update();

        let extent = 2.1e15;
        assert_eq!(
            drawn(&mut app, 1),
            extent * MARGIN,
            "a descended system's mark was left unsized"
        );
    }

    /// A world with the camera inside the widest system on record
    ///
    /// A fifth of a light year across, so a size written to the wrong place
    /// is off by light years rather than by a rounding. Both sizings run, and
    /// the shell wears the sub-grid the descent gives it.
    fn descended() -> App {
        let mut app = sky();
        app.init_resource::<StarExposure>();
        app.add_systems(Update, (size_by_distance, size_photometrically));
        app.world_mut().spawn((
            reaching(1, 5., 2.1e15),
            Shell,
            Transform::default(),
            Visibility::Visible,
            crate::space::system_grid(),
        ));
        app
    }

    /// A star is sized by the radius its point spread clears, and vanishes at
    /// the zero point
    ///
    /// The size law the map keeps for its billboard: the radius is
    /// `PSF_GROWTH·ln(energy)` over the exposed energy
    /// ([`galos_photometry::Magnitude::exposure`]), so it grows with the
    /// logarithm of brightness (a hundredfold brighter is a few pixels larger,
    /// never a hundredfold, and self-bounding with no cap) and is zero once the
    /// energy is under one — a star fainter than the zero point is not seen.
    #[test]
    fn a_star_is_sized_by_the_radius_its_point_spread_clears() {
        assert_eq!(psf_radius(0.5), 0., "under the zero point has no size");
        assert_eq!(psf_radius(1.), 0., "at the zero point has no size");
        assert!(psf_radius(10.) > 0., "over the zero point is drawn");
        assert!(
            psf_radius(100.) > psf_radius(10.),
            "a brighter star is drawn larger"
        );
        let dim = psf_radius(100.) as f64;
        let bright = psf_radius(10_000.) as f64;
        assert!(bright > dim, "a hundredfold brighter is larger");
        assert!(
            bright < 5. * dim,
            "a hundredfold brighter is a few times larger, not a hundredfold"
        );
    }
}
