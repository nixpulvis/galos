//! A built tree on disk: one index file and a payload file per cell.
//!
//! The index is a single small file, a few megabytes over a galaxy, rewritten
//! whole. Each cell that owns any systems is its own payload file, named by
//! level and Morton key, so it is found without the index and a rebuild
//! rewrites only the cells that changed.
//!
//! [`format`](mod@format) is both files' bytes — the index file's header and fixed
//! [`Cell`](crate::tree::cell::Cell) records, a payload's columns, and the
//! version the index file is held to — and the tree they hold is
//! [`crate::tree::index::Index`]. `read` opens them — the index whole, a
//! payload decoded or mapped — and `write` puts them down and sweeps away
//! what the tree no longer names. A reader fetching cells over HTTP reads the
//! same bytes through its own transport.

use crate::codec::Directory;
use crate::codec::layout::INDEX_FILE;
use format::{INDEX_VERSION, index_version};
use std::fs;

pub mod format;
pub(crate) mod read;
pub(crate) mod write;

pub use read::Payload;
pub use write::Swept;

impl Directory<'_> {
    /// The format version a directory claims, where it is not the one this
    /// build reads
    ///
    /// [`None`] for a directory this build can read, or for one there is
    /// nothing of yet — a missing index file is a directory nothing has built,
    /// which is not the same as one built another way.
    ///
    /// **Asked before any migration touches the place.** The automatic
    /// migrations are content-blind — they move files into shards and fold
    /// chunks — so they would run happily over a directory whose payloads this
    /// build cannot read, and the refusal would come later, out of whatever
    /// asked for a cell. See [`crate::ops::migrate::migrate`] and
    /// [`crate::ops::upgrade`].
    pub fn stale_index(self) -> Option<u16> {
        let dir = self.root;
        let bytes = fs::read(dir.join(INDEX_FILE)).ok()?;
        index_version(&bytes).filter(|found| *found != INDEX_VERSION)
    }
}

#[cfg(test)]
pub(super) mod fixtures {
    use crate::system::System;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A scratch directory unique to this run, removed when the guard drops.
    pub(super) struct Scratch(pub(super) PathBuf);

    impl Scratch {
        pub(super) fn new() -> Scratch {
            static SEQ: AtomicU32 = AtomicU32::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "galos_index_store_{}_{}",
                std::process::id(),
                n
            ));
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A cube lattice of systems well inside the root cube, each a touch
    /// fainter than the last.
    pub(super) fn systems(n: usize) -> Vec<System> {
        let side = (n as f64).cbrt().ceil() as usize;
        let step = 80.0;
        let span = (side.saturating_sub(1)) as f64 * step;
        let base = [-span / 2.0, 900.0 - span / 2.0, 24400.0 - span / 2.0];
        let mut out = Vec::new();
        let mut id = 1u64;
        'lattice: for x in 0..side {
            for y in 0..side {
                for z in 0..side {
                    if out.len() >= n {
                        break 'lattice;
                    }
                    out.push(System {
                        id64: id,
                        position: [
                            base[0] + x as f64 * step,
                            base[1] + y as f64 * step,
                            base[2] + z as f64 * step,
                        ],
                        absolute_magnitude: id as f64 * 0.001 - 3.0,
                        temperature: 4000.0 + (id % 5000) as f64,
                        age_bucket: (id % 8) as u32,
                        // Each its own moment, so a payload that dropped the
                        // stamp or carried a neighbour's would show up in the
                        // round trip below.
                        updated_at: 1_700_000_000 + id as u32,
                        kind: crate::core::star::StarKind::G,
                    });
                    id += 1;
                }
            }
        }
        out
    }
}
