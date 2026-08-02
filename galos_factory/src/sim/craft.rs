//! Crafting: refineries and assemblers pull recipe inputs from their
//! station's shared storage at cycle start, progress scaled by power
//! satisfaction and the controlling faction's productivity, and push
//! outputs back to the pool. Shared with extraction via [`step_factory`].

use super::*;
use crate::data::{BuildingKind, RecipeDef, StaticData};
use bevy::prelude::*;

/// True when the factory should idle because the station pool already holds
/// the configured amount of its primary output. Only applies between
/// cycles — work in progress always finishes.
pub fn output_capped(
    cap: &OutputCap,
    recipe: &RecipeDef,
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
/// (power satisfaction, productivity, richness for extractors). Returns
/// produced outputs into the station pool when a cycle completes.
pub fn step_factory(
    recipe: &RecipeDef,
    storage: &mut Storage,
    progress: &mut CraftProgress,
    status: &mut Status,
    rate_milli: u64,
    ledger: Option<&mut Ledger>,
) {
    let mut produced: Vec<(crate::data::ItemId, u32)> = Vec::new();
    let mut consumed: Vec<(crate::data::ItemId, u32)> = Vec::new();

    // Acquire inputs at cycle start.
    if !progress.holding {
        if recipe.inputs.is_empty() {
            progress.holding = true;
        } else if storage.take_all(&recipe.inputs) {
            consumed.extend(recipe.inputs.iter().copied());
            progress.holding = true;
        } else {
            status.0 = FactoryStatus::Starved;
            record(ledger, &produced, &consumed);
            return;
        }
    }

    if rate_milli == 0 {
        status.0 = FactoryStatus::Idle;
        record(ledger, &produced, &consumed);
        return;
    }

    let goal = recipe.ticks as u64 * 1000;
    progress.progress_milli = (progress.progress_milli + rate_milli).min(goal);

    if progress.progress_milli >= goal {
        let needed: u32 = recipe.outputs.iter().map(|(_, q)| q).sum();
        if storage.free() < needed {
            status.0 = FactoryStatus::OutputBlocked;
            record(ledger, &produced, &consumed);
            return;
        }
        for (item, qty) in &recipe.outputs {
            storage.add(*item, *qty);
            produced.push((*item, *qty));
        }
        progress.progress_milli = 0;
        progress.holding = false;
    }
    status.0 = FactoryStatus::Running;
    record(ledger, &produced, &consumed);
}

fn record(
    ledger: Option<&mut Ledger>,
    produced: &[(crate::data::ItemId, u32)],
    consumed: &[(crate::data::ItemId, u32)],
) {
    let Some(ledger) = ledger else { return };
    for (item, qty) in produced {
        *ledger.produced.entry(*item).or_insert(0) += *qty as u64;
    }
    for (item, qty) in consumed {
        *ledger.consumed.entry(*item).or_insert(0) += *qty as u64;
    }
}

/// Multipliers every producing factory shares: power satisfaction, the
/// controlling faction's productivity, and life support.
pub fn base_rate_milli(
    grid: &PowerGrid,
    control: &Control,
    life: &LifeSupport,
) -> u64 {
    let life_milli: u64 = if life.ok { 1000 } else { 500 };
    grid.satisfaction_milli as u64 * control.productivity_milli as u64 / 1000
        * life_milli
        / 1000
}

pub fn craft(
    data: Res<StaticData>,
    controls: Query<&Control>,
    mut actors: Query<&mut Ledger>,
    mut stations: Query<(
        &InSystem,
        &OwnedBy,
        &mut Storage,
        &PowerGrid,
        &LifeSupport,
        Option<&Children>,
    )>,
    mut factories: Query<FactoryWork, Without<MaintenanceDue>>,
) {
    for (in_system, owner, mut storage, grid, life, children) in
        stations.iter_mut()
    {
        let control = controls.get(in_system.0).copied().unwrap_or_default();
        let rate_milli = base_rate_milli(grid, &control, life);

        for &child in children.map(|c| &**c).unwrap_or(&[]) {
            let Ok(mut work) = factories.get_mut(child) else { continue };
            if !matches!(
                work.factory.kind,
                BuildingKind::Refinery | BuildingKind::Assembler
            ) {
                continue;
            }
            let recipe = data.recipe(work.recipe.0);
            if output_capped(work.cap, recipe, &storage, &mut work.progress) {
                work.status.0 = FactoryStatus::Idle;
                continue;
            }
            let mut ledger = actors.get_mut(owner.0).ok();
            step_factory(
                recipe,
                &mut storage,
                &mut work.progress,
                &mut work.status,
                rate_milli,
                ledger.as_deref_mut(),
            );
        }
    }
}
