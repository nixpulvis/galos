//! Putting a built tree down: the index file, a payload per cell, the
//! changes a publish makes, and the sweep of what no cell names.

use crate::codec::Directory;
use crate::codec::bytes::Encode;
use crate::codec::cells::format::payload_bytes;
use crate::codec::layout::{
    INDEX_FILE, PAYLOAD_DIR, legacy_payload_path, payload_path,
};
use crate::core::geometry::CellId;
use crate::tree::cell::CellSystem;
use crate::tree::index::Index;
use std::fs;
use std::io;
use std::path::Path;

impl Index {
    /// Write the index file, and nothing else.
    ///
    /// Rewritten whole every time: the aggregates and the rank ranges, a few
    /// megabytes over today's galaxy, and some 73 MB at two hundred million
    /// systems — written entire on every publish, which is what it costs.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        fs::write(dir.join(INDEX_FILE), self.to_bytes())
    }
}

impl Directory<'_> {
    /// Write a payload file for every cell that owns any systems, and no index.
    ///
    /// Existing files are overwritten; a cell with no systems is left without
    /// one. See [`Snapshot::write`](crate::build::snapshot::Snapshot::write).
    pub(crate) fn write_payloads<'a>(
        self,
        payloads: impl IntoIterator<Item = (CellId, &'a [CellSystem])>,
    ) -> io::Result<()> {
        let dir = self.root;
        fs::create_dir_all(dir.join(PAYLOAD_DIR))?;
        for (id, points) in payloads {
            if !points.is_empty() {
                Directory::at(dir)
                    .write_payload(id, payload_bytes(id, points))?;
            }
        }
        Ok(())
    }

    /// Publish a change: the index whole, the `changed` cells' payloads, and the
    /// `removed` cells' files deleted, in both layouts.
    ///
    /// The directory ends identical to a full write of the same tree. See
    /// [`Snapshot::write_diff`](crate::build::snapshot::Snapshot::write_diff).
    pub(crate) fn write_cell_changes<'a>(
        self,
        index: &Index,
        changed: impl IntoIterator<Item = (CellId, &'a [CellSystem])>,
        removed: impl IntoIterator<Item = CellId>,
    ) -> io::Result<()> {
        let dir = self.root;
        fs::create_dir_all(dir.join(PAYLOAD_DIR))?;
        fs::write(dir.join(INDEX_FILE), index.to_bytes())?;
        for (id, points) in changed {
            self.write_payload(id, payload_bytes(id, points))?;
        }
        for id in removed {
            for path in [payload_path(dir, id), legacy_payload_path(dir, id)] {
                match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }

    /// Write one cell's payload, opening its shard directory the first time
    /// anything lands there.
    ///
    /// Beside the file and renamed over it, as
    /// [`crate::codec::tables::msgpack::write_meta`] and the names table's generations
    /// are. Not for the torn-write reason those have — a payload's header states
    /// its count, and one shorter than that is refused as empty — but because a
    /// payload is **mapped**. `fs::write` truncates and rewrites in
    /// place, so a feed republishing a cell under a reader's mapping would give it
    /// torn bytes, and the truncation itself is a `SIGBUS` on the pages a reader
    /// still holds. A rename leaves the old inode alone for as long as anything has
    /// it open, which is the same guarantee a names generation gives.
    pub(crate) fn write_payload(
        self,
        id: CellId,
        bytes: Vec<u8>,
    ) -> io::Result<()> {
        let dir = self.root;
        let path = payload_path(dir, id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &path)
    }
}

/// What a sweep of the payload directory found.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// Payload files no cell of the index names.
    pub orphans: usize,
    /// What those files hold, in bytes.
    pub bytes: u64,
    /// Whether they were removed rather than only counted.
    pub removed: bool,
}

impl Directory<'_> {
    /// Remove the payloads of cells the published tree does not name.
    ///
    /// A whole-directory build writes its own cells and knows nothing of the tree
    /// that stood before it, so every cell the old tree had and the new one does
    /// not is left behind. The live path has no such debt — a publish deletes what
    /// [`Snapshot::write_diff`](crate::build::snapshot::Snapshot::write_diff)
    /// is told went — and the names table already retires its stale
    /// generations. This is the same sweep for the cells.
    ///
    /// **Call it only once the new index file stands.** An orphan is a file
    /// nothing refers to and a hole is a cell the tree names with no payload
    /// under it, so a sweep that runs early — or is cut short — must leave the
    /// first and never the second. Running after the index is written makes
    /// that so whatever happens: what is swept is exactly what the published
    /// tree does not name, and an interrupted sweep leaves a directory that is
    /// merely larger.
    ///
    /// Both layouts are considered: the sharded [`payload_path`] and the pre-shard
    /// [`legacy_payload_path`]. A loose file for a cell the tree *does* name is
    /// kept, being the payload a reader falls back to — bringing those forward is
    /// [`reshard_cells`](crate::ops::migrate::reshard_cells)'s work, and this must
    /// not stand in for it by deleting them.
    ///
    /// `named` answers whether the published tree names a cell — which is
    /// `|id| index.get(id).is_some()` over the [`Index`] just
    /// written. `apply` false counts and removes nothing, which is what `galos
    /// index sweep` reports before it is asked to act.
    pub fn sweep_payloads(
        self,
        named: &dyn Fn(CellId) -> bool,
        apply: bool,
    ) -> io::Result<Swept> {
        let dir = self.root;
        let root = dir.join(PAYLOAD_DIR);
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Swept::default());
            }
            Err(e) => return Err(e),
        };
        let mut swept = Swept { removed: apply, ..Swept::default() };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                for shard in fs::read_dir(entry.path())? {
                    orphan(&shard?, named, apply, &mut swept)?;
                }
            } else {
                orphan(&entry, named, apply, &mut swept)?;
            }
        }
        Ok(swept)
    }
}

/// One payload file weighed against the tree, and removed where the tree
/// does not name its cell.
fn orphan(
    entry: &fs::DirEntry,
    named: &dyn Fn(CellId) -> bool,
    apply: bool,
    swept: &mut Swept,
) -> io::Result<()> {
    let name = entry.file_name();
    let Some(name) = name.to_str() else { return Ok(()) };
    // Anything that is not a payload is somebody else's: a `.tmp` from a
    // write that did not finish is the writer's to replace, and a file this
    // cannot read the name of is not one to delete on a guess.
    let Some(id) = payload_cell(name) else { return Ok(()) };
    if named(id) {
        return Ok(());
    }
    swept.orphans += 1;
    swept.bytes += entry.metadata()?.len();
    if apply {
        match fs::remove_file(entry.path()) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// The cell a payload file is named for, sharded or loose: `LL-<morton>.bin`
/// in both layouts, so one reading serves them.
fn payload_cell(name: &str) -> Option<CellId> {
    let rest = name.strip_suffix(".bin")?;
    let (level, morton) = rest.split_once('-')?;
    let level: u8 = level.parse().ok()?;
    let morton = u64::from_str_radix(morton, 16).ok()?;
    Some(CellId::from_morton(level, morton))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::snapshot::{BuildParams, Snapshot};
    use crate::codec::cells::fixtures::{Scratch, systems};
    use crate::system::System;
    use std::path::PathBuf;

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
            assert_eq!(payload, built.payload(cell.id));
        }
    }

    /// A sweep removes the payloads of cells the index does not name, and
    /// only those
    ///
    /// What a whole-directory rebuild leaves behind: the tree that stood
    /// there before wrote payloads for cells the new one has no record of,
    /// and nothing else removes them — 200,248 files and 4.9 GB of them on
    /// a directory rebuilt from the database over one built from a dump.
    ///
    /// Both layouts are checked, because a directory that has not been
    /// resharded holds its payloads at [`legacy_payload_path`] and an
    /// orphan there weighs exactly as much.
    #[test]
    fn a_sweep_removes_the_payloads_no_cell_names() {
        let scratch = Scratch::new();
        let built = Snapshot::build(&systems(9000), &BuildParams::default());
        built.write(&scratch.0).unwrap();
        let index = Index::read(&scratch.0).unwrap();
        let live: Vec<CellId> = index.cells().map(|cell| cell.id).collect();

        // Two cells no tree here holds: one filed as a build files them,
        // one at the flat, unsharded path.
        let sharded = CellId { level: 11, x: 3, y: 4, z: 5 };
        let loose = CellId { level: 12, x: 6, y: 7, z: 8 };
        assert!(index.get(sharded).is_none() && index.get(loose).is_none());
        Directory::at(&scratch.0)
            .write_payload(sharded, vec![0u8; 64])
            .unwrap();
        let flat = legacy_payload_path(&scratch.0, loose);
        fs::write(&flat, vec![0u8; 32]).unwrap();

        // Counted and left alone until it is asked.
        let looked = Directory::at(&scratch.0)
            .sweep_payloads(&|id| index.get(id).is_some(), false)
            .unwrap();
        assert_eq!(looked.orphans, 2);
        assert_eq!(looked.bytes, 96);
        assert!(!looked.removed);
        assert!(payload_path(&scratch.0, sharded).exists());
        assert!(flat.exists());

        let swept = Directory::at(&scratch.0)
            .sweep_payloads(&|id| index.get(id).is_some(), true)
            .unwrap();
        assert_eq!(swept.orphans, 2);
        assert!(swept.removed);
        assert!(!payload_path(&scratch.0, sharded).exists());
        assert!(!flat.exists());

        // And every cell the index does name still reads what it held, which
        // is the half that matters: an orphan left behind costs bytes, a
        // payload swept by mistake costs the systems in it.
        for id in live {
            assert_eq!(
                Index::read_payload(&scratch.0, id).unwrap(),
                built.payload(id),
                "{id:?} lost its payload to the sweep",
            );
        }

        // Idempotent: a directory already swept has nothing left to find.
        assert_eq!(
            Directory::at(&scratch.0)
                .sweep_payloads(&|id| index.get(id).is_some(), true)
                .unwrap()
                .orphans,
            0,
        );
    }

    /// The directory read back holds exactly the built tree, cell for cell.
    fn assert_dir_matches(dir: &Path, built: &Snapshot) {
        let index = Index::read(dir).unwrap();
        assert_eq!(index.len(), built.index.len());
        for cell in built.index.cells() {
            assert_eq!(index.get(cell.id), Some(cell));
            let disk = Index::read_payload(dir, cell.id).unwrap();
            assert_eq!(disk, built.payload(cell.id));
        }
    }

    /// Two build directories are byte-for-byte the same file set.
    fn assert_dirs_identical(a: &Path, b: &Path) {
        assert_eq!(
            fs::read(a.join(INDEX_FILE)).unwrap(),
            fs::read(b.join(INDEX_FILE)).unwrap(),
            "index files differ",
        );
        // One level of shard directories, so the comparison is over the
        // payloads and not over how they are filed.
        let names = |d: &Path| {
            let mut v: Vec<PathBuf> = Vec::new();
            for shard in fs::read_dir(d.join(PAYLOAD_DIR)).unwrap() {
                let shard = shard.unwrap();
                if !shard.file_type().unwrap().is_dir() {
                    v.push(PathBuf::from(shard.file_name()));
                    continue;
                }
                for file in fs::read_dir(shard.path()).unwrap() {
                    v.push(
                        PathBuf::from(shard.file_name())
                            .join(file.unwrap().file_name()),
                    );
                }
            }
            v.sort();
            v
        };
        let (na, nb) = (names(a), names(b));
        assert_eq!(na, nb, "payload file sets differ");
        for name in na {
            assert_eq!(
                fs::read(a.join(PAYLOAD_DIR).join(&name)).unwrap(),
                fs::read(b.join(PAYLOAD_DIR).join(&name)).unwrap(),
                "payload {} differs",
                name.display(),
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
            updated_at: 1_800_000_000,
            kind: crate::core::star::StarKind::G,
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
