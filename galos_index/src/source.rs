//! The transport seam: the one place the client asks where cells and metadata
//! come from.
//!
//! Cells and metadata share a transport at all times. Today that is the
//! filesystem, [`FsSource`] over a build directory; tomorrow one HTTP impl
//! over the same layout. The trait swaps the whole transport at once, so a
//! filesystem index is never paired with an HTTP metadata service.
//!
//! Async because the HTTP impl to come is; the FS reads are blocking and
//! their futures resolve at once. Boxed through `async_trait`, so a client
//! holds `Arc<dyn Source>` and picks its transport at runtime.
//!
//! The path helpers are the file-layout contract the builder writes to and
//! this reads from, named once here so the two cannot drift.

use crate::cache::Point;
use crate::geometry::CellId;
use crate::meta::{
    Faction, NameEntry, PopulatedSystem, SystemBodies, SystemBoost, SystemReach,
};
use crate::walk::Index;
use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io;
use std::path::{Path, PathBuf};

/// The populated-systems table, resident once and read for every color.
pub const POPULATED_FILE: &str = "populated.bin";
/// The subdirectory the names table's chunk files live in.
pub const NAMES_DIR: &str = "names";
/// How far each scanned system reaches, resident once and read for every
/// system the map draws.
pub const REACHES_FILE: &str = "reaches.bin";
/// The faction id-to-name table, small and read whole.
pub const FACTIONS_FILE: &str = "factions.bin";
/// Which systems can supercharge a drive, resident for the router.
pub const BOOSTS_FILE: &str = "boosts.bin";
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
/// whole of the layout: a reader takes them in order until one is missing,
/// so the table needs no manifest.
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

/// The supercharge table's path within a build directory.
pub fn boosts_path(dir: &Path) -> PathBuf {
    dir.join(BOOSTS_FILE)
}

/// A system's body file within a build directory, keyed by address.
///
/// Sharded over 4,096 subdirectories, `bodies/{shard:03x}/{address}.bin`,
/// one file per system being more than a flat directory holds at galaxy
/// scale. The shard is the top twelve bits of the address multiplied by the
/// 64-bit golden-ratio constant:
///
/// ```text
/// shard = (address as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 52
/// ```
///
/// The multiply mixes the high bits down: an Elite `id64` packs a mass code
/// and the boxel coordinates into its low bits, so `address % 4096` leaves
/// whole shards empty and piles the rest up. See `ARCHITECTURE.md`.
pub fn bodies_path(dir: &Path, address: i64) -> PathBuf {
    let shard = bodies_shard(address);
    dir.join(BODIES_DIR)
        .join(format!("{shard:03x}"))
        .join(format!("{address}.bin"))
}

/// Which of the 4,096 shards a body file falls in. See [`bodies_path`].
fn bodies_shard(address: i64) -> u64 {
    (address as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 52
}

/// Where a body file sat before the sharding, `bodies/{address}.bin`.
///
/// What a directory published by an older builder holds. Read and never
/// written: [`reshard_bodies`] moves these into their shards on the first
/// open, and until it has, [`read_bodies`] falls back to this path.
pub fn legacy_bodies_path(dir: &Path, address: i64) -> PathBuf {
    dir.join(BODIES_DIR).join(format!("{address}.bin"))
}

/// What the body file for `address` holds, empty where there is none.
///
/// The sharded path first and the flat one after it, so a directory that has
/// not been resharded yet, or one being resharded as this reads, answers with
/// what it has. A system with no file at all is one nobody has scanned, which
/// is [`SystemBodies::default`] rather than an error.
pub fn read_bodies(dir: &Path, address: i64) -> io::Result<SystemBodies> {
    let bytes = match std::fs::read(bodies_path(dir, address)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match std::fs::read(legacy_bodies_path(dir, address)) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Ok(SystemBodies::default());
                }
                Err(e) => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    rmp_serde::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Drop the body file for `address`, answering whether there was one.
///
/// Both paths: removing only the sharded file would leave a pre-sharding
/// flat one for [`read_bodies`] to fall back onto, and the withdrawn scan
/// would go on reading as published.
pub fn remove_bodies(dir: &Path, address: i64) -> io::Result<bool> {
    let mut removed = false;
    for path in [bodies_path(dir, address), legacy_bodies_path(dir, address)] {
        match std::fs::remove_file(&path) {
            Ok(()) => removed = true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(removed)
}

/// How far a reshard got: what it moved, and whether anything is left.
///
/// [`reshard_bodies`] and [`crate::store::reshard_cells`] both answer this.
/// A caller that asked one to stop needs both halves: the count for the
/// line it logs, and `finished` to say whether the next open has the rest
/// of the directory to do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Resharded {
    /// Loose files that are no longer loose: moved into their shard, or
    /// dropped where the shard already held that file.
    pub moved: usize,
    /// Whether nothing loose was left behind. A stop part way answers
    /// `false`; a directory that was already sharded answers `true`, having
    /// had nothing to do.
    pub finished: bool,
}

/// What a directory's layout migration came to, half by half.
///
/// The counts are what a caller logs; `finished` is whether the next open
/// has the rest of that half to do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Migrated {
    /// The body files.
    pub bodies: Resharded,
    /// The cell payloads, or [`None`] where the bodies were abandoned part
    /// way and the payloads were never reached.
    pub cells: Option<Resharded>,
}

/// Bring an existing directory's *layout* up to date, before anything reads
/// or writes it: [`reshard_bodies`] and then
/// [`crate::store::reshard_cells`], each move an idempotent rename.
///
/// Nothing is versioned: a layout this cannot recognise is built again.
///
/// Interruptible, and the one thing at an open that has to be: a galaxy's
/// worth of loose body files is a rename each and minutes of them, which a
/// run asked to stop must not be held up by. What this abandons a later
/// open takes up; a reader falls back to the flat path for whatever is
/// still loose, so a directory left half sharded serves exactly what a
/// finished one does.
///
/// A stop during the bodies leaves the payloads alone rather than opening
/// a second `readdir` on a directory nothing is going to move anything in:
/// `cells` is [`None`] and the next open runs both halves.
pub fn migrate(dir: &Path, stop: &dyn Fn() -> bool) -> io::Result<Migrated> {
    let bodies = reshard_bodies(dir, stop)?;
    if !bodies.finished {
        return Ok(Migrated { bodies, cells: None });
    }
    let cells = crate::store::reshard_cells(dir, stop)?;
    Ok(Migrated { bodies, cells: Some(cells) })
}

/// Move every loose body file into its shard, stopping where asked.
///
/// The one-time migration from the flat layout to the sharded one,
/// idempotent and cheap enough to run at every open: a directory with
/// nothing loose in it costs one `readdir` and no writes, the sharded files
/// living a level down.
///
/// Anything that is not a file named `<i64>.bin` is left alone — the shard
/// directories themselves, and the `.tmp` a builder killed mid-write leaves
/// beside a file.
///
/// `stop` is asked before each move, a rename being a syscall and the
/// question a load. Abandoning is safe wherever it lands: a move is a
/// rename within one tree, hence atomic, and [`read_bodies`] falls back to
/// the flat path for whatever is still loose, so a half-migrated directory
/// answers for every system a finished one does. The next open takes the
/// rest.
///
/// A sharded file already standing where a loose one would land wins, and
/// the loose one is dropped rather than renamed over it: nothing writes the
/// flat path any more, so the sharded file is the newer of the two — what
/// the run that abandoned a migration went on to publish — and a rename
/// would put a withdrawn scan back.
pub fn reshard_bodies(
    dir: &Path,
    stop: &dyn Fn() -> bool,
) -> io::Result<Resharded> {
    let entries = match std::fs::read_dir(dir.join(BODIES_DIR)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Resharded { moved: 0, finished: true });
        }
        Err(e) => return Err(e),
    };
    let mut moved = 0;
    let mut made = [false; 4096];
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(stem) = name.strip_suffix(".bin") else { continue };
        let Ok(address) = stem.parse::<i64>() else { continue };
        if !entry.file_type()?.is_file() {
            continue;
        }
        if stop() {
            return Ok(Resharded { moved, finished: false });
        }
        let to = bodies_path(dir, address);
        let shard = bodies_shard(address) as usize;
        if !made[shard] {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            made[shard] = true;
        }
        match std::fs::metadata(&to) {
            Ok(_) => std::fs::remove_file(entry.path())?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                std::fs::rename(entry.path(), to)?;
            }
            Err(e) => return Err(e),
        }
        moved += 1;
    }
    Ok(Resharded { moved, finished: true })
}

/// Serialize a metadata value to a file, MessagePack-encoded. The builder's
/// writer half; the reader half is [`read_meta`].
///
/// Written beside the file and renamed over it, as [`crate::Checkpoint`] is.
/// A metadata table carries no length, count or magic, so a torn write is
/// the one failure the format cannot detect. The rename is the only step
/// that touches `path`, so a builder killed mid-write leaves the table it
/// published last intact.
pub fn write_meta<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = rmp_serde::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Read a metadata value back from a file, MessagePack-decoded. The reader
/// half of [`write_meta`], and how a builder resuming onto a directory reads
/// its own published tables back.
pub fn read_meta<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = std::fs::read(path)?;
    rmp_serde::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// What a served part was when it was read, for asking whether it has changed
///
/// Opaque: a client keeps the one it was handed and hands it back to ask
/// whether the part still reads the same. The filesystem answers with a
/// modification time and an HTTP transport with a hashed `ETag`; nothing
/// outside a [`Source`] may read more into the number than "the same means
/// unchanged".
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
    /// The supercharge table
    Boosts,
    /// One chunk of the names table, numbered from zero
    ///
    /// Per chunk, the table being a hundred megabytes and a publish moving
    /// one chunk of it: new systems land in the tail, and
    /// [`crate::NameTable::upsert`] leaves a chunk alone where nothing in it
    /// changed. A client re-reads the chunk that moved.
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

    /// The populated-systems table, held resident for filtering and color.
    async fn populated(&self) -> io::Result<Vec<PopulatedSystem>>;

    /// Every system's name and position: the search index and routing graph.
    async fn names(&self) -> io::Result<Vec<NameEntry>>;

    /// One chunk of the names table, numbered from zero, empty past the end
    ///
    /// What a refresh reads: a client holding the table re-reads the chunk
    /// whose [`Stamp`] moved. The numbering has no gaps, so the first chunk
    /// that answers empty is the end of the table.
    async fn names_chunk(&self, chunk: usize) -> io::Result<Vec<NameEntry>>;

    /// The faction id-to-name table, read whole and cached by the caller.
    async fn factions(&self) -> io::Result<Vec<Faction>>;

    /// How far each scanned system reaches from its arrival star, held
    /// resident: the map sizes every system it draws by this and cannot wait
    /// on a fetch for it.
    async fn reaches(&self) -> io::Result<Vec<SystemReach>>;

    /// Which systems can supercharge a drive, and on what
    ///
    /// Held resident: the router weighs it at every step of a search, over
    /// the whole galaxy rather than over what is drawn.
    ///
    /// [`None`] where the directory publishes no such table — one built
    /// before it existed, or one whose builder has not reached it yet — and
    /// told apart from an empty table: a route for a supercharging ship
    /// cannot be answered without this, where an empty table would answer it
    /// with the unaided route.
    async fn boosts(&self) -> io::Result<Option<Vec<SystemBoost>>>;

    /// The bodies inside a system, fetched when a click opens it. Empty where
    /// the system has no scan on record.
    async fn bodies(&self, address: i64) -> io::Result<SystemBodies>;

    /// What `part` is now, or [`None`] where the transport cannot say
    ///
    /// How a client finds out that the index it holds has been republished.
    /// Everything resident is read once and then held — the aggregates, the
    /// payloads of the cells in view, the tables a color and a name are read
    /// from — while a feed rewrites all of it underneath. A client keeps the
    /// stamp it was handed with each part and asks this before re-reading
    /// anything.
    ///
    /// Cheap by contract: a stat on the filesystem, a conditional request's
    /// worth of work over HTTP. A client asks about every part it holds on
    /// every poll.
    ///
    /// [`None`] is "not there": a cell with no payload file, a chunk past the
    /// end of the table, a sidecar an older build never wrote. Two [`None`]s
    /// compare equal and read as unchanged.
    ///
    /// A transport that cannot answer cheaply says so with an error, and the
    /// client leaves that part for the next pass rather than reading it.
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

    async fn boosts(&self) -> io::Result<Option<Vec<SystemBoost>>> {
        match read_meta(&boosts_path(&self.dir)) {
            Ok(table) => Ok(Some(table)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn bodies(&self, address: i64) -> io::Result<SystemBodies> {
        read_bodies(&self.dir, address)
    }

    /// The file's modification time, in nanoseconds since the epoch.
    ///
    /// One `stat`, cheap enough to ask about every part the client holds on
    /// every poll. A missing file — a cell that owns nothing, a chunk past
    /// the end of the table — is [`None`] rather than an error.
    ///
    /// The builder writes the index whole and each changed payload whole,
    /// both in place, so a moved mtime is a republished part. A torn read
    /// costs at most a refresh that reads nothing: the payload codec drops a
    /// partial trailing record and the index's length check refuses a
    /// half-written file.
    ///
    /// A time before the epoch — archives and mirrors do carry them — stamps
    /// as zero rather than as [`None`], [`None`] being reserved for a part
    /// that is not there.
    async fn stamp(&self, part: Part) -> io::Result<Option<Stamp>> {
        let path = match part {
            Part::Index => self.dir.join(crate::store::INDEX_FILE),
            Part::Cell(id) => crate::store::payload_path(&self.dir, id),
            Part::Populated => populated_path(&self.dir),
            Part::Reaches => reaches_path(&self.dir),
            Part::Factions => factions_path(&self.dir),
            Part::Boosts => boosts_path(&self.dir),
            Part::NamesChunk(chunk) => names_chunk_path(&self.dir, chunk),
        };
        match std::fs::metadata(&path).and_then(|it| it.modified()) {
            Ok(at) => Ok(Some(
                at.duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| since.as_nanos() as Stamp),
            )),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::meta::{Barycenter, SystemBodies};
    use elite_journal::body::{AtmosphereType, BodyType};
    use std::path::PathBuf;

    /// An empty scratch directory named after the test using it.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("galos_source_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A system's insides, told apart by the address they belong to.
    fn inside(address: i64) -> SystemBodies {
        SystemBodies {
            barycenters: vec![Barycenter {
                system_address: address,
                id: 0,
                updated_at: "2026-08-08T12:00:00Z".parse().expect("a moment"),
                updated_by: "a test".into(),
                orbit: None,
            }],
            ..SystemBodies::default()
        }
    }

    /// The codec reads back the shared enums' `#[serde(untagged)]`
    /// `Unknown(String)` variants, which every scanned body with an
    /// unfamiliar class or atmosphere carries and a non-self-describing
    /// format cannot.
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

    /// A stamp moves when a part is republished, and says nothing about a
    /// part that is not there
    ///
    /// How a client finds out its index is stale: equal stamps read nothing
    /// and draw the sky as it was. A cell with no payload file and a chunk
    /// past the end of the table are both nothing to hold, hence
    /// [`None`].
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

    /// A file whose mtime is before the epoch stamps as present, not absent
    ///
    /// [`None`] means "not there", and a part answering it every poll
    /// matches the [`None`] recorded at startup and is never re-read.
    /// Archives and mirrors do hand out timestamps before 1970.
    #[test]
    fn a_pre_epoch_part_still_stamps_as_present() {
        pollster::block_on(a_pre_epoch_part_stamps());
    }

    async fn a_pre_epoch_part_stamps() {
        use super::{FsSource, Part, Source};

        let dir = std::env::temp_dir()
            .join(format!("galos_source_epoch_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = super::boosts_path(&dir);
        std::fs::write(&path, b"\x90").expect("an empty table");

        // Ten years before the epoch, which is a negative seconds count on
        // every platform that stores one.
        let old =
            std::time::UNIX_EPOCH - std::time::Duration::from_secs(315_360_000);
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("the table opens");
        file.set_times(std::fs::FileTimes::new().set_modified(old))
            .expect("a pre-epoch mtime");
        let source = FsSource::new(&dir);
        assert_eq!(
            source.stamp(Part::Boosts).await.expect("a stat"),
            Some(0),
            "a file that is there read as a part that is not"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A metadata table is renamed over rather than written through
    ///
    /// The published path is never the file being filled, so a reader working
    /// through the table goes on reading what was published, whole, however
    /// far the next write has got. A torn write is the one failure the format
    /// cannot detect: these tables carry no length, count or magic.
    #[test]
    fn a_metadata_write_does_not_touch_what_it_replaces() {
        use super::{read_meta, write_meta};
        use std::io::Read;

        let dir = std::env::temp_dir()
            .join(format!("galos_source_atomic_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = super::boosts_path(&dir);

        let first: Vec<i64> = (0..1_000).collect();
        write_meta(&path, &first).expect("the first write");

        // A reader that has the table open, as the map does when a pass lands.
        let mut held =
            std::fs::File::open(&path).expect("the table opens for reading");

        let second: Vec<i64> = (0..50_000).collect();
        write_meta(&path, &second).expect("the second write");

        let mut bytes = Vec::new();
        held.read_to_end(&mut bytes).expect("the held table reads");
        let held: Vec<i64> = rmp_serde::from_slice(&bytes)
            .expect("the held table still decodes");
        assert_eq!(
            held, first,
            "a reader holding the table saw the write land in it"
        );

        let read: Vec<i64> = read_meta(&path).expect("the second read");
        assert_eq!(read, second, "the second write did not land whole");
        assert!(
            !path.with_extension("tmp").exists(),
            "the temporary was left beside the table"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A body file written where the layout says it goes is read back by a
    /// [`FsSource`], and a pre-sharding flat file still is
    ///
    /// The two halves of the transport agree on where a body file lives, and
    /// the reader goes on answering for a directory published before the
    /// sharding landed.
    #[test]
    fn a_body_file_reads_back_sharded_or_flat() {
        pollster::block_on(a_body_file_reads_back());
    }

    async fn a_body_file_reads_back() {
        use super::{FsSource, Source, legacy_bodies_path, write_meta};

        let dir = scratch("shardread");
        let source = FsSource::new(&dir);

        let sharded = 4_611_686_020_061_657_985_i64;
        write_meta(&super::bodies_path(&dir, sharded), &inside(sharded))
            .expect("the sharded file writes");
        assert_eq!(
            source.bodies(sharded).await.expect("a read"),
            inside(sharded),
            "a file at the sharded path did not read back",
        );

        let flat = 2_412_116_659_890_i64;
        write_meta(&legacy_bodies_path(&dir, flat), &inside(flat))
            .expect("the flat file writes");
        assert_eq!(
            source.bodies(flat).await.expect("a read"),
            inside(flat),
            "a directory published before the sharding stopped answering",
        );

        assert_eq!(
            source.bodies(7).await.expect("a read"),
            SystemBodies::default(),
            "a system nobody has scanned read as something other than empty",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The migration moves every loose file into its shard and then has
    /// nothing left to do
    ///
    /// It runs at every open, so the second pass costs nothing. What it moves
    /// reads back as what was written, and what it is unsure of it leaves
    /// alone: a `.tmp` beside a file is a torn write, not a body.
    #[test]
    fn resharding_moves_every_loose_file_once() {
        use super::{bodies_path, legacy_bodies_path, read_bodies, write_meta};

        let dir = scratch("reshard");
        let addresses = [
            2_412_116_659_890_i64,
            4_611_686_020_061_657_985,
            10_477_373_803,
            -9_223_372_036_854_775_807,
            0,
        ];
        for address in addresses {
            write_meta(&legacy_bodies_path(&dir, address), &inside(address))
                .expect("a loose file writes");
        }
        let torn = dir.join(super::BODIES_DIR).join("12345.tmp");
        std::fs::write(&torn, b"\x90").expect("a torn write");

        let done = super::reshard_bodies(&dir, &|| false)
            .expect("the migration runs");
        assert_eq!(
            done.moved,
            addresses.len(),
            "the migration skipped a file"
        );
        assert!(done.finished, "the migration said it had more to do");

        for address in addresses {
            assert!(
                bodies_path(&dir, address).exists(),
                "a file did not land in its shard",
            );
            assert!(
                !legacy_bodies_path(&dir, address).exists(),
                "a file was left loose as well as sharded",
            );
            assert_eq!(
                read_bodies(&dir, address).expect("a read"),
                inside(address),
                "a moved file did not read back as what was written",
            );
        }
        assert!(torn.exists(), "the migration took a torn write for a body");

        let again =
            super::reshard_bodies(&dir, &|| false).expect("the second pass");
        assert_eq!(
            again.moved, 0,
            "a second open moved files that were already sharded",
        );
        assert!(again.finished, "a second open found something left to do");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reshard asked to stop leaves a directory that reads the same
    ///
    /// The migration is a rename per file and a galaxy's worth of them, so
    /// a run asked to stop abandons it. What makes that safe is the
    /// fallback: every file it did not reach still reads from the flat
    /// path, and the next open moves the rest.
    #[test]
    fn resharding_stops_when_asked_and_takes_the_rest_later() {
        use super::{bodies_path, legacy_bodies_path, read_bodies, write_meta};
        use std::cell::Cell;

        let dir = scratch("reshardstop");
        let addresses: Vec<i64> =
            (0..400).map(|n| 2_412_116_659_890_i64 + n * 7_919).collect();
        for &address in &addresses {
            write_meta(&legacy_bodies_path(&dir, address), &inside(address))
                .expect("a loose file writes");
        }

        // Asked before each move, so the sixth question stops the sixth.
        let questions = Cell::new(0usize);
        let stop = || {
            questions.set(questions.get() + 1);
            questions.get() > 5
        };
        let part =
            super::reshard_bodies(&dir, &stop).expect("the migration runs");
        assert_eq!(part.moved, 5, "the migration did not stop when asked");
        assert!(!part.finished, "an abandoned migration claimed to be done");

        let sharded = addresses
            .iter()
            .filter(|&&address| bodies_path(&dir, address).exists())
            .count();
        assert_eq!(
            sharded, part.moved,
            "what it said it moved is not what is in the shards",
        );
        for &address in &addresses {
            assert!(
                bodies_path(&dir, address).exists()
                    != legacy_bodies_path(&dir, address).exists(),
                "a system was left in both layouts or in neither",
            );
            assert_eq!(
                read_bodies(&dir, address).expect("a read"),
                inside(address),
                "a half-migrated directory stopped answering for a system",
            );
        }

        let rest = super::reshard_bodies(&dir, &|| false)
            .expect("the migration runs again");
        assert!(rest.finished, "a migration nobody stopped did not finish");
        assert_eq!(
            rest.moved,
            addresses.len() - part.moved,
            "the second pass did not take what the first left",
        );
        for &address in &addresses {
            assert!(
                !legacy_bodies_path(&dir, address).exists(),
                "a file was left loose after a finished migration",
            );
            assert_eq!(
                read_bodies(&dir, address).expect("a read"),
                inside(address),
                "a moved file did not read back as what was written",
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file already in its shard is not written over by the loose one
    ///
    /// What an abandoned migration makes possible: the run goes on to
    /// publish, and a system it writes lands in the shard while the stale
    /// flat file is still there. Renaming that flat file over the new one
    /// at the next open would put the withdrawn scan back.
    #[test]
    fn resharding_keeps_the_sharded_file_over_the_loose_one() {
        use super::{bodies_path, legacy_bodies_path, read_bodies, write_meta};

        let dir = scratch("reshardwins");
        let address = 10_477_373_803_i64;
        write_meta(&legacy_bodies_path(&dir, address), &inside(address))
            .expect("the stale flat file writes");
        let current = inside(address + 1);
        write_meta(&bodies_path(&dir, address), &current)
            .expect("the published file writes");

        let done =
            super::reshard_bodies(&dir, &|| false).expect("the migration");
        assert_eq!(done.moved, 1, "the loose file was left where it was");
        assert!(
            !legacy_bodies_path(&dir, address).exists(),
            "the stale flat file is still there to be fallen back onto",
        );
        assert_eq!(
            read_bodies(&dir, address).expect("a read"),
            current,
            "the stale flat file was renamed over what was published",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A migration stopped in the body files does not go on to the cells
    ///
    /// The two halves are one open's work, and the run has already said it
    /// wants out: reading the payload directory to move nothing would be
    /// another `readdir` over a galaxy's worth of files. `cells` says which
    /// it is — [`None`] for a half that was never reached, against a
    /// [`Resharded`] that found nothing to do.
    #[test]
    fn a_migration_stopped_in_the_bodies_leaves_the_cells() {
        use super::{legacy_bodies_path, write_meta};
        use std::cell::Cell;

        let dir = scratch("migratestop");
        for n in 0..4 {
            let address = 2_412_116_659_890_i64 + n * 7_919;
            write_meta(&legacy_bodies_path(&dir, address), &inside(address))
                .expect("a loose file writes");
        }
        let loose = dir.join(crate::store::PAYLOAD_DIR);
        std::fs::create_dir_all(&loose).expect("the payload directory");
        let payload = loose.join("01-0000000000000001.bin");
        std::fs::write(&payload, b"\x90").expect("a loose payload writes");

        // Asked before each move, so the third question stops the third.
        let questions = Cell::new(0usize);
        let stop = || {
            questions.set(questions.get() + 1);
            questions.get() > 2
        };
        let part = super::migrate(&dir, &stop).expect("the migration runs");
        assert_eq!(part.bodies.moved, 2, "the bodies did not stop when asked");
        assert!(!part.bodies.finished, "an abandoned half claimed to be done");
        assert_eq!(part.cells, None, "the cells were run after a stop");
        assert!(payload.exists(), "a stopped migration moved a payload");

        let rest = super::migrate(&dir, &|| false).expect("the second open");
        assert!(rest.bodies.finished, "the bodies did not finish");
        assert_eq!(rest.bodies.moved, 2, "the second open left a body loose");
        assert_eq!(
            rest.cells,
            Some(super::Resharded { moved: 1, finished: true }),
            "the second open did not take the payload the first left",
        );
        assert!(!payload.exists(), "the payload was left loose");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A withdrawn scan loses its flat file as well as its sharded one
    ///
    /// The removal is what makes a system whose last scan was taken back read
    /// as unscanned. Clearing only the sharded file would leave the
    /// pre-sharding one for the read to fall back onto.
    #[test]
    fn removing_a_body_clears_both_layouts() {
        use super::{
            bodies_path, legacy_bodies_path, read_bodies, remove_bodies,
            write_meta,
        };

        let dir = scratch("remove");
        let address = 2_412_116_659_890_i64;
        write_meta(&bodies_path(&dir, address), &inside(address))
            .expect("the sharded file writes");
        write_meta(&legacy_bodies_path(&dir, address), &inside(address))
            .expect("the flat file writes");

        assert!(
            remove_bodies(&dir, address).expect("the removal"),
            "the removal said there was nothing to remove",
        );
        assert!(!bodies_path(&dir, address).exists());
        assert!(!legacy_bodies_path(&dir, address).exists());
        assert_eq!(
            read_bodies(&dir, address).expect("a read"),
            SystemBodies::default(),
            "a withdrawn scan still reads as one that stands",
        );

        assert!(
            !remove_bodies(&dir, address).expect("a second removal"),
            "removing nothing was reported as removing something",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
