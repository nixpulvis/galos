//! Extraction: extractors mine their host body's deposits (rate scaled by
//! richness), fuel scoops skim scoopable stars. Same cycle machinery as
//! crafting, with the deposit richness folded into the rate.

use super::craft::step_factory;
use super::*;
use crate::data::{BuildingKind, Req, StaticData};
use bevy::prelude::*;

pub fn extract(
    data: Res<StaticData>,
    mods: Res<SystemModifiers>,
    mut stats: ResMut<Stats>,
    mut stations: Query<(
        Entity,
        &Station,
        &mut Storage,
        &PowerGrid,
        &LifeSupport,
    )>,
    bodies: Query<&Deposits>,
    mut factories: Query<(
        &Factory,
        &ActiveRecipe,
        &OutputCap,
        &mut CraftProgress,
        &mut Status,
        &MaintenanceDue,
    )>,
) {
    for (station_entity, station, mut storage, grid, condition) in
        stations.iter_mut()
    {
        for (factory, active, cap, mut progress, mut status, due) in
            factories.iter_mut()
        {
            if factory.station != station_entity
                || !matches!(
                    factory.kind,
                    BuildingKind::Extractor | BuildingKind::FuelScoop
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
            if super::craft::output_capped(cap, recipe, &storage, &mut progress)
            {
                status.0 = FactoryStatus::Idle;
                continue;
            }

            // Deposit richness multiplier (milli); scoops run at nominal.
            let mut richness_milli: u64 = 1000;
            for req in &recipe.requires {
                if let Req::Deposit(item) = req {
                    richness_milli = match station.placement {
                        Placement::Surface(body) => bodies
                            .get(body)
                            .ok()
                            .and_then(|d| {
                                d.0.iter()
                                    .find(|(i, _)| i == item)
                                    .map(|(_, r)| *r as u64)
                            })
                            .unwrap_or(0),
                        Placement::Orbital(_) => 0,
                    };
                }
            }

            let life_milli: u64 =
                if condition.life_support_ok { 1000 } else { 500 };
            let rate_milli = grid.satisfaction_milli as u64
                * mods.productivity_milli as u64
                / 1000
                * richness_milli
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
