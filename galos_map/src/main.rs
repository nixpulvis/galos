//! A 3D Galaxy Map

use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy_egui::{EguiGlobalSettings, EguiPlugin};
#[cfg(feature = "inspector")]
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use galos_index::{FsSource, Source as _};
use galos_map::systems::route::graph::{JumpGraph, Jumps};
use galos_map::*;
use std::sync::Arc;

fn main() {
    // The built index directory the map draws from: the cell tree and the
    // metadata sidecars beside it. Read once at startup, since the aggregates
    // and the resident tables are a few megabytes and every walk reads them.
    let dir = std::env::var("GALOS_INDEX_DIR")
        .unwrap_or_else(|_| "galos_index".to_string());
    let source = FsSource::new(&dir);
    let (index, populated, names, factions) = future::block_on(async {
        let index = source
            .index()
            .await
            .unwrap_or_else(|e| panic!("reading the index at {dir}: {e}"));
        let populated = source.populated().await.unwrap_or_default();
        let names = source.names().await.unwrap_or_default();
        let factions = source.factions().await.unwrap_or_default();
        (index, populated, names, factions)
    });

    // Said before the log plugin is up, so plain stderr. What loaded is the
    // first thing to check when the map draws but nothing is coloured or named.
    eprintln!(
        "galos: index {} has {} cells, {} populated, {} names, {} factions",
        dir,
        index.len(),
        populated.len(),
        names.len(),
        factions.len(),
    );
    // A cell tree with no metadata beside it is a stale or half-written build:
    // the map would draw every system uncoloured and unnamed rather than say so.
    // Loud here rather than a plausible-but-wrong sky.
    if !index.is_empty() && (populated.is_empty() || names.is_empty()) {
        eprintln!(
            "galos: WARNING — {dir} has cells but no metadata sidecars; \
             systems will be uncoloured and unnamed. Rebuild the index with \
             `cargo run -p galos_db --bin galos-db -- index {dir}`."
        );
    }

    // The jump graph the router walks, bucketed once from the resident names.
    let jumps = JumpGraph::new(&names);

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

    // The primary egui context is hosted on a camera of the map's own (see
    // `camera::ui_camera`) rather than the first one bevy_egui finds, so its
    // pass stacks at the top of the camera order and draws over the annotation
    // overlays instead of under them. Off with the automatic one first.
    app.world_mut()
        .resource_mut::<EguiGlobalSettings>()
        .auto_create_primary_context = false;

    app.insert_resource(ClearColor(Color::BLACK));
    app.insert_resource(IndexDir(dir.clone()));
    app.insert_resource(Transport(Arc::new(source)));
    app.insert_resource(ResidentIndex(index));
    app.insert_resource(Populated(Arc::new(
        populated.into_iter().map(|s| (s.address, s)).collect(),
    )));
    app.insert_resource(Jumps(Arc::new(jumps)));
    app.insert_resource(Names::new(names));
    app.insert_resource(Factions(
        factions.into_iter().map(|f| (f.id, f.name)).collect(),
    ));

    app.add_plugins(schedule::plugin);
    app.add_plugins(space::plugin);
    app.add_plugins(camera::plugin);
    app.add_plugins(systems::plugin);
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
