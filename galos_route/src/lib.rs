//! Routes across the galaxy an index directory holds.
//!
//! A* over the places the cell payloads carry ([`graph`]), with the jumps a
//! drive can make and what each costs; the highway planned over the stars a
//! drive can supercharge at ([`highway`]); and the order a set of
//! destinations is flown in when nobody said which ([`tour`]).
//!
//! Pure over the index: no renderer and no database. The map draws a search
//! while it runs from what [`graph::Frontier`] samples, and holds the
//! settings and the graph in resources of its own.

mod boosts;
pub mod graph;
pub mod highway;
pub mod tour;

#[cfg(test)]
mod perf;
#[cfg(test)]
mod testing;

pub use boosts::Boosts;
pub use graph::{Crossing, Drive, Jumps, Routing, Tuning, Weigh};
