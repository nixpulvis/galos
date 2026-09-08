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
        .unwrap_or_else(|_| ".galos_index".to_string());
    let source = FsSource::new(&dir);
    let (index, populated, names, reaches, boosts, factions, held) =
        future::block_on(async {
            // What each part is, before a byte of it is read: a publish
            // landing during the read is then held under the older stamp and
            // re-read on the first poll. Stamped afterwards, a part read
            // before the publish would be filed under the stamp of the
            // publish and never asked for again. See
            // [`galos_map::refresh::Held::before_reading`].
            let held = refresh::Held::before_reading(&source).await;
            let index = source
                .index()
                .await
                .unwrap_or_else(|e| panic!("reading the index at {dir}: {e}"));
            let populated = source.populated().await.unwrap_or_default();
            let names = source.names().await.unwrap_or_default();
            let reaches = source.reaches().await.unwrap_or_default();
            let boosts = source.boosts().await.unwrap_or_default();
            let factions = source.factions().await.unwrap_or_default();
            (index, populated, names, reaches, boosts, factions, held)
        });

    // Said before the log plugin is up, so plain stderr. What loaded is the
    // first thing to check when the map draws but nothing is colored or named.
    eprintln!(
        "galos: index {} has {} cells, {} populated, {} names, \
         {} reaches, {} supercharging, {} factions",
        dir,
        index.len(),
        populated.len(),
        names.len(),
        reaches.len(),
        boosts.as_ref().map_or(0, Vec::len),
        factions.len(),
    );
    // A cell tree with no metadata beside it is a stale or half-written build:
    // the map would draw every system uncolored and unnamed rather than say so.
    // Loud here rather than a plausible-but-wrong sky.
    if !index.is_empty() && (populated.is_empty() || names.is_empty()) {
        eprintln!(
            "galos: WARNING — {dir} has cells but no metadata sidecars; \
             systems will be uncolored and unnamed. Rebuild the index with \
             `cargo run -p galos_db --bin galos-db -- index {dir}`."
        );
    }

    // Which systems can supercharge a drive, which the router plots by and
    // the graph below is built against. Absent where the index publishes no
    // such table, which is not a galaxy without jet cones: the form refuses a
    // supercharged route rather than handing back the unaided one under its
    // name. See [`Boosts::published`].
    let boosts = boosts.map_or_else(Boosts::absent, Boosts::of);
    if !index.is_empty() && !boosts.published() {
        eprintln!(
            "galos: NOTE — {dir} publishes no supercharge table, so routes \
             for a supercharging drive cannot be plotted. Add it with \
             `cargo run -p galos_db --bin galos-db -- index {dir} \
             --only boosts`."
        );
    }

    // The jump graph the router walks, bucketed once from the resident names.
    let jumps = JumpGraph::new(&names, &boosts);

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
    app.insert_resource(IndexDir(dir.clone()));
    app.insert_resource(Transport(Arc::new(source)));
    app.insert_resource(held);
    app.insert_resource(ResidentIndex(index));
    app.insert_resource(Populated(Arc::new(
        populated.into_iter().map(|s| (s.address, s)).collect(),
    )));
    app.insert_resource(Jumps(Arc::new(jumps)));
    app.insert_resource(boosts);
    app.insert_resource(Names::reaching(names, reaches));
    app.insert_resource(Factions(
        factions.into_iter().map(|f| (f.id, f.name)).collect(),
    ));

    app.add_plugins(schedule::plugin);
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
