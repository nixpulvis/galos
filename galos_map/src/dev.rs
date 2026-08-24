//! A developer diagnostics window.
//!
//! What the index loaded, what is spawned on the map, and what the evictor is
//! doing, plus the frame rate — the numbers to look at when the map feels slow
//! or draws the wrong thing. Opened from the top-right button or F3, off to
//! begin with.
//!
//! Read-only: it draws resources the rest of the map keeps and never writes
//! one, so it can be left out of a release build by dropping the plugin and
//! nothing else changes.

use crate::systems::fetch::FetchTasks;
use crate::systems::{Evictions, InReach, Spyglass, System};
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
    store: Res<DiagnosticsStore>,
    systems: Query<(), With<System>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    // A button, always drawn, to open the panel; F3 and the window's own close
    // do the same. Top-right, clear of the search bar and the settings gear on
    // the left.
    let toggle = egui::Area::new(egui::Id::new("diagnostics-toggle"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-MARGIN, MARGIN))
        .show(ctx, |ui| ui.button("diagnostics"));
    if toggle.inner.clicked() {
        show.0 = !show.0;
    }
    if !show.0 {
        return Ok(());
    }

    let spawned = systems.iter().count();
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
            row(ui, "index", |ui| {
                pair(ui, "dir", &dir.0);
                pair(ui, "cells", &index.0.len().to_string());
                pair(ui, "populated", &populated.0.len().to_string());
                pair(ui, "names", &names.entries.len().to_string());
                pair(ui, "factions", &factions.0.len().to_string());
            });
            ui.separator();
            row(ui, "systems", |ui| {
                pair(ui, "spawned", &spawned.to_string());
                pair(ui, "in reach", &in_reach.total.to_string());
                pair(ui, "admitted", &in_reach.admitted.to_string());
                pair(ui, "fetch tasks", &tasks.fetched.len().to_string());
                pair(ui, "surveys", &tasks.surveyed.len().to_string());
            });
            ui.separator();
            row(ui, "eviction", |ui| {
                pair(ui, "spyglass", &format!("{:.0} ly", spyglass.radius));
                pair(
                    ui,
                    "keep within",
                    &format!("{:.0} ly", spyglass.radius * 1.5),
                );
                pair(ui, "dropped (last)", &evictions.last.to_string());
                pair(ui, "dropped (total)", &evictions.total.to_string());
            });
            ui.separator();
            match fps {
                Some(fps) => {
                    ui.label(format!("{fps:.0} fps ({:.1} ms)", 1000.0 / fps))
                }
                None => ui.label("fps —"),
            };
        });

    Ok(())
}

/// A titled block of pairs.
fn row(ui: &mut egui::Ui, title: &str, rows: impl FnOnce(&mut egui::Ui)) {
    ui.strong(title);
    egui::Grid::new(title).num_columns(2).show(ui, rows);
}

/// One `name: value` line inside a [`row`].
fn pair(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.label(name);
    ui.label(value);
    ui.end_row();
}
