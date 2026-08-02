//! Player commands: the only way UI, scripts, and (eventually) network
//! clients mutate sim state. Sent as events, drained at the start of each
//! tick, and **validated against the issuing actor's ownership** — an actor
//! may only spend its own money and act on its own assets.

use super::*;
use crate::data::{BuildingKind, ItemId, RecipeId, Req, SiteKind, StaticData};
use bevy::prelude::*;

pub const OUTPOST_PRICE: i64 = 50_000;
pub const OUTPOST_SLOTS: u32 = 16;
pub const OUTPOST_STORAGE: u32 = 1000;
/// Instant NPC-market purchases pay a courier premium over curve price.
pub const MARKET_BUY_PREMIUM_MILLI: i64 = 1100;

/// A command and the actor issuing it.
#[derive(Event, Clone, Debug)]
pub struct PlayerCommand {
    pub actor: Entity,
    pub action: Action,
}

impl PlayerCommand {
    pub fn new(actor: Entity, action: Action) -> Self {
        PlayerCommand { actor, action }
    }
}

#[derive(Clone, Debug)]
pub enum Action {
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
    /// Standing contract; a supply route when both ends are the actor's.
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
    /// Instant, premium-priced purchase from an NPC market, couriered into
    /// one of the actor's stations. Bulk flows should use contracts; this
    /// exists for construction bootstrap.
    MarketBuy {
        market: Entity,
        to: Entity,
        item: ItemId,
        qty: u32,
    },
    /// Session controls, not owned by any actor.
    SetSpeed(SimSpeed),
    SetRngSeed(u64),
}

type StationQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Station,
        &'static mut Storage,
        &'static Slots,
        &'static OwnedBy,
        &'static InSystem,
        Option<&'static Children>,
    ),
>;

pub fn apply_commands(
    mut commands: Commands,
    (data, clock): (Res<StaticData>, Res<SimClock>),
    (mut incoming, mut speed, mut rng, mut notices): (
        EventReader<PlayerCommand>,
        ResMut<SimSpeed>,
        ResMut<SimRng>,
        ResMut<Notices>,
    ),
    mut actors: Query<(&mut Credits, &mut Ledger)>,
    mut stations: StationQuery,
    bodies: Query<(&Body, &InSystem, &Deposits, &BodyEnv)>,
    systems: Query<&SystemEnv>,
    mut factories: Query<(
        &Factory,
        &Parent,
        &mut CraftProgress,
        &mut OutputCap,
    )>,
    shipyards: Query<(), With<Shipyard>>,
    mut ships: Query<(&mut Ship, &OwnedBy)>,
    contracts: Query<&Contract>,
    mut markets: Query<&mut Market>,
) {
    let tick = clock.tick;
    for PlayerCommand { actor, action } in incoming.read().cloned() {
        let reject = |notices: &mut Notices, reason: &str| {
            notices
                .push(tick, Notice::CommandRejected { reason: reason.into() });
        };

        match action {
            Action::BuyOutpost { body, orbital, name } => {
                let Ok((_, in_system, _, _)) = bodies.get(body) else {
                    reject(&mut notices, "no such body");
                    continue;
                };
                let Ok((mut credits, _)) = actors.get_mut(actor) else {
                    continue;
                };
                if credits.0 < OUTPOST_PRICE {
                    reject(&mut notices, "insufficient credits for outpost");
                    continue;
                }
                credits.0 -= OUTPOST_PRICE;
                let placement = if orbital {
                    Placement::Orbital(Some(body))
                } else {
                    Placement::Surface(body)
                };
                let dist_ls =
                    bodies.get(body).map(|(b, _, _, _)| b.dist_ls).unwrap_or(0);
                commands.spawn(StationBundle {
                    station: Station { name, placement, dist_ls },
                    in_system: *in_system,
                    owner: OwnedBy(actor),
                    slots: Slots { total: OUTPOST_SLOTS },
                    storage: Storage::new(OUTPOST_STORAGE),
                    power: PowerGrid::default(),
                    life_support: LifeSupport::default(),
                });
            }

            Action::Build { station, kind } => {
                let Ok((st, mut storage, slots, owner, _, children)) =
                    stations.get_mut(station)
                else {
                    reject(&mut notices, "no such station");
                    continue;
                };
                if owner.0 != actor {
                    reject(&mut notices, "station belongs to someone else");
                    continue;
                }
                let def = data.building(kind);
                let used = children.map_or(0, |c| c.len() as u32);
                let site_ok = match (def.site, st.placement) {
                    (SiteKind::Any, _) => true,
                    (SiteKind::Surface, Placement::Surface(_)) => true,
                    (SiteKind::Orbital, Placement::Orbital(_)) => true,
                    _ => false,
                };
                let credits_ok = actors
                    .get(actor)
                    .map_or(false, |(c, _)| c.0 >= def.credits_cost as i64);

                let reason = if used >= slots.total {
                    Some("no free slots")
                } else if !site_ok {
                    Some("wrong site kind")
                } else if !credits_ok {
                    Some("insufficient credits")
                } else if !storage.has_all(&def.cost) {
                    Some("missing construction materials")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    reject(&mut notices, reason);
                    continue;
                }

                storage.take_all(&def.cost);
                let station_name = st.name.clone();
                if let Ok((mut credits, _)) = actors.get_mut(actor) {
                    credits.0 -= def.credits_cost as i64;
                }
                commands.spawn(FactoryBundle::new(kind)).set_parent(station);
                notices
                    .push(tick, Notice::Built { station: station_name, kind });
            }

            Action::SetRecipe { factory, recipe, output_cap } => {
                let Ok((fac, parent, _, _)) = factories.get(factory) else {
                    continue;
                };
                let station = parent.get();
                if !owns_station(&stations, station, actor) {
                    reject(&mut notices, "factory belongs to someone else");
                    continue;
                }
                let kind = fac.kind;
                let valid = recipe.map_or(true, |id| {
                    let def = data.recipe(id);
                    def.building == kind
                        && requirements_met(
                            &def.requires,
                            station,
                            &stations,
                            &bodies,
                            &systems,
                        )
                });
                if !valid {
                    reject(&mut notices, "recipe not possible here");
                    continue;
                }
                let Ok((_, _, mut progress, mut cap)) =
                    factories.get_mut(factory)
                else {
                    continue;
                };
                *progress = CraftProgress::default();
                *cap = OutputCap(output_cap);
                // Presence of the component *is* "running something", so
                // clearing a recipe removes it.
                match recipe {
                    Some(id) => {
                        commands.entity(factory).insert(ActiveRecipe(id));
                    }
                    None => {
                        commands.entity(factory).remove::<ActiveRecipe>();
                    }
                }
            }

            Action::Demolish { factory } => {
                let Ok((_, parent, _, _)) = factories.get(factory) else {
                    continue;
                };
                if !owns_station(&stations, parent.get(), actor) {
                    reject(&mut notices, "factory belongs to someone else");
                    continue;
                }
                commands.entity(factory).despawn();
            }

            Action::CreateContract {
                from,
                to,
                item,
                pay_per_unit,
                target,
                reserve,
            } => {
                if from == to
                    || stations.get(from).is_err()
                    || stations.get(to).is_err()
                {
                    reject(&mut notices, "invalid contract endpoints");
                    continue;
                }
                // At least one end must be the issuer's — you cannot move
                // goods between two parties you have nothing to do with.
                if !owns_station(&stations, from, actor)
                    && !owns_station(&stations, to, actor)
                {
                    reject(&mut notices, "neither endpoint belongs to you");
                    continue;
                }
                commands.spawn((
                    Contract { from, to, item, pay_per_unit, target, reserve },
                    OwnedBy(actor),
                ));
            }

            Action::BuyShip { at, class } => {
                if shipyards.get(at).is_err() {
                    reject(&mut notices, "no shipyard here");
                    continue;
                }
                let Ok((mut credits, _)) = actors.get_mut(actor) else {
                    continue;
                };
                if credits.0 < class.price() {
                    reject(&mut notices, "insufficient credits for ship");
                    continue;
                }
                credits.0 -= class.price();
                commands.spawn((
                    Ship {
                        class,
                        contract: None,
                        state: ShipState::Idle { at },
                    },
                    OwnedBy(actor),
                ));
            }

            Action::AssignShip { ship, contract } => {
                let Ok((mut s, owner)) = ships.get_mut(ship) else { continue };
                if owner.0 != actor {
                    reject(&mut notices, "ship belongs to someone else");
                    continue;
                }
                match contract {
                    Some(c) => {
                        if contracts.get(c).is_err() {
                            continue;
                        }
                        s.contract = Some(c);
                        s.state = ShipState::Loading;
                    }
                    None => {
                        if let Some(c) = s.contract.take() {
                            if let Ok(c) = contracts.get(c) {
                                s.state = ShipState::Idle { at: c.from };
                            }
                        }
                    }
                }
            }

            Action::MarketBuy { market, to, item, qty } => {
                if !owns_station(&stations, to, actor) {
                    reject(&mut notices, "destination belongs to someone else");
                    continue;
                }
                let Ok(mut market) = markets.get_mut(market) else { continue };
                let Some(entry) = market.entries.get_mut(&item) else {
                    continue;
                };
                let price = unit_price(entry) * MARKET_BUY_PREMIUM_MILLI / 1000;
                let budget = actors.get(actor).map_or(0, |(c, _)| c.0.max(0));
                let affordable =
                    if price > 0 { (budget / price) as u32 } else { qty };
                let wanted = qty.min(entry.stock).min(affordable);
                if wanted == 0 {
                    continue;
                }
                let Ok((st, mut storage, _, _, _, _)) = stations.get_mut(to)
                else {
                    continue;
                };
                let stored = storage.add(item, wanted);
                if stored == 0 {
                    continue;
                }
                let station_name = st.name.clone();
                let cost = price * stored as i64;
                entry.stock -= stored;
                if let Ok((mut credits, mut ledger)) = actors.get_mut(actor) {
                    credits.0 -= cost;
                    ledger.expenses += cost;
                }
                notices.push(
                    tick,
                    Notice::Bought {
                        station: station_name,
                        item,
                        qty: stored,
                        credits: cost,
                    },
                );
            }

            Action::SetSpeed(s) => *speed = s,
            Action::SetRngSeed(seed) => *rng = SimRng::from_seed(seed),
        }
    }
}

fn owns_station(
    stations: &StationQuery,
    station: Entity,
    actor: Entity,
) -> bool {
    stations
        .get(station)
        .map_or(false, |(_, _, _, owner, _, _)| owner.0 == actor)
}

fn requirements_met(
    reqs: &[Req],
    station: Entity,
    stations: &StationQuery,
    bodies: &Query<(&Body, &InSystem, &Deposits, &BodyEnv)>,
    systems: &Query<&SystemEnv>,
) -> bool {
    let Ok((st, _, _, _, in_system, _)) = stations.get(station) else {
        return false;
    };
    reqs.iter().all(|req| match req {
        Req::Deposit(item) => match st.placement {
            Placement::Surface(body) => bodies
                .get(body)
                .map(|(_, _, deposits, _)| {
                    deposits.0.iter().any(|(i, _)| i == item)
                })
                .unwrap_or(false),
            Placement::Orbital(_) => false,
        },
        Req::ScoopableStar => {
            matches!(st.placement, Placement::Orbital(_))
                && systems
                    .get(in_system.0)
                    .map(|env| env.scoopable_star)
                    .unwrap_or(false)
        }
        Req::Volcanism => match st.placement {
            Placement::Surface(body) => bodies
                .get(body)
                .map(|(_, _, _, env)| env.volcanism)
                .unwrap_or(false),
            Placement::Orbital(_) => false,
        },
    })
}
