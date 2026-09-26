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
pub(crate) mod search;
pub(crate) mod selection;
pub(crate) mod space;

use bevy::prelude::*;

/// Stand the map up: the frame's order, the read of the index, the scene and
/// its cameras, and everything drawn in it.
pub fn plugin(app: &mut App) {
    app.add_plugins(schedule::plugin);
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
