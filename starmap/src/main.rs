//! Shows how to iterate over combinations of query results.

use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use galos_db::Database;
use galos_db::systems::System;
use elite_journal::Allegiance;
use async_std::task;
use camera::PanOrbitState;

mod camera;
mod ui;
mod generate;

#[derive(Resource, Default)]
struct SystemsSearch {
    name: String,
}

#[derive(Component)]
struct SystemMarker;

#[derive(Resource)]
struct FocusSystem(System);

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(EguiPlugin)
        .insert_resource(ClearColor(Color::BLACK))
        .insert_resource(AmbientLight {
            color: Color::default(),
            brightness: 1000.0,
        })
        .init_resource::<SystemsSearch>()
        .add_systems(Startup, generate::bodies)
        .add_systems(Startup, camera::spawn_camera)
        .add_systems(Update, ui::systems_search)
        .add_systems(Update, camera::pan_orbit_camera
            .run_if(any_with_component::<PanOrbitState>))
        .run();
}
