//! The controls the pane and the bar are both built from
//!
//! A switch with its hint, the box beside a slider, a text field that says
//! what it wants while it is empty: small enough to be anyone's, and drawn
//! the same wherever they stand.

use crate::ui::SPINNER;
use crate::ui::text::typed;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Response, Ui};

/// How much of a slider's row the number beside it is given
///
/// The rail takes everything but this, so the boxes line up down the pane and
/// the last of them ends where the pane does. Enough for the widest number a
/// slider here reaches: the spyglass runs to 110,000 light years.
pub(super) const VALUE_WIDTH: f32 = 56.;

/// Hold a slider's value in a box of its own
///
/// A slider left to draw its own value sizes that box to the number in it, so
/// a column of them comes out ragged and none reaches the edge of the pane.
/// Drawn separately, every box is the same width and they line up.
///
/// The caller builds the box, since what clamps and how fast it drags is the
/// slider's own business: the three the spyglass is offered at share one value
/// and none of them may clamp it, where the one the filters are dimmed by is
/// a percentage and clamps to it.
pub(super) fn value_box(ui: &mut Ui, value: egui::DragValue<'_>) -> Response {
    ui.add_sized(egui::vec2(VALUE_WIDTH, ui.spacing().interact_size.y), value)
}

/// Size the next slider to the room it stands in
///
/// Egui draws a rail at [`egui::style::Spacing::slider_width`] and not at
/// whatever room it has, so a slider left alone is an island of the same
/// hundred pixels wherever it is put. Asked here rather than once for the
/// whole pane, so that a slider indented under a checkbox fills what is left
/// of its line rather than running out past it.
///
/// What the slider leaves is `beside`, for whatever stands at the end of the row
/// reading it out, and the gap between the two, so the row ends flush with
/// whatever it is standing in.
pub(super) fn fill_width(ui: &mut Ui, beside: f32) {
    let gap = ui.spacing().item_spacing.x;
    ui.spacing_mut().slider_width =
        (ui.available_width() - beside - gap).max(0.);
}

/// Draw `contents` with what is chosen in it marked in grey
///
/// A tab, and the switch under the clock's reading, are not selected systems.
/// Egui marks a chosen one in the color it marks a selection in, and in this
/// chrome that color means picked out on the map: it is the ring around a
/// star and the dot on a row, and nothing else. So a chosen one is filled in
/// the grey the rest of the chrome is drawn in and says which it is by being
/// filled at all.
///
/// Scoped, since a style set on a `Ui` is set on the rest of that `Ui`: the
/// same color is what egui highlights selected text with, and a field typed
/// into below would lose its own.
pub(super) fn greyed<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let strong = ui.visuals().strong_text_color();
        let filled = ui.visuals().widgets.active.weak_bg_fill;
        let visuals = ui.visuals_mut();
        visuals.selection.bg_fill = filled;
        visuals.selection.stroke.color = strong;

        contents(ui)
    })
    .inner
}

/// A control and what it does, said on hover
///
/// One line, plainly: the ordinary word for the thing that happens when the
/// control is used. "Fetch fresh data periodically", not "go back for what the
/// map holds" — the second is the voice this crate's doc comments are written
/// in, and in a tooltip it is a sentence a reader has to decode to learn that
/// a checkbox fetches anything. The prose belongs in the comments; a hint is
/// for someone who wants to know what a switch does and get on with it.
///
/// So: say the verb. Fetch, show, draw, color, name, hide. Say the noun the
/// user would use for what it acts on — systems, names, the grid — and not the
/// name the code gives it. Leave out why it is there, how it works and what it
/// costs; those are what a doc comment is for, and [`Routing`](crate::map::route::graph::Routing) is an example
/// of one carrying what would not fit here.
///
/// No full stop. It is a label rather than prose, as the control's own name
/// is, and every one of them ends the same way for the same reason.
///
/// Answered through these three rather than by each control reaching for
/// `on_hover_text` itself, so that a control added without a hint reads as
/// odd at the callsite instead of quietly having none.
pub(super) fn check(
    ui: &mut Ui,
    on: &mut bool,
    said: &str,
    hint: &str,
) -> Response {
    ui.checkbox(on, said).on_hover_text(hint)
}

/// Whether the user has just finished with a field by pressing return
pub(super) fn entered(response: &Response, ui: &Ui) -> bool {
    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// How far a text field's text stands from its own edge
///
/// Egui's own is tighter above and below than it is at the sides, which
/// leaves a field looking squeezed against the one under it.
pub(super) const FIELD_PADDING: egui::Margin =
    egui::Margin { left: 4, right: 4, top: 4, bottom: 4 };

/// How tall a row holding one text field comes to
///
/// What [`singleline`] takes: one row of the face the chrome is lettered in,
/// which is what a single line `TextEdit` asks for, and [`FIELD_PADDING`]
/// above and below it. No floor at `interact_size`: egui puts one under a
/// button and not under a field.
///
/// Read by `dated` as well, whose first row is a line of text rather than a
/// field and which stands level with the bar's box. Worked out rather than
/// measured off a drawn field, the pane that wants it being a different pane
/// and drawn before the field is. Held to the field itself by
/// `the_reading_stands_level_with_the_box`.
pub(super) fn field_height(ui: &Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Body)
        + (FIELD_PADDING.top + FIELD_PADDING.bottom) as f32
}

/// The border a text field keeps while nothing is happening to it
///
/// Egui draws a field at rest with no fill and no border, leaving nothing on
/// screen but the text in it. That reads well enough inside a pane with an
/// edge of its own, and not at all against the map.
const FIELD_BORDER: egui::Stroke =
    egui::Stroke { width: 1., color: egui::Color32::from_gray(90) };

/// One text field, showing what it wants when it holds nothing
///
/// `reserved` is room kept clear inside the right hand end, which the caller
/// paints into itself. Kept by widening the field's own margin rather than by
/// narrowing the field, so that the box goes on reaching the full width and a
/// name long enough runs up to what is standing in there rather than under
/// it.
///
/// `placeholer` names the field as well as standing in it, so two fields in
/// one form want two of them. It is the only name a field has, so it is drawn
/// whenever the field is empty, whether or not the caret is in it: a field
/// typed into and emptied again, or one the form put the caret in without
/// being asked, would otherwise be a blank box with nothing on screen to say
/// what belongs in it.
pub(super) fn singleline(
    ui: &mut Ui,
    value: &mut Option<String>,
    placeholer: &str,
    reserved: f32,
    waiting: bool,
) -> Response {
    // Named rather than left to the running count, so that whether the field
    // is being typed into can be asked before it is drawn. What it draws
    // depends on the answer.
    let id = ui.id().with(("field", placeholer));
    let editing = ui.memory(|memory| memory.has_focus(id));

    // Whether what it wants stands in the field as its contents. It does
    // while the caret is elsewhere, so a field holding nothing is a field
    // holding those words. Under the caret they are the hint instead, since
    // contents there are about to be typed into.
    //
    // Read off what the field holds and who is typing, rather than kept up as
    // the focus comes and goes. A field is drawn every frame and the focus
    // moves between two of them in one, so a placeholder put back by the
    // moment of losing it is a placeholder that stays away whenever that
    // moment is not seen. Nothing typed is nothing typed, and an empty string
    // is nothing typed however the field came to hold one.
    let wanting = typed(value).is_none() && !editing;
    let mut text = if wanting {
        placeholer.to_owned()
    } else {
        value.clone().unwrap_or_default()
    };
    // Room for the spinner, inside whatever the caller has already kept for
    // itself, so the two stand side by side rather than one over the other.
    let turning = if waiting {
        ui.text_style_height(&egui::TextStyle::Body) * SPINNER
    } else {
        0.
    };
    let gap = ui.spacing().item_spacing.x;
    let kept = reserved + if waiting { turning + gap } else { 0. };
    let margin = egui::Margin {
        right: FIELD_PADDING.right + kept as i8,
        ..FIELD_PADDING
    };

    // In a scope of its own, since a style set on a `Ui` is set on the rest
    // of that `Ui`: the grey a field wants for what it is holding would go
    // on to grey the headings under it.
    let response = ui
        .scope(|ui| {
            ui.visuals_mut().widgets.inactive.bg_stroke = FIELD_BORDER;
            if wanting {
                ui.visuals_mut().override_text_color =
                    Some(egui::Color32::GRAY);
            }
            ui.add_sized(
                egui::vec2(ui.available_width(), 0.),
                // The hint answers the field being empty with the caret in
                // it, which is the one empty state `wanting` does not cover:
                // the words cannot stand in the field as its contents there,
                // since the caret is about to be typed into them.
                egui::TextEdit::singleline(&mut text)
                    .id(id)
                    .margin(margin)
                    .hint_text(placeholer),
            )
        })
        .inner;

    // The words the field was standing there wanting are not words anybody
    // typed, so a field showing them holds nothing whatever is in the box.
    if !wanting {
        *value = Some(text);
    }

    // Inside the field's own right hand end, where the room was kept, and
    // clear of whatever the caller keeps room for out beyond it. A question
    // is answered under the field it was typed into, so where it is coming
    // from is said in the field itself rather than off in a corner.
    if waiting {
        let rect = response.rect;
        let at = egui::Rect::from_center_size(
            egui::pos2(
                rect.right() - FIELD_PADDING.right as f32 - reserved + gap / 2.
                    - turning / 2.,
                rect.center().y,
            ),
            egui::Vec2::splat(turning),
        );
        egui::Spinner::new().paint_at(ui, at);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::words;
    use crate::ui::testing::{SLACK, clicking};

    /// The box holding the value is the width kept for it
    ///
    /// Every one of them the same, so that a column of sliders ends in a
    /// column of numbers rather than in a ragged edge.
    #[test]
    fn the_value_box_is_the_width_kept_for_it() {
        let ctx = crate::testing::context();
        let mut value = 10_f32;
        let mut width = 0.;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            width =
                value_box(ui, egui::DragValue::new(&mut value)).rect.width();
        });

        assert!((width - VALUE_WIDTH).abs() < SLACK);
    }

    /// A field holding an empty string says what it wants
    ///
    /// Which is the whole of how the placeholder comes back. A field is
    /// clicked into and clicked straight out of again, and the focus moves
    /// between two fields within one frame, so anything that waited to be
    /// told the field had lost the focus would be waiting for a moment the
    /// field is not always drawn to see. Nothing typed is nothing typed,
    /// however the field came to hold an empty string.
    #[test]
    fn a_field_holding_nothing_says_what_it_wants() {
        let mut value = Some(String::new());

        let said = words(|ui| {
            singleline(ui, &mut value, "Search", 0., false);
        });

        assert!(said.contains(&"Search".to_owned()), "{said:?}");
    }

    /// And one holding a name says the name
    #[test]
    fn a_field_holding_a_name_says_the_name() {
        let mut value = Some("SOL".to_owned());

        let said = words(|ui| {
            singleline(ui, &mut value, "Search", 0., false);
        });

        assert!(said.contains(&"SOL".to_owned()), "{said:?}");
        assert!(!said.contains(&"Search".to_owned()), "{said:?}");
    }

    /// A field at rest and the same field with the caret in it
    ///
    /// Answers what each pass painted and what the field was left holding.
    /// Clicked into rather than focused by hand, since where the caret lands
    /// is what settles which of the two the field is drawing.
    fn field_clicked_into() -> (Vec<String>, Vec<String>, Option<String>) {
        let ctx = crate::testing::context();
        let mut value: Option<String> = None;

        let fields = |input, value: &mut Option<String>| {
            let mut at = egui::Rect::NOTHING;
            let output = ctx.run_ui(input, |ui| {
                at = singleline(ui, value, "Search", 0., false).rect;
            });
            let mut said = Vec::new();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    said.push(text.galley.text().to_owned());
                }
            }
            (at, said)
        };

        // Two passes with nothing happening, to place the field.
        let _ = fields(egui::RawInput::default(), &mut value);
        let (at, resting) = fields(egui::RawInput::default(), &mut value);

        fields(clicking(at.center()), &mut value);
        let (_, editing) = fields(egui::RawInput::default(), &mut value);

        (resting, editing, value)
    }

    /// A field says what it wants whether or not the caret is in it
    ///
    /// The words are the only name it has. A form that puts the caret in a
    /// field the user did not click would otherwise hand them a blank box
    /// with nothing on screen to say what belongs in it.
    #[test]
    fn a_field_says_what_it_wants_either_way() {
        let (resting, editing, _) = field_clicked_into();

        assert!(resting.contains(&"Search".to_owned()), "{resting:?}");
        assert!(editing.contains(&"Search".to_owned()), "{editing:?}");
    }

    /// And holds none of them, however they were drawn
    ///
    /// The words a field stands there wanting are not words anybody typed, so
    /// a field showing them holds nothing. Were they its contents while it was
    /// being typed into, the first keystroke would land on the end of them.
    #[test]
    fn a_field_never_holds_what_it_only_wants() {
        let (_, _, value) = field_clicked_into();

        assert_eq!(typed(&value), None, "{value:?}");
    }
}
