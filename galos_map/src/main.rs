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
    // `big_space` computes every `GlobalTransform` relative to the floating
    // origin, which is a different answer than bevy's own propagation gives.
    // Running both would leave whichever wrote last to decide, so bevy's is
    // turned off. See `space` for what replaces it.
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Galos - Starmap".into(),
                    ..default()
                }),
                ..default()
            })
            .build()
            .disable::<TransformPlugin>(),
    );
    app.add_plugins(EguiPlugin {
        // Bevy cannot use bindless textures on Metal, and bevy_egui warns at
        // startup whenever they're requested. This UI is a couple of small
        // windows, so batching texture binds gains nothing anywhere.
        bindless_mode_array_size: None,
        ..default()
    });

    app.insert_resource(ClearColor(Color::BLACK));
    app.insert_resource(Db(db));

    app.add_plugins(schedule::plugin);
    app.add_plugins(space::plugin);
    app.add_plugins(camera::plugin);
    app.add_plugins(systems::plugin);
    // After the systems, whose descent into a star is what carries the ruled
    // plane from light years to light seconds.
    app.add_plugins(grid::plugin);
    app.add_plugins(ui::plugin);
    app.add_plugins(search::plugin);

    #[cfg(feature = "inspector")]
    app.add_plugins(WorldInspectorPlugin::new());

    app.run();
}
