//! The map with the chrome put away
//!
//! For reading the sky, and for a picture of it: the bar, the gear, the rows
//! under the bar, the settings pane and the windows a row opens all go, and
//! what stays is what the picture needs to be read by — the rose, and the
//! color key bare in the corner the chrome stood in, since a map colored by
//! allegiance with nothing to say which color is which is a map of colors.
//! The names, the grid and the
//! time strip keep their own switches; this is not one more of those.
//!
//! Brought back by the key that hid it, by escape, and by the faint eye left
//! standing where the gear was, so a reader who hid it by accident has a
//! way back they can see.

use crate::map::filter::Filters;
use crate::map::galaxy::spawn::ColorBy;
use crate::ui::legend::legend;
use crate::ui::{MARGIN, zone};
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Color32, Context, Sense, Stroke, Vec2};

/// Whether the chrome is put away
#[derive(Resource, Default)]
pub(crate) struct ChromeHidden(pub(crate) bool);

/// How large the eye is drawn, the gear's own size
const EYE: f32 = 18.;

/// How much of its ink the eye keeps while the chrome is hidden and the
/// pointer is not on it
///
/// Faint, being the one thing left over a picture, and not gone, being the
/// way back.
const FAINT: f32 = 0.35;

/// The switch that hides the chrome, and brings it back
///
/// An eye, struck through while the chrome is up — the button hides it — and
/// open while it is hidden, which is the button bringing it back. Painted
/// rather than lettered, the faces the chrome is lettered in holding no eye.
///
/// `at` is where its top left stands: under the gear while the chrome is up,
/// and in the corner the gear stood in while it is not.
pub(super) fn eye(ctx: &Context, at: egui::Pos2, hidden: &mut bool) {
    let clicked = zone("hide-interface")
        .fixed_pos(at)
        .show(ctx, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::splat(EYE), Sense::click());
            let visuals = ui.style().interact(&response);
            let mut ink = visuals.fg_stroke.color;
            if *hidden && !response.hovered() {
                ink = ink.gamma_multiply(FAINT);
            }
            paint_eye(ui.painter(), rect, ink, !*hidden);
            response
                .on_hover_text(match *hidden {
                    true => "Show the interface (I)",
                    false => "Hide the interface (I)",
                })
                .clicked()
        })
        .inner;
    if clicked {
        *hidden = !*hidden;
    }
}

/// An eye in `rect`, struck through where `struck`
fn paint_eye(
    painter: &egui::Painter,
    rect: egui::Rect,
    ink: Color32,
    struck: bool,
) {
    let stroke = Stroke::new(1.5_f32, ink);
    let center = rect.center();
    let half = rect.width() * 0.45;
    let lid = rect.height() * 0.28;
    // Two arcs meeting at the corners, as quadratic curves through the lid's
    // height.
    let lids = [-1., 1.].map(|side: f32| {
        (0..=12)
            .map(|n| {
                let t = n as f32 / 12. * 2. - 1.;
                center + Vec2::new(t * half, side * lid * (1. - t * t))
            })
            .collect::<Vec<_>>()
    });
    for points in lids {
        painter.add(egui::Shape::line(points, stroke));
    }
    painter.circle_filled(center, lid * 0.7, ink);
    if struck {
        let reach = Vec2::splat(rect.width() * 0.38);
        painter.line_segment([center - reach, center + reach], stroke);
    }
}

/// The color key, bare, in the top left corner, under the eye
///
/// Where the chrome stood, so the key is read where the reader was already
/// looking. Frameless, over the picture rather than in a card on it, with
/// the axis it is keyed on named over it. No line saying what a chip does:
/// nothing here can be clicked.
pub(super) fn bare_legend(
    ctx: &Context,
    filters: &Filters,
    axis: ColorBy,
    held: Option<&galos_index::read::inhabited::Inhabited>,
) {
    zone("bare-legend").fixed_pos(egui::pos2(MARGIN, MARGIN * 2. + EYE)).show(
        ctx,
        |ui| {
            ui.label(
                egui::RichText::new(axis.name().to_uppercase()).small().weak(),
            );
            legend(ui, filters, axis, held, false);
        },
    );
}
