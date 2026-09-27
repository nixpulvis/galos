//! The transport seam: the one place a reader asks where cells and metadata
//! come from.
//!
//! Cells and metadata share a transport at all times. Today that is the
//! filesystem, [`FsSource`] over a build directory; tomorrow one HTTP impl
//! over the same layout. The trait swaps the whole transport at once, so a
//! filesystem index is never paired with an HTTP metadata service.
//!
//! Async because the HTTP impl to come is; the FS reads are blocking and
//! their futures resolve at once. Boxed through `async_trait`, so a reader
//! holds `Arc<dyn Source>` and picks its transport at runtime.
//!
//! The path helpers are the file-layout contract the builder writes to and
//! this reads from, named once in [`crate::format::layout`] so the two cannot
//! drift.

use crate::core::geometry::CellId;
use crate::format::layout::{
    factions_path, names_delta_path, names_head_path, populated_path,
    reaches_path,
};
use crate::format::msgpack::read_meta;
use crate::records::{
    Faction, PopulatedSystem, SystemBodies, SystemReach, Table,
};
use crate::store::bodies::read_bodies;
use crate::store::names;
use crate::store::tables;
use crate::tree::cell::CellSystem;
use crate::tree::index::Index;
use async_trait::async_trait;
use std::io;
use std::path::PathBuf;

/// What a served part was when it was read, for asking whether it has changed
///
/// Opaque: a reader keeps the one it was handed and hands it back to ask
/// whether the part still reads the same. The filesystem answers with a
/// modification time and an HTTP transport with a hashed `ETag`; nothing
/// outside a [`Source`] may read more into the number than "the same means
/// unchanged".
pub type Stamp = u64;

/// One part of a served index, for [`Source::stamp`] to be asked about
///
/// The parts a reader holds and would have to re-read, which is every file in
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
    /// The contributed table of this name
    Table(&'static str),
    /// The names table's base, `names/head.bin`
    ///
    /// Stamped by its head rather than by its sections: the head is written
    /// last and is what makes a generation live, so a moved stamp is a
    /// table that has been recompacted whole and a reader re-opens it. That
    /// is rare — a cold build, or a fold of a log that has grown long.
    Names,
    /// The names table's delta log, `names/delta.bin`
    ///
    /// What moves when the feed names a system. A reader holds the byte
    /// offset it has read to and takes only what is past it, so a publish
    /// of fifty arrivals costs fifty rows on both sides.
    NamesDelta,
}

/// Where a reader reads cells and metadata from. One transport for both.
#[async_trait]
pub trait Source: Send + Sync {
    /// The resident tree of cell aggregates the walks plan on.
    async fn index(&self) -> io::Result<Index>;

    /// One cell's payload: its systems, positions in light years. Empty
    /// where the cell owns nothing.
    async fn payload(&self, id: CellId) -> io::Result<Vec<CellSystem>>;

    /// The brightest `limit` of one cell's payload.
    ///
    /// What a draw actually wants: a payload is magnitude-ordered and the
    /// map draws a share of each cell, so the rest is bytes fetched, held
    /// and never looked at. A transport that cannot answer a range answers
    /// the whole and the caller is no worse off than before; a file can,
    /// and does.
    async fn payload_prefix(
        &self,
        id: CellId,
        limit: usize,
    ) -> io::Result<Vec<CellSystem>> {
        let mut points = self.payload(id).await?;
        points.truncate(limit);
        Ok(points)
    }

    /// The populated-systems table, held resident for filtering and color.
    async fn populated(&self) -> io::Result<Vec<PopulatedSystem>>;

    /// Every system's name and position: the search index and routing
    /// graph, base and log together.
    ///
    /// Mapped rather than read where the transport is a local directory —
    /// which is the whole point of the format, a galaxy of names being 5.8
    /// GB. A transport that cannot map has to put the sections somewhere it
    /// can before it answers this; there is no version of a 200 M-row
    /// lookup table that is decoded at startup.
    async fn names(&self) -> io::Result<names::Names>;

    /// The delta log's rows past `from`, and how far the log now reads.
    ///
    /// What a refresh reads when [`Part::NamesDelta`]'s stamp has moved: a
    /// reader hands back the offset it holds and is given the tail.
    async fn names_delta(&self, from: u64) -> io::Result<names::Delta>;

    /// The faction id-to-name table, read whole and cached by the caller.
    async fn factions(&self) -> io::Result<Vec<Faction>>;

    /// How far each scanned system reaches from its arrival star, held
    /// resident: the map sizes every system it draws by this and cannot wait
    /// on a fetch for it.
    async fn reaches(&self) -> io::Result<Vec<SystemReach>>;

    /// The bytes of the contributed table called `name`, or [`None`] where
    /// the directory publishes no such table — one built before it existed,
    /// or one whose builder has not reached it yet.
    ///
    /// Bytes because a transport does not know what a row is: the caller
    /// that does decodes them, through [`table`].
    async fn table(&self, name: &'static str) -> io::Result<Option<Vec<u8>>>;

    /// The bodies inside a system, fetched when a click opens it. Empty where
    /// the system has no scan on record.
    async fn bodies(&self, address: i64) -> io::Result<SystemBodies>;

    /// What `part` is now, or [`None`] where the transport cannot say
    ///
    /// How a reader finds out that the index it holds has been republished.
    /// Everything resident is read once and then held — the aggregates, the
    /// payloads of the cells in view, the tables a color and a name are read
    /// from — while a feed rewrites all of it underneath. A reader keeps the
    /// stamp it was handed with each part and asks this before re-reading
    /// anything.
    ///
    /// Cheap by contract: a stat on the filesystem, a conditional request's
    /// worth of work over HTTP. A reader asks about every part it holds on
    /// every poll.
    ///
    /// [`None`] is "not there": a cell with no payload file, a names table
    /// never written, a sidecar an older build never wrote. Two [`None`]s
    /// compare equal and read as unchanged.
    ///
    /// A transport that cannot answer cheaply says so with an error, and the
    /// reader leaves that part for the next pass rather than reading it.
    async fn stamp(&self, part: Part) -> io::Result<Option<Stamp>>;
}

/// The contributed table `T` as `source` publishes it, or [`None`] where it
/// publishes no such table.
pub async fn table<T: Table>(
    source: &dyn Source,
) -> io::Result<Option<Vec<T::Row>>> {
    match source.table(T::NAME).await? {
        Some(bytes) => Ok(Some(tables::decode::<T>(&bytes)?)),
        None => Ok(None),
    }
}

/// A [`Source`] over a build directory on the local filesystem.
///
/// The directory holds `index.bin`, a `cells/` subdirectory of payloads, and
/// the metadata files this reads beside them. The reads are blocking; a reader
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

    async fn payload(&self, id: CellId) -> io::Result<Vec<CellSystem>> {
        Index::read_payload(&self.dir, id)
    }

    async fn payload_prefix(
        &self,
        id: CellId,
        limit: usize,
    ) -> io::Result<Vec<CellSystem>> {
        Index::read_payload_prefix(&self.dir, id, limit)
    }

    async fn populated(&self) -> io::Result<Vec<PopulatedSystem>> {
        read_meta(&populated_path(&self.dir))
    }

    async fn names(&self) -> io::Result<names::Names> {
        names::Names::open(&self.dir)
    }

    async fn names_delta(&self, from: u64) -> io::Result<names::Delta> {
        names::Delta::since(&self.dir, from)
    }

    async fn factions(&self) -> io::Result<Vec<Faction>> {
        read_meta(&factions_path(&self.dir))
    }

    async fn reaches(&self) -> io::Result<Vec<SystemReach>> {
        read_meta(&reaches_path(&self.dir))
    }

    async fn table(&self, name: &'static str) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(tables::path(&self.dir, name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn bodies(&self, address: i64) -> io::Result<SystemBodies> {
        read_bodies(&self.dir, address)
    }

    /// The file's modification time, in nanoseconds since the epoch.
    ///
    /// One `stat`, cheap enough to ask about every part the reader holds on
    /// every poll. A missing file — a cell that owns nothing, a names table
    /// never written — is [`None`] rather than an error.
    ///
    /// The builder writes the index whole and in place, and each changed
    /// payload whole beside its file and renamed over it, so a moved mtime is
    /// a republished part. A torn read costs at most a refresh that reads
    /// nothing: a payload is never seen half-written, and the index's length
    /// check refuses a half-written file.
    ///
    /// A time before the epoch — archives and mirrors do carry them — stamps
    /// as zero rather than as [`None`], [`None`] being reserved for a part
    /// that is not there.
    async fn stamp(&self, part: Part) -> io::Result<Option<Stamp>> {
        let path = match part {
            Part::Index => self.dir.join(crate::format::layout::INDEX_FILE),
            Part::Cell(id) => {
                crate::format::layout::payload_path(&self.dir, id)
            }
            Part::Populated => populated_path(&self.dir),
            Part::Reaches => reaches_path(&self.dir),
            Part::Factions => factions_path(&self.dir),
            Part::Table(name) => tables::path(&self.dir, name),
            Part::Names => names_head_path(&self.dir),
            Part::NamesDelta => names_delta_path(&self.dir),
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
    use crate::records::{Barycenter, SystemBodies};
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

    /// A stamp moves when a part is republished, and says nothing about a
    /// part that is not there
    ///
    /// How a reader finds out its index is stale: equal stamps read nothing
    /// and draw the sky as it was. A cell with no payload file and the parts
    /// of a names table never written are all nothing to hold, hence
    /// [`None`].
    #[test]
    fn a_stamp_moves_when_a_part_is_republished() {
        pollster::block_on(a_stamp_moves());
    }

    async fn a_stamp_moves() {
        use super::{FsSource, Part, Source};
        use crate::build::snapshot::BuildParams;
        use crate::core::geometry::CellId;
        use crate::system::System;

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
            kind: crate::core::star::StarKind::G,
        };

        let built = crate::build::snapshot::Snapshot::build(
            &[system(1, 0.0)],
            &BuildParams::default(),
        );
        built.write(&dir).expect("the build should write");
        let source = FsSource::new(&dir);

        let before =
            source.stamp(Part::Index).await.expect("a stat").expect("a stamp");

        // A cell nothing was written for, and the parts of a names table
        // that was never written at all.
        let empty = CellId { level: 10, x: 1, y: 2, z: 3 };
        assert_eq!(
            source.stamp(Part::Cell(empty)).await.expect("a stat"),
            None
        );
        assert_eq!(source.stamp(Part::Names).await.expect("a stat"), None);
        assert_eq!(source.stamp(Part::NamesDelta).await.expect("a stat"), None);
        assert!(
            source.names().await.expect("a read").is_empty(),
            "an unpublished table reads empty rather than failing"
        );

        // Republished with a second system: the index is rewritten whole, so
        // its stamp moves.
        std::thread::sleep(std::time::Duration::from_millis(10));
        crate::build::snapshot::Snapshot::build(
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
        let path = super::tables::path(&dir, "cones");
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
            source.stamp(Part::Table("cones")).await.expect("a stat"),
            Some(0),
            "a file that is there read as a part that is not"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A loose body file is read back by a [`FsSource`], at the sharded path
    /// or at the flat one before it
    ///
    /// Neither is written any more — the builder packs bodies into shard
    /// files — but a directory published before the pack still holds them,
    /// and the reader goes on answering for it until the pack has walked
    /// them in.
    #[test]
    fn a_body_file_reads_back_sharded_or_flat() {
        pollster::block_on(a_body_file_reads_back());
    }

    async fn a_body_file_reads_back() {
        use super::{FsSource, Source};
        use crate::format::layout::{bodies_path, legacy_bodies_path};
        use crate::format::msgpack::write_meta;

        let dir = scratch("shardread");
        let source = FsSource::new(&dir);

        let sharded = 4_611_686_020_061_657_985_i64;
        write_meta(&bodies_path(&dir, sharded), &inside(sharded))
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
}
