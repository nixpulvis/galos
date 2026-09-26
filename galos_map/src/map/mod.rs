//! The map: the galaxy drawn from an index directory, and the camera that
//! flies it.
//!
//! Everything here stands on its own. What the chrome over it offers — the
//! settings pane, the bar, the panels, the clock — is [`crate::ui`], which
//! works the map through the resources and messages this module owns and is
//! never asked for anything in return. A map with no chrome at all runs, and
//! [`crate::ui`] is what a person drives it through.

pub(crate) mod bodies;
pub(crate) mod camera;
pub(crate) mod filter;
pub(crate) mod galaxy;
pub(crate) mod grid;
pub mod index;
pub(crate) mod keys;
pub(crate) mod labels;
pub(crate) mod paint;
pub(crate) mod pointing;
pub(crate) mod route;
pub(crate) mod ruled;
pub(crate) mod schedule;
pub(crate) mod screen;
pub(crate) mod search;
pub(crate) mod selection;
pub(crate) mod space;

use bevy::prelude::*;

/// Stand the map up: the frame's order, the read of the index, the scene and
/// its cameras, and everything drawn in it.
pub fn plugin(app: &mut App) {
    app.add_plugins(schedule::plugin);
    app.add_plugins(crate::input::plugin);
    app.add_plugins(crate::style::plugin);
    // Before the plugins it gates, so the state exists by the time their run
    // conditions are built against it.
    app.add_plugins(index::load::plugin);
    app.add_plugins(space::plugin);
    app.add_plugins(camera::plugin);
    app.add_plugins(galaxy::plugin);
    app.add_plugins(paint::plugin);
    app.add_plugins(bodies::plugin);
    app.add_plugins(labels::plugin);
    app.add_plugins(pointing::plugin);
    app.add_plugins(route::plugin);
    app.add_plugins(selection::plugin);
    app.add_plugins(filter::plugin);
    // After the galaxy, whose walk holds the payloads a refresh replaces and
    // the stamps it asks about.
    app.add_plugins(index::refresh::plugin);
    // After the bodies, whose descent into a star is what carries the ruled
    // plane from light years to light seconds.
    app.add_plugins(grid::plugin);
    app.add_plugins(search::plugin);
    app.add_plugins(keys::plugin);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every line of Rust under `dir` that is not a comment, by file
    fn code_under(dir: &Path, into: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("the map's sources") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                code_under(&path, into);
            } else if path.extension().is_some_and(|it| it == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source");
                for line in text.lines() {
                    if !line.trim_start().starts_with("//") {
                        into.push((path.display().to_string(), line.into()));
                    }
                }
            }
        }
    }

    /// The map asks nothing of the chrome over it
    ///
    /// The chrome works the map through what the map owns — its resources and
    /// its messages — and the map never reaches back. What they both read, who
    /// the pointer and the keyboard belong to and the face they are lettered
    /// in, stands beneath the two of them in `crate::input` and `crate::style`.
    /// A map that named `crate::ui` would be one that could not be drawn
    /// without it.
    #[test]
    fn the_map_reaches_nothing_of_the_chrome() {
        let map = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/map");
        let mut code = Vec::new();
        code_under(&map, &mut code);

        // Put together, so that this line is not itself a line naming it.
        let chrome = ["crate", "ui"].join("::");
        let reaching: Vec<_> =
            code.iter().filter(|(_, line)| line.contains(&chrome)).collect();
        assert!(reaching.is_empty(), "the map names the chrome: {reaching:#?}");
    }
}
