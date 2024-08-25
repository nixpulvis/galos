//! Shows how to iterate over combinations of query results.

use bevy::prelude::*;
use bevy_panorbit_camera::PanOrbitCameraPlugin;
use bevy_infinite_grid::InfiniteGridPlugin;
use bevy_mod_picking::DefaultPickingPlugins;
use bevy_egui::EguiPlugin;

mod grid;
mod camera;
mod systems;
mod ui;

#[derive(Event, Debug)]
enum Searched {
    System {
        name: String,
        radius: String,
    },
    Faction {
        name: String,
    },
    Route {
        start: String,
        end: String,
        range: String,
    },
}

#[derive(Event, Debug)]
struct MoveCamera {
    position: Vec3,
}

#[derive(Component)]
struct SystemMarker;

#[derive(Component)]
struct RouteMarker;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Galos - Starmap".into(),
                    ..default()
            }),
            ..default()
        }))
        .add_plugins(PanOrbitCameraPlugin)
        .add_plugins(InfiniteGridPlugin)
        .add_plugins(DefaultPickingPlugins)
        .add_plugins(EguiPlugin)
        .insert_resource(ClearColor(Color::BLACK))
        .insert_resource(AmbientLight {
            color: Color::default(),
            brightness: 1000.0,
        })

        .add_event::<Searched>()
        .add_event::<MoveCamera>()

        .add_systems(Startup, grid::spawn)
        .add_systems(Startup, camera::spawn_camera)
        .add_systems(Update, camera::pan_orbit_camera
            .run_if(any_with_component::<PanOrbitState>))
        .add_systems(Update, camera::move_camera)

        .add_systems(Update, systems::fetch)
        .add_systems(Update, systems::spawn)

        .add_systems(Update, ui::systems_search)
        .add_systems(Update, ui::faction_search)
        .add_systems(Update, ui::route_search)
        .run();
}
