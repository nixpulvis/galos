//! The stores a served directory is made of, each read and written here.
//!
//! [`cells`] is the payload files, one a cell. [`bodies`] is every system's
//! insides, packed a shard to a file. [`names`] is the names table, mapped
//! and searched. [`sidecars`] holds the metadata tables open across a run,
//! each a [`tables::Keyed`] table, beside the ones dependents contribute.

pub mod bodies;
pub mod cells;
pub mod names;
pub mod sidecars;
pub mod tables;

use std::path::Path;

/// A served index directory, as the stores in it are read and written
///
/// The directory is the store: what each module here keeps is files under
/// it, and every operation on them starts from where it is. So each store
/// adds its own operations to this, in an `impl` of its own module —
/// [`bodies`], [`cells`], [`names`] and [`tables`] — rather than taking a
/// path of its own.
///
/// Borrowed and `Copy`: a caller keeps the path it was given, and this is
/// that path with the stores' operations on it.
#[derive(Clone, Copy, Debug)]
pub struct Directory<'a> {
    root: &'a Path,
}

impl<'a> Directory<'a> {
    /// The directory at `root`.
    pub fn at(root: &'a Path) -> Directory<'a> {
        Directory { root }
    }

    /// Where it is.
    pub fn root(self) -> &'a Path {
        self.root
    }
}
