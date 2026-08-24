//! A built tree on disk: one index file and a payload file per cell.
//!
//! The layout is the serving model in miniature. The index is a single small
//! file, rewritten whole because it is a few megabytes over a galaxy and cheap
//! to replace. Each cell that owns any systems is its own payload file, named
//! by level and Morton key so it can be found without the index and so a
//! nightly rebuild rewrites only the cells that changed, the whole point of
//! keeping positions immutable and the churn clustered.
//!
//! The byte formats are [`crate::serialization`]; this is only where they meet
//! the filesystem. A client fetching cells over HTTP reads the same bytes
//! through its own transport, so nothing here is on the client's path; it is
//! the builder's writer and the tests' reader.

use crate::tree::{Snapshot, Dirtied};
use crate::cache::Point;
use crate::geometry::CellId;
use crate::serialization::{Decode, Encode};
use crate::walk::Index;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The index file's name within a build directory.
pub const INDEX_FILE: &str = "index.bin";

/// The subdirectory the per-cell payload files live in.
pub const PAYLOAD_DIR: &str = "cells";

/// The file a cell's payload lives in, named by level and Morton key so the
/// name is stable and a cell is found without consulting the index.
fn payload_path(dir: &Path, id: CellId) -> PathBuf {
    dir.join(PAYLOAD_DIR).join(format!("{:02}-{:016x}.bin", id.level, id.morton()))
}

impl Snapshot {
    /// Write the whole tree to a directory: the index file and one payload file
    /// per cell that owns any systems. Existing files are overwritten; a cell
    /// that has emptied is not cleaned up here, since a full write goes to a
    /// fresh directory and an incremental [`write_diff`](Self::write_diff) hands
    /// back exactly which files to touch.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir.join(PAYLOAD_DIR))?;
        fs::write(dir.join(INDEX_FILE), self.index.to_bytes())?;
        for (&id, points) in &self.payloads {
            if !points.is_empty() {
                fs::write(payload_path(dir, id), points.as_slice().to_bytes())?;
            }
        }
        Ok(())
    }

    /// Apply a diff to a directory already holding the previous tree: rewrite
    /// the index whole, write the changed cells, and delete the removed ones.
    /// The directory ends identical to a full [`write`](Self::write) of this
    /// tree, having touched only the cells whose systems moved.
    pub fn write_diff(&self, dir: &Path, dirtied: &Dirtied) -> io::Result<()> {
        fs::create_dir_all(dir.join(PAYLOAD_DIR))?;
        fs::write(dir.join(INDEX_FILE), self.index.to_bytes())?;
        for &id in &dirtied.changed {
            fs::write(payload_path(dir, id), self.payload(id).to_bytes())?;
        }
        for &id in &dirtied.removed {
            match fs::remove_file(payload_path(dir, id)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl Index {
    /// Read an index from a build directory.
    pub fn read(dir: &Path) -> io::Result<Index> {
        let bytes = fs::read(dir.join(INDEX_FILE))?;
        Index::from_bytes(&bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not an index file"))
    }

    /// Read one cell's payload from a build directory, empty when the cell owns
    /// nothing and so has no file. Positions are cell-relative to `id`.
    pub fn read_payload(dir: &Path, id: CellId) -> io::Result<Vec<Point>> {
        match fs::read(payload_path(dir, id)) {
            Ok(bytes) => Ok(Vec::<Point>::from_bytes(&bytes).unwrap_or_default()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{BuildParams, System};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A scratch directory unique to this run, removed when the guard drops.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            static SEQ: AtomicU32 = AtomicU32::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("galos_index_store_{}_{}", std::process::id(), n));
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A cube lattice of systems well inside the root cube, each a touch
    /// brighter than the last so the magnitude ordering is unambiguous.
    fn systems(n: usize) -> Vec<System> {
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
                        age_bucket: (id % 8) as usize,
                    });
                    id += 1;
                }
            }
        }
        out
    }

    /// A build written to disk and read back is the same index and the same
    /// payloads, cell for cell.
    #[test]
    fn a_build_round_trips_through_a_directory() {
        let scratch = Scratch::new();
        let built = Snapshot::build(&systems(9000), &BuildParams::default());
        built.write(&scratch.0).unwrap();

        let index = Index::read(&scratch.0).unwrap();
        assert_eq!(index.len(), built.index.len());
        for cell in built.index.cells() {
            assert_eq!(index.get(cell.id), Some(cell));
            let payload = Index::read_payload(&scratch.0, cell.id).unwrap();
            let want = built.payload(cell.id);
            assert_eq!(payload.len(), want.len());
            for (a, b) in payload.iter().zip(want) {
                assert_eq!(a.id64, b.id64);
                assert_eq!(a.pos, b.pos);
                assert_eq!(a.temp_bucket, b.temp_bucket);
            }
        }
    }

    /// A cell that owns nothing has no file, and asking for it reads back empty
    /// rather than erroring.
    #[test]
    fn an_absent_payload_reads_empty() {
        let scratch = Scratch::new();
        let built = Snapshot::build(&systems(10), &BuildParams::default());
        built.write(&scratch.0).unwrap();
        let deep = CellId { level: 10, x: 1, y: 2, z: 3 };
        assert!(Index::read_payload(&scratch.0, deep).unwrap().is_empty());
    }

    /// The directory read back holds exactly the built tree, cell for cell.
    fn assert_dir_matches(dir: &Path, built: &Snapshot) {
        let index = Index::read(dir).unwrap();
        assert_eq!(index.len(), built.index.len());
        for cell in built.index.cells() {
            assert_eq!(index.get(cell.id), Some(cell));
            // Compare through the same lossy codec both sides pass, so the
            // hundredth-magnitude rounding is not mistaken for a difference.
            let disk = Index::read_payload(dir, cell.id).unwrap();
            let bytes = built.payload(cell.id).to_bytes();
            let want = Vec::<Point>::from_bytes(&bytes).unwrap();
            assert_eq!(disk, want);
        }
    }

    /// Two build directories are byte-for-byte the same file set.
    fn assert_dirs_identical(a: &Path, b: &Path) {
        assert_eq!(
            fs::read(a.join(INDEX_FILE)).unwrap(),
            fs::read(b.join(INDEX_FILE)).unwrap(),
            "index files differ",
        );
        let names = |d: &Path| {
            let mut v: Vec<String> = fs::read_dir(d.join(PAYLOAD_DIR))
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            v.sort();
            v
        };
        let (na, nb) = (names(a), names(b));
        assert_eq!(na, nb, "payload file sets differ");
        for name in na {
            assert_eq!(
                fs::read(a.join(PAYLOAD_DIR).join(&name)).unwrap(),
                fs::read(b.join(PAYLOAD_DIR).join(&name)).unwrap(),
                "payload {name} differs",
            );
        }
    }

    /// A diff written over the previous build lands the directory exactly where
    /// a full write of the new tree would (the incremental publish is honest)
    /// while touching only a fraction of the cells.
    #[test]
    fn a_diff_write_equals_a_full_write() {
        let scratch = Scratch::new();
        let params = BuildParams::default();
        let mut s = systems(9000);
        let prev = Snapshot::build(&s, &params);
        prev.write(&scratch.0).unwrap();

        // The shapes churn takes: one system moved within the ordering, one
        // new faint system, one dropped.
        s[100].absolute_magnitude += 2.0;
        s.push(System {
            id64: 999_999,
            position: [40.0, 940.0, 24440.0],
            absolute_magnitude: 9.0,
            temperature: 3500.0,
            age_bucket: 0,
        });
        s.remove(0);

        let (next, dirtied) = prev.rebuild(&s, &params);
        next.write_diff(&scratch.0, &dirtied).unwrap();
        assert_dir_matches(&scratch.0, &next);

        let fresh = Scratch::new();
        next.write(&fresh.0).unwrap();
        assert_dirs_identical(&scratch.0, &fresh.0);

        let touched = dirtied.changed.len() + dirtied.removed.len();
        assert!(touched < next.index.len(), "diff touched the whole tree");
    }
}
