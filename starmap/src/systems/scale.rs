use super::System;
use bevy::prelude::*;
use bevy_panorbit_camera::PanOrbitCamera;

#[derive(Resource, Debug, PartialEq)]
pub enum View {
    // #[default]
    Systems,
    Stars,
    // TODO(#44): Bodies
}

#[derive(Resource, Debug)]
pub struct ScalePopulation(pub bool);

pub fn scale_systems(
    scale_population: Res<ScalePopulation>,
    mut set: ParamSet<(
        Query<(&mut Transform, &System)>,
        Query<&Transform, With<PanOrbitCamera>>,
    )>,
) {
    if !set.p0().is_empty() {
        let camera_translation = set.p1().single().translation;
        let pop_avg = if scale_population.0 {
            // TODO(#45): This is *very* slow and should be precomputed when
            // the set of systems changes.
            set.p0().iter().map(|(_, s)| s.population).sum::<u64>()
                / set.p0().iter().len() as u64
        } else {
            0
        };

        // The goal is to avoid fading out any stars, but scale them as the
        // camera moves further away from them.
        // TODO(#46): We should still change rgba color/emmisivity as needed.
        for (mut system_transform, system) in set.p0().iter_mut() {
            let dist =
                camera_translation.distance(system_transform.translation);
            let mut scale = 4e-4 * dist + 8.5e-2;
            if scale_population.0 {
                let pop_factor = system.population as f32 / pop_avg as f32;
                scale *= 0.2 * pop_factor.ln();
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
