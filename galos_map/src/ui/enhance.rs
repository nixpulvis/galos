//! The chrome over an enhanced picture: the launcher that asks for one, the
//! line at the foot of the window while there is one, and the rest of the
//! chrome put away meanwhile
//!
//! The picture itself, and everything asked of it, is
//! [`crate::map::enhance`]'s; what is here works it through
//! [`Enhance`] alone.

use super::hide::ChromeHidden;
use super::{MARGIN, zone};
use crate::map::enhance::{Ask, Driven, Enhance, Job, Phase, Picture, SCALES};
use crate::map::schedule::PaintSet;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, hide_chrome.after(Driven));
    app.add_systems(EguiPrimaryContextPass, controls.in_set(PaintSet::Ui));
}

/// Put the chrome away while there is a picture, as `I` does, and back as it
/// was once the picture is
///
/// A picture is read, not worked on: the bar's filters and rows would change
/// a map the picture no longer follows. What stays is what `I` leaves, the
/// bare color key the picture was drawn in, the rose and the picture's own
/// controls.
fn hide_chrome(
    enhance: Res<Enhance>,
    mut hidden: ResMut<ChromeHidden>,
    // How the chrome stood when the picture was asked for.
    mut was: Local<Option<bool>>,
) {
    match (enhance.picture.is_some(), *was) {
        (true, None) => {
            *was = Some(hidden.0);
            hidden.0 = true;
        }
        // Held hidden, the eye in the corner included.
        (true, Some(_)) if !hidden.0 => hidden.0 = true,
        (false, Some(stood)) => {
            hidden.0 = stood;
            *was = None;
        }
        _ => {}
    }
}

/// A count of systems as the progress reads it
fn systems(count: u64) -> String {
    match count {
        0..1_000 => format!("{count}"),
        1_000..1_000_000 => format!("{:.0}k", count as f64 / 1e3),
        _ => format!("{:.1}M", count as f64 / 1e6),
    }
}

/// What stands over a picture: the pointer over it, and its own line at the
/// foot of the window
///
/// The line, rather than a card in a corner: the chrome is put away while
/// there is a picture, and what is left is read along the bottom edge
/// without covering the rose or the key. Asking for a picture is
/// [`launcher`]'s, in the chrome's own column.
fn controls(
    mut contexts: EguiContexts,
    mut enhance: ResMut<Enhance>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let enhance = &mut *enhance;

    // While there is a picture, the window under the chrome is the
    // picture's: the map takes no pointer, and in a shown picture the wheel
    // and a drag look about in it.
    if let Some(picture) = &mut enhance.picture {
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("enhance-picture"))
            .order(egui::Order::Background)
            .fixed_pos(screen.min)
            .constrain(false)
            .show(ctx, |ui| {
                let response =
                    ui.allocate_rect(screen, egui::Sense::click_and_drag());
                if enhance.phase != Phase::Shown {
                    return;
                }
                if response.dragged() {
                    let by = response.drag_delta();
                    picture.pan(Vec2::new(by.x, by.y));
                }
                if response.hovered() {
                    let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                    if scroll != 0.
                        && let Some(at) = response.hover_pos()
                    {
                        let at = at - screen.min;
                        picture.zoom_about(
                            Vec2::new(at.x, at.y),
                            (scroll / 200.).exp(),
                        );
                    }
                }
            });
    }

    // The picture's own line. Nothing while there is no picture: the
    // launcher in the chrome's column is what asks for one.
    if enhance.phase == Phase::Off {
        return Ok(());
    }
    zone("enhance")
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0., -MARGIN))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let asked = match enhance.phase {
                        Phase::Off => None,
                        Phase::Drawing => {
                            if let Some(picture) = &enhance.picture
                                && let Some(job) = &picture.job
                            {
                                drawing(ui, picture, job);
                            }
                            ui.small_button("cancel")
                                .clicked()
                                .then_some(Ask::Close)
                        }
                        Phase::Shown => {
                            enhance.picture.as_ref().and_then(|picture| {
                                shown(
                                    ui,
                                    picture,
                                    enhance.saved.as_ref(),
                                    enhance.saved_at,
                                )
                            })
                        }
                    };
                    if asked.is_some() {
                        enhance.asked = asked;
                    }
                });
            });
        });
    Ok(())
}

/// How far a picture being drawn has got, along one line: the part under
/// way, the systems summed against what the cells in view own, and what is
/// left; the rate under the pointer
fn drawing(ui: &mut egui::Ui, picture: &Picture, job: &Job) {
    let progress = &job.progress;
    let parts = picture.parts();
    let done = progress.parts.load(Relaxed).min(parts);
    let part = match done {
        0 => "the view".to_owned(),
        n if n < parts => format!("piece {n} of {}", parts - 1),
        _ => "laying it down".to_owned(),
    };
    ui.label(format!("{}× enhance · {part}", picture.scale));
    let bar = |share: f32| egui::ProgressBar::new(share).desired_width(140.);
    if !progress.planned.load(Relaxed) {
        ui.add(bar(0.));
        ui.weak("finding the cells in view");
        return;
    }
    let counted = progress.counted.load(Relaxed);
    let total = progress.total.load(Relaxed).max(1);
    let elapsed = job.started.elapsed();
    let rate = counted as f64 / elapsed.as_secs_f64().max(1e-3);
    ui.add(bar((counted as f32 / total as f32).clamp(0., 1.)))
        .on_hover_text(format!("{} systems a second", systems(rate as u64)));
    let left = (counted > 0)
        .then(|| (total - counted.min(total)) as f64 / rate.max(1.));
    ui.weak(format!(
        "{} of {} systems{}",
        systems(counted),
        systems(total),
        left.map_or(String::new(), |left| match left < 10. {
            true => format!(" · {left:.1}s left"),
            false => format!(" · {left:.0}s left"),
        }),
    ));
}

/// How long the line says a picture was saved
///
/// Long enough to be read, and gone after: it is news, and the path stays
/// under the pointer of nothing once it is. Why one was not saved stays,
/// being something to act on.
const SAVED_FOR: Duration = Duration::from_secs(3);

/// A shown picture's line: how far into it the window is, saving it and
/// putting it away; what is asked of it, where anything was
fn shown(
    ui: &mut egui::Ui,
    picture: &Picture,
    saved: Option<&Result<PathBuf, String>>,
    saved_at: Option<Instant>,
) -> Option<Ask> {
    ui.label(format!("{}× enhance · {:.1}× zoom", picture.scale, picture.zoom));
    let mut asked = None;
    match (&picture.saving, &picture.asking_where) {
        (Some(saving), _) => {
            ui.add_enabled(
                false,
                egui::Button::new(format!(
                    "saving {:.0}%",
                    saving.share(picture) * 100.
                ))
                .small(),
            );
        }
        (None, Some(_)) => {
            ui.add_enabled(false, egui::Button::new("choosing where…").small());
        }
        (None, None) => {
            if ui.small_button("save png").clicked() {
                asked = Some(Ask::Save);
            }
        }
    }
    match saved {
        Some(Ok(path))
            if saved_at.is_some_and(|at| at.elapsed() < SAVED_FOR) =>
        {
            ui.weak("saved").on_hover_text(path.display().to_string());
        }
        Some(Err(why)) => {
            ui.weak("not saved").on_hover_text(why);
        }
        // Nothing saved, or saved long enough ago to be old news.
        _ => {}
    }
    if ui.small_button("close").clicked() {
        asked = Some(Ask::Close);
    }
    asked
}

/// How large the launcher's mark is drawn, the eye's own size
const MARK: f32 = 18.;

/// The button that asks for a picture, in the chrome's own column under the
/// eye, and the scales it offers
///
/// A viewfinder, painted as the eye and the gear above it are, and nothing
/// else standing on the map: a click puts the scales out beside it, and a
/// scale clicked asks for the picture there and then. `P` asks for one at
/// the scale last picked. Nothing while there is a picture, the chrome
/// being put away then.
pub(super) fn launcher(
    ctx: &egui::Context,
    at: egui::Pos2,
    enhance: &mut Enhance,
) {
    if enhance.phase != Phase::Off {
        return;
    }
    let mark = zone("enhance-launcher")
        .fixed_pos(at)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::Vec2::splat(MARK),
                egui::Sense::click(),
            );
            let ink = ui.style().interact(&response).fg_stroke.color;
            paint_viewfinder(ui.painter(), rect, ink);
            response.on_hover_text("Enhance this view (P)")
        })
        .inner;
    if mark.clicked() {
        enhance.choosing = !enhance.choosing;
    }
    if !enhance.choosing {
        return;
    }
    let scales = zone("enhance-scales")
        .pivot(egui::Align2::LEFT_CENTER)
        .fixed_pos(egui::pos2(
            mark.rect.right() + MARGIN / 2.,
            mark.rect.center().y,
        ))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("enhance");
                    for offered in SCALES {
                        if ui
                            .selectable_label(
                                offered == enhance.scale,
                                format!("{offered}×"),
                            )
                            .on_hover_text(format!(
                                "Every system in this view, drawn {offered} \
                                 windows across"
                            ))
                            .clicked()
                        {
                            enhance.scale = offered;
                            enhance.asked = Some(Ask::Start);
                            enhance.choosing = false;
                        }
                    }
                });
                if let Some(Err(why)) = &enhance.saved {
                    ui.weak(format!("the last was not drawn: {why}"));
                }
            });
        });
    if scales.response.clicked_elsewhere() && !mark.clicked() {
        enhance.choosing = false;
    }
}

/// A viewfinder in `rect`: four corners and a point between them
///
/// Not a magnifier, which stands beside the search box and reads as one.
fn paint_viewfinder(
    painter: &egui::Painter,
    rect: egui::Rect,
    ink: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.5_f32, ink);
    let frame = rect.shrink(rect.width() * 0.08);
    let arm = frame.width() * 0.3;
    for (corner, x, y) in [
        (frame.left_top(), 1., 1.),
        (frame.right_top(), -1., 1.),
        (frame.left_bottom(), 1., -1.),
        (frame.right_bottom(), -1., -1.),
    ] {
        painter
            .line_segment([corner, corner + egui::vec2(x * arm, 0.)], stroke);
        painter
            .line_segment([corner, corner + egui::vec2(0., y * arm)], stroke);
    }
    painter.circle_filled(frame.center(), frame.width() * 0.1, ink);
}
