//! A 3D Galaxy Map

use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy_egui::EguiPlugin;
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

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Galos - Starmap".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(PanOrbitCameraPlugin)
        .add_plugins(DefaultPickingPlugins)
        .add_plugins(EguiPlugin)
        .insert_resource(ClearColor(Color::BLACK))
        .insert_resource(AmbientLight {
            color: Color::default(),
            brightness: 1000.0,
        })
        .insert_resource(Db(db))
        .insert_resource(systems::ScalePopulation(false))
        .insert_resource(systems::SpyglassRadius(50.))
        .insert_resource(systems::AlwaysFetch(true))
        .insert_resource(systems::AlwaysDespawn(true))
        .insert_resource(systems::Fetched(HashSet::new()))
        .insert_resource(systems::FetchTasks {
            fetched: HashMap::new(),
        })
        .add_event::<camera::MoveCamera>()
        .add_event::<search::Searched>()
        .add_systems(Startup, camera::spawn_camera)
        .add_systems(Update, camera::move_camera)
        .add_systems(Update, camera::keyboard)
        .add_systems(Update, systems::fetch)
        .add_systems(Update, systems::spawn.after(camera::move_camera))
        .add_systems(Update, systems::scale_with_camera.after(systems::spawn))
        .add_systems(Update, ui::settings)
        .add_systems(Update, ui::search.after(ui::settings))
        .add_systems(Update, ui::route.after(ui::search))
        .add_systems(Update, search::system)
        .run();
}
