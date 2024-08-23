//! Shows how to iterate over combinations of query results.

use bevy::prelude::*;
use bevy_mod_picking::DefaultPickingPlugins;
use bevy_egui::EguiPlugin;
use camera::PanOrbitState;

mod camera;
mod ui;
mod generate;

#[derive(Resource, Default)]
struct SystemsSearch {
    name: String,
    radius: String,
}

#[derive(Component)]
struct SystemMarker;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(DefaultPickingPlugins)
        .add_plugins(EguiPlugin)
        .insert_resource(ClearColor(Color::BLACK))
        .insert_resource(AmbientLight {
            color: Color::default(),
            brightness: 1000.0,
        })
        .init_resource::<SystemsSearch>()
        .add_systems(Startup, camera::spawn_camera.before(
                generate::bodies))
        .add_systems(Update, ui::systems_search)
        .add_systems(Update, camera::pan_orbit_camera
            .run_if(any_with_component::<PanOrbitState>))
        .run();
}
