//! Raising the tree: at once as a [`Snapshot`](snapshot::Snapshot), a region
//! at a time with a [`Build`](cold::Build), or an edit at a time on a
//! [`Tree`](tree::Tree).
//!
//! [`snapshot`] is the batch build of a galaxy held whole and the write of a
//! built tree. [`cold`] raises the same tree from records without ever
//! holding the galaxy, cutting it into regions and spilling each to disk
//! first. [`tree`] is the tree held open, so an edit moves one system.

pub(crate) mod bucket;
pub mod cold;
pub(crate) mod region;
pub mod snapshot;
pub mod tree;
