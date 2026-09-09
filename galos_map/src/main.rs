//! A 3D Galaxy Map

use bevy::prelude::*;
use bevy_egui::{EguiGlobalSettings, EguiPlugin};
#[cfg(feature = "inspector")]
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use galos_index::FsSource;
use galos_map::*;
use std::sync::Arc;

fn main() {
    // The built index directory the map draws from: the cell tree and the
    // metadata sidecars beside it. Named here and read by `loading`, which
    // stands the window up first and reads it behind a loading screen: it runs
    // to a hundred and thirty megabytes, and a window that waits on it is a
    // launch that looks hung.
    let dir = std::env::var("GALOS_INDEX_DIR")
        .unwrap_or_else(|_| ".galos_index".to_string());
    let source = FsSource::new(&dir);

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

    // egui's primary context is attached by hand, to the annotations camera
    // (`camera::annotations`), which draws last of the map's three. egui
    // renders in the graph of whatever camera holds its context, so from there
    // the chrome lands over the star field and the annotations rather than
    // under them; left to itself bevy_egui takes the first camera it finds,
    // which is the scene's, and drew the chrome under the field. Off with the
    // automatic one first, so nothing stands a second context up.
    app.world_mut()
        .resource_mut::<EguiGlobalSettings>()
        .auto_create_primary_context = false;

    app.insert_resource(ClearColor(Color::BLACK));
    // The two the read itself needs: where to read from, and what to read
    // through. Everything the read comes back with is handed over by
    // `loading` when it lands.
    app.insert_resource(IndexDir(dir));
    app.insert_resource(Transport(Arc::new(source)));

    app.add_plugins(schedule::plugin);
    // Before the plugins it gates, so the state exists by the time their run
    // conditions are built against it.
    app.add_plugins(loading::plugin);
    app.add_plugins(space::plugin);
    app.add_plugins(camera::plugin);
    app.add_plugins(systems::plugin);
    // After the systems, whose bounded source holds the payloads a refresh
    // replaces and the stamps it asks about.
    app.add_plugins(refresh::plugin);
    // After the systems, whose descent into a star is what carries the ruled
    // plane from light years to light seconds.
    app.add_plugins(grid::plugin);
    app.add_plugins(ui::plugin);
    app.add_plugins(search::plugin);
    app.add_plugins(keys::plugin);
    // After `ui`, whose `lettering` the diagnostics panel is drawn in.
    app.add_plugins(dev::plugin);

    #[cfg(feature = "inspector")]
    app.add_plugins(WorldInspectorPlugin::new());

    app.run();
}
