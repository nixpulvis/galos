//! A built tree on disk: one index file and a payload file per cell.
//!
//! The index is a single small file, a few megabytes over a galaxy, rewritten
//! whole. Each cell that owns any systems is its own payload file, named by
//! level and Morton key, so it is found without the index and a rebuild
//! rewrites only the cells that changed.
//!
//! The byte formats are [`crate::serialization`]; this is only where they
//! meet the filesystem. A client fetching cells over HTTP reads the same
//! bytes through its own transport, so this is the builder's writer and the
//! tests' reader.

use crate::cache::Point;
use crate::geometry::CellId;
use crate::serialization::{
    Decode, Encode, FixedCodec, INDEX_VERSION, index_version,
};
use crate::source::Resharded;
use crate::tree::{Dirtied, Snapshot};
use crate::walk::Index;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The index file's name within a build directory.
pub const INDEX_FILE: &str = "index.bin";

/// The subdirectory the per-cell payload files live in.
pub const PAYLOAD_DIR: &str = "cells";

/// The file a cell's payload lives in: sharded by the low bits of its Morton
/// key, then named by level and key, so the name is stable and a cell is
/// found without consulting the index.
///
/// Sharded as `bodies/` is: a cell holds at most [`LEAF_CAP`] systems, so
/// the file count grows with the galaxy and outgrows one directory. The
/// shard is the Morton key's low twelve bits, unmixed — a Morton key
/// interleaves the coordinates, so its low bits are position's fine
/// structure and already spread evenly.
///
/// [`LEAF_CAP`]: crate::tree::LEAF_CAP
pub(crate) fn payload_path(dir: &Path, id: CellId) -> PathBuf {
    let morton = id.morton();
    dir.join(PAYLOAD_DIR)
        .join(format!("{:03x}", morton & 0xfff))
        .join(format!("{:02}-{morton:016x}.bin", id.level))
}

/// Where a cell's payload was written before the sharding: `cells/` flat.
/// Read where the sharded path is absent, never written.
pub(crate) fn legacy_payload_path(dir: &Path, id: CellId) -> PathBuf {
    dir.join(PAYLOAD_DIR).join(format!(
        "{:02}-{:016x}.bin",
        id.level,
        id.morton()
    ))
}

/// Move every loose `cells/*.bin` into its shard, stopping where asked.
///
/// [`crate::source::reshard_bodies`]'s twin, and [`crate::migrate`] is the
/// pair: a one-time migration, idempotent, a rename each. A directory
/// already sharded costs one `readdir`.
///
/// `stop` is asked before each move, and abandoning is safe wherever it
/// lands: [`legacy_payload_path`] is read where the sharded path is absent,
/// so a half-migrated directory serves every cell a finished one does, and
/// the next open takes the rest. A payload already standing in its shard
/// wins over the loose one, for the reason [`crate::source::reshard_bodies`]
/// gives.
pub fn reshard_cells(
    dir: &Path,
    stop: &dyn Fn() -> bool,
) -> io::Result<Resharded> {
    let entries = match fs::read_dir(dir.join(PAYLOAD_DIR)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Resharded { moved: 0, finished: true });
        }
        Err(e) => return Err(e),
    };
    let mut moved = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_suffix(".bin") else { continue };
        let Some((_, morton)) = rest.split_once('-') else { continue };
        let Ok(morton) = u64::from_str_radix(morton, 16) else { continue };
        if !entry.file_type()?.is_file() {
            continue;
        }
        if stop() {
            return Ok(Resharded { moved, finished: false });
        }
        let to = dir
            .join(PAYLOAD_DIR)
            .join(format!("{:03x}", morton & 0xfff))
            .join(name);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::metadata(&to) {
            Ok(_) => fs::remove_file(entry.path())?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::rename(entry.path(), to)?;
            }
            Err(e) => return Err(e),
        }
        moved += 1;
    }
    Ok(Resharded { moved, finished: true })
}

impl Snapshot {
    /// Write the whole tree to a directory: the index file and one payload file
    /// per cell that owns any systems. Existing files are overwritten; a
    /// cell that has emptied is not cleaned up here — a full write goes to a
    /// fresh directory, and [`write_diff`](Self::write_diff) names the files
    /// an incremental publish must touch.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        self.write_payloads(dir)?;
        self.index.write(dir)
    }

    /// Write the payloads and no index file.
    ///
    /// What a regional build writes: its cells are only part of the
    /// galaxy's, so the index file belongs to whoever joins them — see
    /// [`crate::region`]. A whole build is this and then the index.
    pub fn write_payloads(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir.join(PAYLOAD_DIR))?;
        for (&id, points) in &self.payloads {
            if !points.is_empty() {
                write_payload(dir, id, points.as_slice().to_bytes())?;
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
            write_payload(dir, id, self.payload(id).to_bytes())?;
        }
        for &id in &dirtied.removed {
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
}

/// Write one cell's payload, opening its shard directory the first time
/// anything lands there.
///
/// Beside the file and renamed over it, as [`crate::source::write_meta`]
/// and the names table's generations are. Not for the torn-write reason
/// those have — a payload carries no header and a short read drops its
/// last record, which a reader already tolerates — but because a payload
/// is **mapped**. `fs::write` truncates and rewrites in place, so a feed
/// republishing a cell under a reader's mapping would give it torn bytes,
/// and the truncation itself is a `SIGBUS` on the pages a reader still
/// holds. A rename leaves the old inode alone for as long as anything has
/// it open, which is the same guarantee a names generation gives.
fn write_payload(dir: &Path, id: CellId, bytes: Vec<u8>) -> io::Result<()> {
    let path = payload_path(dir, id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, &path)
}

impl Index {
    /// Read an index from a build directory.
    ///
    /// A directory at another format version is refused here and nowhere
    /// else: its payloads carry no header, so reading it would decode every
    /// field of every system out of the wrong bytes. The error names the
    /// version met, and a rebuild is the fix — `bring_level` falls back to a
    /// full build when a resume cannot read what is there.
    pub fn read(dir: &Path) -> io::Result<Index> {
        let bytes = fs::read(dir.join(INDEX_FILE))?;
        Index::from_bytes(&bytes).ok_or_else(|| {
            let what = match index_version(&bytes) {
                Some(found) if found != INDEX_VERSION => format!(
                    "index format version {found}, this build reads \
                     {INDEX_VERSION}: the payload record changed width, so \
                     rebuild the directory"
                ),
                _ => "not an index file".to_string(),
            };
            io::Error::new(io::ErrorKind::InvalidData, what)
        })
    }

    /// Write the index file, and nothing else.
    ///
    /// Rewritten whole every time: the aggregates and the rank ranges, a few
    /// megabytes over today's galaxy. Writing it whole per publish is its own
    /// item in `TODO-scale.md`.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        fs::write(dir.join(INDEX_FILE), self.to_bytes())
    }

    /// Read one cell's payload from a build directory, empty when the cell owns
    /// nothing and so has no file. Positions are in light years.
    ///
    /// The sharded path first and the flat one after it, so a directory
    /// published before the sharding, or one being resharded as this reads,
    /// answers with what it has.
    pub fn read_payload(dir: &Path, id: CellId) -> io::Result<Vec<Point>> {
        let bytes = match fs::read(payload_path(dir, id)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                match fs::read(legacy_payload_path(dir, id)) {
                    Ok(bytes) => bytes,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        return Ok(Vec::new());
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };
        Ok(Vec::<Point>::from_bytes(&bytes).unwrap_or_default())
    }
}

/// One cell's payload, mapped rather than decoded.
///
/// [`Index::read_payload`] reads the file and decodes a [`Point`] per
/// record into a `Vec`, which is right for drawing — the map wants owned
/// points to build entities from — and wrong for anything that asks
/// repeatedly. The router asks per expansion, half a million times a
/// route, and the LOD walk asks for 152 M points in a zoom and pays 24 s
/// and 6.1 GB of `Vec` for it.
///
/// So: the bytes where they lie, and a field read out of them when asked.
/// A record is [`Point::LEN`] bytes and nothing here is aligned to
/// anything, so every read is `from_le_bytes` over a slice — which is what
/// makes the odd width free rather than costly.
pub struct Payload {
    map: memmap2::Mmap,
    count: usize,
}

impl Payload {
    /// Where a system's position sits within its record.
    const POS: usize = 8;

    /// Map a cell's payload, or [`None`] where the cell owns nothing and so
    /// has no file.
    ///
    /// The sharded path first and the flat one after it, as
    /// [`Index::read_payload`] does, so a directory part way through a
    /// reshard answers with what it has.
    pub fn open(dir: &Path, id: CellId) -> io::Result<Option<Payload>> {
        let file = match fs::File::open(payload_path(dir, id)) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                match fs::File::open(legacy_payload_path(dir, id)) {
                    Ok(file) => file,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        return Ok(None);
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };
        let len = file.metadata()?.len() as usize;
        if len < Point::LEN {
            return Ok(None);
        }
        // SAFETY: a payload is written beside its path and renamed over it
        // (`write_payload`), so the bytes under a mapping are never
        // rewritten and the file is never truncated while mapped — a
        // republished cell is a new inode and this one lives as long as the
        // mapping does.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        // A trailing part-record is dropped, which is what the decoding
        // reader does with one too.
        Ok(Some(Payload { count: len / Point::LEN, map }))
    }

    /// How many systems the cell owns.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it owns none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The address of the `at`th system.
    pub fn id64_at(&self, at: usize) -> u64 {
        let from = at * Point::LEN;
        u64::from_le_bytes(self.map[from..from + 8].try_into().unwrap())
    }

    /// Where the `at`th system sits, in light years.
    pub fn position_at(&self, at: usize) -> [f64; 3] {
        let from = at * Point::LEN + Payload::POS;
        let axis = |n: usize| {
            let from = from + n * 8;
            f64::from_le_bytes(self.map[from..from + 8].try_into().unwrap())
        };
        [axis(0), axis(1), axis(2)]
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
                        age_bucket: (id % 8) as u32,
                        // Each its own moment, so a payload that dropped the
                        // stamp or carried a neighbour's would show up in the
                        // round trip below.
                        updated_at: 1_700_000_000 + id as u32,
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
            assert_eq!(payload, built.payload(cell.id));
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

    /// A mapped payload answers what the decoding reader answers, record for
    /// record — and it is the same bytes, so nothing may disagree.
    #[test]
    fn a_mapped_payload_is_the_payload() {
        let scratch = Scratch::new();
        let built = Snapshot::build(&systems(9000), &BuildParams::default());
        built.write(&scratch.0).unwrap();

        let mut seen = 0usize;
        for cell in built.index.cells() {
            let decoded = Index::read_payload(&scratch.0, cell.id).unwrap();
            let mapped = Payload::open(&scratch.0, cell.id).unwrap();
            match mapped {
                None => assert!(decoded.is_empty(), "{:?} maps as none", cell.id),
                Some(mapped) => {
                    assert_eq!(mapped.len(), decoded.len(), "{:?}", cell.id);
                    for (at, point) in decoded.iter().enumerate() {
                        assert_eq!(mapped.id64_at(at), point.id64);
                        assert_eq!(mapped.position_at(at), point.pos);
                        seen += 1;
                    }
                }
            }
        }
        assert_eq!(seen, 9000, "every system was read through the mapping");
    }

    /// A cell that owns nothing maps as none rather than erroring.
    #[test]
    fn an_absent_payload_maps_as_none() {
        let scratch = Scratch::new();
        let built = Snapshot::build(&systems(10), &BuildParams::default());
        built.write(&scratch.0).unwrap();
        let deep = CellId { level: 10, x: 1, y: 2, z: 3 };
        assert!(Payload::open(&scratch.0, deep).unwrap().is_none());
    }

    /// A republished cell is a new file, so a mapping taken before it keeps
    /// reading what it was given.
    ///
    /// Which is the whole reason `write_payload` renames: the feed rewrites
    /// a cell as systems arrive, and a truncating write under a reader's
    /// mapping is torn bytes at best and `SIGBUS` at worst.
    #[test]
    fn a_republished_cell_leaves_a_mapping_alone() {
        let scratch = Scratch::new();
        let first = Snapshot::build(&systems(400), &BuildParams::default());
        first.write(&scratch.0).unwrap();
        let id = first
            .index
            .cells()
            .find(|cell| !first.payload(cell.id).is_empty())
            .expect("a cell that owns systems")
            .id;

        let held = Payload::open(&scratch.0, id).unwrap().expect("a payload");
        let before: Vec<u64> =
            (0..held.len()).map(|at| held.id64_at(at)).collect();

        // The same directory built again over twice the galaxy: this cell's
        // file is written afresh.
        Snapshot::build(&systems(4000), &BuildParams::default())
            .write(&scratch.0)
            .unwrap();

        let after: Vec<u64> =
            (0..held.len()).map(|at| held.id64_at(at)).collect();
        assert_eq!(before, after, "the mapping moved under its reader");
    }

    /// A directory written by an older codec is refused by name, not read.
    ///
    /// Its payloads are 39-byte records where this build reads 41, and a
    /// payload block carries nothing that could say so. The message has to
    /// carry the version met and the remedy, because the alternative is a
    /// galaxy of plausible nonsense.
    #[test]
    fn a_directory_at_an_older_version_is_refused_by_name() {
        let scratch = Scratch::new();
        let built = Snapshot::build(&systems(100), &BuildParams::default());
        built.write(&scratch.0).unwrap();

        let path = scratch.0.join(INDEX_FILE);
        let mut bytes = fs::read(&path).unwrap();
        bytes[4..6].copy_from_slice(&(INDEX_VERSION - 1).to_le_bytes());
        fs::write(&path, &bytes).unwrap();

        let err = Index::read(&scratch.0).expect_err("a stale index reads");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let said = err.to_string();
        assert!(
            said.contains(&format!("version {}", INDEX_VERSION - 1)),
            "the version met is not named: {said}"
        );
        assert!(said.contains("rebuild"), "no remedy named: {said}");
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
