//! A 3D Galaxy Map

use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy_egui::EguiPlugin;
#[cfg(feature = "inspector")]
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use galos_db::Database;
use galos_map::*;

fn main() {
    let db = future::block_on(async { Database::new().await.unwrap() });

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Galos - Starmap".into(),
            ..default()
        }),
        ..default()
    }));
    app.add_plugins(EguiPlugin);

    app.insert_resource(ClearColor(Color::BLACK));
    app.insert_resource(AmbientLight {
        color: Color::default(),
        brightness: 1e3,
    });
    app.insert_resource(Db(db));

    app.add_plugins(camera::plugin);
    app.add_plugins(systems::plugin);
    app.add_plugins(ui::plugin);
    app.add_plugins(search::plugin);

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
