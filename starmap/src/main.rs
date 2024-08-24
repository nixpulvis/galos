//! Shows how to iterate over combinations of query results.

use bevy::prelude::*;
use bevy_mod_picking::DefaultPickingPlugins;
use bevy_egui::EguiPlugin;
use camera::PanOrbitState;

mod camera;
mod ui;
mod generate;

#[derive(Event, Debug)]
struct SystemsSearched {
    name: String,
    radius: String,
}

#[derive(Event, Debug)]
struct MoveCamera {
    position: Vec3,
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

        .add_event::<SystemsSearched>()
        .add_event::<MoveCamera>()

        .add_systems(Startup, camera::spawn_camera)
        .add_systems(Update, generate::star_systems)
        .add_systems(Update, camera::move_camera)

        .add_systems(Update, ui::systems_search)
        .add_systems(Update, camera::pan_orbit_camera
            .run_if(any_with_component::<PanOrbitState>))
        .run();
}
