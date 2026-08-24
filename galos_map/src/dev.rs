//! A developer diagnostics window.
//!
//! What the index loaded, what the spyglass reaches, and what the systems are
//! doing, plus the frame rate — the numbers to look at when the map feels slow
//! or draws the wrong thing. Opened from the top-right button or F3, off to
//! begin with.
//!
//! Read-only: it draws resources the rest of the map keeps and never writes
//! one, so it can be left out of a release build by dropping the plugin and
//! nothing else changes.

use crate::camera::OrbitCamera;
use crate::systems::fetch::FetchTasks;
use crate::systems::spawn::PendingSpawns;
use crate::systems::{Evictions, InReach, PendingEvictions, Spyglass, System};
use crate::{Factions, IndexDir, Names, Populated, ResidentIndex};
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

pub fn plugin(app: &mut App) {
    app.add_plugins(FrameTimeDiagnosticsPlugin::default());
    app.init_resource::<ShowDiagnostics>();
    app.add_systems(Update, toggle);
    // After the lettering, so the panel is drawn in the map's own face rather
    // than egui's default.
    app.add_systems(
        EguiPrimaryContextPass,
        diagnostics.after(crate::ui::lettering),
    );
}

/// Whether the diagnostics window is drawn. Off to begin with; the button in
/// the top-right corner and F3 both open it.
#[derive(Resource)]
pub struct ShowDiagnostics(pub bool);

impl Default for ShowDiagnostics {
    fn default() -> Self {
        ShowDiagnostics(false)
    }
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, mut show: ResMut<ShowDiagnostics>) {
    if keys.just_pressed(KeyCode::F3) {
        show.0 = !show.0;
    }
}

/// How far the toggle button and window sit from the edges, in points.
const MARGIN: f32 = 8.;

/// The diagnostics window's starting width, in points. Narrow: two short
/// columns of counts and nothing that needs the room.
const WIDTH: f32 = 200.;

/// Draw the diagnostics window from what the map holds
///
/// Every count is read live rather than tallied here: the spawned count is the
/// systems on the map this frame, the reach counts are [`InReach`]'s (which
/// [`crate::systems::visibility`] already settled), and the eviction and fetch
/// numbers come straight off their resources.
#[allow(clippy::too_many_arguments)]
fn diagnostics(
    mut contexts: EguiContexts,
    mut show: ResMut<ShowDiagnostics>,
    dir: Res<IndexDir>,
    index: Res<ResidentIndex>,
    populated: Res<Populated>,
    names: Res<Names>,
    factions: Res<Factions>,
    tasks: Res<FetchTasks>,
    spyglass: Res<Spyglass>,
    in_reach: Res<InReach>,
    evictions: Res<Evictions>,
    queued_spawns: Res<PendingSpawns>,
    queued_evictions: Res<PendingEvictions>,
    store: Res<DiagnosticsStore>,
    systems: Query<(), With<System>>,
    camera: Query<&OrbitCamera>,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    // A button, always drawn, to open the panel; F3 and the window's own close
    // do the same. Top-right, clear of the search bar and the settings gear on
    // the left, and on the chrome's layer under the windows like the rest of
    // it.
    let toggle = egui::Area::new(egui::Id::new("diagnostics-toggle"))
        .order(egui::Order::Background)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-MARGIN, MARGIN))
        .show(ctx, |ui| ui.button("diagnostics"));
    if toggle.inner.clicked() {
        show.0 = !show.0;
    }
    if !show.0 {
        return Ok(());
    }

    let spawned = systems.iter().count();
    let settled = camera.single().map(OrbitCamera::is_settled).unwrap_or(true);
    let fps = store
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|fps| fps.smoothed());

    // Pinned under the toggle against the right edge, a standard margin off
    // both, so it clears the same distance the button does. Anchored rather
    // than positioned: egui puts the true right edge at the margin whatever
    // the window's width turns out to be.
    let top = toggle.response.rect.bottom() + MARGIN;
    egui::Window::new("diagnostics")
        .default_width(WIDTH)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-MARGIN, top))
        .resizable(false)
        .open(&mut show.0)
        .show(ctx, |ui| {
            row(
                ui,
                "index",
                "What the client read from the build directory at startup and \
                 holds for the whole session.",
                |ui| {
                    pair(
                        ui,
                        "dir",
                        &dir.0,
                        "Where the index and its metadata sidecars were read \
                         from.",
                    );
                    pair(
                        ui,
                        "cells",
                        &index.0.len().to_string(),
                        "Cells in the resident aggregate tree — the spatial \
                         index every walk reads without a fetch.",
                    );
                    pair(
                        ui,
                        "populated",
                        &populated.0.len().to_string(),
                        "Systems in the political table — population, \
                         allegiance, government — held resident for colour and \
                         filtering. Most of the galaxy is absent from it.",
                    );
                    pair(
                        ui,
                        "names",
                        &names.entries.len().to_string(),
                        "Systems in the names-and-positions table: the search \
                         index and the router's graph.",
                    );
                    pair(
                        ui,
                        "factions",
                        &factions.0.len().to_string(),
                        "Faction id-to-name entries.",
                    );
                },
            );
            ui.separator();
            row(
                ui,
                "spyglass",
                "The reach around what the camera looks at, and the fetching \
                 that fills it.",
                |ui| {
                    pair(
                        ui,
                        "camera",
                        if settled { "settled" } else { "easing" },
                        "Whether the view has come to rest, or is still easing \
                         toward its target. While easing the reach moves and \
                         the evictor works.",
                    );
                    pair(
                        ui,
                        "radius",
                        &format!("{:.0} ly", spyglass.radius),
                        "How far the spyglass reaches from what the camera \
                         looks at. Everything inside is fetched and drawn.",
                    );
                    let keep =
                        spyglass.radius as f64 * crate::systems::EVICT_MARGIN;
                    pair(
                        ui,
                        "keep",
                        &format!("{keep:.0} ly"),
                        "How far a system is kept before it is dropped: the \
                         radius times the eviction margin. Wider than the \
                         reach so the edge does not churn.",
                    );
                    pair(
                        ui,
                        "fetch",
                        on_off(spyglass.fetch),
                        "Whether the spyglass is asking the index for the \
                         systems in its reach.",
                    );
                    pair(
                        ui,
                        "clear",
                        on_off(spyglass.clear),
                        "Whether systems out of reach are dropped. Off, \
                         everything ever loaded stays on the map.",
                    );
                    pair(
                        ui,
                        "fetch tasks",
                        &tasks.fetched.len().to_string(),
                        "Region reads in flight, not yet landed.",
                    );
                    pair(
                        ui,
                        "surveys",
                        &tasks.surveyed.len().to_string(),
                        "Regions the map remembers holding, so it does not ask \
                         again. Clamped to what the evictor still holds.",
                    );
                },
            );
            ui.separator();
            row(
                ui,
                "systems",
                "The system entities on the map, and the queues they arrive \
                 and leave through.",
                |ui| {
                    pair(
                        ui,
                        "spawned",
                        &spawned.to_string(),
                        "System entities on the map this frame, drawn or not.",
                    );
                    pair(
                        ui,
                        "in reach",
                        &in_reach.total.to_string(),
                        "How many of them the spyglass reaches.",
                    );
                    pair(
                        ui,
                        "admitted",
                        &in_reach.admitted.to_string(),
                        "How many of those in reach the filters admit, and so \
                         draw at full.",
                    );
                    pair(
                        ui,
                        "spawn queue",
                        &queued_spawns.queued().to_string(),
                        "Fetched systems waiting to become entities, drained a \
                         budget a frame. The green dot on the bar's count.",
                    );
                    pair(
                        ui,
                        "evict queue",
                        &queued_evictions.queued().to_string(),
                        "Systems marked to drop, despawned a budget a frame. \
                         The red dot on the bar's count.",
                    );
                    pair(
                        ui,
                        "dropped (last)",
                        &evictions.last.to_string(),
                        "Systems the evictor despawned last frame.",
                    );
                    pair(
                        ui,
                        "dropped (total)",
                        &evictions.total.to_string(),
                        "Systems dropped since the map opened.",
                    );
                },
            );
            ui.separator();
            let frame = match fps {
                Some(fps) => {
                    ui.label(format!("{fps:.0} fps ({:.1} ms)", 1000.0 / fps))
                }
                None => ui.label("fps —"),
            };
            frame.on_hover_text(
                "Frames per second, smoothed, with the time a frame took.",
            );
        });

    Ok(())
}

/// A titled block of pairs, the title carrying its own tooltip.
fn row(
    ui: &mut egui::Ui,
    title: &str,
    help: &str,
    rows: impl FnOnce(&mut egui::Ui),
) {
    ui.strong(title).on_hover_text(help);
    egui::Grid::new(title).num_columns(2).show(ui, rows);
}

/// One `name: value` line inside a [`row`], both cells hovering the same help.
fn pair(ui: &mut egui::Ui, name: &str, value: &str, help: &str) {
    ui.label(name).on_hover_text(help);
    ui.label(value).on_hover_text(help);
    ui.end_row();
}

/// A flag as the word for the state it is in.
fn on_off(flag: bool) -> &'static str {
    if flag { "on" } else { "off" }
}
