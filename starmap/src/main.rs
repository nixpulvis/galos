//! A 3D Galaxy Map

use bevy::tasks::futures_lite::future;
use bevy::{prelude::*, window::WindowMode};
use bevy_egui::EguiPlugin;
#[cfg(feature = "inspector")]
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use bevy_mod_billboard::prelude::*;
use bevy_mod_picking::prelude::*;
use bevy_panorbit_camera::PanOrbitCameraPlugin;
use galos_db::Database;
use starmap::*;
use std::collections::{HashMap, HashSet};

fn main() {
    let db = future::block_on(async { Database::new().await.unwrap() });

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Galos - Starmap".into(),
            mode: WindowMode::BorderlessFullscreen,
            ..default()
        }),
        ..default()
    }));
    app.add_plugins(PanOrbitCameraPlugin);
    app.add_plugins(DefaultPickingPlugins);
    app.add_plugins(BillboardPlugin);
    app.add_plugins(EguiPlugin);

    app.insert_resource(ClearColor(Color::BLACK));
    app.insert_resource(AmbientLight {
        color: Color::default(),
        brightness: 1000.0,
    });
    app.insert_resource(Db(db));

    app.insert_resource(systems::scale::View::Systems);
    app.insert_resource(systems::spawn::ColorBy::Allegiance);
    app.insert_resource(systems::scale::ScalePopulation(false));
    app.insert_resource(systems::spawn::ShowNames(false));
    app.insert_resource(systems::Spyglass {
        radius: 50.,
        fetch: true,
        filter: true,
    });

    app.insert_resource(systems::fetch::Fetched(HashSet::new()));
    app.insert_resource(systems::fetch::FetchTasks { fetched: HashMap::new() });

    app.add_event::<camera::MoveCamera>();
    app.add_systems(Startup, camera::spawn_camera);
    app.add_systems(Update, camera::move_camera);

    app.add_event::<systems::spawn::Despawn>();
    app.add_systems(Update, systems::fetch::fetch); // TODO: rename
    app.add_systems(Update, systems::spawn::spawn.after(camera::move_camera)); // TODO: rename
    app.add_systems(Update, systems::visibility.after(systems::spawn::spawn));
    app.add_systems(
        Update,
        systems::labels::respawn.after(systems::spawn::spawn),
    );
    app.add_systems(
        Update,
        systems::labels::scale.after(systems::labels::respawn),
    );
    app.add_systems(
        Update,
        systems::labels::visibility
            .after(systems::labels::respawn)
            .before(systems::labels::scale),
    );
    app.add_systems(Update, systems::spawn::despawn);
    app.add_systems(
        Update,
        systems::scale::scale_systems
            .after(systems::spawn::spawn)
            .run_if(resource_equals(systems::scale::View::Systems)),
    );
    app.add_systems(
        Update,
        systems::scale::scale_stars
            .after(systems::spawn::spawn)
            .run_if(resource_equals(systems::scale::View::Stars)),
    );

    app.add_event::<search::Searched>();
    app.add_systems(Update, search::searched);

    app.add_systems(Update, ui::panel);

    #[cfg(feature = "inspector")]
    app.add_plugins(WorldInspectorPlugin::new());

    app.add_systems(Update, exit);
    app.run();
}

fn exit(
    keys: Res<ButtonInput<KeyCode>>,
    mut events: ResMut<Events<bevy::app::AppExit>>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        events.send(AppExit::Success);
    }
}
