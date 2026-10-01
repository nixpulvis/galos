//! The cell tree as it is served: every [`Cell`](cell::Cell), the
//! [`CellSystem`](cell::CellSystem)s a cell holds, and the
//! [`Index`](index::Index) over them all.
//!
//! [`cell`] is one node of the tree and the record each system in its payload
//! is packed as; [`index`] is the whole tree held in memory for the walks; and
//! [`lights`] is every cell's light, which only the realistic view reads and
//! which is served beside the cells rather than in them.
//!
//! Nothing here is mutable: the tree a feed edits is [`crate::build::tree`],
//! which publishes into these. Nothing here knows a byte either: what these
//! are written as is [`crate::codec::cells`]'s.

pub mod cell;
pub mod index;
pub mod lights;
