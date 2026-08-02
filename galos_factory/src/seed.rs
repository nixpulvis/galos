//! Seeding: maps a [`SystemSnapshot`] onto the ECS world — bodies with
//! deposits derived from geology, NPC stations with markets seeded from
//! listings, and system modifiers from BGS state. Implements the BGS →
//! gameplay table in DESIGN.md.

use crate::data::StaticData;
use crate::sim::*;
use crate::snapshot::*;
use bevy::prelude::*;
use elite_journal::faction::{Happiness, State};
use elite_journal::station::StationType;
use elite_journal::system::Security;
use std::collections::HashMap;

pub const NPC_STORAGE: u32 = 10_000;

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
fn deposits_for(
    data: &StaticData,
    body: &BodySnapshot,
) -> Vec<(crate::data::ItemId, u32)> {
    let item = |name: &str| data.item_by_name(name).expect("catalog item");
    let volcanism_bonus = if body.volcanism.is_some() { 500 } else { 0 };
    let metal = |richness: u32| -> Vec<(crate::data::ItemId, u32)> {
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

/// Seeds the world and returns the station name → entity mapping so hosts
/// can wire up scenarios.
pub fn apply(
    world: &mut World,
    snapshot: &SystemSnapshot,
) -> HashMap<String, Entity> {
    let data = world.resource::<StaticData>().clone();

    // System modifiers from BGS context.
    let controlling = snapshot
        .factions
        .iter()
        .max_by(|a, b| a.influence.total_cmp(&b.influence));
    // Happier workforces build faster; an unknown band is neutral.
    let productivity_milli =
        match controlling.map(|f| f.happiness).unwrap_or(Happiness::None) {
            Happiness::Elated => 1100,
            Happiness::Happy => 1050,
            Happiness::Discontented | Happiness::None => 1000,
            Happiness::Unhappy => 900,
            Happiness::Despondent => 800,
        };
    let tax_milli = match controlling.map(|f| f.influence).unwrap_or(0.0) {
        i if i >= 60.0 => 25,
        i if i >= 40.0 => 50,
        i if i >= 20.0 => 75,
        _ => 100,
    };
    let piracy_milli = match snapshot.security {
        Security::High => 0,
        Security::Medium => 20,
        Security::Low => 50,
        // Anarchy and unknown security are equally lawless.
        Security::Anarchy | Security::None => 100,
    };
    let boom =
        controlling.map(|f| matches!(f.state, State::Boom)).unwrap_or(false);
    let star_class = snapshot
        .stars
        .first()
        .map(|s| s.class.as_str())
        .unwrap_or("")
        .to_string();
    world.insert_resource(SystemModifiers {
        productivity_milli,
        tax_milli,
        piracy_milli,
        solar_milli: solar_milli(&star_class),
        scoopable_star: scoopable(&star_class),
    });

    // Bodies.
    let mut body_entities: HashMap<String, Entity> = HashMap::new();
    for body in &snapshot.bodies {
        let deposits = Deposits(deposits_for(&data, body));
        let entity = world
            .spawn((
                Body { name: body.name.clone(), dist_ls: body.dist_ls },
                deposits,
                BodyEnv {
                    volcanism: body.volcanism.is_some(),
                    overhead_milli: 1000,
                },
            ))
            .id();
        body_entities.insert(body.name.clone(), entity);
    }

    // NPC stations with markets. Boom raises demand baselines.
    let mut station_entities = HashMap::new();
    for station in &snapshot.stations {
        let placement = match (&station.body, station.surface) {
            (Some(body), true) => {
                Placement::Surface(body_entities[body.as_str()])
            }
            (Some(body), false) => {
                Placement::Orbital(Some(body_entities[body.as_str()]))
            }
            (None, _) => Placement::Orbital(None),
        };
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
                Station {
                    name: station.name.clone(),
                    placement,
                    owner: Owner::Npc,
                    dist_ls: station.dist_ls,
                },
                Slots { total: slots_for(&station.ty) },
                Storage::new(NPC_STORAGE),
                PowerGrid::default(),
                LifeSupport::default(),
                market,
            ))
            .id();
        if station.shipyard {
            world.entity_mut(entity).insert(Shipyard);
        }
        station_entities.insert(station.name.clone(), entity);
    }

    station_entities
}

/// Returns the body name → entity mapping (post-seed helper for scenarios).
pub fn body_by_name(world: &mut World, name: &str) -> Option<Entity> {
    let mut query = world.query::<(Entity, &Body)>();
    query.iter(world).find(|(_, b)| b.name == name).map(|(e, _)| e)
}
