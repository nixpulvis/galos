//! The galaxy's spatial index: the on-disk cell tree and the reader over it.
//!
//! One sparse adaptive octree stands over every system, and three walks read
//! it: the level of detail, the sky's discrete stars, and the glow behind
//! both. The builder writes it beside the database; the client draws the
//! galaxy from it with no database at all. This crate is the format both agree
//! on and the machinery that reads it back.
//!
//! It rests on one thing above all: the aggregates a cell carries must compose
//! exactly, so that a region drawn coarse and the same region drawn fine
//! integrate to the same totals and a cross-fade between them cannot pump
//! brightness or lose a star. That is [`moments`], and it is built and tested
//! first because everything else leans on it.
//!
//! Pure and dependency-light on purpose. Physics is [`galos_photometry`];
//! nothing here knows the database or how the galaxy is drawn.

pub mod moments;
pub mod aggregate;
pub mod geometry;
pub mod walk;
pub mod cache;
pub mod serialization;
pub mod store;
pub mod tree;

// The core API, re-exported at the crate root so a caller writes
// `galos_index::Tree` rather than reaching through the modules. The modules
// stay public for everything past the core.
pub use aggregate::{Aggregate, Cell};
pub use cache::{Point, Resident, ResidentCell};
pub use geometry::{Aabb, CellId};
pub use moments::Moments;
pub use tree::{BuildParams, Snapshot, Dirtied, System, Tree};
pub use walk::{Index, Mode, Needed, View};
pub use serialization::{Codec, Decode, Encode, FixedCodec};
