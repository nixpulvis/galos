//! How large a system is drawn
//!
//! Two sizings, one per [`View`]. Neither is a real size: a system drawn at
//! its own scale is invisible from the next one over, so what is drawn is
//! whatever keeps it on screen and tells the viewer something.
//!
//! # Where this is going
//!
//! Once the camera is close enough to see inside a system, its extent should
//! come from what is in it, so that its drawn radius is the orbit of its
//! outermost body and zooming in lands on the system at its true size.
//! Further out that is far too small to see, and has to give way to the
//! sizings here, which exist so a system stays visible from light years off
//! and can carry population or anything else the galaxy view wants to show.
//!
//! The two want blending over the range where neither is right on its own,
//! rather than switching between them at a threshold.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;

use super::System;
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
}

#[derive(Resource, Debug, PartialEq)]
pub enum View {
    // #[default]
    Systems,
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

/// How large a system is drawn from far off, in radians
///
/// The angle a shell holds on screen while nothing else decides its size, so a
/// system stands out at about the same size wherever it is. Whether a system is
/// drawn at all is a question about how bright it is rather than how large, and
/// is not asked here.
const ANGULAR: f32 = 4e-4;

/// The least a shell is drawn at, in metres
///
/// Standing in for the size of the system, which is not known until its bodies
/// have been read. Once they have, this gives way to what they say — the
/// expression is the same either way, an angle plus a size, and only the size
/// is a guess.
const FLOOR: f32 = (8.5e-2 * crate::space::LIGHT_YEAR) as f32;

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
    camera: Query<&OrbitCamera>,
    systems: Query<&System>,
    mut shells: Query<(&mut Transform, &ChildOf), With<Shell>>,
) {
    if !shells.is_empty() {
        let Ok(eye) = camera.single().map(|c| c.eye) else { return };

        // TODO(#46): We should still change rgba color/emmisivity as needed.
        for (mut shell, child_of) in shells.iter_mut() {
            let Ok(system) = systems.get(child_of.parent()) else { continue };
            let away = crate::space::metres(eye - DVec3::from(system.position))
                .length() as f32;
            let mut scale = ANGULAR * away + FLOOR;
            if scale_population.0 {
                scale *=
                    population_factor(system.population, stats.population_mean);
            }
            shell.scale = Vec3::splat(scale);
        }
    }
}

/// Draw every system the same size, whatever the camera is doing
///
/// This view is a picture of where things are rather than of how far away
/// they are, so nothing here reads the camera.
pub fn size_uniformly(mut shells: Query<&mut Transform, With<Shell>>) {
    // TODO(#46): Change rgba color/emmisivity. The goal is to fade out to
    // transparent when they are too far away.
    for mut shell in shells.iter_mut() {
        shell.scale = Vec3::splat((1e-2 * crate::space::LIGHT_YEAR) as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::tests::system;

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
}
