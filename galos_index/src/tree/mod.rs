//! The cell tree as it is served: every cell, and the systems a cell holds.
//!
//! [`cell`] is one node of the tree and the record each system in its payload
//! is packed as; [`index`] is the whole tree held in memory for the walks.
//! Nothing here is mutable: the tree a feed edits is [`crate::build::tree`],
//! which publishes into these.

pub mod cell;
pub mod index;
