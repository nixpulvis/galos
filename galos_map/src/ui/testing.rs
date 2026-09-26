//! What more than one of the chrome's test modules draws with
//!
//! A helper only one module's tests use stays in that module's `tests`.

use crate::map::bodies::Contents;
use crate::map::filter::{Filter, Filters};
use crate::map::search::SearchResults;
use crate::map::search::tests::row;
use crate::map::selection::{Picked, PickedBody, Selection};
use crate::ui::bar::selection::selected;
use crate::ui::panels::Panels;
use bevy::math::DVec3;
use bevy_egui::egui;
use bevy_egui::egui::Ui;

/// A faction filter, by id, called after it
pub(super) fn faction(id: i32) -> Filter {
    Filter::Faction { id, name: format!("Faction {id}") }
}

/// Every word the pass painted, and where it was painted
///
/// [`words`] answers what was said and not where, and where is the whole
/// question when two things laid out from opposite ends share a line.
pub(super) fn placed(
    contents: impl FnOnce(&mut Ui),
) -> Vec<(String, egui::Rect)> {
    let ctx = crate::testing::context();
    let mut contents = Some(contents);
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        if let Some(contents) = contents.take() {
            contents(ui);
        }
    });

    fn walk(shape: &egui::Shape, into: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => into.push((
                text.galley.text().to_owned(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, into);
                }
            }
            _ => {}
        }
    }

    let mut found = Vec::new();
    for shape in &output.shapes {
        walk(&shape.shape, &mut found);
    }
    found
}

/// Where `word` was painted in `output`, if it was
pub(super) fn spoken_at(
    output: &egui::FullOutput,
    word: &str,
) -> Option<egui::Rect> {
    fn walk(shape: &egui::Shape, word: &str, found: &mut Option<egui::Rect>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == word => {
                *found = Some(egui::Rect::from_min_size(
                    text.pos,
                    text.galley.size(),
                ));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, word, found);
                }
            }
            _ => {}
        }
    }

    let mut found = None;
    for shape in &output.shapes {
        walk(&shape.shape, word, &mut found);
    }
    found
}

/// A results list holding `names`
pub(super) fn results(names: &[&str], _placed: bool) -> SearchResults {
    let found: Vec<_> = names.iter().map(|name| row(name)).collect();
    let mut results = SearchResults::default();
    results.set(found);
    results
}

/// Which system a name stands for
///
/// Derived from the name so that one name means one system across two
/// passes and two names never mean the same one. Numbering them by where
/// they sit in the list would give every first row the same system, and a
/// test for what happens when the system changes would never change it.
pub(super) fn address_of(name: &str) -> i64 {
    name.bytes().map(i64::from).sum()
}

/// A selection holding `names`
pub(super) fn holding(names: &[&str]) -> Selection {
    let mut selection = Selection::default();
    for name in names {
        selection.toggle(Picked::System(crate::map::galaxy::tests::named(
            address_of(name),
            name,
        )));
    }
    selection
}

/// A body called `name`, picked out `away` light years from the origin
pub(super) fn body(id: i16, name: &str, away: f64) -> Picked {
    Picked::Body(PickedBody::new(1, id, name, DVec3::new(away, 0., 0.)))
}

/// A selection holding a system at each of `places`, on the x axis
pub(super) fn strung_out(places: &[f64]) -> Selection {
    let mut selection = Selection::default();
    for (address, away) in places.iter().enumerate() {
        selection.toggle(Picked::System(crate::map::galaxy::tests::at(
            address as i64,
            *away,
        )));
    }
    selection
}

// Only the debug-only passes below use it: egui compiles its
// between-pass id check out of a release build. See
// [`crate::testing::between_passes`].
#[cfg(debug_assertions)]
/// Draw the selection rows holding `names`
pub(super) fn draw_selected<'a>(
    names: &'a [&'a str],
) -> impl FnMut(&mut Ui) + 'a {
    move |ui: &mut Ui| {
        let mut selection = holding(names);
        let mut panels = Panels::default();
        let mut filters = Filters::default();
        let mut travelled = None;
        selected(
            ui,
            &mut selection,
            &Contents::default(),
            None,
            &mut travelled,
            &mut panels,
            &mut filters,
            &mut 0,
        );
    }
}

/// A results list holding `names`, all of them placed
pub(super) fn results_of(names: &[&str]) -> SearchResults {
    results(names, true)
}

/// How far a measured width may sit from the one asked for
///
/// Egui rounds a rectangle to whole pixels, so two of them that agree can
/// still differ by a fraction of one.
pub(super) const SLACK: f32 = 1.;

/// A gesture at `from`, released `drift` further on
///
/// Egui reads a press and a release apart as a drag rather than a click,
/// and it is a click it lets go of the focus on. Zero drift is a click.
pub(super) fn dragged(from: egui::Pos2, drift: f32) -> Vec<egui::RawInput> {
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let to = egui::pos2(from.x + drift, from.y);
    let frame = |events| egui::RawInput { events, ..Default::default() };

    vec![
        frame(vec![egui::Event::PointerMoved(from), button(from, true)]),
        frame(vec![egui::Event::PointerMoved(to)]),
        frame(vec![button(to, false)]),
    ]
}

/// A primary click at `at`, and the pointer moved there to make it
pub(super) fn clicking(at: egui::Pos2) -> egui::RawInput {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };

    egui::RawInput {
        events: vec![
            egui::Event::PointerMoved(at),
            button(true),
            button(false),
        ],
        ..Default::default()
    }
}
