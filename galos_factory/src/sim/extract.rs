//! Extraction: extractors mine their host body's deposits (rate scaled by
//! richness), fuel scoops skim scoopable stars. Same cycle machinery as
//! crafting, with the deposit richness folded into the rate.

use super::craft::{base_rate_milli, output_capped, step_factory};
use super::*;
use crate::data::{BuildingKind, Req, StaticData};
use bevy::prelude::*;

pub fn extract(
    data: Res<StaticData>,
    controls: Query<&Control>,
    mut actors: Query<&mut Ledger>,
    bodies: Query<&Deposits>,
    mut stations: Query<(
        &Station,
        &InSystem,
        &OwnedBy,
        &mut Storage,
        &PowerGrid,
        &LifeSupport,
        Option<&Children>,
    )>,
    mut factories: Query<
        (&Factory, &ActiveRecipe, &OutputCap, &mut CraftProgress, &mut Status),
        Without<MaintenanceDue>,
    >,
) {
    for (station, in_system, owner, mut storage, grid, life, children) in
        stations.iter_mut()
    {
        let control = controls.get(in_system.0).copied().unwrap_or_default();
        let base_milli = base_rate_milli(grid, &control, life);

        for &child in children.map(|c| &**c).unwrap_or(&[]) {
            let Ok((factory, active, cap, mut progress, mut status)) =
                factories.get_mut(child)
            else {
                continue;
            };
            if !matches!(
                factory.kind,
                BuildingKind::Extractor | BuildingKind::FuelScoop
            ) {
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

            let mut ledger = actors.get_mut(owner.0).ok();
            step_factory(
                recipe,
                &mut storage,
                &mut progress,
                &mut status,
                base_milli * richness_milli / 1000,
                ledger.as_deref_mut(),
            );
        }
    }
}
