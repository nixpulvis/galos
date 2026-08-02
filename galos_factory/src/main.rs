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

/// The demo scenario: a commander with a mining outpost feeding a refinery
/// line, selling computer components at the system's trade hub — the full
/// core loop. Sites are chosen from whatever the snapshot provides, so any
/// hand-authored system can be run, not just the Sol fixture.
fn demo_scenario(world: &mut World, snapshot: &SystemSnapshot) -> Entity {
    let seeded = seed::apply(world, snapshot);

    let mine_site = snapshot
        .bodies
        .iter()
        .find(|body| body.landable && body.planet_class.contains("etal"))
        .map(|body| seeded.bodies[&body.name])
        .expect("the system needs a landable metal body to mine");
    // The hub is the richest market, preferring one with a shipyard.
    let hub_name = snapshot
        .stations
        .iter()
        .max_by_key(|station| (station.shipyard, station.listings.len()))
        .map(|station| station.name.clone())
        .expect("the system needs at least one station to trade with");
    let hub = seeded.stations[&hub_name];

    let data = world.resource::<StaticData>().clone();
    let recipe = |name: &str| data.recipe_by_name(name).expect("known recipe");
    let item = |name: &str| data.item_by_name(name).expect("known item");

    // A commander who has been hauling contracts for a while. Under-
    // capitalization is the classic early-game death spiral (credits hit
    // zero → ships can't fuel → imports stall → life support fails), so the
    // demo starts with a healthy float. They fly for whoever runs the hub.
    let mut commander = world.spawn(CommanderBundle::new("Demo", 800_000));
    if let Some(faction) = seeded.factions.values().next() {
        commander.insert(MemberOf(*faction));
    }
    let commander = commander.id();
    let send = |world: &mut World, action: Action| {
        world.send_event(PlayerCommand::new(commander, action));
    };

    send(
        world,
        Action::BuyOutpost {
            body: mine_site,
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
                market: hub,
                to: outpost,
                item: item(what),
                qty,
            },
        );
    }
    // Build the chain: power, mining, smelting, purifying, assembling —
    // ending in computer components, the product the hub market is
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
    if factories.is_empty() {
        eprintln!(
            "warning: nothing could be built — the hub market did not stock \
             the construction materials this scenario needs",
        );
    }

    // (recipe, output cap): caps throttle everything without a fast
    // consumer or export, so the shared pool never silts up. Plans zip
    // against what actually got built, so a thin fixture degrades rather
    // than panicking.
    let plan = [
        (BuildingKind::PowerPlant, &[("burn_hydrogen", None)][..]),
        (
            BuildingKind::Extractor,
            &[
                ("mine_bauxite", Some(200)),
                ("mine_gallite", Some(150)),
                ("mine_gallite", Some(150)),
                ("mine_copper", Some(200)),
            ][..],
        ),
        (
            BuildingKind::Refinery,
            &[
                ("smelt_aluminium", Some(150)),
                ("smelt_gallium", Some(80)),
                ("smelt_gallium", Some(80)),
                ("purify_copper", Some(60)),
            ][..],
        ),
        (
            BuildingKind::Assembler,
            &[
                ("make_semiconductors", Some(80)),
                ("make_computercomponents", None),
            ][..],
        ),
    ];
    let mut assignments: Vec<(Entity, &str, Option<u32>)> = Vec::new();
    for (kind, recipes) in plan {
        let built = of_kind(kind);
        if kind == BuildingKind::PowerPlant {
            // Every plant burns fuel; the others take one recipe each.
            for plant in built {
                assignments.push((plant, recipes[0].0, recipes[0].1));
            }
            continue;
        }
        for (factory, (name, cap)) in built.into_iter().zip(recipes) {
            assignments.push((factory, name, *cap));
        }
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
        ("computercomponents", outpost, hub, None, 0u32),
        ("aluminium", outpost, hub, None, 60),
        ("copper", outpost, hub, None, 60),
        ("hydrogenfuel", hub, outpost, Some(100), 0),
        ("polymers", hub, outpost, Some(60), 0),
        ("water", hub, outpost, Some(60), 0),
        ("foodcartridges", hub, outpost, Some(60), 0),
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
        send(world, Action::BuyShip { at: hub, class: ShipClass::Hauler });
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
    for (ship, contract) in ships.into_iter().zip(contracts) {
        send(world, Action::AssignShip { ship, contract: Some(contract) });
    }
    commander
}

fn run_headless(ticks: u64, fixture: Option<String>) {
    let snapshot = load_fixture(fixture);
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, sim_plugin));
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
            let lines: String = storage
                .iter()
                .map(|(item, qty)| {
                    format!("\n    {:<24} {:>6}", data.item(item).id, qty)
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
    app.add_plugins((sim_plugin, galos_factory::ui::ui_plugin));
    let commander = demo_scenario(app.world_mut(), &snapshot);
    app.insert_resource(galos_factory::ui::LocalCommander(commander));
    app.add_systems(Update, exit_on_escape);
    app.run();
}

/// Escape quits, matching `galos_map`.
#[cfg(feature = "ui")]
fn exit_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut events: ResMut<Events<bevy::app::AppExit>>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        events.send(AppExit::Success);
    }
}
