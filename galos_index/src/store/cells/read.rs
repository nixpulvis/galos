//! Reading a built tree back: the index file whole, and a cell's payload
//! decoded or mapped where it lies.

use crate::core::aggregate::TempBucket;
use crate::core::codec::Decode;
use crate::core::geometry::CellId;
use crate::format::layout::{INDEX_FILE, legacy_payload_path, payload_path};
use crate::format::payload::{INDEX_VERSION, index_version};
use crate::tree::cell::CellSystem;
use crate::tree::index::Index;
use std::fs;
use std::io;
use std::path::Path;

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
                     {INDEX_VERSION}: the payloads changed layout, so run \
                     `galos index migrate` over the directory"
                ),
                _ => "not an index file".to_string(),
            };
            io::Error::new(io::ErrorKind::InvalidData, what)
        })
    }

    /// Read one cell's payload from a build directory, empty when the cell owns
    /// nothing and so has no file. Positions are in light years.
    ///
    /// The sharded path first and the flat one after it, so a directory
    /// published before the sharding, or one being resharded as this reads,
    /// answers with what it has.
    pub fn read_payload(dir: &Path, id: CellId) -> io::Result<Vec<CellSystem>> {
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
        Ok(crate::format::payload::payload_points(id, &bytes)
            .unwrap_or_default())
    }

    /// The first `limit` systems of a cell's payload, brightest first.
    ///
    /// **What a draw asks for is a share of a cell and not the cell.** The
    /// payload is magnitude-ordered, the map draws the brightest few of it
    /// (`galos_map`'s `bounded::wanted`), and reading the rest is bytes
    /// faulted, decoded, held and never looked at: measured over
    /// `.index/full`, a flight that drew eight thousand marks read 121 M
    /// points and 5.8 GB to do it.
    ///
    /// Mapped rather than read, so the pages behind the rows nobody asked
    /// for are never touched. A directory written before the columnar
    /// layout has no head to map and falls back to the whole file, which is
    /// what it could always do.
    pub fn read_payload_prefix(
        dir: &Path,
        id: CellId,
        limit: usize,
    ) -> io::Result<Vec<CellSystem>> {
        let Some(payload) = Payload::open(dir, id)? else {
            let mut points = Index::read_payload(dir, id)?;
            points.truncate(limit);
            return Ok(points);
        };
        let take = limit.min(payload.len());
        Ok((0..take).map(|at| payload.point_at(at)).collect())
    }
}

/// One cell's payload, mapped rather than decoded.
///
/// [`Index::read_payload`](crate::Index::read_payload) reads the file and
/// decodes a [`CellSystem`] per record into a `Vec`, which is right for drawing —
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
    /// supercharge. See [`crate::core::star::StarKind`].
    pub fn kind_at(&self, at: usize) -> crate::core::star::StarKind {
        crate::core::star::StarKind::from_code(self.map[self.kinds + at])
    }

    /// The photometry of the `at`th system: how bright, how hot, how lately
    /// heard from.
    ///
    /// A column of its own because the router never reads it and the
    /// drawing always does.
    pub fn lit_at(&self, at: usize) -> (f32, TempBucket, u32) {
        let from = self.lit + at * 9;
        let bytes = &self.map[from..from + 9];
        (
            f32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            TempBucket::new(bytes[4]),
            u32::from_le_bytes(bytes[5..9].try_into().unwrap()),
        )
    }

    /// The `at`th system as a drawable point, columns joined.
    ///
    /// One row out of five columns, which is the shape a draw wants and the
    /// shape the layout is deliberately not in: the router reads one column
    /// of millions of rows, and the map reads every column of a handful.
    pub fn point_at(&self, at: usize) -> CellSystem {
        let (magnitude, temp_bucket, updated_at) = self.lit_at(at);
        CellSystem {
            id64: self.id64_at(at),
            position: self.position_at(at),
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
    use crate::store::cells::fixtures::{Scratch, systems};

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
                        assert_eq!(mapped.position_at(at), point.position);
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
}
