//! What everything else is built from: the cube ([`CellId`](geometry::CellId)),
//! the sums a cell carries ([`Moments`](moments::Moments)), a star's
//! [`StarKind`](star::StarKind), where a system [`standing`] stands in the
//! order a cell's slice is taken in, and a system's
//! [`SystemName`](name::SystemName).
//!
//! Nothing here reads a file, knows a table or spells a byte: what these are
//! written as is [`crate::codec`]'s. [`geometry`] is the cube and its
//! addresses, [`moments`] and [`aggregate`] are the sums that let a region
//! drawn coarse and drawn fine integrate to the same totals, [`photometry`]
//! is the light the realistic view alone asks about, served beside them, [`star`] is the
//! one thing a system carries about its arrival star, and [`name`] and
//! [`procedural`] are what a system is called — including the names the
//! galaxy's own arithmetic spells.

pub mod aggregate;
pub mod geometry;
pub mod moments;
pub mod name;
pub mod photometry;
pub mod procedural;
pub mod standing;
pub mod star;
