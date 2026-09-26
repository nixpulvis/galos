//! A built tree on disk: one index file and a payload file per cell.
//!
//! The index is a single small file, a few megabytes over a galaxy, rewritten
//! whole. Each cell that owns any systems is its own payload file, named by
//! level and Morton key, so it is found without the index and a rebuild
//! rewrites only the cells that changed.
//!
//! The byte formats are [`crate::format::payload`]; this is only where they
//! meet the filesystem. A client fetching cells over HTTP reads the same
//! bytes through its own transport, so this is the builder's writer and the
//! tests' reader.

use crate::core::geometry::CellId;
use crate::core::record::Point;
use crate::format::layout::{
    INDEX_FILE, PAYLOAD_DIR, legacy_payload_path, payload_path,
};
use crate::format::payload::{INDEX_VERSION, index_version};
use std::fs;
use std::io;
use std::path::Path;

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

/// Remove the payloads of cells the published tree does not name.
///
/// A whole-directory build writes its own cells and knows nothing of the tree
/// that stood before it, so every cell the old tree had and the new one does
/// not is left behind: 200,248 files and 4.9 GB of them, measured on a
/// directory rebuilt from the database over one built from a dump. The live
/// path has no such debt — a publish deletes what
/// [`Snapshot::write_diff`](crate::Snapshot::write_diff) is told went — and the
/// names table already retires its stale generations. This is the same sweep
/// for the cells.
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
/// `|id| index.get(id).is_some()` over the [`Index`](crate::Index) just
/// written. `apply` false counts and removes nothing, which is what `galos
/// index sweep` reports before it is asked to act.
pub fn sweep_payloads(
    dir: &Path,
    named: &dyn Fn(CellId) -> bool,
    apply: bool,
) -> io::Result<Swept> {
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
    let (x, y, z) = crate::core::geometry::morton_decode(morton);
    Some(CellId { level, x, y, z })
}

/// Write one cell's payload, opening its shard directory the first time
/// anything lands there.
///
/// Beside the file and renamed over it, as
/// [`crate::format::msgpack::write_meta`] and the names table's generations
/// are. Not for the torn-write reason those have — a payload carries no header
/// and a short read drops its last record, which a reader already tolerates —
/// but because a payload is **mapped**. `fs::write` truncates and rewrites in
/// place, so a feed republishing a cell under a reader's mapping would give it
/// torn bytes, and the truncation itself is a `SIGBUS` on the pages a reader
/// still holds. A rename leaves the old inode alone for as long as anything has
/// it open, which is the same guarantee a names generation gives.
pub(crate) fn write_payload(
    dir: &Path,
    id: CellId,
    bytes: Vec<u8>,
) -> io::Result<()> {
    let path = payload_path(dir, id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, &path)
}

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
pub fn stale(dir: &Path) -> Option<u16> {
    let bytes = fs::read(dir.join(INDEX_FILE)).ok()?;
    index_version(&bytes).filter(|found| *found != INDEX_VERSION)
}

/// One cell's payload, mapped rather than decoded.
///
/// [`Index::read_payload`](crate::Index::read_payload) reads the file and
/// decodes a [`Point`] per record into a `Vec`, which is right for drawing —
/// the map wants owned points to build entities from — and wrong for anything
/// that asks repeatedly. The router asks per expansion, half a million times a
/// route, and the LOD walk asks for 152 M points in a zoom and pays 24 s and
/// 6.1 GB of `Vec` for it.
///
/// So: the bytes where they lie, and a field read out of them when asked.
/// Nothing here is aligned to anything, so every read is `from_le_bytes`
/// over a slice — which is what makes an odd width free rather than costly.
///
/// **The columns are the point of the layout.** A position is six bytes and
/// a star kind is one, laid in runs of their own, so an expansion that
/// measures every system in a cell walks 6 bytes a row and touches the
/// magnitude, the temperature and the moment not at all. See
/// [`crate::format::payload::payload_bytes`].
pub struct Payload {
    map: memmap2::Mmap,
    count: usize,
    /// How wide one axis of a position is, 2 bytes or 4; see
    /// [`crate::format::payload::position_width`].
    width: usize,
    /// The cell's low corner, which a position is counted from.
    origin: [f64; 3],
    /// Where the kind column starts.
    kinds: usize,
    /// Where the address column starts.
    ids: usize,
    /// Where the photometry column starts.
    lit: usize,
}

impl Payload {
    /// Map a cell's payload, or [`None`] where the cell owns nothing and so
    /// has no file — or where what stands there is not a payload of this
    /// layout, which is what a directory built before the columns is.
    ///
    /// The sharded path first and the flat one after it, as
    /// [`Index::read_payload`](crate::Index::read_payload) does, so a directory
    /// part way through a reshard answers with what it has.
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
        if len < crate::format::payload::PAYLOAD_HEADER {
            return Ok(None);
        }
        // SAFETY: a payload is written beside its path and renamed over it
        // (`write_payload`), so the bytes under a mapping are never
        // rewritten and the file is never truncated while mapped — a
        // republished cell is a new inode and this one lives as long as the
        // mapping does.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        let Some(held) = crate::format::payload::payload_head(&map) else {
            return Ok(None);
        };
        if len < crate::format::payload::payload_len(held.count, held.width) {
            return Ok(None);
        }
        Ok(Some(Payload {
            map,
            count: held.count,
            width: held.width as usize,
            origin: id.min_ly(),
            kinds: held.kinds,
            ids: held.ids,
            lit: held.lit,
        }))
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
        let from = self.ids + at * 8;
        u64::from_le_bytes(self.map[from..from + 8].try_into().unwrap())
    }

    /// Where the `at`th system sits, in light years.
    ///
    /// Counted out from the cell's own corner on the galaxy's
    /// thirty-second-of-a-light-year grid, which is exact for every position
    /// the game states: see
    /// [`POSITION_STEP`](crate::format::payload::POSITION_STEP).
    pub fn position_at(&self, at: usize) -> [f64; 3] {
        let from = crate::format::payload::PAYLOAD_HEADER + at * 3 * self.width;
        let axis = |n: usize| {
            let from = from + n * self.width;
            let counts = match self.width {
                2 => u16::from_le_bytes(
                    self.map[from..from + 2].try_into().unwrap(),
                ) as f64,
                _ => u32::from_le_bytes(
                    self.map[from..from + 4].try_into().unwrap(),
                ) as f64,
            };
            self.origin[n] + counts * crate::format::payload::POSITION_STEP
        };
        [axis(0), axis(1), axis(2)]
    }

    /// What kind of star the `at`th system arrives at
    ///
    /// One byte, which is why it is here: a route asks it of every system it
    /// expands, for whether a ship can refuel and whether it can
    /// supercharge. See [`crate::core::record::StarKind`].
    pub fn kind_at(&self, at: usize) -> crate::core::record::StarKind {
        crate::core::record::StarKind::from_code(self.map[self.kinds + at])
    }

    /// The photometry of the `at`th system: how bright, how hot, how lately
    /// heard from.
    ///
    /// A column of its own because the router never reads it and the
    /// drawing always does.
    pub fn lit_at(&self, at: usize) -> (f32, u8, u32) {
        let from = self.lit + at * 9;
        let bytes = &self.map[from..from + 9];
        (
            f32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            bytes[4],
            u32::from_le_bytes(bytes[5..9].try_into().unwrap()),
        )
    }

    /// The `at`th system as a drawable point, columns joined.
    ///
    /// One row out of five columns, which is the shape a draw wants and the
    /// shape the layout is deliberately not in: the router reads one column
    /// of millions of rows, and the map reads every column of a handful.
    pub fn point_at(&self, at: usize) -> Point {
        let (magnitude, temp_bucket, updated_at) = self.lit_at(at);
        Point {
            id64: self.id64_at(at),
            pos: self.position_at(at),
            magnitude,
            temp_bucket,
            updated_at,
            kind: self.kind_at(at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::snapshot::{BuildParams, Snapshot};
    use crate::core::record::System;
    use crate::read::index::Index;
    use std::path::PathBuf;
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
                        kind: crate::core::record::StarKind::G,
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

    /// A sweep removes the payloads of cells the index does not name, and
    /// only those
    ///
    /// What a whole-directory rebuild leaves behind: the tree that stood
    /// there before wrote payloads for cells the new one has no record of,
    /// and nothing ever removed them — 200,248 files and 4.9 GB of them on
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
        // one where a build before the sharding would have put it.
        let sharded = CellId { level: 11, x: 3, y: 4, z: 5 };
        let loose = CellId { level: 12, x: 6, y: 7, z: 8 };
        assert!(index.get(sharded).is_none() && index.get(loose).is_none());
        write_payload(&scratch.0, sharded, vec![0u8; 64]).unwrap();
        let flat = legacy_payload_path(&scratch.0, loose);
        fs::write(&flat, vec![0u8; 32]).unwrap();

        // Counted and left alone until it is asked.
        let looked =
            sweep_payloads(&scratch.0, &|id| index.get(id).is_some(), false)
                .unwrap();
        assert_eq!(looked.orphans, 2);
        assert_eq!(looked.bytes, 96);
        assert!(!looked.removed);
        assert!(payload_path(&scratch.0, sharded).exists());
        assert!(flat.exists());

        let swept =
            sweep_payloads(&scratch.0, &|id| index.get(id).is_some(), true)
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
            sweep_payloads(&scratch.0, &|id| index.get(id).is_some(), true)
                .unwrap()
                .orphans,
            0,
        );
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
                None => {
                    assert!(decoded.is_empty(), "{:?} maps as none", cell.id)
                }
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
    /// Its payloads are laid out another way and the alternative to
    /// refusing them is a galaxy of plausible nonsense — so the message has
    /// to carry both the version met and **the command that fixes it**,
    /// which is the whole reason that command is named for the job rather
    /// than for this version's layout.
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
        assert!(
            said.contains("galos index migrate"),
            "the remedy is not named: {said}",
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
            kind: crate::core::record::StarKind::G,
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
