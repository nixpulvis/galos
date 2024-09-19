//! A 3D Galaxy Map

use bevy::tasks::futures_lite::future;
use bevy::{prelude::*, window::WindowMode};
use bevy_egui::EguiPlugin;
#[cfg(feature = "inspector")]
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use bevy_mod_picking::prelude::*;
use bevy_panorbit_camera::PanOrbitCameraPlugin;
use galos_db::Database;
use std::collections::{HashMap, HashSet};

mod camera;
mod search;
mod systems;
mod ui;

#[derive(Resource)]
struct Db(Database);

fn main() {
    let db = future::block_on(async { Database::new().await.unwrap() });

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Galos - Starmap".into(),
            mode: WindowMode::BorderlessFullscreen,
            // resizable: false,
            ..default()
        }),
        ..default()
    }));
    app.add_plugins(PanOrbitCameraPlugin);
    app.add_plugins(DefaultPickingPlugins);
    app.add_plugins(EguiPlugin);
    app.insert_resource(ClearColor(Color::BLACK));
    app.insert_resource(AmbientLight {
        color: Color::default(),
        brightness: 1000.0,
    });
    app.insert_resource(Db(db));
    app.insert_resource(systems::View::Systems);
    app.insert_resource(systems::ColorBy::Allegiance);
    app.insert_resource(systems::ScalePopulation(false));

    app.insert_resource(systems::Spyglass {
        radius: 50.,
        fetch: true,
        // filter: true,
    });

    app.insert_resource(systems::Fetched(HashSet::new()));
    app.insert_resource(systems::FetchTasks { fetched: HashMap::new() });
    app.add_event::<camera::MoveCamera>();
    app.add_event::<search::Searched>();
    app.add_systems(Startup, camera::spawn_camera);
    app.add_systems(Update, camera::move_camera);
    app.add_systems(Update, camera::keyboard);
    app.add_systems(Update, systems::fetch);
    app.add_systems(Update, systems::spawn.after(camera::move_camera));
    app.add_systems(
        Update,
        systems::scale_systems
            .after(systems::spawn)
            .run_if(resource_equals(systems::View::Systems)),
    );
    app.add_systems(
        Update,
        systems::scale_stars
            .after(systems::spawn)
            .run_if(resource_equals(systems::View::Stars)),
    );
    app.add_systems(Update, ui::settings);
    app.add_systems(Update, ui::search.after(ui::settings));
    app.add_systems(Update, ui::route.after(ui::search));
    app.add_systems(Update, search::system);

    #[cfg(feature = "inspector")]
    app.add_plugins(WorldInspectorPlugin::new());

    app.run();
}
