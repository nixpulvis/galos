//! Ships fulfilling contracts: Loading → Outbound → deliver → Returning.
//! Departures burn hydrogenfuel from the origin station's pool (no fuel, no
//! departure). Piracy rolls per arrival lose the ship and its cargo.
//! Deliveries into NPC market stations by player-issued contracts sell on
//! arrival at curve price (after tax).
//!
//! TODO: charge fuel on the return leg too (needs fuel sourcing at the
//! destination — bought from its market when available).

use super::*;
use crate::data::StaticData;
use bevy::prelude::*;
use rand::Rng;

pub fn shipping(
    mut commands: Commands,
    data: Res<StaticData>,
    mods: Res<SystemModifiers>,
    clock: Res<SimClock>,
    mut rng: ResMut<SimRng>,
    mut credits: ResMut<Credits>,
    mut stats: ResMut<Stats>,
    mut notices: ResMut<Notices>,
    contracts: Query<&Contract>,
    mut ships: Query<(Entity, &mut Ship)>,
    mut stations: Query<(&Station, &mut Storage)>,
    mut markets: Query<&mut Market>,
) {
    let fuel =
        data.item_by_name("hydrogenfuel").expect("hydrogenfuel in items");

    for (ship_entity, mut ship) in ships.iter_mut() {
        let Some(contract_entity) = ship.contract else { continue };
        let Ok(contract) = contracts.get(contract_entity) else {
            ship.contract = None;
            continue;
        };

        match ship.state.clone() {
            ShipState::Idle { .. } => {
                ship.state = ShipState::Loading;
            }
            ShipState::Loading => {
                // Request threshold: idle at origin while the destination
                // is stocked to target.
                if let Some(target) = contract.target {
                    let dest_stock = stations
                        .get(contract.to)
                        .map(|(_, storage)| storage.count(contract.item))
                        .unwrap_or(0);
                    if dest_stock >= target {
                        continue;
                    }
                }
                let Ok((station, mut storage)) =
                    stations.get_mut(contract.from)
                else {
                    continue;
                };
                let station_name = station.name.clone();

                // Fuel for the outbound leg: station pool first, else bought
                // from the origin's market (import contracts fuel up at the
                // NPC station they load from).
                let fuel_needed = ship.class.fuel_per_leg();
                let mut fuel_have = storage.take(fuel, fuel_needed);
                if fuel_have < fuel_needed && ship.owner == Owner::Player {
                    if let Ok(mut market) = markets.get_mut(contract.from) {
                        fuel_have += market_buy(
                            &mut market,
                            fuel,
                            fuel_needed - fuel_have,
                            &mut credits,
                            &mut stats,
                        );
                    }
                }
                if fuel_have < fuel_needed {
                    storage.add(fuel, fuel_have); // Put partial fuel back.
                    notices.0.push((
                        clock.tick,
                        Notice::NoFuel { station: station_name },
                    ));
                    continue;
                }

                // Never load a sell-delivery the destination market doesn't
                // list — the cargo would have nowhere to go.
                if contract.issuer == Owner::Player {
                    if let Ok(market) = markets.get(contract.to) {
                        if !market.entries.contains_key(&contract.item) {
                            storage.add(fuel, fuel_have);
                            continue;
                        }
                    }
                }

                // Cargo: station pool first (respecting the origin reserve);
                // a player contract loading at an NPC market station buys
                // the remainder at curve price.
                let above_reserve = storage
                    .count(contract.item)
                    .saturating_sub(contract.reserve);
                let mut cargo = storage.take(
                    contract.item,
                    ship.class.cargo_cap().min(above_reserve),
                );
                if cargo < ship.class.cargo_cap()
                    && contract.issuer == Owner::Player
                {
                    if let Ok(mut market) = markets.get_mut(contract.from) {
                        cargo += market_buy(
                            &mut market,
                            contract.item,
                            ship.class.cargo_cap() - cargo,
                            &mut credits,
                            &mut stats,
                        );
                    }
                }
                if cargo == 0 {
                    storage.add(fuel, fuel_have); // Nothing to haul; refund fuel.
                    continue;
                }
                let (from_ls, to_ls) =
                    station_distances(&stations, contract.from, contract.to);
                ship.state = ShipState::Outbound {
                    ticks_left: travel_ticks(from_ls, to_ls),
                    cargo,
                };
            }
            ShipState::Outbound { ticks_left, cargo } => {
                if ticks_left > 1 {
                    ship.state = ShipState::Outbound {
                        ticks_left: ticks_left - 1,
                        cargo,
                    };
                    continue;
                }
                // Arrival: piracy roll first.
                if mods.piracy_milli > 0
                    && rng.0.gen_range(0..1000u32) < mods.piracy_milli
                {
                    notices.0.push((
                        clock.tick,
                        Notice::PiracyLoss { item: contract.item, qty: cargo },
                    ));
                    commands.entity(ship_entity).despawn();
                    continue;
                }

                let dest_is_npc_market = stations
                    .get(contract.to)
                    .map(|(st, _)| st.owner == Owner::Npc)
                    .unwrap_or(false)
                    && markets.get(contract.to).is_ok();

                if dest_is_npc_market && contract.issuer == Owner::Player {
                    // Sell on delivery at curve price, after tax.
                    let mut market = markets.get_mut(contract.to).unwrap();
                    if let Some(entry) = market.entries.get_mut(&contract.item)
                    {
                        let gross = unit_price(entry) * cargo as i64;
                        let net = gross * (1000 - mods.tax_milli as i64) / 1000;
                        entry.stock += cargo;
                        credits.0 += net;
                        stats.revenue += net;
                        *stats.sold.entry(contract.item).or_insert(0) +=
                            cargo as u64;
                        let station_name = stations
                            .get(contract.to)
                            .map(|(s, _)| s.name.clone())
                            .unwrap();
                        notices.0.push((
                            clock.tick,
                            Notice::Sold {
                                station: station_name,
                                item: contract.item,
                                qty: cargo,
                                credits: net,
                            },
                        ));
                    }
                } else if let Ok((_, mut storage)) =
                    stations.get_mut(contract.to)
                {
                    storage.add(contract.item, cargo);
                }

                // Carrier pay from a foreign issuer.
                if contract.issuer == Owner::Npc && ship.owner == Owner::Player
                {
                    let pay = contract.pay_per_unit as i64 * cargo as i64;
                    credits.0 += pay;
                    stats.revenue += pay;
                }

                let (from_ls, to_ls) =
                    station_distances(&stations, contract.from, contract.to);
                ship.state = ShipState::Returning {
                    ticks_left: travel_ticks(from_ls, to_ls),
                };
            }
            ShipState::Returning { ticks_left } => {
                ship.state = if ticks_left > 1 {
                    ShipState::Returning { ticks_left: ticks_left - 1 }
                } else {
                    ShipState::Loading
                };
            }
        }
    }
}

/// Buys up to `qty` from a market entry at curve price, budget and stock
/// permitting; returns the quantity bought.
fn market_buy(
    market: &mut Market,
    item: crate::data::ItemId,
    qty: u32,
    credits: &mut Credits,
    stats: &mut Stats,
) -> u32 {
    let Some(entry) = market.entries.get_mut(&item) else { return 0 };
    let price = unit_price(entry);
    let affordable =
        if price > 0 { (credits.0.max(0) / price) as u32 } else { qty };
    let bought = qty.min(entry.stock).min(affordable);
    if bought > 0 {
        let cost = price * bought as i64;
        entry.stock -= bought;
        credits.0 -= cost;
        stats.expenses += cost;
    }
    bought
}

fn station_distances(
    stations: &Query<(&Station, &mut Storage)>,
    from: Entity,
    to: Entity,
) -> (u32, u32) {
    let from_ls = stations.get(from).map(|(s, _)| s.dist_ls).unwrap_or(0);
    let to_ls = stations.get(to).map(|(s, _)| s.dist_ls).unwrap_or(0);
    (from_ls, to_ls)
}
