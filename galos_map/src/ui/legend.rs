//! The color key's pieces, wherever it is drawn: the swatch a value is shown
//! by, and the mini legend naming the color row's chips
//!
//! Three places draw the key — the Filter tab's rows, the color row's chips
//! and the mini legend — and all three read [`crate::map::filter::key`]'s one
//! list and paint it through [`Swatch`], so a value looks the same whichever
//! of them it is read in.

use crate::map::filter::Filters;
use crate::map::filter::key::{Hidden, Item, Tier, held_tiers};
use crate::map::filter::mask::Mask;
use crate::map::galaxy::spawn::{ColorBy, Hue};
use crate::style::color32;
use bevy_egui::egui;
use bevy_egui::egui::{Color32, Response, Sense, Stroke, Ui, Vec2};

/// How wide a swatch's outline is drawn, where it is not filled
const OUTLINE: f32 = 1.5;

/// How round a square swatch's corners are
const CORNER: f32 = 2.;

/// What a swatch shows: the color, and how much of what it stands for is
/// hidden
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Swatch {
    /// One color: a value, or a group every member of which is drawn in it
    Hue { hue: Hue, hidden: Hidden },
    /// Several colors, a slice each: the Other of allegiance
    Pie { hues: Vec<Hue>, hidden: Hidden },
    /// The systems nobody lives in, which have no color of their own
    Uninhabited { hidden: bool },
}

impl Swatch {
    /// The swatch a top-tier row is shown by
    pub(crate) fn of_tier(tier: &Tier, axis: ColorBy, mask: &Mask) -> Swatch {
        let hidden = tier.hidden(axis, mask);
        match tier {
            Tier::Item(item) => Swatch::of_item(item, axis, mask),
            Tier::Group { hue: Some(hue), .. } => {
                Swatch::Hue { hue: *hue, hidden }
            }
            Tier::Group { hue: None, items, .. } => Swatch::Pie {
                hues: items.iter().map(|item| item.hue).collect(),
                hidden,
            },
        }
    }

    /// The swatch one value is shown by
    pub(crate) fn of_item(item: &Item, axis: ColorBy, mask: &Mask) -> Swatch {
        Swatch::Hue { hue: item.hue, hidden: item.hidden(axis, mask) }
    }

    /// The swatch the systems nobody lives in are shown by
    pub(crate) fn uninhabited(mask: &Mask) -> Swatch {
        Swatch::Uninhabited { hidden: mask.hides_uninhabited() }
    }

    /// Paint the swatch into a square of `size` laid inline
    ///
    /// Painted rather than lettered, so it is a shape whatever the font
    /// holds, as the rows' own dots are.
    pub(crate) fn paint(&self, ui: &mut Ui, size: f32) -> Response {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let painter = ui.painter();
        let muted = ui.visuals().weak_text_color();
        match self {
            Swatch::Hue { hue, hidden } => {
                let color = color32(hue.swatch());
                match hidden {
                    Hidden::None => {
                        painter.rect_filled(rect, CORNER, color);
                    }
                    Hidden::All => {
                        painter.rect_stroke(
                            rect.shrink(OUTLINE / 2.),
                            CORNER,
                            Stroke::new(OUTLINE, color.gamma_multiply(0.5)),
                            egui::StrokeKind::Middle,
                        );
                    }
                    Hidden::Some { .. } => {
                        let mut left = rect;
                        left.set_right(rect.center().x);
                        painter.rect_filled(left, CORNER, color);
                        painter.rect_stroke(
                            rect.shrink(OUTLINE / 2.),
                            CORNER,
                            Stroke::new(OUTLINE, color),
                            egui::StrokeKind::Middle,
                        );
                    }
                }
            }
            Swatch::Pie { hues, hidden } => {
                let alpha = match hidden {
                    Hidden::None => 1.,
                    Hidden::Some { .. } => 0.6,
                    Hidden::All => 0.,
                };
                if alpha > 0. {
                    pie(painter, rect, hues, alpha);
                }
                if *hidden != Hidden::None {
                    painter.circle_stroke(
                        rect.center(),
                        rect.width() / 2. - OUTLINE / 2.,
                        Stroke::new(OUTLINE, muted),
                    );
                }
            }
            Swatch::Uninhabited { hidden } => {
                let color = match hidden {
                    true => muted.gamma_multiply(0.4),
                    false => muted,
                };
                dashed_circle(
                    painter,
                    rect.center(),
                    rect.width() / 2. - OUTLINE / 2.,
                    Stroke::new(OUTLINE, color),
                );
            }
        }
        response
    }
}

/// A disc cut into an even slice for each of `hues`
///
/// A fan of triangles, egui having no conic fill. Twenty-four steps round the
/// whole, which at thirteen pixels across is rounder than the eye can tell.
fn pie(painter: &egui::Painter, rect: egui::Rect, hues: &[Hue], alpha: f32) {
    if hues.is_empty() {
        return;
    }
    const STEPS: usize = 24;
    let center = rect.center();
    let radius = rect.width() / 2.;
    let mut mesh = egui::Mesh::default();
    let slice = std::f32::consts::TAU / hues.len() as f32;
    for (n, hue) in hues.iter().enumerate() {
        let color = color32(hue.swatch()).gamma_multiply(alpha);
        let steps = (STEPS / hues.len()).max(2);
        let from = slice * n as f32 - std::f32::consts::FRAC_PI_2;
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(center, color);
        for step in 0..=steps {
            let angle = from + slice * step as f32 / steps as f32;
            mesh.colored_vertex(
                center + radius * Vec2::new(angle.cos(), angle.sin()),
                color,
            );
        }
        for step in 0..steps as u32 {
            mesh.add_triangle(base, base + 1 + step, base + 2 + step);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// A circle drawn in dashes, which is empty space drawn as a shape
fn dashed_circle(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    stroke: Stroke,
) {
    const DASHES: usize = 8;
    let step = std::f32::consts::TAU / (DASHES * 2) as f32;
    for dash in 0..DASHES {
        let from = step * (dash * 2) as f32;
        let points: Vec<egui::Pos2> = (0..=4)
            .map(|n| {
                let angle = from + step * n as f32 / 4.;
                center + radius * Vec2::new(angle.cos(), angle.sin())
            })
            .collect();
        painter.add(egui::Shape::line(points, stroke));
    }
}

/// How much of a group is showing, as `k/n` says it, where it is partly
/// hidden
pub(crate) fn showing(hidden: Hidden) -> Option<String> {
    match hidden {
        Hidden::Some { hidden, of } => Some(format!("{}/{of}", of - hidden)),
        _ => None,
    }
}

/// The color attention is drawn in: something hidden that the reader may
/// have forgotten was
pub(crate) fn attention(ui: &Ui) -> Color32 {
    ui.visuals().warn_fg_color
}

/// How large a swatch stands in the mini legend
const LEGEND_SWATCH: f32 = 10.;

/// Name the color row's chips, one line a chip
///
/// The one list, drawn in two places: framed as a popover under the color
/// row while the pointer is over it, and bare in the top left while the
/// interface is hidden. `hint` says whether the line saying what a chip does
/// is wanted under it, which it is only where a chip can be clicked.
///
/// `held` is the galaxy's colonies, for which values there are lines for.
pub(crate) fn legend(
    ui: &mut Ui,
    filters: &Filters,
    axis: ColorBy,
    held: Option<&galos_index::read::inhabited::Inhabited>,
    hint: bool,
) {
    let mask = filters.mask();
    let muted = ui.visuals().weak_text_color();
    for tier in held_tiers(axis, held) {
        let hidden = tier.hidden(axis, mask);
        ui.horizontal(|ui| {
            Swatch::of_tier(&tier, axis, mask).paint(ui, LEGEND_SWATCH);
            let name = match &tier {
                // A government's color stands for several, and says how
                // many: "Red (7)".
                Tier::Group { hue: Some(_), items, .. } => {
                    format!("{} ({})", tier.name(), items.len())
                }
                _ => tier.name().to_owned(),
            };
            let text = egui::RichText::new(name);
            ui.label(match hidden {
                Hidden::All => text.strikethrough().color(muted),
                _ => text,
            });
            if let Some(showing) = showing(hidden) {
                ui.label(egui::RichText::new(showing).color(attention(ui)));
            }
        });
    }
    ui.horizontal(|ui| {
        Swatch::uninhabited(mask).paint(ui, LEGEND_SWATCH);
        let text = egui::RichText::new("Uninhabited");
        ui.label(match mask.hides_uninhabited() {
            true => text.strikethrough().color(muted),
            false => text,
        });
    });
    if hint {
        ui.label(egui::RichText::new("click a chip to toggle").weak().small());
    }
}
