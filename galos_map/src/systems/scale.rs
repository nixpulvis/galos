//! How large a system is drawn
//!
//! Two sizings, one per [`View`]. Neither is a real size: a system drawn at
//! its own scale is invisible from the next one over, so what is drawn is
//! whatever keeps it on screen and tells the viewer something.
//!
//! [`View::Systems`] blends the two: a mark that says a system is there, held
//! at a size in the world so the sky reads as depth, and the system's own
//! extent, which is what is left once the camera is near enough for the mark
//! to have been squeezed down to nothing. [`View::Stars`] does not, and is the
//! older of the two.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;

use super::System;
use super::bodies::STAND_IN;
use super::bodies::spawn::Body;
use super::labels::{depth_of, world_per_pixel};
use super::roundness::Roundness;
use super::spawn::Shell;
use bevy::math::DVec3;
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    app.insert_resource(View::Systems);
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
            .ambiguous_with(size_uniformly)
            .run_if(resource_equals(View::Systems)),
    );
    app.add_systems(
        Update,
        size_uniformly
            .in_set(MapSet::Present)
            .ambiguous_with(size_by_distance)
            .run_if(resource_equals(View::Stars)),
    );
    // Reads where a body ended up rather than deciding it, and `big_space`
    // writes that during `PostUpdate`, so it waits as `pointing::size_bodies`
    // does. What it writes is read by the next frame's propagation, which is
    // a frame behind and nowhere near enough movement to see.
    app.add_systems(PostUpdate, size_inside.after(TransformSystems::Propagate));
}

#[derive(Resource, Debug, PartialEq)]
pub enum View {
    // TODO: Settle this one by eye. The size a shell is drawn at now falls
    // to the system's own extent rather than to a fixed floor, and nothing
    // has looked at what that does across the whole range, a crowded sky
    // especially.
    // #[default]
    Systems,
    // TODO(#46): Draw a star at the size and color it actually is, and give
    // this a name that says so. `size_uniformly` draws every system at a
    // hundredth of a light year, whatever the star is and wherever the camera
    // stands, which was a stand-in from before the map was laid out in metres
    // and a star had a radius worth drawing. Whether a shell belongs in this
    // view at all, or only what is inside one, is the same question.
    Stars,
    // TODO(#44): Bodies
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
const MARGIN: f32 = 1.2;

/// How large a system is drawn, in metres
///
/// The system itself, and a mark around it saying one is there. A system at
/// its true size is invisible from the next one over, and a mark is no use
/// once the camera is inside the system, so the two are added and each
/// answers for the range the other cannot.
///
/// The mark is [`MARK`] across the middle of the map, which is a size in the
/// world: twice as far off draws about half as large, and the sky reads as
/// depth. It gives way to an angle at either end, where a size in the world is
/// too large to get past or too small to see. Being an angle holds it still on
/// screen as the camera moves, and close in that is what lets the system's own
/// extent come up through it: the shell settles onto the outermost orbit
/// rather than arriving at it.
///
/// `prominence` scales the mark and not the system. A busy system is worth a
/// larger mark; it is not worth a larger volume, and a quarter of one would
/// put the shell inside the orbits it stands around.
fn shell(extent: f32, away: f32, prominence: f32) -> f32 {
    let mark = MARK.min(NEAREST * away) + ANGULAR * away;

    extent * MARGIN + mark * prominence
}

/// Draw each system large enough to be seen from where the camera is
///
/// The size goes on the [`Shell`], not on the [`System`] holding it, so that
/// labels and anything else hanging off the system keep their own size.
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
    contents: Res<super::bodies::Contents>,
    camera: Query<(&OrbitCamera, &Camera)>,
    systems: Query<&System>,
    roundness: Res<Roundness>,
    mut shells: Query<(&mut Transform, &ChildOf, &mut Mesh3d), With<Shell>>,
) {
    if !shells.is_empty() {
        let Ok((orbit, camera)) = camera.single() else { return };
        let Some(viewport) = camera.logical_viewport_size() else { return };
        let cot_half_fov = camera.clip_from_view().y_axis.y;
        let eye = orbit.eye;

        // TODO(#46): We should still change rgba color/emmisivity as needed.
        for (mut drawn, child_of, mut mesh) in shells.iter_mut() {
            let Ok(system) = systems.get(child_of.parent()) else { continue };
            let away = crate::space::metres(eye - DVec3::from(system.position))
                .length() as f32;
            // Only the one system the map is holding the insides of can say
            // how far it reaches. Every other is drawn at the stand-in, which
            // is what not knowing looks like.
            let extent = if contents.holds(system.address) {
                contents.extent().unwrap_or(STAND_IN)
            } else {
                STAND_IN
            };
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

/// Draw every system the same size, whatever the camera is doing
///
/// This view is a picture of where things are rather than of how far away they
/// are, so the size here reads nothing. How round that size is drawn is a
/// different question and has to ask, since a shell held at one size in the
/// world still covers everything from half a pixel to half the screen.
pub fn size_uniformly(
    camera: Query<(&OrbitCamera, &Camera)>,
    systems: Query<&System>,
    roundness: Res<Roundness>,
    mut shells: Query<(&mut Transform, &ChildOf, &mut Mesh3d), With<Shell>>,
) {
    let size = (1e-2 * crate::space::LIGHT_YEAR) as f32;
    // Nothing to be round for where there is no viewport to be round in, and
    // the size is written either way.
    let seen = match camera.single() {
        Ok((orbit, camera)) => camera.logical_viewport_size().map(|viewport| {
            (orbit.eye, camera.clip_from_view().y_axis.y, viewport.y)
        }),
        Err(_) => None,
    };

    // TODO(#46): Change rgba color/emmisivity. The goal is to fade out to
    // transparent when they are too far away.
    for (mut drawn, child_of, mut mesh) in shells.iter_mut() {
        // Only where it moved, as everything that sizes a shell is. The size
        // here is one number for the whole map, so past the frame a shell is
        // spawned this never writes at all.
        if drawn.scale.x != size {
            drawn.scale = Vec3::splat(size);
        }

        let Some((eye, cot_half_fov, height)) = seen else { continue };
        let Ok(system) = systems.get(child_of.parent()) else { continue };
        let away = crate::space::metres(eye - DVec3::from(system.position))
            .length() as f32;

        let per_pixel = world_per_pixel(cot_half_fov, height, away.max(1.));
        let wanted = roundness.at(&mesh.0, size / per_pixel);
        if mesh.0 != *wanted {
            mesh.0 = wanted.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::tests::{at, system};

    /// A shell always holds the system it stands around
    ///
    /// What the rest rests on. Swept over the whole range of distances the map
    /// allows and over extents from a compact system to the widest on record,
    /// since a floor-shaped mistake passes on a large system and fails on
    /// every smaller one.
    #[test]
    fn a_shell_holds_the_system_inside_it() {
        // A light second out to a light hour, which is compact to the widest
        // on record.
        for extent in [3e8f32, 1.5e12, 1.7e14, 2.1e14] {
            // A metre out to the far rim of the galaxy.
            for away in [1f32, 1e9, 1e13, 1e17, 4.7e20] {
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
    /// Halving the distance halves what is left over above the true size, so
    /// there is no distance at which the shell stops shrinking and nothing to
    /// cross.
    #[test]
    fn a_shell_settles_onto_its_system() {
        let extent = 1.7e14;
        let held = extent * MARGIN;

        let far = shell(extent, 1e16, 1.) - held;
        let near = shell(extent, 5e15, 1.) - held;

        assert!(
            (far / near - 2.).abs() < 1e-3,
            "half the distance left {near} over against {far}"
        );
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
        app.init_resource::<crate::systems::bodies::Contents>();
        app.insert_resource(ScalePopulation(false));
        app.add_plugins(crate::systems::roundness::plugin);
        app.world_mut()
            .spawn((OrbitCamera::default(), crate::systems::tests::seeing()));
        app
    }

    /// A system `away` light years off, with a shell standing around it
    fn shelled(app: &mut App, address: i64, away: f64) {
        let system = app.world_mut().spawn(at(address, away)).id();
        let shell = app
            .world_mut()
            .spawn((Shell, Transform::default(), Mesh3d::default()))
            .id();
        app.world_mut().entity_mut(system).add_child(shell);
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
        shelled(&mut app, 1, 5.);

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
        shelled(&mut app, 1, 5.);

        app.update();
        app.update();
        let settled = writes(&app);

        let mut cameras = app.world_mut().query::<&mut OrbitCamera>();
        cameras.single_mut(app.world_mut()).unwrap().eye =
            DVec3::new(2., 0., 0.);
        app.update();

        assert!(writes(&app) > settled, "left a shell at the size it was");
    }

    /// A shell drawn at one size for the whole map is written once
    ///
    /// This view draws every system the same size whatever the camera is
    /// doing, so past the frame a shell is spawned there is never anything to
    /// write at all.
    #[test]
    fn an_evenly_drawn_shell_is_sized_once() {
        let mut app = sky();
        app.add_systems(Update, (size_uniformly, count_writes).chain());
        shelled(&mut app, 1, 5.);

        app.update();
        let settled = writes(&app);

        let mut cameras = app.world_mut().query::<&mut OrbitCamera>();
        cameras.single_mut(app.world_mut()).unwrap().eye =
            DVec3::new(2., 0., 0.);
        app.update();

        assert_eq!(writes(&app), settled, "sized a shell that draws one size");
    }
}
