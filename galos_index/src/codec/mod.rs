//! Everything on disk: every file a [`Directory`] is made of, one module a
//! kind of file, each owning its names' use, its bytes, its version, and its
//! read and write.
//!
//! [`layout`] names every file and directory, once, so a writer and a reader
//! cannot drift. [`bytes`] is the fixed-width byte traits and the `record!`
//! a record's layout is stated in. Then the files:
//!
//! - [`cells`]: the index file and a payload a cell.
//! - [`names`]: the names table, mapped and searched.
//! - [`bodies`]: every system's insides, packed a shard to a file.
//! - [`tables`]: the MessagePack tables beside the cells — the
//!   [`Sidecars`](tables::sidecars::Sidecars) the index keeps and the ones
//!   dependents contribute.
//!
//! And beside the served directory, never inside it, the writer's own:
//! [`checkpoint`] is the resume point it keeps, [`lock`] how one writer at a
//! time is kept to, and [`parts`] names what a build or a repair can write
//! alone. The rest is scratch a cold build writes and nothing serves.
//!
//! What the files hold is named elsewhere: the tree in [`crate::tree`], the
//! sums a cell carries in [`crate::core`], the rows in [`crate::records`].
//! None of those knows a byte.

pub mod bodies;
pub mod bytes;
pub mod cells;
pub mod checkpoint;
pub mod layout;
pub mod lock;
pub mod names;
pub mod parts;
pub(crate) mod rows;
pub(crate) mod spill;
pub mod tables;

use std::path::Path;

/// A served index directory, as the files in it are read and written
///
/// What each module here keeps is files under it, and every operation on
/// them starts from where it is. So each kind of file adds its own
/// operations to this, in an `impl` of its own module — [`bodies`],
/// [`cells`], [`names`] and [`tables`] — rather than taking a path of its
/// own.
///
/// Borrowed and `Copy`: a caller keeps the path it was given, and this is
/// that path with every file's operations on it.
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
