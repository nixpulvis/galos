//! The order the map's systems run in
//!
//! Most of the map is a pipeline. A search becomes database queries, queries
//! become stars, stars decide where the camera points, and everything drawn
//! is derived from where the camera ended up. Running those out of order
//! still works, it just does each step with the previous frame's answer.
//!
//! Bevy runs systems in an arbitrary order unless told otherwise, so the
//! stages are spelled out here as [`MapSet`] rather than left to chance.

use bevy::prelude::*;

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
            .chain(),
    );
}

/// The stages of a frame, in the order they run
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapSet {
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
