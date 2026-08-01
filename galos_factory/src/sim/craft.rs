//! Crafting: refineries and assemblers pull recipe inputs from their
//! station's shared storage at cycle start, progress scaled by power
//! satisfaction and productivity, and push outputs back to the pool.
//! Shared with extraction via [`step_factory`].

use super::*;
use crate::data::{BuildingKind, RecipeDef, StaticData};
use bevy::prelude::*;

/// True when the factory should idle because the station pool already holds
/// the configured amount of its primary output. Only applies between
/// cycles — work in progress always finishes.
pub fn output_capped(
    cap: &OutputCap,
    recipe: &crate::data::RecipeDef,
    storage: &Storage,
    progress: &mut CraftProgress,
) -> bool {
    if progress.holding {
        return false;
    }
    let (Some(cap), Some((primary, _))) = (cap.0, recipe.outputs.first())
    else {
        return false;
    };
    storage.count(*primary) >= cap
}

/// Advances one factory one tick. `rate_milli` folds in every multiplier
/// (power satisfaction, productivity, richness for extractors).
/// Returns produced outputs when a cycle completes.
pub fn step_factory(
    recipe: &RecipeDef,
    storage: &mut Storage,
    progress: &mut CraftProgress,
    status: &mut Status,
    rate_milli: u64,
    stats: &mut Stats,
) {
    // Acquire inputs at cycle start.
    if !progress.holding {
        if recipe.inputs.is_empty() {
            progress.holding = true;
        } else if storage.take_all(&recipe.inputs) {
            for (item, qty) in &recipe.inputs {
                *stats.consumed.entry(*item).or_insert(0) += *qty as u64;
            }
            progress.holding = true;
        } else {
            status.0 = FactoryStatus::Starved;
            return;
        }
    }

    if rate_milli == 0 {
        status.0 = FactoryStatus::Idle;
        return;
    }

    let goal = recipe.ticks as u64 * 1000;
    progress.progress_milli = (progress.progress_milli + rate_milli).min(goal);

    if progress.progress_milli >= goal {
        let needed: u32 = recipe.outputs.iter().map(|(_, q)| q).sum();
        if storage.free() < needed {
            status.0 = FactoryStatus::OutputBlocked;
            return;
        }
        for (item, qty) in &recipe.outputs {
            storage.add(*item, *qty);
            *stats.produced.entry(*item).or_insert(0) += *qty as u64;
        }
        progress.progress_milli = 0;
        progress.holding = false;
    }
    status.0 = FactoryStatus::Running;
}

pub fn craft(
    data: Res<StaticData>,
    mods: Res<SystemModifiers>,
    mut stats: ResMut<Stats>,
    mut stations: Query<(Entity, &mut Storage, &PowerGrid, &LifeSupport)>,
    mut factories: Query<(
        &Factory,
        &ActiveRecipe,
        &OutputCap,
        &mut CraftProgress,
        &mut Status,
        &MaintenanceDue,
    )>,
) {
    for (station_entity, mut storage, grid, condition) in stations.iter_mut() {
        for (factory, active, cap, mut progress, mut status, due) in
            factories.iter_mut()
        {
            if factory.station != station_entity
                || !matches!(
                    factory.kind,
                    BuildingKind::Refinery | BuildingKind::Assembler
                )
            {
                continue;
            }
            if due.0 {
                status.0 = FactoryStatus::Offline;
                continue;
            }
            let Some(recipe_id) = active.0 else {
                status.0 = FactoryStatus::Idle;
                continue;
            };
            let recipe = data.recipe(recipe_id);
            if output_capped(cap, recipe, &storage, &mut progress) {
                status.0 = FactoryStatus::Idle;
                continue;
            }
            let life_milli: u64 =
                if condition.life_support_ok { 1000 } else { 500 };
            let rate_milli = grid.satisfaction_milli as u64
                * mods.productivity_milli as u64
                / 1000
                * life_milli
                / 1000;
            step_factory(
                recipe,
                &mut storage,
                &mut progress,
                &mut status,
                rate_milli,
                &mut stats,
            );
        }
    }
}
