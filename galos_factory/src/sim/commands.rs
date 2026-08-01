//! Player commands: the only way UI/hosts mutate sim state. Queued into
//! [`CommandQueue`] and applied at the start of each tick, keeping every
//! action tick-aligned and replayable.

use super::*;
use crate::data::{BuildingKind, ItemId, RecipeId, Req, SiteKind, StaticData};
use bevy::prelude::*;

pub const OUTPOST_PRICE: i64 = 50_000;
pub const OUTPOST_SLOTS: u32 = 16;
pub const OUTPOST_STORAGE: u32 = 1000;
/// Instant NPC-market purchases pay a courier premium over curve price.
pub const MARKET_BUY_PREMIUM_MILLI: i64 = 1100;

#[derive(Clone, Debug)]
pub enum PlayerCommand {
    /// Buy a turnkey outpost on (or orbiting) a body.
    BuyOutpost {
        body: Entity,
        orbital: bool,
        name: String,
    },
    Build {
        station: Entity,
        kind: BuildingKind,
    },
    SetRecipe {
        factory: Entity,
        recipe: Option<RecipeId>,
        output_cap: Option<u32>,
    },
    Demolish {
        factory: Entity,
    },
    /// Standing contract; a player supply route when self-issued.
    CreateContract {
        from: Entity,
        to: Entity,
        item: ItemId,
        pay_per_unit: u32,
        target: Option<u32>,
        reserve: u32,
    },
    BuyShip {
        at: Entity,
        class: ShipClass,
    },
    AssignShip {
        ship: Entity,
        contract: Option<Entity>,
    },
    /// Instant, premium-priced purchase from an NPC market, couriered into a
    /// player station. Bulk flows should use contracts; this is for
    /// construction bootstrap. TODO: replace with real haulage.
    MarketBuy {
        market: Entity,
        to: Entity,
        item: ItemId,
        qty: u32,
    },
    SetSpeed(SimSpeed),
    SetRngSeed(u64),
}

pub fn apply_commands(
    mut commands: Commands,
    (data, mods, clock): (Res<StaticData>, Res<SystemModifiers>, Res<SimClock>),
    (mut credits, mut queue, mut speed, mut rng, mut notices): (
        ResMut<Credits>,
        ResMut<CommandQueue>,
        ResMut<SimSpeed>,
        ResMut<SimRng>,
        ResMut<Notices>,
    ),
    mut stations: Query<(&Station, &mut Storage, &Slots)>,
    bodies: Query<(&Body, &Deposits, &BodyEnv)>,
    factories: Query<&Factory>,
    mut recipe_q: Query<(
        &Factory,
        &mut ActiveRecipe,
        &mut CraftProgress,
        &mut OutputCap,
    )>,
    shipyards: Query<(), With<Shipyard>>,
    mut ships: Query<&mut Ship>,
    contracts: Query<&Contract>,
    mut markets: Query<&mut Market>,
) {
    let tick = clock.tick;
    for command in queue.0.drain(..) {
        match command {
            PlayerCommand::BuyOutpost { body, orbital, name } => {
                if bodies.get(body).is_err() {
                    continue;
                }
                if credits.0 < OUTPOST_PRICE {
                    notices.0.push((
                        tick,
                        Notice::BuildFailed {
                            station: name,
                            reason: "insufficient credits for outpost".into(),
                        },
                    ));
                    continue;
                }
                credits.0 -= OUTPOST_PRICE;
                let placement = if orbital {
                    Placement::Orbital(Some(body))
                } else {
                    Placement::Surface(body)
                };
                let dist_ls =
                    bodies.get(body).map(|(b, _, _)| b.dist_ls).unwrap_or(0);
                commands.spawn((
                    Station { name, placement, owner: Owner::Player, dist_ls },
                    Slots { total: OUTPOST_SLOTS },
                    Storage::new(OUTPOST_STORAGE),
                    PowerGrid::default(),
                    LifeSupport::default(),
                ));
            }
            PlayerCommand::Build { station, kind } => {
                let Ok((st, mut storage, slots)) = stations.get_mut(station)
                else {
                    continue;
                };
                let def = data.building(kind);
                let used =
                    factories.iter().filter(|f| f.station == station).count()
                        as u32;
                let site_ok = match (def.site, st.placement) {
                    (SiteKind::Any, _) => true,
                    (SiteKind::Surface, Placement::Surface(_)) => true,
                    (SiteKind::Orbital, Placement::Orbital(_)) => true,
                    _ => false,
                };
                let reason = if used >= slots.total {
                    Some("no free slots")
                } else if !site_ok {
                    Some("wrong site kind")
                } else if credits.0 < def.credits_cost as i64 {
                    Some("insufficient credits")
                } else if !storage.has_all(&def.cost) {
                    Some("missing construction materials")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    notices.0.push((
                        tick,
                        Notice::BuildFailed {
                            station: st.name.clone(),
                            reason: reason.into(),
                        },
                    ));
                    continue;
                }
                storage.take_all(&def.cost);
                credits.0 -= def.credits_cost as i64;
                commands.spawn((
                    Factory { kind, station },
                    ActiveRecipe(None),
                    OutputCap(None),
                    CraftProgress::default(),
                    Status::default(),
                    MaintenanceDue(false),
                ));
                notices.0.push((
                    tick,
                    Notice::Built { station: st.name.clone(), kind },
                ));
            }
            PlayerCommand::SetRecipe { factory, recipe, output_cap } => {
                let Ok((fac, mut active, mut progress, mut cap)) =
                    recipe_q.get_mut(factory)
                else {
                    continue;
                };
                let valid = recipe.map_or(true, |id| {
                    let def = data.recipe(id);
                    def.building == fac.kind
                        && requirements_met(
                            &def.requires,
                            fac.station,
                            &stations,
                            &bodies,
                            &mods,
                            &data,
                        )
                });
                if valid {
                    *active = ActiveRecipe(recipe);
                    *progress = CraftProgress::default();
                    *cap = OutputCap(output_cap);
                }
            }
            PlayerCommand::Demolish { factory } => {
                if recipe_q.get(factory).is_ok() {
                    commands.entity(factory).despawn();
                }
            }
            PlayerCommand::CreateContract {
                from,
                to,
                item,
                pay_per_unit,
                target,
                reserve,
            } => {
                if stations.get(from).is_ok()
                    && stations.get(to).is_ok()
                    && from != to
                {
                    commands.spawn(Contract {
                        issuer: Owner::Player,
                        from,
                        to,
                        item,
                        pay_per_unit,
                        target,
                        reserve,
                    });
                }
            }
            PlayerCommand::BuyShip { at, class } => {
                if shipyards.get(at).is_err() || credits.0 < class.price() {
                    continue;
                }
                credits.0 -= class.price();
                commands.spawn(Ship {
                    class,
                    owner: Owner::Player,
                    contract: None,
                    state: ShipState::Idle { at },
                });
            }
            PlayerCommand::AssignShip { ship, contract } => {
                let Ok(mut s) = ships.get_mut(ship) else { continue };
                if let Some(c) = contract {
                    if contracts.get(c).is_err() {
                        continue;
                    }
                    s.contract = Some(c);
                    s.state = ShipState::Loading;
                } else {
                    if let Some(c) = s.contract.take() {
                        let at = contracts.get(c).map(|c| c.from).ok();
                        if let Some(at) = at {
                            s.state = ShipState::Idle { at };
                        }
                    }
                }
            }
            PlayerCommand::MarketBuy { market, to, item, qty } => {
                let Ok(mut m) = markets.get_mut(market) else { continue };
                let Some(entry) = m.entries.get_mut(&item) else { continue };
                let qty = qty.min(entry.stock);
                if qty == 0 {
                    continue;
                }
                let cost =
                    unit_price(entry) * qty as i64 * MARKET_BUY_PREMIUM_MILLI
                        / 1000;
                if credits.0 < cost {
                    continue;
                }
                let Ok((st, mut storage, _)) = stations.get_mut(to) else {
                    continue;
                };
                let stored = storage.add(item, qty);
                if stored == 0 {
                    continue;
                }
                let cost = unit_price(entry)
                    * stored as i64
                    * MARKET_BUY_PREMIUM_MILLI
                    / 1000;
                entry.stock -= stored;
                credits.0 -= cost;
                notices.0.push((
                    tick,
                    Notice::Bought {
                        station: st.name.clone(),
                        item,
                        qty: stored,
                        credits: cost,
                    },
                ));
            }
            PlayerCommand::SetSpeed(s) => *speed = s,
            PlayerCommand::SetRngSeed(seed) => *rng = SimRng::from_seed(seed),
        }
    }
}

fn requirements_met(
    reqs: &[Req],
    station: Entity,
    stations: &Query<(&Station, &mut Storage, &Slots)>,
    bodies: &Query<(&Body, &Deposits, &BodyEnv)>,
    mods: &SystemModifiers,
    data: &StaticData,
) -> bool {
    let Ok((st, _, _)) = stations.get(station) else { return false };
    reqs.iter().all(|req| match req {
        Req::Deposit(name) => {
            let Some(item) = data.item_by_name(name) else { return false };
            match st.placement {
                Placement::Surface(body) => bodies
                    .get(body)
                    .map(|(_, deposits, _)| {
                        deposits.0.iter().any(|(i, _)| *i == item)
                    })
                    .unwrap_or(false),
                Placement::Orbital(_) => false,
            }
        }
        Req::ScoopableStar => {
            matches!(st.placement, Placement::Orbital(_)) && mods.scoopable_star
        }
        Req::Volcanism => match st.placement {
            Placement::Surface(body) => bodies
                .get(body)
                .map(|(_, _, env)| env.volcanism)
                .unwrap_or(false),
            Placement::Orbital(_) => false,
        },
    })
}
