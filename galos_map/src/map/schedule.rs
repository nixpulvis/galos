//! The order the map's systems run in
//!
//! Most of the map is a pipeline. A search becomes database queries, queries
//! become stars, stars decide where the camera points, and everything drawn
//! is derived from where the camera ended up. Running those out of order
//! still works, it just does each step with the previous frame's answer.
//!
//! Bevy runs systems in an arbitrary order unless told otherwise, so the
//! stages are spelled out here as `MapSet` rather than left to chance.

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

pub fn plugin(app: &mut App) {
    app.configure_sets(
        Update,
        (
            MapSet::Search,
            MapSet::Fetch,
            MapSet::Populate,
            MapSet::Camera,
            MapSet::Present,
        )
            .chain()
            // Nothing the map does means anything until the index is read,
            // and every one of these reads a table the read delivers. One
            // gate over the whole pipeline rather than a condition per
            // system: what they have in common is exactly that they cannot
            // run without it. See [`crate::map::index::load`].
            .run_if(in_state(crate::map::index::load::Opening::Drawn)),
    );
    app.configure_sets(
        EguiPrimaryContextPass,
        (PaintSet::Map, PaintSet::Style, PaintSet::Ui).chain(),
    );
}

/// What is painted flat over the frame, in the order it stacks
///
/// The egui pass runs after `Update`, so none of this is in [`MapSet`]. The
/// map's own annotations go first and under everything — the readouts, the
/// rings, the names — then the lettering is set, then whatever chrome is
/// drawn over the map. The map puts its painters in [`PaintSet::Map`] without
/// knowing whether any chrome is there to follow them.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PaintSet {
    /// The map's annotations, pinned among themselves where they are
    /// registered
    Map,
    /// The face everything is lettered in; see [`crate::style`]
    Style,
    /// The chrome over the map, and anything else drawn over it
    Ui,
}

/// The stages of a frame, in the order they run
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MapSet {
    /// Turn what the user asked for into camera moves, despawns and spyglass
    /// changes
    Search,
    /// Start database queries and collect the ones that have finished
    Fetch,
    /// Create and destroy star entities
    Populate,
    /// Point the camera
    Camera,
    /// Size, place and show everything else, given where the camera is
    Present,
}
