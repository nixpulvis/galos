//! Headless sim tests: no window, no GPU, no wall clock — the SimTick
//! schedule is driven directly, so every run is exactly reproducible.

use bevy::prelude::*;
use galos_factory::data::{BuildingKind, StaticData};
use galos_factory::sim::*;
use galos_factory::{sim_plugin, SimTick};

fn headless_app() -> App {
    let mut app = App::new();
    sim_plugin(&mut app);
    app
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
    let station = world
        .spawn((
            Station {
                name: "Test".into(),
                placement: Placement::Orbital(None),
                owner: Owner::Player,
                dist_ls: 0,
            },
            Slots { total: 4 },
            storage,
            PowerGrid::default(),
            LifeSupport::default(),
        ))
        .id();
    world.spawn((
        Factory { kind: BuildingKind::SolarArray, station },
        ActiveRecipe(Some(data.recipe_by_name("solar_power").unwrap())),
        OutputCap(None),
        CraftProgress::default(),
        Status::default(),
        MaintenanceDue(false),
    ));
    world.spawn((
        Factory { kind: BuildingKind::Refinery, station },
        ActiveRecipe(Some(data.recipe_by_name("smelt_aluminium").unwrap())),
        OutputCap(None),
        CraftProgress::default(),
        Status::default(),
        MaintenanceDue(false),
    ));

    // 10 crafts of 6 ticks each: 30 bauxite -> 20 aluminium.
    for _ in 0..60 {
        world.run_schedule(SimTick);
    }
    let storage = world.entity(station).get::<Storage>().unwrap();
    assert_eq!(storage.count(aluminium), 20);
    assert_eq!(storage.count(bauxite), 0);
    assert_eq!(world.resource::<SimClock>().tick, 60);
}

/// Without power, nothing moves; with half power, everything browns out
/// proportionally.
#[test]
fn brownout_scales_production() {
    let mut app = headless_app();
    let world = app.world_mut();
    let data = world.resource::<StaticData>().clone();
    let bauxite = data.item_by_name("bauxite").unwrap();
    let aluminium = data.item_by_name("aluminium").unwrap();

    let mut storage = Storage::new(1000);
    storage.add(bauxite, 300);
    let station = world
        .spawn((
            Station {
                name: "Dark".into(),
                placement: Placement::Orbital(None),
                owner: Owner::Player,
                dist_ls: 0,
            },
            Slots { total: 4 },
            storage,
            PowerGrid::default(),
            LifeSupport::default(),
        ))
        .id();
    world.spawn((
        Factory { kind: BuildingKind::Refinery, station },
        ActiveRecipe(Some(data.recipe_by_name("smelt_aluminium").unwrap())),
        OutputCap(None),
        CraftProgress::default(),
        Status::default(),
        MaintenanceDue(false),
    ));

    for _ in 0..60 {
        world.run_schedule(SimTick);
    }
    let storage = world.entity(station).get::<Storage>().unwrap();
    // No generator at all: zero progress (inputs held for the first craft).
    assert_eq!(storage.count(aluminium), 0);
    let grid = world.entity(station).get::<PowerGrid>().unwrap();
    assert_eq!(grid.satisfaction_milli, 0);
}

/// Same seed, same commands => identical worlds after N ticks.
#[test]
fn runs_are_reproducible() {
    fn run() -> (i64, u64) {
        let mut app = App::new();
        sim_plugin(&mut app);
        let world = app.world_mut();
        let snapshot: galos_factory::snapshot::SystemSnapshot =
            ron::from_str(include_str!("../data/fixtures/sol.ron")).unwrap();
        galos_factory::seed::apply(world, &snapshot);
        world.resource_mut::<Credits>().0 = 100_000;
        for _ in 0..500 {
            world.run_schedule(SimTick);
        }
        let credits = world.resource::<Credits>().0;
        let tick = world.resource::<SimClock>().tick;
        (credits, tick)
    }
    assert_eq!(run(), run());
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
