//! Standing costs, charged every [`UPKEEP_PERIOD`] ticks: buildings consume
//! maintenance items (unpaid → [`MaintenanceDue`], which takes them offline
//! until the next successful charge) and stations consume life support per
//! active slot (short → productivity penalty via [`LifeSupport`]).
//!
//! Only commander-owned stations pay today. Faction-owned stations withering
//! under unmet upkeep is designed but not yet simulated.

use super::*;
use crate::data::StaticData;
use bevy::prelude::*;

pub fn upkeep(
    mut commands: Commands,
    data: Res<StaticData>,
    clock: Res<SimClock>,
    mut notices: ResMut<Notices>,
    commanders: Query<(), With<Commander>>,
    mut stations: Query<(
        &Station,
        &OwnedBy,
        &mut Storage,
        &mut LifeSupport,
        Option<&Children>,
    )>,
    factories: Query<(&Factory, Has<MaintenanceDue>)>,
) {
    let (water, food) = (
        data.item_by_name("water").expect("water in items"),
        data.item_by_name("foodcartridges").expect("foodcartridges in items"),
    );

    for (station, owner, mut storage, mut life, children) in stations.iter_mut()
    {
        if commanders.get(owner.0).is_err() {
            continue;
        }

        let mut active_slots = 0u32;
        for &child in children.map(|c| &**c).unwrap_or(&[]) {
            let Ok((factory, due)) = factories.get(child) else { continue };
            active_slots += 1;
            let def = data.building(factory.kind);
            if def.upkeep.is_empty() {
                continue;
            }
            if storage.take_all(&def.upkeep) {
                if due {
                    commands.entity(child).remove::<MaintenanceDue>();
                }
            } else {
                if !due {
                    commands.entity(child).insert(MaintenanceDue);
                }
                notices.push(
                    clock.tick,
                    Notice::MaintenanceShort {
                        station: station.name.clone(),
                        kind: factory.kind,
                    },
                );
            }
        }

        let life_support = [(water, active_slots), (food, active_slots)];
        if active_slots == 0 || storage.take_all(&life_support) {
            life.ok = true;
        } else {
            life.ok = false;
            notices.push(
                clock.tick,
                Notice::LifeSupportShort { station: station.name.clone() },
            );
        }
    }
}
