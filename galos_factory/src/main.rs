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
use galos_factory::data::{BuildingKind, ItemId, StaticData};
use galos_factory::seed::Seeded;
use galos_factory::sim::*;
use galos_factory::{seed, sim_plugin, snapshot::SystemSnapshot, SimTick};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let headless = args.iter().position(|a| a == "--headless");

    if let Some(i) = headless {
        let ticks: u64 =
            args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(2000);
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
            ron::from_str(&text)
                .unwrap_or_else(|e| panic!("parsing {path}: {e}"))
        }
        None => ron::from_str(include_str!("../data/fixtures/sol.ron"))
            .expect("embedded sol fixture parses"),
    }
}

/// The demo scenario: a commander with a mining outpost on Mercury feeding
/// a refinery line, selling computer components at Abraham Lincoln — the
/// full core loop.
fn demo_scenario(world: &mut World, snapshot: &SystemSnapshot) -> Entity {
    let Seeded { bodies, stations, factions, .. } =
        seed::apply(world, snapshot);
    let mercury = bodies["Mercury"];
    let lincoln = stations["Abraham Lincoln"];

    let data = world.resource::<StaticData>().clone();
    let recipe = |name: &str| data.recipe_by_name(name).expect("known recipe");
    let item = |name: &str| data.item_by_name(name).expect("known item");

    // A commander who has been hauling contracts for a while. Under-
    // capitalization is the classic early-game death spiral (credits hit
    // zero → ships can't fuel → imports stall → life support fails), so the
    // demo starts with a healthy float.
    let commander = world
        .spawn((
            CommanderBundle::new("Demo Commander", 800_000),
            MemberOf(factions["Mother Gaia"]),
        ))
        .id();
    let send = |world: &mut World, action: Action| {
        world.send_event(PlayerCommand::new(commander, action));
    };

    send(
        world,
        Action::BuyOutpost {
            body: mercury,
            orbital: false,
            name: "Demo Diggings".into(),
        },
    );
    world.run_schedule(SimTick);

    let outpost = {
        let mut query = world.query::<(Entity, &Station, &OwnedBy)>();
        query
            .iter(world)
            .find(|(_, _, owner)| owner.0 == commander)
            .map(|(e, _, _)| e)
            .expect("outpost spawned")
    };

    // Bootstrap: buy construction materials, fuel, and life support from
    // the NPC market, couriered to the outpost.
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
        send(
            world,
            Action::MarketBuy {
                market: lincoln,
                to: outpost,
                item: item(what),
                qty,
            },
        );
    }
    // Build the chain: power, mining, smelting, purifying, assembling —
    // ending in computer components, the product Lincoln's market is
    // actually starved for. Refining capacity is sized to eat the ore.
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
        send(world, Action::Build { station: outpost, kind });
    }
    world.run_schedule(SimTick);

    // Assign recipes (factories spawned in entity order).
    let factories: Vec<(Entity, BuildingKind)> = {
        let mut query = world.query::<(Entity, &Factory)>();
        query.iter(world).map(|(e, f)| (e, f.kind)).collect()
    };
    let of_kind = |kind: BuildingKind| -> Vec<Entity> {
        factories.iter().filter(|(_, k)| *k == kind).map(|(e, _)| *e).collect()
    };
    let mut extractors = of_kind(BuildingKind::Extractor).into_iter();
    let mut refineries = of_kind(BuildingKind::Refinery).into_iter();

    // (recipe, output cap): caps throttle everything without a fast
    // consumer or export, so the shared pool never silts up.
    let mut assignments: Vec<(Entity, &str, Option<u32>)> = Vec::new();
    for plant in of_kind(BuildingKind::PowerPlant) {
        assignments.push((plant, "burn_hydrogen", None));
    }
    assignments.push((extractors.next().unwrap(), "mine_bauxite", Some(200)));
    assignments.push((extractors.next().unwrap(), "mine_gallite", Some(150)));
    assignments.push((extractors.next().unwrap(), "mine_gallite", Some(150)));
    assignments.push((extractors.next().unwrap(), "mine_copper", Some(200)));
    assignments.push((
        refineries.next().unwrap(),
        "smelt_aluminium",
        Some(150),
    ));
    assignments.push((refineries.next().unwrap(), "smelt_gallium", Some(80)));
    assignments.push((refineries.next().unwrap(), "smelt_gallium", Some(80)));
    assignments.push((refineries.next().unwrap(), "purify_copper", Some(60)));
    let mut assembler_recipes = [
        ("make_semiconductors", Some(80u32)),
        ("make_computercomponents", None),
    ]
    .into_iter();
    for assembler in of_kind(BuildingKind::Assembler) {
        let (name, cap) = assembler_recipes.next().unwrap();
        assignments.push((assembler, name, cap));
    }
    for (factory, name, output_cap) in assignments {
        send(
            world,
            Action::SetRecipe {
                factory,
                recipe: Some(recipe(name)),
                output_cap,
            },
        );
    }

    // Contracts: sell computer components at Abraham Lincoln, export
    // surplus metals, import fuel/polymers/life support back. Imports load
    // by buying from Lincoln's market at curve price — the colony lives on
    // trade until local water/oil/algae mining makes it self-sufficient
    // (the player's first expansion decision).
    // (item, from, to, target: dest ceiling, reserve: origin floor)
    let routes = [
        ("computercomponents", outpost, lincoln, None, 0u32),
        ("aluminium", outpost, lincoln, None, 60),
        ("copper", outpost, lincoln, None, 60),
        ("hydrogenfuel", lincoln, outpost, Some(100), 0),
        ("polymers", lincoln, outpost, Some(60), 0),
        ("water", lincoln, outpost, Some(60), 0),
        ("foodcartridges", lincoln, outpost, Some(60), 0),
    ];
    for (what, from, to, target, reserve) in routes {
        send(
            world,
            Action::CreateContract {
                from,
                to,
                item: item(what),
                pay_per_unit: 0,
                target,
                reserve,
            },
        );
        send(world, Action::BuyShip { at: lincoln, class: ShipClass::Hauler });
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
    for (ship, contract) in ships.into_iter().zip(contracts) {
        send(world, Action::AssignShip { ship, contract: Some(contract) });
    }
    commander
}

fn run_headless(ticks: u64, fixture: Option<String>) {
    let snapshot = load_fixture(fixture);
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    sim_plugin(&mut app);
    let commander = demo_scenario(app.world_mut(), &snapshot);

    let world = app.world_mut();
    for _ in 0..ticks {
        world.run_schedule(SimTick);
    }
    report(world, commander, ticks);
}

fn report(world: &mut World, commander: Entity, ticks: u64) {
    let data = world.resource::<StaticData>().clone();
    let clock_tick = world.resource::<SimClock>().tick;
    let credits = world.entity(commander).get::<Credits>().unwrap().0;
    let ledger = world.entity(commander).get::<Ledger>().unwrap().clone();

    println!("=== galos_factory headless report ===");
    println!("ticks: {clock_tick} (requested {ticks})  credits: {credits} cr");

    println!("\n item                 produced  consumed      sold   /100t");
    let mut items: Vec<_> = data.items.iter().enumerate().collect();
    items.sort_by_key(|(_, def)| (def.tier, def.id.clone()));
    for (index, def) in items {
        let id = ItemId(index as u16);
        let produced = ledger.produced.get(&id).copied().unwrap_or(0);
        let consumed = ledger.consumed.get(&id).copied().unwrap_or(0);
        let sold = ledger.sold.get(&id).copied().unwrap_or(0);
        if produced == 0 && consumed == 0 && sold == 0 {
            continue;
        }
        let rate = produced as f64 * 100.0 / clock_tick.max(1) as f64;
        println!(
            " {:<20} {:>8} {:>9} {:>9}  {:>6.1}",
            def.id, produced, consumed, sold, rate
        );
    }
    println!(
        "\n revenue: {} cr   expenses: {} cr",
        ledger.revenue, ledger.expenses
    );

    println!("\n station                      stored  power(sup/dem)  life");
    let mut stations =
        world
            .query::<(&Station, &OwnedBy, &Storage, &PowerGrid, &LifeSupport)>(
            );
    let rows: Vec<String> = stations
        .iter(world)
        .filter(|(_, owner, _, _, _)| owner.0 == commander)
        .map(|(station, _, storage, grid, life)| {
            let mut inventory: Vec<_> = storage.pool.iter().collect();
            inventory.sort_by_key(|(item, _)| item.0);
            let lines: String = inventory
                .iter()
                .map(|(item, qty)| {
                    format!("\n    {:<24} {:>6}", data.item(**item).id, qty)
                })
                .collect();
            format!(
                " {:<28} {:>5}/{:<5} {:>5}/{:<5}      {}{}",
                station.name,
                storage.total(),
                storage.cap,
                grid.supply_mw,
                grid.demand_mw,
                if life.ok { "ok" } else { "SHORT" },
                lines,
            )
        })
        .collect();
    for row in rows {
        println!("{row}");
    }

    let notices = world.resource::<Notices>();
    println!("\n last notices:");
    for (tick, notice) in notices.recent(12) {
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
    let commander = demo_scenario(app.world_mut(), &snapshot);
    app.insert_resource(galos_factory::ui::LocalCommander(commander));
    app.run();
}
