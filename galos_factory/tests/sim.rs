//! Headless sim tests: no window, no GPU, no wall clock — the SimTick
//! schedule is driven directly, so every run is exactly reproducible.

use bevy::prelude::*;
use galos_factory::data::{BuildingKind, StaticData};
use galos_factory::sim::*;
use galos_factory::{seed, sim_plugin, snapshot::SystemSnapshot, SimTick};

fn headless_app() -> App {
    let mut app = App::new();
    sim_plugin(&mut app);
    app
}

fn sol() -> SystemSnapshot {
    ron::from_str(include_str!("../data/fixtures/sol.ron")).unwrap()
}

/// Spawns a commander with money, and an orbital station they own, in a
/// bare system with no BGS context.
fn commander_with_station(
    world: &mut World,
    storage: Storage,
) -> (Entity, Entity) {
    let system = world
        .spawn(StarSystemBundle {
            system: StarSystem { address: 1, name: "Test".into() },
            env: SystemEnv::default(),
            control: Control::default(),
        })
        .id();
    let commander = world.spawn(CommanderBundle::new("Tester", 1_000_000)).id();
    let station = world
        .spawn(StationBundle {
            station: Station {
                name: "Test Station".into(),
                placement: Placement::Orbital(None),
                dist_ls: 0,
            },
            in_system: InSystem(system),
            owner: OwnedBy(commander),
            slots: Slots { total: 8 },
            storage,
            power: PowerGrid::default(),
            life_support: LifeSupport::default(),
        })
        .id();
    (commander, station)
}

fn add_factory(
    world: &mut World,
    station: Entity,
    kind: BuildingKind,
    recipe: &str,
) -> Entity {
    let id = world
        .resource::<StaticData>()
        .recipe_by_name(recipe)
        .expect("known recipe");
    let factory =
        world.spawn((FactoryBundle::new(kind), ActiveRecipe(id))).id();
    world.entity_mut(station).add_child(factory);
    factory
}

#[test]
fn embedded_data_validates() {
    let data = StaticData::load().expect("embedded RON data is valid");
    assert!(data.items.len() >= 20);
    assert!(data.recipes.len() >= 20);
    // Spot-check the superset rule: ED ids join listings, pure grades don't.
    let copper = data.item_by_name("copper").unwrap();
    let pure = data.item_by_name("pure_copper").unwrap();
    assert!(data.item(copper).ed);
    assert!(!data.item(pure).ed);
    // Purification is 1:1.
    let purify = data.recipe(data.recipe_by_name("purify_copper").unwrap());
    assert_eq!(purify.inputs[0].1, purify.outputs[0].1);
}

/// A solar-powered orbital refinery smelting bauxite: exact output after a
/// fixed number of ticks.
#[test]
fn smelter_output_is_exact() {
    let mut app = headless_app();
    let world = app.world_mut();
    let data = world.resource::<StaticData>().clone();
    let bauxite = data.item_by_name("bauxite").unwrap();
    let aluminium = data.item_by_name("aluminium").unwrap();

    let mut storage = Storage::new(1000);
    storage.add(bauxite, 30);
    let (commander, station) = commander_with_station(world, storage);
    add_factory(world, station, BuildingKind::SolarArray, "solar_power");
    add_factory(world, station, BuildingKind::Refinery, "smelt_aluminium");

    // 10 crafts of 6 ticks each: 30 bauxite -> 20 aluminium.
    for _ in 0..60 {
        world.run_schedule(SimTick);
    }
    let storage = world.entity(station).get::<Storage>().unwrap();
    assert_eq!(storage.count(aluminium), 20);
    assert_eq!(storage.count(bauxite), 0);
    assert_eq!(world.resource::<SimClock>().tick, 60);

    // Production is booked to the station's owner, not a global counter.
    let ledger = world.entity(commander).get::<Ledger>().unwrap();
    assert_eq!(ledger.produced.get(&aluminium), Some(&20));
    assert_eq!(ledger.consumed.get(&bauxite), Some(&30));
}

/// Without power, nothing moves.
#[test]
fn brownout_scales_production() {
    let mut app = headless_app();
    let world = app.world_mut();
    let data = world.resource::<StaticData>().clone();
    let bauxite = data.item_by_name("bauxite").unwrap();
    let aluminium = data.item_by_name("aluminium").unwrap();

    let mut storage = Storage::new(1000);
    storage.add(bauxite, 300);
    let (_, station) = commander_with_station(world, storage);
    add_factory(world, station, BuildingKind::Refinery, "smelt_aluminium");

    for _ in 0..60 {
        world.run_schedule(SimTick);
    }
    let storage = world.entity(station).get::<Storage>().unwrap();
    // No generator at all: zero progress (inputs held for the first craft).
    assert_eq!(storage.count(aluminium), 0);
    let grid = world.entity(station).get::<PowerGrid>().unwrap();
    assert_eq!(grid.satisfaction_milli, 0);
}

/// An actor may only act on assets it owns.
#[test]
fn commands_are_validated_against_ownership() {
    let mut app = headless_app();
    let world = app.world_mut();
    let (_, station) = commander_with_station(world, Storage::new(1000));

    // A second commander tries to build in the first one's station.
    let intruder =
        world.spawn(CommanderBundle::new("Intruder", 1_000_000)).id();
    world.send_event(PlayerCommand::new(
        intruder,
        Action::Build { station, kind: BuildingKind::Refinery },
    ));
    world.run_schedule(SimTick);

    assert!(
        world.entity(station).get::<Children>().is_none(),
        "no factory should have been built by a non-owner",
    );
    let rejected = world
        .resource::<Notices>()
        .entries
        .iter()
        .any(|(_, n)| matches!(n, Notice::CommandRejected { .. }));
    assert!(rejected, "the attempt should be reported as rejected");
    // And the intruder's money is untouched.
    assert_eq!(world.entity(intruder).get::<Credits>().unwrap().0, 1_000_000);
}

/// Same seed, same commands => identical worlds after N ticks.
#[test]
fn runs_are_reproducible() {
    fn run() -> (i64, u64) {
        let mut app = App::new();
        sim_plugin(&mut app);
        let world = app.world_mut();
        let seeded = seed::apply(world, &sol());
        let commander =
            world.spawn(CommanderBundle::new("Tester", 100_000)).id();
        world.send_event(PlayerCommand::new(
            commander,
            Action::BuyOutpost {
                body: seeded.bodies["Mercury"],
                orbital: false,
                name: "Repro".into(),
            },
        ));
        for _ in 0..500 {
            world.run_schedule(SimTick);
        }
        let credits = world.entity(commander).get::<Credits>().unwrap().0;
        let tick = world.resource::<SimClock>().tick;
        (credits, tick)
    }
    assert_eq!(run(), run());
}

/// Happiness lives in the sim as `elite_journal::Happiness`; the band
/// number is purely a wire representation and never escapes serde.
#[test]
fn happiness_round_trips_as_a_band_number() {
    use galos_factory::snapshot::FactionSnapshot;

    for (band, variant) in [
        (0, "None"),
        (1, "Elated"),
        (2, "Happy"),
        (3, "Discontented"),
        (4, "Unhappy"),
        (5, "Despondent"),
    ] {
        let text = format!(
            r#"(name: "F", influence: 10.0, state: Boom, happiness_band: {band})"#
        );
        let faction: FactionSnapshot = ron::from_str(&text).unwrap();
        // Happiness has a deliberately non-reflexive PartialEq upstream
        // (None != None), so compare the variant, not the value.
        assert_eq!(format!("{:?}", faction.happiness), variant);
        assert!(ron::to_string(&faction)
            .unwrap()
            .contains(&format!("happiness_band:{band}")));
    }

    assert!(ron::from_str::<FactionSnapshot>(
        r#"(name: "F", influence: 1.0, state: Boom, happiness_band: 9)"#
    )
    .is_err());
}

/// Seeding spawns the real BGS structure — factions as corporations with a
/// presence per system — and control is re-derived from it each tick.
#[test]
fn seeding_spawns_factions_and_derives_control() {
    use elite_journal::station::StationType;

    let mut app = headless_app();
    let world = app.world_mut();
    let seeded = seed::apply(world, &sol());
    world.run_schedule(SimTick); // resolve_control runs

    assert_eq!(seeded.factions.len(), 3, "every faction is an entity");
    let mut presences = world.query::<&Presence>();
    assert_eq!(presences.iter(world).count(), 3, "one presence per faction");

    // Sol is High security; Mother Gaia (45% influence, Happy) controls it.
    let env = world.entity(seeded.system).get::<SystemEnv>().unwrap();
    assert_eq!(env.piracy_milli, 0);
    assert!(env.scoopable_star, "G-class star is scoopable");

    let control = world.entity(seeded.system).get::<Control>().unwrap();
    assert_eq!(control.faction, Some(seeded.factions["Mother Gaia"]));
    assert_eq!(control.tax_milli, 50);
    assert_eq!(control.productivity_milli, 1050);
    assert!(control.boom);

    // Stations belong to their real controlling faction, and slot counts
    // come from the station type.
    let lincoln = seeded.stations["Abraham Lincoln"];
    assert_eq!(
        world.entity(lincoln).get::<OwnedBy>().unwrap().0,
        seeded.factions["Mother Gaia"],
    );
    assert_eq!(
        world.entity(lincoln).get::<Slots>().unwrap().total,
        seed::slots_for(&StationType::Coriolis),
    );
    assert!(world.entity(lincoln).contains::<Shipyard>());

    // Boom lifts demand baselines 50% above the listing.
    let data = world.resource::<StaticData>().clone();
    let robotics = data.item_by_name("robotics").unwrap();
    let market = world.entity(lincoln).get::<Market>().unwrap();
    assert_eq!(market.entries[&robotics].demand_baseline, 1050);
}

#[test]
fn price_curve_shape() {
    let entry = |stock| MarketEntry {
        base_price: 1000,
        stock,
        demand_baseline: 1000,
        consumption_milli: 0,
        consum_accum_milli: 0,
    };
    // Scarce -> premium, balanced -> discount begins, glut -> floor.
    assert_eq!(price_milli(&entry(0)), 1600);
    assert_eq!(price_milli(&entry(1000)), 800);
    assert_eq!(price_milli(&entry(10_000)), 400);
    assert!(unit_price(&entry(0)) > unit_price(&entry(500)));
}

/// Storage iterates in item-id order regardless of insertion order, so a
/// system that consumes the pool in iteration order stays reproducible.
#[test]
fn storage_iteration_is_deterministic() {
    let data = StaticData::load().unwrap();
    let names = ["titanium", "bauxite", "water", "aluminium", "cobalt"];
    let mut ids: Vec<_> =
        names.iter().map(|n| data.item_by_name(n).unwrap()).collect();

    let mut forwards = Storage::new(1000);
    for id in &ids {
        forwards.add(*id, 5);
    }
    let mut backwards = Storage::new(1000);
    for id in ids.iter().rev() {
        backwards.add(*id, 5);
    }

    ids.sort();
    let expected: Vec<_> = ids.into_iter().map(|id| (id, 5)).collect();
    assert_eq!(forwards.iter().collect::<Vec<_>>(), expected);
    assert_eq!(backwards.iter().collect::<Vec<_>>(), expected);
    assert_eq!(forwards.total(), 25);

    // Emptying a slot drops it from iteration but keeps the order.
    forwards.take(expected[0].0, 5);
    assert_eq!(forwards.iter().count(), 4);
    assert_eq!(forwards.total(), 20);
}

/// The notice feed is bounded — a long game must not grow it forever.
#[test]
fn notices_are_bounded() {
    let mut notices = Notices { cap: 4, ..Default::default() };
    for tick in 0..100 {
        notices.push(tick, Notice::NoFuel { station: "X".into() });
    }
    assert_eq!(notices.entries.len(), 4);
    assert_eq!(notices.entries.front().unwrap().0, 96);
}
