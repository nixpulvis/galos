//! The galaxy's spatial index: the on-disk cell tree and the reader over it.
//!
//! One sparse adaptive octree stands over every system, and three walks read
//! it: the level of detail, the sky's discrete stars, and the glow behind
//! both. The builder writes it beside the database; the map and the router read the
//! galaxy from it with no database at all. This crate is the format both agree
//! on and the machinery that reads it back.
//!
//! It rests on one thing above all: the aggregates a cell carries must compose
//! exactly, so that a region drawn coarse and the same region drawn fine
//! integrate to the same totals and a cross-fade between them cannot pump
//! brightness or lose a star. That is [`core::moments`], and everything else
//! leans on it.
//!
//! Two inputs stand beside the format, and neither names a source.
//! [`build::cold`] takes records — a database's rows, a dump's lines — and
//! raises the whole tree at once. [`Galaxy`](prelude::Galaxy) takes events
//! one at a time, from a feed or from a commander's own files, and
//! accumulates them into [`System`](prelude::System) records and the
//! metadata sidecars, keeping what a scan found in [`accumulate::bodies`].
//!
//! The core API is gathered in [`prelude`]; everything else is named by its
//! module path.
//!
//! Pure and dependency-light on purpose. Physics is [`galos_photometry`];
//! nothing here knows the database or how the galaxy is drawn.

pub mod accumulate;
pub mod build;
pub mod codec;
pub mod core;
pub mod ops;
pub mod prelude;
pub mod read;
pub mod records;
pub mod system;
pub mod tree;
