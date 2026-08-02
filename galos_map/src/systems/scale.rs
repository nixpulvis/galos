use crate::schedule::MapSet;

use super::System;
use bevy::prelude::*;
use bevy_panorbit_camera::PanOrbitCamera;

pub fn plugin(app: &mut App) {
    app.insert_resource(View::Systems);
    app.insert_resource(ScalePopulation(false));
    // One view is drawn at a time, so these two never run in the same frame.
    // The scheduler cannot see that from the run conditions alone.
    app.add_systems(
        Update,
        scale_systems
            .in_set(MapSet::Present)
            .ambiguous_with(scale_stars)
            .run_if(resource_equals(View::Systems)),
    );
    app.add_systems(
        Update,
        scale_stars
            .in_set(MapSet::Present)
            .ambiguous_with(scale_systems)
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

pub fn scale_systems(
    scale_population: Res<ScalePopulation>,
    mut set: ParamSet<(
        Query<(&mut Transform, &System)>,
        Query<&Transform, With<PanOrbitCamera>>,
    )>,
) {
    if !set.p0().is_empty() {
        let Ok(camera_translation) = set.p1().single().map(|c| c.translation)
        else {
            return;
        };
        let pop_avg = if scale_population.0 {
            // TODO(#45): This is *very* slow and should be precomputed when
            // the set of systems changes.
            let total: f64 =
                set.p0().iter().map(|(_, s)| s.population as f64).sum();
            total / set.p0().iter().len() as f64
        } else {
            0.
        };

        // The goal is to avoid fading out any stars, but scale them as the
        // camera moves further away from them.
        // TODO(#46): We should still change rgba color/emmisivity as needed.
        for (mut system_transform, system) in set.p0().iter_mut() {
            let dist =
                camera_translation.distance(system_transform.translation);
            let mut scale = 4e-4 * dist + 8.5e-2;
            if scale_population.0 {
                scale *= population_factor(system.population, pop_avg);
            }
            system_transform.scale = Vec3::splat(scale);
        }
    }
}

pub fn scale_stars(mut query: Query<(&mut Transform, &System)>) {
    if !query.is_empty() {
        // TODO(#46): Change rgba color/emmisivity. The goal is to fade out to
        // transparent when they are too far away.
        for (mut system_transform, _system) in query.iter_mut() {
            system_transform.scale = Vec3::splat(1e-2);
        }
    }
}
