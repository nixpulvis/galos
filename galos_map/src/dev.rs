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
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

/// The tables the client loaded once and holds, bundled so the panel stays
/// under a system's parameter limit.
#[derive(SystemParam)]
struct Loaded<'w> {
    dir: Res<'w, IndexDir>,
    index: Res<'w, ResidentIndex>,
    populated: Res<'w, Populated>,
    names: Res<'w, Names>,
    factions: Res<'w, Factions>,
}

/// What the descent into a system is made of, bundled for the same reason
/// [`Loaded`] is
#[derive(SystemParam)]
struct Descent<'w, 's> {
    contents: Res<'w, crate::systems::bodies::Contents>,
    handover: Res<'w, crate::grid::Handover>,
    systems: Query<'w, 's, (&'static System, Has<big_space::prelude::Grid>)>,
    bodies: Query<'w, 's, (), With<crate::systems::bodies::spawn::Inside>>,
    /// What each of the two ruled planes came to, so an unruled sky can be
    /// told apart from a ruling the handover has put on the other plane.
    planes: Query<
        'w,
        's,
        (&'static crate::grid::Ruler, &'static crate::ruled::Reading),
    >,
}

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
pub(crate) struct ShowDiagnostics(pub(crate) bool);

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
    loaded: Loaded,
    tasks: Res<FetchTasks>,
    spyglass: Res<Spyglass>,
    in_reach: Res<InReach>,
    evictions: Res<Evictions>,
    queued_spawns: Res<PendingSpawns>,
    queued_evictions: Res<PendingEvictions>,
    planned: Res<crate::systems::aggregate::Planned>,
    store: Res<DiagnosticsStore>,
    systems: Query<(), With<System>>,
    camera: Query<&OrbitCamera>,
    descent: Descent,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    // A button, always drawn, to open the panel; F3 and the window's own close
    // do the same. Top-right, clear of the search bar and the settings gear on
    // the left, and on the chrome's layer under the windows like the rest of
    // it; see `crate::ui::settings_pane` for why that layer is `Middle`.
    let toggle = egui::Area::new(egui::Id::new("diagnostics-toggle"))
        .order(egui::Order::Middle)
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
        // Over the chrome, as a panel is.
        .order(egui::Order::Foreground)
        .default_width(WIDTH)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-MARGIN, top))
        .resizable(false)
        .open(&mut show.0)
        .show(ctx, |ui| {
            // Scrolled, and no taller than the room under the toggle it hangs
            // from. Every block here is drawn whatever the viewport is, and a
            // short window ran the readouts off the bottom of it with the
            // frame rate at the end out of reach. No height is imposed:
            // `scrolling` grows to what it is given and stops at what is in
            // it, so this is as tall as the readouts and no taller.
            crate::ui::scrolling(ui, ui.available_height(), "diagnostics", |ui| {
            row(
                ui,
                "index",
                "What the client read from the build directory at startup and \
                 holds for the whole session.",
                |ui| {
                    pair(
                        ui,
                        "dir",
                        &loaded.dir.0,
                        "Where the index and its metadata sidecars were read \
                         from.",
                    );
                    pair(
                        ui,
                        "cells",
                        &loaded.index.0.len().to_string(),
                        "Cells in the resident aggregate tree — the spatial \
                         index every walk reads without a fetch.",
                    );
                    pair(
                        ui,
                        "populated",
                        &loaded.populated.0.len().to_string(),
                        "Systems in the political table — population, \
                         allegiance, government — held resident for colour and \
                         filtering. Most of the galaxy is absent from it.",
                    );
                    pair(
                        ui,
                        "names",
                        &loaded.names.entries.len().to_string(),
                        "Systems in the names-and-positions table: the search \
                         index and the router's graph.",
                    );
                    pair(
                        ui,
                        "reaches",
                        &loaded.names.reaches.len().to_string(),
                        "Systems with a reach on record — how far each one \
                         holds anything scanned, which is what the star field \
                         sizes the mark it paints for the system from. A \
                         system absent from it has nothing scanned in it and \
                         is stood in for.",
                    );
                    pair(
                        ui,
                        "factions",
                        &loaded.factions.0.len().to_string(),
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
                        &reach(spyglass.radius),
                        "How far the spyglass reaches from what the camera \
                         looks at. Everything inside is fetched and drawn. It \
                         follows the camera all the way in, so inside a \
                         system it is a fraction of a light year.",
                    );
                    let keep =
                        spyglass.radius as f64 * crate::systems::EVICT_MARGIN;
                    pair(
                        ui,
                        "keep",
                        &reach(keep as f32),
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
                "descent",
                "What the map holds about the system the camera is standing \
                 in, and how far along the descent into it has got. Every \
                 step here has to happen for a system's bodies and its own \
                 ruled grid to be drawn.",
                |ui| {
                    pair(
                        ui,
                        "held",
                        &descent
                            .contents
                            .of()
                            .map_or("—".to_string(), |it| it.to_string()),
                        "Which system the body poll is holding, by address. \
                         The nearest one to what the camera looks at, within \
                         five light years of it.",
                    );
                    pair(
                        ui,
                        "rows",
                        &match descent.contents.extent() {
                            Some(extent) => format!(
                                "{} stars, {} bodies, {:.2e} m",
                                descent.contents.stars().len(),
                                descent.contents.bodies().len(),
                                extent,
                            ),
                            None => "none".to_string(),
                        },
                        "What came back about it, and how far the rows say it \
                         reaches. Nothing here is nothing to descend into: \
                         the camera is held off at a floor and no sub-grid is \
                         ever drawn.",
                    );
                    // The one the poll would hold, worked out the way
                    // `bodies::fetch::choose` works it out: whichever system
                    // is nearest what the camera looks at. Named rather than
                    // addressed, a `System`'s address being its own module's.
                    let nearest = camera.single().ok().and_then(|orbit| {
                        descent
                            .systems
                            .iter()
                            .map(|(system, grid)| {
                                (
                                    system,
                                    grid,
                                    orbit.center.distance(system.position()),
                                )
                            })
                            .min_by(|(_, _, one), (_, _, other)| {
                                one.total_cmp(other)
                            })
                            .map(|(system, grid, _)| (system, grid, orbit))
                    });
                    pair(
                        ui,
                        "nearest",
                        &nearest.map_or("—".to_string(), |(system, ..)| {
                            system.name().to_string()
                        }),
                        "The system nearest what the camera looks at, which \
                         is the one the poll asks about.",
                    );
                    pair(
                        ui,
                        "seen",
                        &nearest.map_or(
                            "—".to_string(),
                            |(system, _, orbit)| {
                                let away = crate::space::metres(
                                    orbit.eye - system.position(),
                                )
                                .length()
                                    as f32;
                                format!("{:.2e} rad", system.reach() / away)
                            },
                        ),
                        "How much of the sky it takes up from where the eye \
                         stands. Its bodies are drawn past 1.00e-2 of a \
                         radian and kept past 8.00e-3, so anything under \
                         that is a camera not yet near enough to descend.",
                    );
                    pair(
                        ui,
                        "inside",
                        &nearest.map_or("—".to_string(), |(_, grid, _)| {
                            if grid {
                                "descended".to_string()
                            } else {
                                "not descended".to_string()
                            }
                        }),
                        "Whether it is wearing a grid of its own, which it \
                         does only while its contents are drawn. This is what \
                         the sub-grid's ruled plane hangs from.",
                    );
                    pair(
                        ui,
                        "bodies",
                        &descent.bodies.iter().count().to_string(),
                        "How many things are drawn inside it.",
                    );
                    pair(
                        ui,
                        "shell",
                        &nearest.map_or(
                            "—".to_string(),
                            |(system, _, orbit)| {
                                let away = crate::space::metres(
                                    orbit.eye - system.position(),
                                )
                                .length()
                                    as f32;
                                let shell = crate::systems::scale::drawn_shell(
                                    system.reach(),
                                );
                                format!("{:.2} out", away / shell)
                            },
                        ),
                        "Where the eye stands against the system's shell, as a \
                         multiple of that shell's radius. The shell is the rim \
                         of the disc the star field paints for the system, and \
                         the map's one boundary for being inside it: one is \
                         that rim, so anything under one is a camera inside \
                         the system, and that is what the ruler changes hands \
                         across.",
                    );
                    pair(
                        ui,
                        "mark",
                        &nearest.map_or(
                            "—".to_string(),
                            |(system, _, orbit)| {
                                format!(
                                    "{:.2} left",
                                    crate::systems::bodies::spawn::standing_for(
                                        system, orbit.eye,
                                    )
                                )
                            },
                        ),
                        "How much of the mark standing for it is left, which \
                         is what its shell is painted at. One is a whole \
                         mark, nothing is a mark wholly given way to the \
                         system drawn in its place. Only the system the map \
                         is holding ever fades.",
                    );
                    pair(
                        ui,
                        "ruler",
                        &format!("{:.2} out", descent.handover.0),
                        "How far the ruler has changed hands: one is the \
                         galaxy's light-year grid, nothing is the system's \
                         own light-second one, and between them neither is \
                         drawn. Read against `mark` above, which is what it \
                         follows.",
                    );
                    pair(
                        ui,
                        "planes",
                        &{
                            let strength = |inside: bool| {
                                descent
                                    .planes
                                    .iter()
                                    .find(|(ruler, _)| ruler.inside == inside)
                                    .map_or(f32::NAN, |(_, reading)| {
                                        reading.strength
                                    })
                            };
                            format!(
                                "galaxy {:.2}, system {:.2}",
                                strength(false),
                                strength(true)
                            )
                        },
                        "What each plane came to, which is the share above \
                         after the plane's own ladder and its horizon have \
                         had their say. Both at nothing is an unruled sky: \
                         either the handover is passing between them, or \
                         whichever has the share cannot draw at this zoom. A \
                         dash for the system's is no plane at all, the camera \
                         not being in one.",
                    );
                },
            );
            ui.separator();
            row(
                ui,
                "walk",
                "What the index walk asks the view for, off the aggregates \
                 alone: the cells to draw as discrete systems, and the cells \
                 drawn as one aggregate splat which the glow renders.",
                |ui| {
                    pair(
                        ui,
                        "mode",
                        match planned.0.mode {
                            galos_index::Mode::Shell => "shell",
                            galos_index::Mode::Real => "real",
                        },
                        "Shell is the map's balls over a political field on a \
                         point budget; Real is the photometric sky.",
                    );
                    pair(
                        ui,
                        "marks",
                        &planned.0.marks.len().to_string(),
                        "Cells drawn as discrete systems — the fetch set once \
                         spawning is bounded to it.",
                    );
                    pair(
                        ui,
                        "splats",
                        &planned.0.splats.len().to_string(),
                        "Cells the walk would draw as a glow field, kept for \
                         the field renderer — not drawn yet.",
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

/// A reach in light years, said at a precision that survives being small
///
/// The reach follows the camera all the way in
/// ([`crate::systems::reach_with_camera`]), so inside a system it is
/// thousandths of a light year and whole light years read as nought. Said in
/// light seconds under a hundredth of one, which is the scale a system's own
/// grid is ruled in.
fn reach(light_years: f32) -> String {
    if light_years >= 1. {
        format!("{light_years:.0} ly")
    } else if light_years >= 1e-2 {
        format!("{light_years:.3} ly")
    } else {
        let seconds =
            f64::from(light_years) * crate::space::LIGHT_YEAR / 299_792_458.;
        format!("{seconds:.0} Ls")
    }
}
