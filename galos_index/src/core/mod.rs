//! What everything else is built from: the cube, the sums a cell carries,
//! the fixed-width records, and a system's name.
//!
//! Nothing here reads a file or knows a table. [`geometry`] is the cube and
//! its addresses, [`moments`] and [`aggregate`] are the sums that let a
//! region drawn coarse and drawn fine integrate to the same totals, [`codec`]
//! and [`record`] are the bytes those sums and the systems themselves are
//! written as, and [`name`] and [`procedural`] are what a system is called —
//! including the names the galaxy's own arithmetic spells.

pub mod aggregate;
pub mod codec;
pub mod geometry;
pub mod moments;
pub mod name;
pub mod procedural;
pub mod record;
