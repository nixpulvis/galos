//! Standalone runner for the production sim.
//!
//! Headless (works without the `ui` feature):
//!     galos-factory --headless 2000 [fixture.ron]
//! steps the demo colony N ticks and prints the production report — the
//! balancing tool.
//!
//! Windowed (`ui` feature): runs the sim with the shared egui panels, no
//! 3D map involved.

use bevy::prelude::*;
use galos_factory::data::{BuildingKind, StaticData};
use galos_factory::sim::commands::PlayerCommand;
use galos_factory::sim::*;
use galos_factory::{seed, sim_plugin, snapshot::SystemSnapshot, SimTick};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let headless = args.iter().position(|a| a == "--headless");

    if let Some(i) = headless {
        let ticks: u64 = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(2000);
        let fixture = args.get(i + 2).cloned();
        run_headless(ticks, fixture);
    } else {
        #[cfg(feature = "ui")]
        run_windowed();
        #[cfg(not(feature = "ui"))]
        {
            eprintln!("built without the `ui` feature; use --headless <ticks>");
            std::process::exit(2);
        }
    }
}

fn load_fixture(path: Option<String>) -> SystemSnapshot {
    match path {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("reading {path}: {e}"));
            ron::from_str(&text).unwrap_or_else(|e| panic!("parsing {path}: {e}"))
        }
        None => ron::from_str(include_str!("../data/fixtures/sol.ron"))
            .expect("embedded sol fixture parses"),
    }
}

/// The demo scenario: a mining outpost on Mercury feeding a refinery line,
/// selling computer components at Abraham Lincoln — the full core loop.
fn demo_scenario(world: &mut World, snapshot: &SystemSnapshot) {
    let stations = seed::apply(world, snapshot);
    let mercury = seed::body_by_name(world, "Mercury").expect("Mercury in fixture");
    let lincoln = stations["Abraham Lincoln"];

    let data = world.resource::<StaticData>().clone();
    let recipe = |name: &str| data.recipe_by_name(name).expect("known recipe");

    // Fund the tutorial-skip: credits as if we'd been hauling for a while.
    // Undercapitalization is the classic early-game death spiral (credits
    // hit zero → ships can't fuel → imports stall → life support fails →
    // production halves) — the demo starts with a healthy float instead.
    world.resource_mut::<Credits>().0 = 800_000;

    let mut queue = world.resource_mut::<CommandQueue>();
    queue.0.push(PlayerCommand::BuyOutpost {
        body: mercury,
        orbital: false,
        name: "Demo Diggings".into(),
    });
    world.run_schedule(SimTick);

    let outpost = {
        let mut query = world.query::<(Entity, &Station)>();
        query
            .iter(world)
            .find(|(_, s)| s.owner == Owner::Player)
            .map(|(e, _)| e)
            .expect("outpost spawned")
    };

    // Bootstrap: buy construction materials + fuel + life support from the
    // NPC market, couriered to the outpost.
    let item = |name: &str| data.item_by_name(name).expect("known item");
    let mut queue = world.resource_mut::<CommandQueue>();
    for (what, qty) in [
        ("aluminium", 320u32),
        ("titanium", 100),
        ("copper", 40),
        ("polymers", 100),
        ("semiconductors", 16),
        ("powergenerators", 16),
        ("hydrogenfuel", 100),
        ("water", 60),
        ("foodcartridges", 60),
    ] {
        queue.0.push(PlayerCommand::MarketBuy {
            market: lincoln,
            to: outpost,
            item: item(what),
            qty,
        });
    }
    // Build the chain: power, mining, smelting, purifying, assembling —
    // ending in computer components, the product Lincoln's market is
    // actually starved for. Refining capacity is sized to eat the ore
    // (surplus metal gets exported rather than hoarded).
    for kind in [
        BuildingKind::PowerPlant,
        BuildingKind::PowerPlant,
        BuildingKind::PowerPlant,
        BuildingKind::Extractor,
        BuildingKind::Extractor,
        BuildingKind::Extractor,
        BuildingKind::Extractor,
        BuildingKind::Refinery,
        BuildingKind::Refinery,
        BuildingKind::Refinery,
        BuildingKind::Refinery,
        BuildingKind::Assembler,
        BuildingKind::Assembler,
    ] {
        queue.0.push(PlayerCommand::Build { station: outpost, kind });
    }
    world.run_schedule(SimTick);

    // Assign recipes (factories spawned in entity order).
    let factories: Vec<(Entity, BuildingKind)> = {
        let mut query = world.query::<(Entity, &Factory)>();
        query.iter(world).map(|(e, f)| (e, f.kind)).collect()
    };
    let mut extractors =
        factories.iter().filter(|(_, k)| *k == BuildingKind::Extractor).map(|(e, _)| *e);
    let mut refineries =
        factories.iter().filter(|(_, k)| *k == BuildingKind::Refinery).map(|(e, _)| *e);
    let plants =
        factories.iter().filter(|(_, k)| *k == BuildingKind::PowerPlant).map(|(e, _)| *e);
    let assemblers =
        factories.iter().filter(|(_, k)| *k == BuildingKind::Assembler).map(|(e, _)| *e);

    // (recipe, output cap): caps throttle everything without a fast consumer
    // or export, so the shared pool never silts up.
    let mut assignments: Vec<(Entity, &str, Option<u32>)> = Vec::new();
    for plant in plants {
        assignments.push((plant, "burn_hydrogen", None));
    }
    assignments.push((extractors.next().unwrap(), "mine_bauxite", Some(200)));
    assignments.push((extractors.next().unwrap(), "mine_gallite", Some(150)));
    assignments.push((extractors.next().unwrap(), "mine_gallite", Some(150)));
    assignments.push((extractors.next().unwrap(), "mine_copper", Some(200)));
    assignments.push((refineries.next().unwrap(), "smelt_aluminium", Some(150)));
    assignments.push((refineries.next().unwrap(), "smelt_gallium", Some(80)));
    assignments.push((refineries.next().unwrap(), "smelt_gallium", Some(80)));
    assignments.push((refineries.next().unwrap(), "purify_copper", Some(60)));
    let mut assembler_recipes =
        [("make_semiconductors", Some(80u32)), ("make_computercomponents", None)].iter();
    for assembler in assemblers {
        let (name, cap) = assembler_recipes.next().unwrap();
        assignments.push((assembler, name, *cap));
    }

    let mut queue = world.resource_mut::<CommandQueue>();
    for (factory, name, output_cap) in assignments {
        queue.0.push(PlayerCommand::SetRecipe {
            factory,
            recipe: Some(recipe(name)),
            output_cap,
        });
    }

    // Contracts: sell semiconductors at Abraham Lincoln; import fuel,
    // polymers, and life support back. Import contracts load by buying from
    // Lincoln's market at curve price — the colony lives on trade until
    // local water/oil/algae mining makes it self-sufficient (the player's
    // first expansion decision).
    // (item, from, to, target: dest ceiling, reserve: origin floor)
    let routes = [
        // Exports: the money-maker plus surplus metals (keeps the pool clear).
        ("computercomponents", outpost, lincoln, None, 0u32),
        ("aluminium", outpost, lincoln, None, 60),
        ("copper", outpost, lincoln, None, 60),
        // Imports with request thresholds.
        ("hydrogenfuel", lincoln, outpost, Some(100u32), 0),
        ("polymers", lincoln, outpost, Some(60), 0),
        ("water", lincoln, outpost, Some(60), 0),
        ("foodcartridges", lincoln, outpost, Some(60), 0),
    ];
    for (what, from, to, target, reserve) in routes {
        queue.0.push(PlayerCommand::CreateContract {
            from,
            to,
            item: item(what),
            pay_per_unit: 0,
            target,
            reserve,
        });
        queue.0.push(PlayerCommand::BuyShip { at: lincoln, class: ShipClass::Hauler });
    }
    world.run_schedule(SimTick);

    let contracts: Vec<Entity> = {
        let mut query = world.query::<(Entity, &Contract)>();
        query.iter(world).map(|(e, _)| e).collect()
    };
    let ships: Vec<Entity> = {
        let mut query = world.query::<(Entity, &Ship)>();
        query.iter(world).map(|(e, _)| e).collect()
    };
    assert_eq!(contracts.len(), ships.len(), "one ship per contract");
    let mut queue = world.resource_mut::<CommandQueue>();
    for (ship, contract) in ships.into_iter().zip(contracts) {
        queue.0.push(PlayerCommand::AssignShip { ship, contract: Some(contract) });
    }
}

fn run_headless(ticks: u64, fixture: Option<String>) {
    let snapshot = load_fixture(fixture);
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    sim_plugin(&mut app);
    demo_scenario(app.world_mut(), &snapshot);

    let world = app.world_mut();
    for _ in 0..ticks {
        world.run_schedule(SimTick);
    }
    report(world, ticks);
}

fn report(world: &mut World, ticks: u64) {
    let data = world.resource::<StaticData>().clone();
    let clock_tick = world.resource::<SimClock>().tick;
    let credits = world.resource::<Credits>().0;

    println!("=== galos_factory headless report ===");
    println!("ticks: {clock_tick} (requested {ticks})  credits: {credits} cr");

    let stats = world.resource::<Stats>();
    println!("\n item                 produced  consumed      sold   /100t");
    let mut items: Vec<_> = data.items.iter().enumerate().collect();
    items.sort_by_key(|(_, def)| (def.tier, def.id.clone()));
    for (index, def) in items {
        let id = galos_factory::data::ItemId(index as u16);
        let produced = stats.produced.get(&id).copied().unwrap_or(0);
        let consumed = stats.consumed.get(&id).copied().unwrap_or(0);
        let sold = stats.sold.get(&id).copied().unwrap_or(0);
        if produced == 0 && consumed == 0 && sold == 0 {
            continue;
        }
        let rate = produced as f64 * 100.0 / clock_tick.max(1) as f64;
        println!(" {:<20} {:>8} {:>9} {:>9}  {:>6.1}", def.id, produced, consumed, sold, rate);
    }
    println!("\n revenue: {} cr   expenses: {} cr", stats.revenue, stats.expenses);

    let mut stations = world.query::<(&Station, &Storage, &PowerGrid, Option<&LifeSupport>)>();
    println!("\n station                      stored  power(sup/dem)  life");
    for (station, storage, grid, condition) in stations.iter(world) {
        if station.owner != Owner::Player {
            continue;
        }
        println!(
            " {:<28} {:>5}/{:<5} {:>5}/{:<5}      {}",
            station.name,
            storage.total(),
            storage.cap,
            grid.supply_mw,
            grid.demand_mw,
            condition.map_or("-", |c| if c.life_support_ok { "ok" } else { "SHORT" }),
        );
        let mut inventory: Vec<_> = storage.pool.iter().collect();
        inventory.sort_by_key(|(item, _)| item.0);
        for (item, qty) in inventory {
            println!("    {:<24} {:>6}", data.item(*item).id, qty);
        }
    }

    let notices = world.resource::<Notices>();
    println!("\n last notices:");
    for (tick, notice) in notices.0.iter().rev().take(12).rev() {
        println!("  [{tick:>6}] {notice:?}");
    }
}

#[cfg(feature = "ui")]
fn run_windowed() {
    let snapshot = load_fixture(std::env::args().nth(1));
    let mut app = App::new();
    app.add_plugins(DefaultPlugins);
    app.add_plugins(bevy_egui::EguiPlugin);
    sim_plugin(&mut app);
    galos_factory::ui::ui_plugin(&mut app);
    demo_scenario(app.world_mut(), &snapshot);
    app.run();
}
