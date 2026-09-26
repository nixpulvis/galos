//! A row of the bar, and the buttons it ends with

use crate::ui::bar::ROW_PADDING;
use crate::ui::{AGAIN, CLOSE, INFO};
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Response, Ui};

/// Take a row's worth of the bar, and answer for it under an id of its own
///
/// Every id in these rows is spelled out rather than taken from the order the
/// widgets happened to be drawn in. Egui hands out an unspelled id from that
/// order, and the rows of the bar do not keep their places within it: the note
/// about a name that resolved to nothing comes and goes above them, and the
/// selection's rows come and go with the selection. Either shifts the count
/// every row below is numbered by.
///
/// What is spelled out is where the row sits in the bar's one column, counted
/// across every kind of row in it, and not which system or filter it is about.
/// A row here is clicked and hovered and nothing else, and that is all egui
/// keeps against an id, so a place is the honest key: the pointer is over the
/// third row, whichever row now stands there.
///
/// Across the kinds and not within each, because the kinds share the column
/// and are drawn to the same height. Letting go of one selected system moves
/// every filter row up by exactly one row, so each lands in a rectangle a
/// selection row was drawn in. Numbered apart, the two would put a fresh id
/// at a rectangle that kept its place, which is the very thing this is for.
///
/// Keying a row on what it holds paints a red rectangle across the bar.
/// Between one pass and the next egui looks for a rect that kept its place
/// while everything in it changed identity, which is what a replaced selection
/// and a dropped filter both are, and it cannot tell that apart from one
/// widget taking another's state. It warns and paints the rect in red.
///
/// `push_id` does not answer this either, and makes it worse: a child `Ui`
/// registers a rect of its own, so a parent named for the row's system adds a
/// second widget at the row's rect that changes identity along with it.
///
/// The space is taken without a widget of its own, since the row is what
/// answers for it and two things at one rect is what the ordering was
/// complaining of in the first place.
pub(super) fn row_of(
    ui: &mut Ui,
    height: f32,
    of: impl std::hash::Hash,
) -> (egui::Rect, Response) {
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), height));
    let row = ui.interact(rect, ui.id().with(of), egui::Sense::click());

    (rect, row)
}

/// The buttons a row in the bar ends with
///
/// Info opens a panel about whatever the row names, close lets go of it, and
/// again asks a route over. Close stands outermost, where a window's own
/// close button stands, so that the gesture is in the same place wherever it
/// is offered — and each row draws the prefix of them it has a use for, so
/// the column of buttons reads straight down however many any one row ends
/// with.
pub(super) struct Buttons {
    /// Nothing where the row names nothing a panel could describe
    pub(super) info: Option<Response>,
    pub(super) close: Response,
    /// Nothing where the row names nothing that was searched for
    pub(super) again: Option<Response>,
}

/// The glyphs those buttons are drawn with, outermost first
const GLYPHS: [&str; 3] = [CLOSE, INFO, AGAIN];

/// Lay the buttons out without placing them
///
/// A row needs their width before it can be allocated, since what is left is
/// the room its name has, and it cannot be painted into before it exists. So
/// they are measured here and placed by [`place_buttons`] once there is a row
/// to place them in.
pub(super) fn lay_out_buttons(ui: &Ui) -> Vec<std::sync::Arc<egui::Galley>> {
    lay_out(ui, &GLYPHS[..2])
}

/// Those two and the one that asks again, for a row that names a route
///
/// Only a route: it is the one filter that was *searched* for, so it is the
/// only one there is anything to ask again. See [`crate::map::search::Search::Replot`].
pub(super) fn lay_out_replot(ui: &Ui) -> Vec<std::sync::Arc<egui::Galley>> {
    lay_out(ui, &GLYPHS)
}

/// The close button alone, for a row with nothing to describe
///
/// Close is outermost, so a row that ends here ends where every other row
/// ends and the column of buttons reads straight down.
pub(super) fn lay_out_close(ui: &Ui) -> Vec<std::sync::Arc<egui::Galley>> {
    lay_out(ui, &GLYPHS[..1])
}

fn lay_out(ui: &Ui, glyphs: &[&str]) -> Vec<std::sync::Arc<egui::Galley>> {
    glyphs
        .iter()
        .map(|glyph| {
            // Laid out in nothing, so that the color can be chosen once the
            // pointer has been asked about, which cannot happen until the row
            // has been placed.
            egui::WidgetText::from(
                egui::RichText::new(*glyph).color(egui::Color32::PLACEHOLDER),
            )
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
        })
        .collect()
}

/// How much room the buttons take at the end of a row, gaps included
pub(super) fn buttons_width(
    buttons: &[std::sync::Arc<egui::Galley>],
    gap: f32,
) -> f32 {
    buttons.iter().map(|button| button.size().x + gap).sum()
}

/// Paint the buttons into the right hand end of `rect` and answer for each
///
/// Asked about after the row they sit in, so that they are the ones answering
/// where they overlap it. Under it the row would have to work out what it was
/// not being clicked on.
pub(super) fn place_buttons(
    ui: &mut Ui,
    rect: egui::Rect,
    buttons: Vec<std::sync::Arc<egui::Galley>>,
    of: impl std::hash::Hash,
) -> Buttons {
    let middle = rect.center().y;
    let gap = ui.spacing().item_spacing.x;
    let mut right = rect.right() - ROW_PADDING;
    let mut answers = Vec::new();

    for (which, galley) in buttons.into_iter().enumerate() {
        let width = galley.size().x;
        let at = egui::Rect::from_min_max(
            egui::pos2(right - width, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let response = ui.interact(
            at,
            ui.id().with((&of, "row-button", which)),
            egui::Sense::click(),
        );
        // Lit for the pointer resting on it and for the keyboard reaching it
        // alike. A stop that shows nothing when it is reached reads as the
        // focus having gone missing.
        //
        // The glyph brightens and nothing is painted behind it. The row it
        // sits in lights up under the pointer already, and a second rectangle
        // inside that one reads as a button dropped into a row rather than as
        // part of it.
        let lit = response.hovered() || response.has_focus();
        let height = galley.size().y;
        ui.painter().galley(
            egui::pos2(at.left(), middle - height / 2.),
            galley,
            if lit {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().weak_text_color()
            },
        );

        right = at.left() - gap;
        answers.push(response.on_hover_cursor(egui::CursorIcon::PointingHand));
    }

    let mut answers = answers.into_iter();
    // In `GLYPHS` order, which is close first. A row laid out with the close
    // button alone ends there.
    let close = answers.next().expect("a close button");
    let info = answers.next();
    let again = answers.next();
    Buttons { info, close, again }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row is keyed on what it is about, not on where it was drawn
    ///
    /// The rows of the bar do not keep their places: a note comes and goes
    /// above them, the selection's row comes and goes with the selection, and
    /// dropping one filter moves every row below it up. An id taken from the
    /// draw order would hand the row that moved up whatever egui had been
    /// remembering against the one that left.
    #[test]
    fn a_row_is_keyed_on_what_it_is_about() {
        let ctx = crate::testing::context();
        let (mut first, mut moved, mut other) = (None, None, None);

        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            first = Some(row_of(ui, 20., ("filter-row", 7)).1.id);
            // Anything at all between them shifts what comes after.
            ui.label("a note that comes and goes");
            moved = Some(row_of(ui, 20., ("filter-row", 7)).1.id);
            other = Some(row_of(ui, 20., ("filter-row", 9)).1.id);
        });

        assert_eq!(first, moved, "the same row moved and changed identity");
        assert_ne!(first, other, "two rows share one identity");
    }

    /// And so are the marks it ends with
    #[test]
    fn the_marks_are_keyed_on_the_row_they_end() {
        let ctx = crate::testing::context();
        let (mut first, mut moved) = (None, None);
        let at =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(80., 20.));

        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let buttons = lay_out_buttons(ui);
            first = Some(
                place_buttons(ui, at, buttons, ("filter-row", 7)).close.id,
            );
            ui.label("a note that comes and goes");
            let buttons = lay_out_buttons(ui);
            moved = Some(
                place_buttons(ui, at, buttons, ("filter-row", 7)).close.id,
            );
        });

        assert_eq!(first, moved);
    }

    /// What `contents` painted with the pointer resting at `at`
    ///
    /// Two passes, since egui works out what the pointer is over from where
    /// the widgets were the pass before. The second is the one read.
    fn under_pointer(
        at: egui::Pos2,
        mut contents: impl FnMut(&mut Ui),
    ) -> Vec<egui::Shape> {
        let ctx = crate::testing::context();
        let input = || egui::RawInput {
            events: vec![egui::Event::PointerMoved(at)],
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |ui| contents(ui));
        let output = ctx.run_ui(input(), |ui| contents(ui));

        output.shapes.into_iter().map(|clipped| clipped.shape).collect()
    }

    /// Every filled rectangle among `shapes`, however deeply nested
    fn rectangles(shapes: &[egui::Shape]) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, into: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) => into.push(rect.rect),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut found = Vec::new();
        for shape in shapes {
            walk(shape, &mut found);
        }
        found
    }

    /// The color each piece of text among `shapes` was painted in
    fn colors(shapes: &[egui::Shape]) -> Vec<egui::Color32> {
        fn walk(shape: &egui::Shape, into: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Text(text) => into.push(text.fallback_color),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut found = Vec::new();
        for shape in shapes {
            walk(shape, &mut found);
        }
        found
    }

    /// A mark under the pointer brightens and paints nothing behind itself
    ///
    /// The row a mark sits in lights up under the pointer already, so a
    /// rectangle drawn inside that one reads as a button dropped into the
    /// row rather than as part of it.
    #[test]
    fn a_mark_lights_without_a_background() {
        let row = egui::Rect::from_min_size(
            egui::pos2(0., 0.),
            egui::vec2(200., 20.),
        );
        // Just inside the close mark, which stands outermost.
        let on_the_mark =
            egui::pos2(row.right() - ROW_PADDING - 1., row.center().y);

        let painted = under_pointer(on_the_mark, |ui| {
            let buttons = lay_out_buttons(ui);
            place_buttons(ui, row, buttons, "row");
        });

        // The mark answered the pointer, which is what leaves the assertion
        // below with something to say.
        assert!(
            colors(&painted)
                .contains(&egui::Visuals::default().strong_text_color()),
            "nothing lit up, so nothing was under the pointer"
        );
        assert_eq!(rectangles(&painted), Vec::new(), "a mark painted a box");
    }
}
