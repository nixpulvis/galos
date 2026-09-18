//! A 3D Galaxy Map

use bevy::prelude::*;
use bevy_egui::{EguiGlobalSettings, EguiPlugin};
#[cfg(feature = "inspector")]
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use clap::Parser;
use galos_index::FsSource;
use galos_map::*;
use std::sync::Arc;

/// Default index directory, matching `galos-sync --index` with no DIR.
const INDEX_DIR: &str = ".galos_index";

/// Draw Elite's galaxy from a built index directory.
#[derive(Parser)]
#[command(name = "galos-map", version, about)]
struct Cli {
    /// Index directory to draw, as built by `galos-sync --index DIR`.
    ///
    /// Falls back to GALOS_INDEX, then to `.galos_index`.
    #[arg(
        short = 'i',
        long,
        value_name = "DIR",
        env = "GALOS_INDEX",
        default_value = INDEX_DIR
    )]
    index: String,

    /// Draw one frame to this path as a PNG, then exit.
    ///
    /// What a render change is checked against: the window is the only place
    /// the map exists, and nothing outside the process can read it.
    #[arg(long, value_name = "PATH")]
    screenshot: Option<String>,

    /// How far to stand the camera off before a screenshot, in light years.
    #[arg(long, value_name = "LY")]
    screenshot_radius: Option<f32>,

    /// Frames to let the map settle before a screenshot is taken.
    #[arg(long, value_name = "N", default_value_t = 240)]
    screenshot_after: u32,
}

fn main() {
    // The built index directory the map draws from: the cell tree and the
    // metadata sidecars beside it. Named here and read by `loading`, which
    // stands the window up first and reads it behind a loading screen: it runs
    // to a hundred and thirty megabytes, and a window that waits on it is a
    // launch that looks hung.
    let cli = Cli::parse();
    let dir = cli.index;
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
    // Said as soon as the layer is up, which is where `DefaultPlugins` built
    // `LogPlugin`. Tracy is the other way round from what a reader expects:
    // the profiled program listens and the profiler dials in, so this is the
    // address to point `tracy` or `tracy-capture -a` at.
    #[cfg(feature = "tracy")]
    tracy_listening();
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
    if let Some(path) = cli.screenshot {
        app.insert_resource(dev::Shot {
            path,
            radius: cli.screenshot_radius,
            after: cli.screenshot_after,
        });
    }

    #[cfg(feature = "inspector")]
    app.add_plugins(WorldInspectorPlugin::new());

    app.run();
}

/// Say where the Tracy client is listening, once the layer is up
///
/// The client binds a socket and waits; a profiler connects to it, and until
/// one does everything it is told is held in memory (which is what bevy warns
/// about on the line above this one). The port is the Tracy default unless
/// `TRACY_PORT` says otherwise, which the client reads itself — so this reads
/// the same variable rather than being told, and a run started with one says
/// the number it is actually on.
#[cfg(feature = "tracy")]
fn tracy_listening() {
    /// What Tracy binds with no `TRACY_PORT` in the environment.
    const TRACY_PORT: u16 = 8086;

    let port = std::env::var("TRACY_PORT")
        .ok()
        .and_then(|it| it.parse::<u16>().ok())
        .unwrap_or(TRACY_PORT);
    info!(
        "Tracy is listening on 0.0.0.0:{port}: `tracy` or \
         `tracy-capture -a 127.0.0.1 -p {port} -o galos.tracy` to connect"
    );
}
