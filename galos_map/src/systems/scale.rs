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
//! [`super::spawn::photometry`], which paints it.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;

use super::System;
use super::bodies::spawn::{Body, WORTH_KEEPING, WORTH_SIZING};
use super::labels::{depth_of, world_per_pixel};
use super::roundness::Roundness;
use super::spawn::{Shell, StarExposure, StarSprite};
use bevy::camera::visibility::ViewVisibility;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::Grid;
use galos_photometry::{Distance, Magnitude};

pub fn plugin(app: &mut App) {
    app.insert_resource(View::Map);
    app.insert_resource(ScalePopulation(false));
    app.init_resource::<SystemsStats>();
    // Reads what the fetch spawned and the despawn took away, so it belongs
    // after both, and answers `size_by_distance`, so it belongs before that.
    app.add_systems(
        Update,
        recount.in_set(MapSet::Present).before(size_by_distance),
    );
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
    // `pull_stars` once relocated the realistic view's shell billboards onto a
    // near plane to keep the f32 clip transform from tearing them; the field
    // draws that view now, in screen space, so nothing relocates a shell any
    // more. The function is kept for its tests until the shell-draw machinery
    // is retired wholesale; see [`crate::systems::field`].
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
    // the Milky Way — is not drawn yet; see galaxy.md.
    Realistic,
}

#[derive(Resource, Debug)]
pub struct ScalePopulation(pub bool);

/// What the systems on the map add up to
///
/// Figures drawn from every system at once, which is more than anything
/// wanting one should have to walk to find out. Held so that they are worked
/// out when the systems behind them move, rather than once per frame
/// regardless.
///
/// Over every system loaded, not the ones the spyglass reaches. That narrower
/// question is [`super::InReach`]'s, and it is asked and answered afresh every
/// frame because the camera moving changes the answer without anything on the
/// map having moved at all.
#[derive(Resource, Debug, Default)]
pub struct SystemsStats {
    /// What the average system is populated by
    pub population_mean: f64,
}

/// Keep the stats answering to what is on the map
///
/// Three things move them, and they are not all visible the same way. A row
/// arriving and a row being written over both mark a [`System`] changed,
/// which covers the two ways [`super::spawn::spawn_systems`] admits one: a
/// system fetched again has its row inserted over the old one rather than
/// being respawned, and what it carries is free to differ. A system leaving
/// the map takes its component with it, so there is nothing left to mark and
/// it has to be asked after separately.
pub fn recount(
    systems: Query<&System>,
    touched: Query<(), Changed<System>>,
    mut gone: RemovedComponents<System>,
    mut stats: ResMut<SystemsStats>,
) {
    // Both asked before either is acted on. Removals are read through a
    // cursor, and one left unread is one held over to be answered again on
    // the next frame that recounts.
    let any_gone = gone.read().count() > 0;
    let any_touched = !touched.is_empty();
    if !any_gone && !any_touched {
        return;
    }

    let (total, count) = systems
        .iter()
        .fold((0., 0.), |(t, n), s| (t + s.population as f64, n + 1.));
    // An empty map has no average to give. Dividing to find one anyway
    // yields a NaN, and every star sized against it draws at no size at all.
    stats.population_mean = if count > 0. { total / count } else { 0. };
}

/// How strongly population pulls a system's size around
///
/// Applied to the log of how a system compares to the average, so each step
/// of this is one e-fold of population.
const POP_SPREAD: f32 = 0.2;

/// Bounds on what population may do to a system's size
///
/// Population runs from nobody to tens of billions. Left unbounded the busy
/// end swallows the map and the quiet end shrinks to nothing.
const POP_MIN: f32 = 0.25;
const POP_MAX: f32 = 4.;

/// How much bigger or smaller a system draws for its population
///
/// One at the average, larger above it, smaller below, and never zero or
/// negative. An uninhabited system is still a system and has to be drawn:
/// most of the galaxy is uninhabited, and scaling by the bare log of the
/// ratio sent all of it to a negative size.
fn population_factor(population: u64, average: f64) -> f32 {
    if average <= 0. {
        return 1.;
    }
    let ratio = (population as f64 / average) as f32;
    (1. + POP_SPREAD * ratio.max(f32::MIN_POSITIVE).ln())
        .clamp(POP_MIN, POP_MAX)
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
/// Which is what this and [`ANGULAR`] are held to: the two of them together,
/// at the most [`POP_MAX`] can make of a mark, stay well under one, so a mark
/// is always a smaller length than the distance it is seen from.
/// `a_mark_never_encloses_the_camera` is what holds them.
const NEAREST: f32 = 4e-3;

/// How large a system is drawn from far off, in radians
///
/// What is left once distance has taken the rest away, which is past about two
/// hundred light years. Half a pixel down the same window: by then every
/// system in the sky is the same dot, and a mark that went on shrinking would
/// leave nothing to see at all.
///
/// TODO(#72): Half a pixel is under what a sphere can be sampled at, and this
/// is [`SMALLEST_DRAWN`]'s trouble at half the size. A mark this wide falls
/// across two to five of the four samples a pixel is drawn from, so its
/// brightness nearly doubles and halves with where it lands between them.
/// Down a 600 line window it is 0.29 of a pixel, under the 0.354 a lattice of
/// four to the pixel can miss entirely, and the mark blinks out at some
/// positions altogether.
const ANGULAR: f32 = 4e-4;

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
const MARGIN: f32 = 1.2;

/// How large a system is drawn, in metres
///
/// The wider of the system itself and a mark saying one is there. A system at
/// its true size is invisible from the next one over, and a mark is no use
/// once the camera is inside the system, so each answers for the range the
/// other cannot.
///
/// The mark is [`MARK`] across the middle of the map, which is a size in the
/// world: twice as far off draws about half as large, and the sky reads as
/// depth. It gives way to an angle at either end, where a size in the world is
/// too large to get past or too small to see. Being an angle holds it still on
/// screen as the camera moves, and close in that is what lets the system's own
/// extent come up through it: the mark shrinks into the shell rather than the
/// shell arriving out of it.
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
/// `prominence` scales the mark and not the system. A busy system is worth a
/// larger mark; it is not worth a larger volume, and a quarter of one would
/// put the shell inside the orbits it stands around.
fn shell(extent: f32, away: f32, prominence: f32) -> f32 {
    let mark = MARK.min(NEAREST * away) + ANGULAR * away;
    let seen = extent / away.max(1.);
    let counting =
        ((seen - WORTH_SIZING) / (WORTH_KEEPING - WORTH_SIZING)).clamp(0., 1.);

    (extent * MARGIN * counting).max(mark * prominence)
}

/// Draw each system large enough to be seen from where the camera is
///
/// The size goes on the shell, which now shares an entity with the [`System`]
/// it stands for. The labels hung off that entity are drawn far smaller and
/// divide the shell's scale back out; see [`super::labels::face_camera`].
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
pub fn size_by_distance(
    scale_population: Res<ScalePopulation>,
    stats: Res<SystemsStats>,
    camera: Query<(&OrbitCamera, &Camera)>,
    roundness: Res<Roundness>,
    mut shells: Query<
        (&mut Transform, &System, &mut Mesh3d, &Visibility),
        With<Shell>,
    >,
) {
    if !shells.is_empty() {
        let Ok((orbit, camera)) = camera.single() else { return };
        let Some(viewport) = camera.logical_viewport_size() else { return };
        let cot_half_fov = camera.clip_from_view().y_axis.y;
        let eye = orbit.eye;

        // TODO(#46): We should still change rgba color/emmisivity as needed.
        for (mut drawn, system, mut mesh, visible) in shells.iter_mut() {
            // Out of the spyglass is not drawn, so the size it would draw at
            // is not worked out. Its scale is left where it last stood, which
            // is close enough for the frame it comes back on.
            if *visible == Visibility::Hidden {
                continue;
            }
            let away = crate::space::metres(eye - DVec3::from(system.position))
                .length() as f32;
            let extent = system.reach();
            let prominence = if scale_population.0 {
                population_factor(system.population, stats.population_mean)
            } else {
                1.
            };

            let size = shell(extent, away, prominence);
            // Only where it moved, as `size_inside` is. A scale assigned
            // regardless marks every shell in the sky changed every frame, and
            // both the transform propagation and the mesh extraction that read
            // it are gated on that mark.
            if drawn.scale.x != size {
                drawn.scale = Vec3::splat(size);
            }

            // A shell coming back from the realistic view was turned to face
            // the eye. A sphere reads the same at any turn, but leave it square
            // so nothing downstream meets a tilted mark.
            if drawn.rotation != Quat::IDENTITY {
                drawn.rotation = Quat::IDENTITY;
            }

            // Measured out along the line to the system rather than into the
            // view, which is what the size itself is measured by. A mark off
            // to one side is drawn a little coarser than it strictly asks for,
            // by well under the pixel the rungs are set by, and the two agree
            // about how far away a system is.
            let per_pixel =
                world_per_pixel(cot_half_fov, viewport.y, away.max(1.));
            let wanted = roundness.at(&mesh.0, size / per_pixel);
            if mesh.0 != *wanted {
                mesh.0 = wanted.clone();
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
/// writes relative to the camera, as [`super::pointing::size_bodies`] measures
/// the same body for the same reason.
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

/// How fast a star's drawn radius grows with brightness, in screen pixels per
/// e-fold of flux
///
/// A star is a point; what reaches the screen is the instrument's point spread,
/// the same shape ([`super::spawn::star_psf`]) for every star. A brighter star
/// is not drawn wider — it clears more of that one fixed shape above the eye's
/// floor. That cleared radius grows with the *logarithm* of brightness
/// (galaxy.md), the law the eye reads by and the one that never runs away: each
/// doubling of flux adds a fixed step, so even the sky's most luminous stars
/// stay a bounded glint with no cap to impose. Tuned against a long exposure of
/// a real sky.
const PSF_GROWTH: f64 = 0.45;

/// The smallest a drawn star may be, as a radius in screen pixels
///
/// A star that clears the floor is drawn at least this large so it lands as a
/// stable dot rather than a sub-pixel speck that flickers as the camera moves
/// (galaxy.md's "smallest mark that draws stably"). Most of the sky sits here —
/// a field of tiny dots — with only the brighter stars grown past it by their
/// point spread. Not a cap: the floor is the pixel grid, and brightness above
/// it still grows the star.
const DOT_RADIUS: f32 = 0.6;

/// The size a star below the flux floor shrinks to, as a fraction of a pixel
///
/// Not zero. A star fainter than the exposure floor is not drawn, but a name
/// may still be hung on it — selected, pointed at, or a route stop — and a
/// name is a child of the star's entity and inherits its scale, so a zero
/// would collapse the name to nothing (see [`super::labels::face_camera`]). A
/// thousandth of a pixel is far below what any screen shows, so the star stays
/// invisible while the name it carries keeps a scale to be read against. This
/// is how the realistic view keeps a marked-but-undrawn star named, the way the
/// map view keeps a selection named from behind the spyglass.
const UNSEEN: f32 = 1e-3;

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
// itself. See galaxy.md "The instrument" and roadmap item 7 "Real mode". DO NOT
// FORGET THIS.
fn psf_radius(energy: f64) -> f32 {
    if energy <= 1. {
        return 0.;
    }
    ((PSF_GROWTH * energy.ln()) as f32).max(DOT_RADIUS)
}

/// Draw each system as its point spread, for the realistic view
///
/// A star is a billboard carrying the fixed Moffat point spread (see
/// [`super::spawn::star_psf`]), sized to the radius that spread clears above
/// the eye's floor by [`psf_radius`] and turned to face the camera. A brighter
/// star clears more of the same profile, so it draws larger and a fainter one
/// smaller, both by the log of their brightness; opening the exposure grows
/// them all and draws fainter ones in; and a star whose peak is under the floor
/// has no size and drops out. The tint and the core's shape are the sprite's.
///
/// A shell the camera has descended into is skipped: it wears a [`Grid`] now
/// and its transform is that sub-grid's placement, read by `big_space`, not a
/// billboard's facing. Turning it to face the eye would tilt the sub-grid the
/// camera hangs in and swing the galaxy's whole sky off with it; the entity is
/// left to `super::bodies::spawn::draw`, which squares its transform when it
/// hands the shell over to the grid.
pub(crate) fn size_photometrically(
    camera: Query<(&OrbitCamera, &Camera)>,
    exposure: Res<StarExposure>,
    sprite: Res<StarSprite>,
    mut shells: Query<
        (&mut Transform, &System, &mut Mesh3d, &Visibility),
        (With<Shell>, Without<Grid>),
    >,
) {
    let Ok((orbit, camera)) = camera.single() else {
        return;
    };
    let Some(viewport) = camera.logical_viewport_size() else {
        return;
    };
    let cot_half_fov = camera.clip_from_view().y_axis.y;
    let zero_point = exposure.zero_point();
    // Turned to line up with the camera, written straight in as a local
    // rotation the way [`super::labels::face_camera`] does: a system is never
    // itself rotated, so its local frame is the world's.
    let facing = orbit.rotation;
    for (mut drawn, system, mut mesh, visible) in shells.iter_mut() {
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
        if drawn.scale.x != size {
            drawn.scale = Vec3::splat(size);
        }
        if drawn.rotation != facing {
            drawn.rotation = facing;
        }
        if mesh.0 != sprite.quad {
            mesh.0 = sprite.quad.clone();
        }
    }
}

/// Draw each system's shell on a plane the camera carries, not where it is
///
/// A shell sits at its system's true coordinate, up to ~1e17 m from the
/// floating origin, and there the f32 clip transform `view_proj · model` runs
/// out of bits: the mesh comes out torn or gone, and which triangles survive
/// turns with the camera, so half the sky blinks in and out as it pitches and
/// zooms (see `docs/night-sky.md`). So the shell is not drawn where it is. Its
/// centre is projected on the CPU in f64 and the mesh placed on the eye→system
/// ray at [`super::pointing::overlay_plane`]'s depth — the same near plane the
/// rings are drawn on, close enough to the origin to stay precise — kept at the
/// pixel size and facing [`size_by_distance`] and [`size_photometrically`]
/// worked out, so it reads the same while it draws at all.
///
/// Only the shells the map is still standing in for. One flown into wears a
/// [`Grid`] and is drawn as the system itself, near the origin, where the
/// precision holds.
///
/// The `GlobalTransform` is overwritten, not the `Transform`: a system's true
/// coordinate is what everything else asks of it — where its name is laid out,
/// where the pointer is tested — so the shell keeps it and only what reaches
/// the renderer is moved. Written after `big_space` has propagated, so it is
/// this that the renderer reads.
pub fn pull_stars(
    camera: Query<(&OrbitCamera, &GlobalTransform)>,
    mut shells: Query<
        (&System, &Transform, &mut GlobalTransform, &ViewVisibility),
        (With<Shell>, Without<Grid>, Without<OrbitCamera>),
    >,
) {
    let Ok((orbit, eye)) = camera.single() else { return };
    let eye_render = eye.translation();
    let forward = (orbit.rotation * Vec3::NEG_Z).as_dvec3();
    // Just past the near clip plane: the smallest coordinate a star can be
    // drawn at without being clipped, and so the most precise. The plane is a
    // ten-thousandth of the zoom, held under [`crate::camera::NEAR_CEILING`],
    // which keeps this close to the eye's own neighbourhood at every zoom —
    // where the f32 transform holds — rather than out where a field spread to
    // the edges tears. Three times it, to clear the plane at the corners of
    // the view where the ray runs longer than down the middle.
    let near = ((orbit.radius as f64 * crate::space::LIGHT_YEAR) as f32
        * crate::camera::NEAR_FRACTION)
        .min(crate::camera::NEAR_CEILING)
        * 3.;

    // How far a shell has to be for pulling it in to be worth it. Nearer than
    // this its true coordinate is already in the eye's own precise
    // neighbourhood, where it draws cleanly at full size; pulling it onto the
    // near plane would shrink its mesh below what a float resolves at the eye's
    // own magnitude and collapse it — which is what tore a shell apart as the
    // camera came in on it. Well under where the clip transform starts to tear
    // one left where it is.
    const PULL_BEYOND: f32 = 1e16;

    for (system, local, mut global, visible) in &mut shells {
        if !visible.get() {
            continue;
        }
        // Close enough to draw where it is: see [`PULL_BEYOND`]. Leaving it
        // keeps its true depth, so it still sorts against whatever it is near.
        if global.translation().length() < PULL_BEYOND {
            continue;
        }
        let offset =
            crate::space::metres(DVec3::from(system.position) - orbit.eye);
        let away = offset.length() as f32;
        // Behind the eye, or sitting on it: nothing to draw where it is seen.
        // Its mesh is left with no size rather than at a coordinate that tears.
        if offset.dot(forward) <= 0. || away < 1. {
            *global = GlobalTransform::from(Transform::from_scale(Vec3::ZERO));
            continue;
        }
        // Along the ray to the system, out at the near plane: the same screen
        // point, at a depth the f32 transform holds.
        let direction = (offset / away as f64).as_vec3();
        let translation = eye_render + direction * near;
        // The size was worked out for the true distance; a pixel covers that
        // much less world out here, so the world size shrinks by the same ratio
        // and the mark holds the pixels it was given.
        let scale = local.scale * (near / away);
        *global = GlobalTransform::from(Transform {
            translation,
            rotation: local.rotation,
            scale,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::bodies::STAND_IN;
    use crate::systems::tests::{at, reaching, system};

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
                for prominence in [POP_MIN, 1., POP_MAX] {
                    let drawn = shell(extent, away, prominence);

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

        assert!(shell(extent, 1e18, 1.) > held, "the mark had already gone");
        assert_eq!(shell(extent, 1e16, 1.), held);
        assert_eq!(shell(extent, 1e12, 1.), held);
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
        let widest = shell(2.1e15, away, 1.);
        let ordinary = shell(1e14, away, 1.);
        let mark = shell(0., away, 1.);

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
            let drawn = shell(extent, away, 1.);

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
            let mut before = shell(extent, away, 1.) / away;

            while away > extent {
                away *= 0.98;
                let seen = shell(extent, away, 1.) / away;

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
            // drawn, and as large as a population can make one.
            let drawn = shell(0., away, POP_MAX);

            assert!(
                drawn < away,
                "a mark drawn {drawn}m wide was seen from {away}m"
            );
        }
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
        let unknown = shell(STAND_IN, away, 1.);

        for extent in [STAND_IN, 1e13, 1e14] {
            let known = shell(extent, away, 1.);
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
        shell(STAND_IN, away, 1.) / away
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

    /// The far sky does not go dark
    ///
    /// A size in the world comes to nothing at the far rim, which is most of
    /// what a map of the galaxy has on screen.
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
        let seen = shell(STAND_IN, away, 1.) / away;

        assert!(seen < 0.01, "a system {away}m off subtended {seen} radians");
    }

    /// A world that keeps the stats current, and nothing else
    fn map() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SystemsStats>();
        app.add_systems(Update, recount);
        app
    }

    /// A system with `population` living in it
    fn populated(address: i64, population: u64) -> System {
        let mut system = system(address);
        system.population = population;
        system
    }

    /// What the map currently takes the average population to be
    fn mean(app: &App) -> f64 {
        app.world().resource::<SystemsStats>().population_mean
    }

    /// A system written over carries the average with it
    ///
    /// The case a count of the systems on the map cannot see, and the reason
    /// the recount is asked for by what has changed rather than by how many
    /// there are. A row fetched again is inserted over the one already there
    /// rather than respawned, so nothing arrives and nothing leaves, and the
    /// population it carries is free to differ from the one it replaces.
    #[test]
    fn a_system_written_over_moves_the_average() {
        let mut app = map();
        let entity = app.world_mut().spawn(populated(1, 100)).id();
        app.world_mut().spawn(populated(2, 300));
        app.update();
        assert_eq!(mean(&app), 200.);

        app.world_mut().entity_mut(entity).insert(populated(1, 700));
        app.update();
        assert_eq!(
            mean(&app),
            500.,
            "kept the population of a row that had been replaced"
        );
    }

    /// A system arriving moves the average
    #[test]
    fn a_system_arriving_moves_the_average() {
        let mut app = map();
        app.world_mut().spawn(populated(1, 100));
        app.update();
        assert_eq!(mean(&app), 100.);

        app.world_mut().spawn(populated(2, 300));
        app.update();
        assert_eq!(mean(&app), 200.);
    }

    /// A system leaving moves the average
    ///
    /// Leaving takes the component with it, so there is nothing left to mark
    /// as changed and this is the one of the three that has to be asked
    /// after separately.
    #[test]
    fn a_system_leaving_moves_the_average() {
        let mut app = map();
        let entity = app.world_mut().spawn(populated(1, 100)).id();
        app.world_mut().spawn(populated(2, 300));
        app.update();
        assert_eq!(mean(&app), 200.);

        app.world_mut().entity_mut(entity).despawn();
        app.update();
        assert_eq!(mean(&app), 300., "kept a system that had gone");
    }

    /// A frame that moves nothing leaves the average where it stands
    ///
    /// This is what holding the average is for. The systems it averages are
    /// walked when they move, and not on the frames in between.
    #[test]
    fn a_resting_frame_leaves_the_average_alone() {
        let mut app = map();
        app.world_mut().spawn(populated(1, 100));
        app.update();

        // Set to something the map does not add up to, so that only a
        // recount would put it back.
        app.world_mut().resource_mut::<SystemsStats>().population_mean = 42.;
        app.update();
        assert_eq!(mean(&app), 42., "recounted a map that had not moved");
    }

    /// An empty map has no average rather than a NaN
    ///
    /// Every star's size is multiplied by a factor worked out from this, so
    /// a NaN here would spread to the size of every star drawn.
    #[test]
    fn an_emptied_map_has_no_average() {
        let mut app = map();
        let entity = app.world_mut().spawn(populated(1, 100)).id();
        app.update();
        assert_eq!(mean(&app), 100.);

        app.world_mut().entity_mut(entity).despawn();
        app.update();
        assert_eq!(mean(&app), 0., "averaged an empty map to a NaN");
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
        assert_eq!(drawn(&mut app, 1), shell(2.1e15, away, 1.));
        assert_eq!(drawn(&mut app, 2), shell(STAND_IN, away, 1.));
    }

    /// How many shells were written to
    #[derive(Resource, Default)]
    struct Writes(usize);

    fn count_writes(
        mut writes: ResMut<Writes>,
        shells: Query<(), (Changed<Transform>, With<Shell>)>,
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
        app.init_resource::<Assets<Mesh>>();
        app.init_resource::<SystemsStats>();
        app.init_resource::<Writes>();
        app.insert_resource(ScalePopulation(false));
        app.add_plugins(crate::systems::roundness::plugin);
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
            Mesh3d::default(),
            Visibility::Visible,
        ));
    }

    /// How large the shell around the system at `address` was drawn
    fn drawn(app: &mut App, address: i64) -> f32 {
        let mut shells = app
            .world_mut()
            .query_filtered::<(&Transform, &System), With<Shell>>();
        shells
            .iter(app.world())
            .find(|(_, system)| system.address == address)
            .expect("a shell for that system")
            .0
            .scale
            .x
    }

    /// What has been written to a shell so far
    fn writes(app: &App) -> usize {
        app.world().resource::<Writes>().0
    }

    /// A frame that moves nothing leaves a shell's size alone
    ///
    /// Both the transform propagation and the mesh extraction that read a
    /// shell's size look only at what changed since the last frame. Assigning
    /// it regardless hands them every star in the sky every frame, whether or
    /// not the camera has moved.
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
