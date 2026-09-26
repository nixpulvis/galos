//! The window the key bindings are read in
//!
//! Opened and shut from the keyboard, which is what it is about. The keys
//! themselves are answered in [`crate::ui::keys`].

use bevy_egui::egui;
use bevy_egui::egui::{Context, Ui};

/// Every key the map answers, and what each does
///
/// The same table the README carries, kept here so that the map says it too:
/// a binding nobody can find from inside the map is a binding for whoever
/// wrote it. Held to the README's table by
/// `the_pane_and_the_readme_list_the_same_keys`, so the two cannot drift.
///
/// Each is a key struck on its own, but for the four that want shift — the
/// three that put a question in the bar's box and the `?` that opens this
/// window — which is what [`crate::map::keys`] promises and the README says.
const BINDINGS: [(&str, &str); 16] = [
    ("W A S D", "Pan along the ruled plane"),
    ("Q E", "Pan down and up through it"),
    ("Z X", "Swing the camera round what it looks at"),
    ("C V", "Lower and raise it over the plane"),
    ("F R", "Zoom in and out"),
    ("Space", "Fly to what is picked out, one at a time"),
    ("H", "Go home: Sol, from where the map opened"),
    ("L", "Show or hide the labels"),
    ("O", "Show or hide the orbit lines"),
    ("G", "Show or hide the grid"),
    ("/ or Shift-S", "Search the box for a system"),
    ("Shift-F", "Ask the box for a faction to filter on"),
    ("Shift-R", "Ask the box for systems to route between"),
    ("Esc", "Put away the bindings, or everything the chrome has open"),
    ("F1 or ?", "Show or hide these bindings"),
    ("F3", "Show or hide the diagnostics window"),
];

/// The window the bindings are read in
///
/// A window rather than a panel: it is read against whatever the reader was
/// doing when they wanted it, and it is moved out of the way rather than
/// closed. Its own close mark as well as the key that opened it, a window
/// being the one thing on screen a reader already knows how to shut.
///
/// Not resizable and not collapsible. There is one thing in it, it is as wide
/// as the widest binding, and a reference rolled up into its title bar is a
/// reference nobody can read.
pub(super) fn keys_window(ctx: &Context, open: &mut bool) {
    egui::Window::new("Keys")
        .open(open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, keys_reference);
}

/// Say what the keys do
///
/// Two columns, the key set as a heading is and what it does in the ordinary
/// text, so the column of keys is what the eye runs down.
fn keys_reference(ui: &mut Ui) {
    egui::Grid::new("keys-reference").num_columns(2).show(ui, |ui| {
        for (key, does) in BINDINGS {
            ui.label(egui::RichText::new(key).strong());
            ui.label(egui::RichText::new(does).weak());
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::words;

    /// The keys the pane lists are the keys the README lists
    ///
    /// One table said in two places, and this is what keeps the two saying
    /// the same thing: a key added to one and not the other fails here.
    #[test]
    fn the_pane_and_the_readme_list_the_same_keys() {
        let readme = include_str!("../../README.md");
        let rows: Vec<&str> =
            readme.lines().filter(|line| line.starts_with("| `")).collect();

        assert_eq!(rows.len(), BINDINGS.len(), "{rows:?}");
        for ((key, _), row) in BINDINGS.iter().zip(rows) {
            // The README sets each key in its own backticks, `W` `A` `S` `D`
            // for the four that pan, where the pane says them in a run.
            let plain = row
                .trim_start_matches("| ")
                .split(" | ")
                .next()
                .unwrap_or_default()
                .replace('`', "");
            assert_eq!(*key, plain, "{row}");
        }
    }

    /// And the pane draws every one of them
    #[test]
    fn the_pane_says_what_the_keys_do() {
        let said = words(keys_reference);

        assert!(said.contains(&"H".to_owned()), "{said:?}");
        assert!(said.contains(&"Space".to_owned()), "{said:?}");
        assert_eq!(said.len(), BINDINGS.len() * 2);
    }
}
