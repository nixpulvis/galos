//! Raising the tree: at once, a region at a time, or an edit at a time.
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
