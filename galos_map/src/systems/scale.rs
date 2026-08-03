//! How large a system's stars are drawn
//!
//! Two sizings, one per [`View`]. Neither is a real size: a star drawn at its
//! own scale is invisible from the next system over, so what is drawn is
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
use super::spawn::Star;
use bevy::math::DVec3;
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    app.insert_resource(View::Systems);
    app.insert_resource(ScalePopulation(false));
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

/// Draw each star large enough to be seen from where the camera is
///
/// The size goes on the [`Star`], not on the [`System`] holding it, so that
/// labels and anything else hanging off the system keep their own size.
///
/// Distance is measured between two absolute galactic positions, the
/// camera's [`OrbitCamera::eye`] and the system's. A system's `Transform`
/// holds only the remainder left over from its grid cell, and its
/// `GlobalTransform` is written after this runs, so neither answers where it
/// is.
pub fn size_by_distance(
    scale_population: Res<ScalePopulation>,
    camera: Query<&OrbitCamera>,
    systems: Query<&System>,
    mut stars: Query<(&mut Transform, &ChildOf), With<Star>>,
) {
    if !stars.is_empty() {
        let Ok(eye) = camera.single().map(|c| c.eye) else { return };
        let pop_avg = if scale_population.0 {
            // TODO(#45): This is *very* slow and should be precomputed when
            // the set of systems changes.
            let (total, count) = systems
                .iter()
                .fold((0., 0.), |(t, n), s| (t + s.population as f64, n + 1.));
            total / count
        } else {
            0.
        };

        // The goal is to avoid fading out any stars, but scale them as the
        // camera moves further away from them.
        // TODO(#46): We should still change rgba color/emmisivity as needed.
        for (mut star_transform, child_of) in stars.iter_mut() {
            let Ok(system) = systems.get(child_of.parent()) else { continue };
            let dist = eye.distance(DVec3::from(system.position)) as f32;
            let mut scale = 4e-4 * dist + 8.5e-2;
            if scale_population.0 {
                scale *= population_factor(system.population, pop_avg);
            }
            star_transform.scale = Vec3::splat(scale);
        }
    }
}

/// Draw every star the same size, whatever the camera is doing
///
/// This view is a picture of where things are rather than of how far away
/// they are, so nothing here reads the camera.
pub fn size_uniformly(mut stars: Query<&mut Transform, With<Star>>) {
    // TODO(#46): Change rgba color/emmisivity. The goal is to fade out to
    // transparent when they are too far away.
    for mut star_transform in stars.iter_mut() {
        star_transform.scale = Vec3::splat(1e-2);
    }
}
