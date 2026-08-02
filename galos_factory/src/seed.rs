//! Seeding: maps a [`SystemSnapshot`] onto the ECS world — a star system
//! with its environment, the factions present in it (as corporations, with
//! a treasury and one [`Presence`] per system they operate in), bodies with
//! deposits derived from geology, and faction-owned stations whose markets
//! come from real listings. Implements the BGS → gameplay table in
//! DESIGN.md.

use crate::data::{ItemId, StaticData};
use crate::sim::*;
use crate::snapshot::*;
use bevy::prelude::*;
use elite_journal::station::StationType;
use elite_journal::system::Security;
use std::collections::HashMap;

pub const NPC_STORAGE: u32 = 10_000;
/// Starting treasury for a seeded faction. Real faction wealth is not
/// something the BGS exposes, so this is a game-balance number.
pub const FACTION_TREASURY: i64 = 100_000_000;

/// What a seeded system yielded, so hosts can wire up scenarios.
pub struct Seeded {
    pub system: Entity,
    pub bodies: HashMap<String, Entity>,
    pub stations: HashMap<String, Entity>,
    pub factions: HashMap<String, Entity>,
}

/// Leasable factory slots by station type — the big hub ports carry real
/// industry, outposts barely any, carriers none.
pub fn slots_for(ty: &StationType) -> u32 {
    match ty {
        StationType::Orbis | StationType::Ocellus | StationType::Coriolis => 24,
        StationType::AsteroidBase | StationType::MegaShip => 16,
        StationType::CraterPort => 12,
        StationType::Outpost | StationType::CraterOutpost => 6,
        StationType::FleetCarrier => 0,
    }
}

/// Deposits granted per planet class: `(item, richness_milli)`.
fn deposits_for(data: &StaticData, body: &BodySnapshot) -> Vec<(ItemId, u32)> {
    let item = |name: &str| data.item_by_name(name).expect("catalog item");
    let volcanism_bonus = if body.volcanism.is_some() { 500 } else { 0 };
    let metal = |richness: u32| -> Vec<(ItemId, u32)> {
        vec![
            (item("bauxite"), richness + volcanism_bonus),
            (item("rutile"), richness + volcanism_bonus),
            (item("gallite"), richness / 2 + volcanism_bonus),
        ]
    };
    let mut deposits = match body.planet_class.as_str() {
        "Metal rich body" => {
            let mut d = metal(1500);
            d.push((item("cobalt"), 1200));
            d.push((item("copper"), 1500));
            d
        }
        "High metal content body" => {
            let mut d = metal(1000);
            d.push((item("cobalt"), 800));
            d.push((item("copper"), 1000));
            d
        }
        "Rocky body" => {
            let mut d = vec![(item("bauxite"), 600), (item("cobalt"), 400)];
            if body.atmosphere.is_some() {
                d.push((item("mineraloil"), 1000));
            }
            d
        }
        "Icy body" | "Rocky ice body" => vec![(item("water"), 1500)],
        "Water world" | "Earthlike body" => vec![(item("algae"), 1000)],
        _ => vec![],
    };
    deposits.retain(|(_, richness)| *richness > 0);
    deposits
}

fn scoopable(class: &str) -> bool {
    matches!(
        class.chars().next(),
        Some('O' | 'B' | 'A' | 'F' | 'G' | 'K' | 'M')
    )
}

fn solar_milli(class: &str) -> u32 {
    match class.chars().next() {
        Some('O' | 'B' | 'A') => 1500,
        Some('F' | 'G') => 1000,
        Some('K') => 800,
        Some('M') => 600,
        _ => 300,
    }
}

fn piracy_milli(security: Security) -> u32 {
    match security {
        Security::High => 0,
        Security::Medium => 20,
        Security::Low => 50,
        // Anarchy and unknown security are equally lawless.
        Security::Anarchy | Security::None => 100,
    }
}

/// Seeds one star system and everything in it.
pub fn apply(world: &mut World, snapshot: &SystemSnapshot) -> Seeded {
    let data = world.resource::<StaticData>().clone();
    let star_class =
        snapshot.stars.first().map(|s| s.class.as_str()).unwrap_or("");

    let system = world
        .spawn(StarSystemBundle {
            system: StarSystem {
                address: snapshot.address,
                name: snapshot.name.clone(),
            },
            env: SystemEnv {
                piracy_milli: piracy_milli(snapshot.security),
                solar_milli: solar_milli(star_class),
                scoopable_star: scoopable(star_class),
            },
            // Derived from Presence on the first tick.
            control: Control::default(),
        })
        .id();

    // Factions are corporations: an account, and a presence per system.
    // A snapshot with no factions at all still needs somebody to own the
    // stations, so stand up an independent one.
    let mut factions = HashMap::new();
    if snapshot.factions.is_empty() && !snapshot.stations.is_empty() {
        let entity = world
            .spawn(FactionBundle::new("Independent", FACTION_TREASURY))
            .id();
        factions.insert("Independent".to_string(), entity);
    }
    for faction in &snapshot.factions {
        let entity = world
            .spawn(FactionBundle::new(faction.name.clone(), FACTION_TREASURY))
            .id();
        world.spawn(Presence {
            faction: entity,
            system,
            influence: faction.influence,
            state: faction.state,
            happiness: faction.happiness,
        });
        factions.insert(faction.name.clone(), entity);
    }
    // Whoever holds the most influence also owns the unattributed stations.
    let default_owner = snapshot
        .factions
        .iter()
        .max_by(|a, b| a.influence.total_cmp(&b.influence))
        .and_then(|f| factions.get(&f.name).copied())
        .or_else(|| factions.values().next().copied());

    let mut bodies = HashMap::new();
    for body in &snapshot.bodies {
        let entity = world
            .spawn(BodyBundle {
                body: Body { name: body.name.clone(), dist_ls: body.dist_ls },
                in_system: InSystem(system),
                deposits: Deposits(deposits_for(&data, body)),
                env: BodyEnv {
                    volcanism: body.volcanism.is_some(),
                    overhead_milli: 1000,
                },
            })
            .id();
        bodies.insert(body.name.clone(), entity);
    }

    let boom = snapshot
        .factions
        .iter()
        .max_by(|a, b| a.influence.total_cmp(&b.influence))
        .map(|f| matches!(f.state, elite_journal::faction::State::Boom))
        .unwrap_or(false);

    let mut stations = HashMap::new();
    for station in &snapshot.stations {
        let placement = match (&station.body, station.surface) {
            (Some(name), surface) => {
                let body =
                    bodies.get(name.as_str()).copied().unwrap_or_else(|| {
                        panic!(
                            "station `{}` sits at unknown body `{name}`",
                            station.name,
                        )
                    });
                if surface {
                    Placement::Surface(body)
                } else {
                    Placement::Orbital(Some(body))
                }
            }
            (None, _) => Placement::Orbital(None),
        };
        let owner = station
            .controlling_faction
            .as_ref()
            .and_then(|name| factions.get(name).copied())
            .or(default_owner)
            .unwrap_or_else(|| {
                panic!("station `{}` has no owning faction", station.name)
            });

        let mut market = Market::default();
        for listing in &station.listings {
            let Some(item) = data.item_by_name(&listing.item) else { continue };
            let demand_baseline = if boom {
                listing.demand + listing.demand / 2
            } else {
                listing.demand
            };
            market.entries.insert(
                item,
                MarketEntry {
                    base_price: listing.mean_price,
                    stock: listing.stock,
                    demand_baseline: demand_baseline.max(1),
                    // Bracket-ish NPC drain: consume the demand baseline
                    // over ~2000 ticks.
                    consumption_milli: (demand_baseline as u64 * 1000 / 2000)
                        .max(1) as u32,
                    consum_accum_milli: 0,
                },
            );
        }

        let entity = world
            .spawn((
                StationBundle {
                    station: Station {
                        name: station.name.clone(),
                        placement,
                        dist_ls: station.dist_ls,
                    },
                    in_system: InSystem(system),
                    owner: OwnedBy(owner),
                    slots: Slots { total: slots_for(&station.ty) },
                    storage: Storage::new(NPC_STORAGE),
                    power: PowerGrid::default(),
                    life_support: LifeSupport::default(),
                },
                market,
            ))
            .id();
        if station.shipyard {
            world.entity_mut(entity).insert(Shipyard);
        }
        stations.insert(station.name.clone(), entity);
    }

    Seeded { system, bodies, stations, factions }
}
