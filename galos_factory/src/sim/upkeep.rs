//! Standing costs, charged every [`UPKEEP_PERIOD`] ticks: buildings consume
//! maintenance items (unpaid → offline until next successful charge) and
//! stations consume life support per active slot (short → productivity
//! penalty via [`LifeSupport`]).

use super::*;
use crate::data::{StaticData, UPKEEP_PERIOD};
use bevy::prelude::*;

pub fn upkeep(
    data: Res<StaticData>,
    clock: Res<SimClock>,
    mut notices: ResMut<Notices>,
    mut stations: Query<(Entity, &Station, &mut Storage, &mut LifeSupport)>,
    mut factories: Query<(&Factory, &mut MaintenanceDue)>,
) {
    if clock.tick == 0 || clock.tick % UPKEEP_PERIOD != 0 {
        return;
    }
    let (water, food) = (
        data.item_by_name("water").expect("water in items"),
        data.item_by_name("foodcartridges").expect("foodcartridges in items"),
    );

    for (station_entity, station, mut storage, mut condition) in stations.iter_mut() {
        // NPC stations manage their own affairs (their markets are their
        // stores); upkeep pressure applies to the player's estate.
        if station.owner == Owner::Npc {
            continue;
        }

        let mut active_slots = 0u32;
        for (factory, mut due) in factories.iter_mut() {
            if factory.station != station_entity {
                continue;
            }
            active_slots += 1;
            let def = data.building(factory.kind);
            if def.upkeep.is_empty() {
                continue;
            }
            if storage.take_all(&def.upkeep) {
                due.0 = false;
            } else {
                due.0 = true;
                notices.0.push((
                    clock.tick,
                    Notice::MaintenanceShort {
                        station: station.name.clone(),
                        kind: factory.kind,
                    },
                ));
            }
        }

        let life_support = [(water, active_slots), (food, active_slots)];
        if active_slots == 0 || storage.take_all(&life_support) {
            condition.life_support_ok = true;
        } else {
            condition.life_support_ok = false;
            notices
                .0
                .push((clock.tick, Notice::LifeSupportShort { station: station.name.clone() }));
        }
    }
}
