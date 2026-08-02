//! Per-station power balance. Generators are factories too: power plants
//! burn fuel from station storage on a craft cycle, solar arrays scale with
//! the star (halved on surfaces), geothermal needs volcanism. A deficit
//! browns the whole station out proportionally — no hard stop.

use super::*;
use crate::data::{BuildingKind, StaticData};
use bevy::prelude::*;

pub fn power_balance(
    data: Res<StaticData>,
    systems: Query<&SystemEnv>,
    mut stations: Query<(
        &Station,
        &InSystem,
        &mut Storage,
        &mut PowerGrid,
        Option<&Children>,
    )>,
    mut factories: Query<
        (&Factory, &ActiveRecipe, &mut CraftProgress, &mut Status),
        Without<MaintenanceDue>,
    >,
) {
    for (station, in_system, mut storage, mut grid, children) in
        stations.iter_mut()
    {
        let env = systems.get(in_system.0).copied().unwrap_or_default();
        let mut supply: u32 = 0;
        let mut demand: u32 = 0;

        for &child in children.map(|c| &**c).unwrap_or(&[]) {
            let Ok((factory, active, mut progress, mut status)) =
                factories.get_mut(child)
            else {
                continue;
            };
            let Some(recipe_id) = active.0 else { continue };
            let recipe = data.recipe(recipe_id);

            if recipe.power_mw >= 0 {
                demand += recipe.power_mw as u32;
                continue;
            }

            // Generator.
            let mw = (-recipe.power_mw) as u32;
            match factory.kind {
                BuildingKind::SolarArray => {
                    let placement_milli = match station.placement {
                        Placement::Surface(_) => 500,
                        Placement::Orbital(_) => 1000,
                    };
                    supply +=
                        mw * placement_milli / 1000 * env.solar_milli / 1000;
                    status.0 = FactoryStatus::Running;
                }
                _ if recipe.inputs.is_empty() => {
                    supply += mw;
                    status.0 = FactoryStatus::Running;
                }
                _ => {
                    // Fuel-burning cycle: consume inputs at cycle start,
                    // produce power for `ticks` ticks.
                    if !progress.holding {
                        if storage.take_all(&recipe.inputs) {
                            progress.holding = true;
                            progress.progress_milli = 0;
                        } else {
                            status.0 = FactoryStatus::Starved;
                            continue;
                        }
                    }
                    supply += mw;
                    status.0 = FactoryStatus::Running;
                    progress.progress_milli += 1000;
                    if progress.progress_milli >= recipe.ticks as u64 * 1000 {
                        progress.holding = false;
                    }
                }
            }
        }

        grid.supply_mw = supply;
        grid.demand_mw = demand;
        grid.satisfaction_milli = if demand == 0 {
            1000
        } else {
            ((supply as u64 * 1000) / demand as u64).min(1000) as u32
        };
    }
}
