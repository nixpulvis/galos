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
//! Two inputs stand beside the format, and neither names a source.
//! [`cold`] takes records — a database's rows, a dump's lines — and raises
//! the whole tree at once. [`galaxy`] takes events one at a time, from a
//! feed or from a commander's own files, and accumulates them into
//! [`System`] records and the metadata sidecars, keeping what a scan found
//! in [`bodies`].
//!
//! Pure and dependency-light on purpose. Physics is [`galos_photometry`];
//! nothing here knows the database or how the galaxy is drawn.

pub mod aggregate;
pub mod bodies;
pub mod bucket;
pub mod cache;
pub mod checkpoint;
pub mod cold;
pub mod derive;
pub mod galaxy;
pub mod geometry;
pub mod inside;
pub mod lock;
pub mod merge;
pub mod meta;
pub mod moments;
pub mod names;
pub mod orbit;
pub mod region;
pub mod report;
pub mod serialization;
pub mod sidecars;
pub mod source;
pub mod spill;
pub mod store;
pub mod tree;
pub mod walk;

// The core API, re-exported at the crate root so a caller writes
// `galos_index::Tree` rather than reaching through the modules. The modules
// stay public for everything past the core.
pub use aggregate::{Aggregate, Cell};
pub use bodies::{Bodies, Kept, Published};
pub use cache::{Point, Resident, ResidentCell};
pub use checkpoint::{By, Checkpoint, Compaction, Pending};
pub use cold::{
    Abandoned, Build, Built, ColdReport, LeftOff, Start, Taking, left_off,
    region_budget, scratch,
};
pub use galaxy::Galaxy;
pub use geometry::{Aabb, CellId};
pub use inside::STAND_IN;
pub use lock::Lock;
pub use meta::{
    Barycenter, Body, Boost, Economies, Faction, NameEntry, Parent,
    PopulatedSystem, Star, Surface, SystemBodies, SystemBoost, SystemReach,
};
pub use moments::Moments;
pub use names::{Chunks, NameTable};
pub use orbit::{Orbit, Orbits, Spacing};
pub use report::SystemReport;
pub use serialization::{Codec, Decode, Encode, FixedCodec};
pub use sidecars::{Rows, Sidecars};
pub use source::{FsSource, Migrated, Part, Resharded, Source, Stamp, migrate};
pub use tree::{BuildParams, Dirtied, Snapshot, System, Tree};
pub use walk::{
    Index, MARK_SEPARATION_PX, Mode, Needed, STAR_SEPARATION_PX, SplatRef,
    View, resolvable_count,
};
