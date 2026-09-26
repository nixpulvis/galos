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
//! brightness or lose a star. That is [`core::moments`], and it is built and
//! tested first because everything else leans on it.
//!
//! The modules are layers, each reading only the ones above it in this list:
//!
//! - [`core`]: the cube, the sums, the fixed-width records.
//! - [`records`]: the serde rows the client reads beside the cells.
//! - [`system`]: where things are inside one system.
//! - [`format`](mod@format): file names, byte layouts and the resume point.
//! - [`store`]: the stores a directory is made of, read and written.
//! - [`read`]: the client's walks over the resident index.
//! - [`build`]: raising the tree, whole or a region or an edit at a time.
//! - [`accumulate`]: events and reports folded into records.
//! - [`ops`]: what an operator does to a whole directory.
//!
//! Two inputs stand beside the format, and neither names a source.
//! [`build::cold`] takes records — a database's rows, a dump's lines — and
//! raises the whole tree at once. [`Galaxy`] takes events one at a time,
//! from a feed or from a commander's own files, and accumulates them into
//! [`System`] records and the metadata sidecars, keeping what a scan found in
//! [`accumulate::bodies`].
//!
//! Pure and dependency-light on purpose. Physics is [`galos_photometry`];
//! nothing here knows the database or how the galaxy is drawn.

pub mod accumulate;
pub mod build;
pub mod core;
pub mod format;
pub mod ops;
pub mod read;
pub mod records;
pub mod store;
pub mod system;

// The core API, re-exported at the crate root so a caller writes
// `galos_index::Tree` rather than reaching through the modules. Everything
// else is named by its module path, and by that one path only.
pub use crate::accumulate::galaxy::Galaxy;
pub use crate::accumulate::report::SystemReport;
pub use crate::build::snapshot::{BuildParams, Snapshot};
pub use crate::build::tree::Tree;
pub use crate::core::aggregate::Cell;
pub use crate::core::geometry::CellId;
pub use crate::core::moments::Moments;
pub use crate::core::name::SystemName;
pub use crate::core::record::{Boost, Point, StarKind, System};
pub use crate::format::lock::Lock;
pub use crate::format::payload::INDEX_VERSION;
pub use crate::read::index::Index;
pub use crate::read::sky::Sky;
pub use crate::read::source::{FsSource, Part, Source, Stamp};
pub use crate::read::walk::{Mode, Needed, View};
pub use crate::store::names::Names;
