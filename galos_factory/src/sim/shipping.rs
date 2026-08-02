//! Ships fulfilling contracts: Loading → Outbound → deliver → Returning.
//! Departures burn hydrogenfuel from the origin station's pool (no fuel, no
//! departure). Piracy rolls per arrival lose the ship and its cargo.
//!
//! Money moves between actors here: selling into a faction's market credits
//! the contract issuer and debits the faction's treasury (less its own tax,
//! which it keeps), and carrying someone else's contract pays the ship's
//! owner from the issuer's account.
//!
//! TODO: charge fuel on the return leg too (needs fuel sourcing at the
//! destination — bought from its market when available).

use super::*;
use crate::data::{ItemId, StaticData};
use bevy::prelude::*;
use rand::Rng;

type StationQuery<'w, 's> =
    Query<'w, 's, (&'static Station, &'static InSystem, &'static mut Storage)>;

pub fn shipping(
    mut commands: Commands,
    (data, clock): (Res<StaticData>, Res<SimClock>),
    (mut rng, mut notices): (ResMut<SimRng>, ResMut<Notices>),
    envs: Query<&SystemEnv>,
    controls: Query<&Control>,
    mut accounts: Query<(&mut Credits, &mut Ledger)>,
    contracts: Query<(&Contract, &OwnedBy)>,
    mut ships: Query<(Entity, &mut Ship, &OwnedBy)>,
    mut stations: StationQuery,
    station_owners: Query<&OwnedBy, With<Station>>,
    mut markets: Query<&mut Market>,
) {
    let fuel =
        data.item_by_name("hydrogenfuel").expect("hydrogenfuel in items");

    for (ship_entity, mut ship, ship_owner) in ships.iter_mut() {
        let Some(contract_entity) = ship.contract else { continue };
        let Ok((contract, issuer)) = contracts.get(contract_entity) else {
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
                        .map(|(_, _, storage)| storage.count(contract.item))
                        .unwrap_or(0);
                    if dest_stock >= target {
                        continue;
                    }
                }
                let Ok((station, _, mut storage)) =
                    stations.get_mut(contract.from)
                else {
                    continue;
                };
                let station_name = station.name.clone();

                // Fuel for the outbound leg: station pool first, else
                // bought from the origin's market.
                let fuel_needed = ship.class.fuel_per_leg();
                let mut fuel_have = storage.take(fuel, fuel_needed);
                if fuel_have < fuel_needed {
                    fuel_have += buy_from_market(
                        &mut markets,
                        contract.from,
                        fuel,
                        fuel_needed - fuel_have,
                        issuer.0,
                        &mut accounts,
                    );
                }
                if fuel_have < fuel_needed {
                    storage.add(fuel, fuel_have); // Put partial fuel back.
                    notices.push(
                        clock.tick,
                        Notice::NoFuel { station: station_name },
                    );
                    continue;
                }

                // Never load a delivery the destination market cannot take.
                let dest_sells = markets
                    .get(contract.to)
                    .map(|m| m.entries.contains_key(&contract.item));
                if dest_sells == Ok(false) {
                    storage.add(fuel, fuel_have);
                    continue;
                }

                // Cargo: station pool first (respecting the origin
                // reserve), then bought from the origin's market.
                let above_reserve = storage
                    .count(contract.item)
                    .saturating_sub(contract.reserve);
                let mut cargo = storage.take(
                    contract.item,
                    ship.class.cargo_cap().min(above_reserve),
                );
                if cargo < ship.class.cargo_cap() {
                    cargo += buy_from_market(
                        &mut markets,
                        contract.from,
                        contract.item,
                        ship.class.cargo_cap() - cargo,
                        issuer.0,
                        &mut accounts,
                    );
                }
                if cargo == 0 {
                    storage.add(fuel, fuel_have); // Nothing to haul.
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

                // Arrival: piracy roll first, using the destination
                // system's security.
                let piracy_milli = stations
                    .get(contract.to)
                    .ok()
                    .and_then(|(_, in_system, _)| envs.get(in_system.0).ok())
                    .map(|env| env.piracy_milli)
                    .unwrap_or(0);
                if piracy_milli > 0
                    && rng.0.gen_range(0..1000u32) < piracy_milli
                {
                    notices.push(
                        clock.tick,
                        Notice::PiracyLoss { item: contract.item, qty: cargo },
                    );
                    commands.entity(ship_entity).despawn();
                    continue;
                }

                if markets.get(contract.to).is_ok() {
                    sell_into_market(
                        &mut markets,
                        &mut stations,
                        &station_owners,
                        &controls,
                        contract,
                        issuer.0,
                        cargo,
                        &mut accounts,
                        &mut notices,
                        clock.tick,
                    );
                } else if let Ok((_, _, mut storage)) =
                    stations.get_mut(contract.to)
                {
                    storage.add(contract.item, cargo);
                }

                // Carrying someone else's contract pays the ship's owner.
                if issuer.0 != ship_owner.0 && contract.pay_per_unit > 0 {
                    let pay = contract.pay_per_unit as i64 * cargo as i64;
                    transfer(&mut accounts, issuer.0, ship_owner.0, pay);
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

/// Sells `cargo` into the destination market at curve price. The buying
/// faction pays from its treasury; the faction controlling the system keeps
/// the tax.
#[allow(clippy::too_many_arguments)]
fn sell_into_market(
    markets: &mut Query<&mut Market>,
    stations: &mut StationQuery,
    station_owners: &Query<&OwnedBy, With<Station>>,
    controls: &Query<&Control>,
    contract: &Contract,
    seller: Entity,
    cargo: u32,
    accounts: &mut Query<(&mut Credits, &mut Ledger)>,
    notices: &mut Notices,
    tick: u64,
) {
    let Ok(mut market) = markets.get_mut(contract.to) else { return };
    let Some(entry) = market.entries.get_mut(&contract.item) else { return };

    let gross = unit_price(entry) * cargo as i64;
    entry.stock += cargo;

    let Ok((station, in_system, _)) = stations.get(contract.to) else { return };
    let station_name = station.name.clone();
    let control = controls.get(in_system.0).copied().unwrap_or_default();
    let tax = gross * control.tax_milli as i64 / 1000;
    let net = gross - tax;

    // The station's owning faction buys the goods.
    if let Ok(buyer) = station_owners.get(contract.to) {
        if let Ok((mut credits, _)) = accounts.get_mut(buyer.0) {
            credits.0 -= gross;
        }
    }
    if let Ok((mut credits, mut ledger)) = accounts.get_mut(seller) {
        credits.0 += net;
        ledger.revenue += net;
        *ledger.sold.entry(contract.item).or_insert(0) += cargo as u64;
    }
    // Tax to whoever runs the system.
    if let Some(ruler) = control.faction {
        if let Ok((mut credits, _)) = accounts.get_mut(ruler) {
            credits.0 += tax;
        }
    }

    notices.push(
        tick,
        Notice::Sold {
            station: station_name,
            item: contract.item,
            qty: cargo,
            credits: net,
        },
    );
}

/// Buys up to `qty` from a station's market at curve price, budget and
/// stock permitting; returns the quantity bought.
fn buy_from_market(
    markets: &mut Query<&mut Market>,
    station: Entity,
    item: ItemId,
    qty: u32,
    buyer: Entity,
    accounts: &mut Query<(&mut Credits, &mut Ledger)>,
) -> u32 {
    let Ok(mut market) = markets.get_mut(station) else { return 0 };
    let Some(entry) = market.entries.get_mut(&item) else { return 0 };
    let price = unit_price(entry);
    let budget = accounts.get(buyer).map_or(0, |(c, _)| c.0.max(0));
    let affordable = if price > 0 { (budget / price) as u32 } else { qty };
    let bought = qty.min(entry.stock).min(affordable);
    if bought == 0 {
        return 0;
    }
    let cost = price * bought as i64;
    entry.stock -= bought;
    if let Ok((mut credits, mut ledger)) = accounts.get_mut(buyer) {
        credits.0 -= cost;
        ledger.expenses += cost;
    }
    bought
}

fn transfer(
    accounts: &mut Query<(&mut Credits, &mut Ledger)>,
    from: Entity,
    to: Entity,
    amount: i64,
) {
    if let Ok((mut credits, mut ledger)) = accounts.get_mut(from) {
        credits.0 -= amount;
        ledger.expenses += amount;
    }
    if let Ok((mut credits, mut ledger)) = accounts.get_mut(to) {
        credits.0 += amount;
        ledger.revenue += amount;
    }
}

fn station_distances(
    stations: &StationQuery,
    from: Entity,
    to: Entity,
) -> (u32, u32) {
    let from_ls = stations.get(from).map(|(s, _, _)| s.dist_ls).unwrap_or(0);
    let to_ls = stations.get(to).map(|(s, _, _)| s.dist_ls).unwrap_or(0);
    (from_ls, to_ls)
}
