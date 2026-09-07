//! The transport seam: the one place the client asks where cells and metadata
//! come from.
//!
//! Cells and metadata share a transport at all times. Today that is the
//! filesystem, [`FsSource`] over a build directory; tomorrow it is one HTTP
//! impl over the same layout. The trait is what lets the client swap the whole
//! transport at once rather than half of it, so a filesystem index is never
//! paired with an HTTP metadata service or the other way about.
//!
//! It is async for that reason and no other: the FS reads are blocking and
//! their futures resolve at once, but an HTTP impl to come is genuinely async,
//! and a sync trait now would force every call site to change when it lands.
//! Boxed through `async_trait` so a client can hold `Arc<dyn Source>`
//! and pick
//! its transport at runtime.
//!
//! The path helpers are the file-layout contract the builder writes to and this
//! reads from, named once here so the two cannot drift.

use crate::cache::Point;
use crate::geometry::CellId;
use crate::meta::{
    Faction, NameEntry, PopulatedSystem, SystemBodies, SystemReach,
};
use crate::walk::Index;
use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io;
use std::path::{Path, PathBuf};

/// The populated-systems table, resident once and read for every colour.
pub const POPULATED_FILE: &str = "populated.bin";
/// The subdirectory the names table's chunk files live in.
pub const NAMES_DIR: &str = "names";
/// How far each scanned system reaches, resident once and read for every
/// system the map draws.
pub const REACHES_FILE: &str = "reaches.bin";
/// The faction id-to-name table, small and read whole.
pub const FACTIONS_FILE: &str = "factions.bin";
/// The subdirectory of per-system body files.
pub const BODIES_DIR: &str = "bodies";

/// The populated table's path within a build directory.
pub fn populated_path(dir: &Path) -> PathBuf {
    dir.join(POPULATED_FILE)
}

/// The names table's chunk directory within a build directory.
pub fn names_dir(dir: &Path) -> PathBuf {
    dir.join(NAMES_DIR)
}

/// One chunk of the names table, numbered from zero. The numbering is the
/// whole of the layout: a reader takes them in order until one is missing, so
/// the table needs no manifest kept in step with it.
pub fn names_chunk_path(dir: &Path, chunk: usize) -> PathBuf {
    names_dir(dir).join(format!("{chunk:05}.bin"))
}

/// The whole names table, every chunk of `dir` in order. The client's read;
/// the builder holds the same chunks open as a [`NameTable`](crate::NameTable).
pub fn read_names(dir: &Path) -> io::Result<Vec<NameEntry>> {
    Ok(crate::names::read_chunks(dir)?.concat())
}

/// The factions table's path within a build directory.
pub fn factions_path(dir: &Path) -> PathBuf {
    dir.join(FACTIONS_FILE)
}

/// The reaches table's path within a build directory.
pub fn reaches_path(dir: &Path) -> PathBuf {
    dir.join(REACHES_FILE)
}

/// A system's body file within a build directory, keyed by address.
pub fn bodies_path(dir: &Path, address: i64) -> PathBuf {
    dir.join(BODIES_DIR).join(format!("{address}.bin"))
}

/// Serialize a metadata value to a file, MessagePack-encoded. The builder's
/// writer half; the reader half is [`read_meta`]. Both name the format in one
/// place so a write and a read cannot disagree on it.
pub fn write_meta<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = rmp_serde::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, bytes)
}

/// Read a metadata value back from a file, MessagePack-decoded. The reader half
/// of [`write_meta`], and what a builder resuming onto a directory reads its own
/// published tables back through.
pub fn read_meta<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = std::fs::read(path)?;
    rmp_serde::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// What a served part was when it was read, for asking whether it has changed
///
/// Opaque: a client keeps the one it was handed and hands it back to ask
/// whether the part still reads the same. The filesystem answers with a
/// modification time and an HTTP transport would answer with a hashed
/// `ETag`; nothing outside a [`Source`] may read anything into the number
/// beyond "the same means unchanged".
pub type Stamp = u64;

/// One part of a served index, for [`Source::stamp`] to be asked about
///
/// The parts a client holds and would have to re-read, which is every file in
/// the layout but the body files: those are fetched when a click opens a
/// system and never held, so a stale one cannot be on screen.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// The cell aggregates, `index.bin`
    Index,
    /// One cell's payload
    Cell(CellId),
    /// The populated-systems table
    Populated,
    /// The reaches table
    Reaches,
    /// The factions table
    Factions,
    /// One chunk of the names table, numbered from zero
    ///
    /// Per chunk and not per table, because the table is a hundred megabytes
    /// and a publish moves one chunk of it: new systems land in the tail, and
    /// [`crate::NameTable::upsert`] leaves a chunk alone where nothing in it
    /// changed. A client re-reads the chunk that moved rather than the galaxy.
    NamesChunk(usize),
}

/// Where the client reads cells and metadata from. One transport for both.
#[async_trait]
pub trait Source: Send + Sync {
    /// The resident tree of cell aggregates the walks plan on.
    async fn index(&self) -> io::Result<Index>;

    /// One cell's payload: its systems, positions in light years. Empty
    /// where the cell owns nothing.
    async fn payload(&self, id: CellId) -> io::Result<Vec<Point>>;

    /// The populated-systems table, held resident for filtering and colour.
    async fn populated(&self) -> io::Result<Vec<PopulatedSystem>>;

    /// Every system's name and position: the search index and routing graph.
    async fn names(&self) -> io::Result<Vec<NameEntry>>;

    /// One chunk of the names table, numbered from zero, empty past the end
    ///
    /// What a refresh reads: a publish moves one chunk of a hundred-megabyte
    /// table, so a client that has the table already re-reads the chunk whose
    /// [`Stamp`] moved. The numbering has no gaps, so the first chunk that
    /// answers empty is the end of the table.
    async fn names_chunk(&self, chunk: usize) -> io::Result<Vec<NameEntry>>;

    /// The faction id-to-name table, read whole and cached by the caller.
    async fn factions(&self) -> io::Result<Vec<Faction>>;

    /// How far each scanned system reaches from its arrival star, held
    /// resident: the map sizes every system it draws by this and cannot wait
    /// on a fetch for it.
    async fn reaches(&self) -> io::Result<Vec<SystemReach>>;

    /// The bodies inside a system, fetched when a click opens it. Empty where
    /// the system has no scan on record.
    async fn bodies(&self, address: i64) -> io::Result<SystemBodies>;

    /// What `part` is now, or [`None`] where the transport cannot say
    ///
    /// The whole of how a client finds out that the index it holds has been
    /// republished. Everything resident is read once and then held — the
    /// aggregates, the payloads of the cells in view, the tables a colour and
    /// a name are read from — and a feed rewrites all of it underneath. A
    /// client keeps the stamp it was handed with each part and asks this
    /// before re-reading anything, so a still map with a quiet index reads
    /// nothing at all.
    ///
    /// Cheap by contract: a stat on the filesystem, a conditional request's
    /// worth of work over HTTP. A client asks about every part it holds on
    /// every poll, so an implementation that reads the part to answer would
    /// cost more than the refresh it is meant to avoid.
    ///
    /// [`None`] is "cannot say", not "unchanged": a transport with no cheap
    /// answer says so and leaves the client to re-read on its own cadence.
    /// A part that does not exist answers [`None`] as well — a cell with no
    /// payload file and a chunk past the end of the table are both nothing to
    /// hold a stamp for.
    async fn stamp(&self, part: Part) -> io::Result<Option<Stamp>>;
}

/// A [`Source`] over a build directory on the local filesystem.
///
/// The directory holds `index.bin`, a `cells/` subdirectory of payloads, and
/// the metadata files this reads beside them. The reads are blocking; a client
/// drives them off its own task pool.
pub struct FsSource {
    dir: PathBuf,
}

impl FsSource {
    /// A source over the build directory at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> FsSource {
        FsSource { dir: dir.into() }
    }
}

#[async_trait]
impl Source for FsSource {
    async fn index(&self) -> io::Result<Index> {
        Index::read(&self.dir)
    }

    async fn payload(&self, id: CellId) -> io::Result<Vec<Point>> {
        Index::read_payload(&self.dir, id)
    }

    async fn populated(&self) -> io::Result<Vec<PopulatedSystem>> {
        read_meta(&populated_path(&self.dir))
    }

    async fn names(&self) -> io::Result<Vec<NameEntry>> {
        read_names(&self.dir)
    }

    async fn names_chunk(&self, chunk: usize) -> io::Result<Vec<NameEntry>> {
        match read_meta(&names_chunk_path(&self.dir, chunk)) {
            Ok(entries) => Ok(entries),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    async fn factions(&self) -> io::Result<Vec<Faction>> {
        read_meta(&factions_path(&self.dir))
    }

    async fn reaches(&self) -> io::Result<Vec<SystemReach>> {
        read_meta(&reaches_path(&self.dir))
    }

    async fn bodies(&self, address: i64) -> io::Result<SystemBodies> {
        match std::fs::read(bodies_path(&self.dir, address)) {
            Ok(bytes) => rmp_serde::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Ok(SystemBodies::default())
            }
            Err(e) => Err(e),
        }
    }

    /// The file's modification time, in nanoseconds since the epoch.
    ///
    /// One `stat`, which is what makes it cheap enough to ask about every part
    /// the client holds on every poll. A missing file — a cell that owns
    /// nothing, a chunk past the end of the table — is [`None`] rather than an
    /// error, since a part that is not there is nothing to hold a stamp for.
    ///
    /// The builder writes the index whole and each changed payload whole, both
    /// in place, so a moved mtime is a republished part. A torn read is
    /// possible in principle and harmless in practice: the payload codec drops
    /// a partial trailing record and the index's own length check refuses a
    /// half-written file, so the worst a race costs is a refresh that reads
    /// nothing and comes round again on the next poll.
    async fn stamp(&self, part: Part) -> io::Result<Option<Stamp>> {
        let path = match part {
            Part::Index => self.dir.join(crate::store::INDEX_FILE),
            Part::Cell(id) => crate::store::payload_path(&self.dir, id),
            Part::Populated => populated_path(&self.dir),
            Part::Reaches => reaches_path(&self.dir),
            Part::Factions => factions_path(&self.dir),
            Part::NamesChunk(chunk) => names_chunk_path(&self.dir, chunk),
        };
        match std::fs::metadata(&path).and_then(|it| it.modified()) {
            Ok(at) => Ok(at
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|since| since.as_nanos() as Stamp)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use elite_journal::body::{AtmosphereType, BodyType};

    /// The metadata codec must read back the shared enums' `#[serde(untagged)]`
    /// `Unknown(String)` variants, which a non-self-describing format cannot and
    /// which every scanned body with an unfamiliar class or atmosphere carries.
    /// This is why the codec is MessagePack rather than postcard.
    #[test]
    fn untagged_enums_round_trip() {
        let cases = [
            AtmosphereType::Unknown("SomethingNew".into()),
            AtmosphereType::Oxygen,
            AtmosphereType::None,
        ];
        for want in cases {
            let bytes = rmp_serde::to_vec(&want).expect("encodes");
            let got: AtmosphereType =
                rmp_serde::from_slice(&bytes).expect("decodes");
            assert_eq!(want, got);
        }

        let want = BodyType::Unknown("Ringworld".into());
        let bytes = rmp_serde::to_vec(&want).expect("encodes");
        let got: BodyType = rmp_serde::from_slice(&bytes).expect("decodes");
        assert_eq!(want, got);
    }

    /// A stamp moves when a part is republished, and says nothing about a part
    /// that is not there
    ///
    /// The whole of how a client finds out its index is stale. It has to move
    /// on a rewrite — a client comparing equal stamps reads nothing and draws
    /// the sky as it was — and it has to be [`None`] for a missing part, since
    /// a cell with no payload file and a chunk past the end of the table are
    /// both nothing to hold.
    #[test]
    fn a_stamp_moves_when_a_part_is_republished() {
        pollster::block_on(a_stamp_moves());
    }

    async fn a_stamp_moves() {
        use super::{FsSource, Part, Source};
        use crate::geometry::CellId;
        use crate::tree::{BuildParams, System};

        let dir = std::env::temp_dir()
            .join(format!("galos_source_stamp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let system = |id: u64, at: f64| System {
            id64: id,
            position: [at, 900.0, 24400.0],
            absolute_magnitude: id as f64,
            temperature: 5000.0,
            age_bucket: 0,
            updated_at: 0,
        };

        let built = crate::tree::Snapshot::build(
            &[system(1, 0.0)],
            &BuildParams::default(),
        );
        built.write(&dir).expect("the build should write");
        let source = FsSource::new(&dir);

        let before =
            source.stamp(Part::Index).await.expect("a stat").expect("a stamp");

        // A cell nothing was written for, and a chunk past the end of a table
        // that was never written at all.
        let empty = CellId { level: 10, x: 1, y: 2, z: 3 };
        assert_eq!(
            source.stamp(Part::Cell(empty)).await.expect("a stat"),
            None
        );
        assert_eq!(
            source.stamp(Part::NamesChunk(0)).await.expect("a stat"),
            None
        );
        assert!(
            source.names_chunk(0).await.expect("a read").is_empty(),
            "a chunk past the end reads empty rather than failing"
        );

        // Republished with a second system: the index is rewritten whole, so
        // its stamp moves.
        std::thread::sleep(std::time::Duration::from_millis(10));
        crate::tree::Snapshot::build(
            &[system(1, 0.0), system(2, 40.0)],
            &BuildParams::default(),
        )
        .write(&dir)
        .expect("the rebuild should write");

        let after =
            source.stamp(Part::Index).await.expect("a stat").expect("a stamp");
        assert_ne!(before, after, "a republished index reads as changed");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
