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
    Faction, PopulatedSystem, SystemBodies, SystemBoost, SystemReach,
};
use crate::names;
use crate::walk::Index;
use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io;
use std::path::{Path, PathBuf};

/// The populated-systems table, resident once and read for every color.
pub const POPULATED_FILE: &str = "populated.bin";
/// The subdirectory the names table's sections and log live in.
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

/// The names table's directory within a build directory.
pub fn names_dir(dir: &Path) -> PathBuf {
    dir.join(NAMES_DIR)
}

/// The names table's head, which names the live generation.
///
/// The one file a reader opens first and the one a writer renames last:
/// it is what makes a generation of sections live, so a client that has
/// read it has a whole table or none.
pub fn names_head_path(dir: &Path) -> PathBuf {
    names_dir(dir).join(crate::names::HEAD_FILE)
}

/// The names table's delta log, which the feed appends to.
pub fn names_delta_path(dir: &Path) -> PathBuf {
    names_dir(dir).join("delta.bin")
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
/// The layout before [`crate::pack`]: one file a system, sharded over 4,096
/// subdirectories, `bodies/{shard:03x}/{address}.bin`. Read and never
/// written — [`crate::pack::pack`] walks these into the shard files on the
/// first open, and until it has, [`read_bodies`] falls back to this path.
pub fn bodies_path(dir: &Path, address: i64) -> PathBuf {
    let shard = crate::pack::shard_of(address);
    dir.join(BODIES_DIR)
        .join(format!("{shard:03x}"))
        .join(format!("{address}.bin"))
}

/// Where a body file sat before the sharding, `bodies/{address}.bin`.
///
/// What a directory published by an older builder holds. Read and never
/// written: [`reshard_bodies`] moves these into their shards on the first
/// open, and until it has, [`read_bodies`] falls back to this path.
pub fn legacy_bodies_path(dir: &Path, address: i64) -> PathBuf {
    dir.join(BODIES_DIR).join(format!("{address}.bin"))
}

/// What the bodies of `address` are, empty where nothing has scanned it.
///
/// Three layouts, newest first: the packed shard files, then the loose file
/// a system in its shard directory, then the flat one from before the
/// sharding. A directory part way through a packing answers out of whichever
/// holds the system, and a system the pack says was *withdrawn* is empty
/// rather than whatever a loose file it replaced still says.
///
/// A system nothing has scanned is [`SystemBodies::default`] rather than an
/// error.
pub fn read_bodies(dir: &Path, address: i64) -> io::Result<SystemBodies> {
    match crate::pack::find(dir, address)? {
        crate::pack::Found::Bodies(inside) => return Ok(inside),
        crate::pack::Found::Withdrawn => return Ok(SystemBodies::default()),
        crate::pack::Found::Absent => {}
    }
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

/// Withdraw the bodies of `address`, answering whether there were any.
///
/// All three layouts: a tombstone in the pack, and the two loose files
/// removed. Leaving either loose file behind would leave the withdrawn scan
/// for a fallback to read, and leaving out the tombstone would leave it in
/// the pack.
pub fn remove_bodies(dir: &Path, address: i64) -> io::Result<bool> {
    let mut removed = crate::pack::remove(dir, address)?;
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

/// What a directory's layout migration came to, part by part.
///
/// The counts are what a caller logs; `finished` is whether the next open
/// has the rest of that part to do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Migrated {
    /// The body files, walked into the packed shard files.
    pub bodies: crate::pack::Packed,
    /// The cell payloads, or [`None`] where the bodies were abandoned part
    /// way and the payloads were never reached.
    pub cells: Option<Resharded>,
    /// Systems in the names table folded out of MessagePack chunks into a
    /// mapped base, or [`None`] where the directory had no chunks — which
    /// is every directory built since.
    pub names: Option<usize>,
    /// Supercharge rows given the place they had always implied, or
    /// [`None`] where the table already carried one.
    pub boosts: Option<usize>,
    /// The format version the directory claims, where this build cannot
    /// read it and a manual upgrade is what is wanted
    ///
    /// Nothing was done in that case: see [`migrate`]. The caller's job is
    /// to *say so*, naming `galos-index upgrade`, rather than to carry on
    /// and let the refusal fall out of the first cell anybody asks for.
    pub upgrade: Option<u16>,
}

/// The supercharge table's rows as they were published before they carried
/// a place.
///
/// Read only by [`place_boosts`], which is how a directory written by an
/// older builder is brought forward. Two fields, so it decodes exactly the
/// rows [`SystemBoost`]'s three cannot.
#[derive(serde::Deserialize)]
struct Unplaced {
    address: i64,
    boost: crate::meta::Boost,
}

/// Give the supercharge table the places its rows always implied,
/// answering how many rows were rewritten — or [`None`] where there was
/// nothing to do.
///
/// The table used to be addresses and classes, which left the router to
/// join four million of them against the names table's address column to
/// find out where the jet cones are: 4 GB of mapping faulted and 7.9 s
/// before a galactic route could start planning, once a session. The place
/// belongs in the published row, and this is the one pass that puts it
/// there — the same join, run once, by the side that publishes.
///
/// A row the names table cannot place is dropped rather than placed at the
/// origin, which would put a jet cone at the galactic centre and plan every
/// route through it. It is the rule the derivations already follow.
///
/// Not interruptible and it need not be: it is one read of the table, one
/// pass over the address column, and one write.
pub fn place_boosts(dir: &Path) -> io::Result<Option<usize>> {
    let path = boosts_path(dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    // Already placed, which is every table written since. Asked first, so
    // a current directory pays one decode and nothing else.
    if rmp_serde::from_slice::<Vec<SystemBoost>>(&bytes).is_ok() {
        return Ok(None);
    }
    let mut old: Vec<Unplaced> = rmp_serde::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    // The table is published in address order, and the join walks the
    // column in that order; sorted here rather than trusted, since a file
    // that says otherwise would answer places for the wrong systems.
    old.sort_unstable_by_key(|row| row.address);
    let addresses: Vec<i64> = old.iter().map(|row| row.address).collect();

    // **The places come from the payloads, one pass over the galaxy.**
    // They used to come from the names table's own position column, and
    // that column no longer exists: a system's place is in the cell that
    // owns it and nowhere else. Asking the tree per address would be a
    // sphere query apiece — milliseconds by four million rows — where the
    // cells hold every place already, in an order this does not care
    // about.
    let index = crate::Index::read(dir)?;
    let mut placed: Vec<SystemBoost> = Vec::with_capacity(old.len());
    for cell in index.cells() {
        for point in crate::Index::read_payload(dir, cell.id)? {
            let Ok(which) = addresses.binary_search(&(point.id64 as i64))
            else {
                continue;
            };
            placed.push(SystemBoost {
                address: old[which].address,
                boost: old[which].boost,
                position: [
                    point.pos[0] as f32,
                    point.pos[1] as f32,
                    point.pos[2] as f32,
                ],
            });
        }
    }
    // Address order, as every published table is, so the row a reader
    // binary-searches for is where it expects: the walk above is in cell
    // order, which is no order at all to a caller.
    placed.sort_unstable_by_key(|row| row.address);
    write_meta(&path, &placed)?;
    Ok(Some(placed.len()))
}

/// Bring an existing directory's *layout* up to date, before anything reads
/// or writes it: [`crate::pack::pack`], [`crate::store::reshard_cells`],
/// and then the names chunks.
///
/// Nothing is versioned: a layout this cannot recognise is built again.
///
/// Interruptible, and the one thing at an open that has to be: a galaxy's
/// worth of loose body files is hours of them, which a run asked to stop
/// must not be held up by. What this abandons a later open takes up; a
/// reader falls back to the loose paths for whatever has not been packed,
/// so a directory left half migrated serves exactly what a finished one
/// does.
///
/// A stop during the bodies leaves the payloads alone rather than opening
/// a second `readdir` on a directory nothing is going to move anything in:
/// `cells` is [`None`] and the next open runs both halves.
///
/// The names fold is last and is *not* interruptible, because it cannot
/// serve half: a directory has either the chunks or a base, and until the
/// fold finishes the chunks are still what stands. A galaxy's worth of them
/// is one external sort — minutes, against the afternoon that derived them
/// — and it happens once, ever, per directory.
pub fn migrate(
    dir: &Path,
    stop: &(dyn Fn() -> bool + Sync),
) -> io::Result<Migrated> {
    // **Said rather than worked around.** Everything below moves files
    // about without reading what is in them, so it would run to completion
    // over a directory whose payloads this build cannot read — and the
    // refusal would surface later, out of whatever first asked for a cell,
    // as a failed open with no remedy attached. A layout this build does
    // not read is not something an open can fix: it is hours of re-encoding
    // and a sweep of the scan record, which is `galos-index upgrade`.
    if let Some(found) = crate::store::stale(dir) {
        return Ok(Migrated {
            bodies: crate::pack::Packed { moved: 0, finished: true },
            cells: None,
            names: None,
            boosts: None,
            upgrade: Some(found),
        });
    }

    let bodies = crate::pack::pack(dir, stop)?;
    if !bodies.finished {
        return Ok(Migrated {
            bodies,
            cells: None,
            names: None,
            boosts: None,
            upgrade: None,
        });
    }
    let cells = crate::store::reshard_cells(dir, stop)?;
    let names = crate::names::fold_chunks(dir)?;
    // After the fold, because it reads the names table and the fold is
    // what decides which generation that is.
    let boosts = place_boosts(dir)?;
    Ok(Migrated { bodies, cells: Some(cells), names, boosts, upgrade: None })
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
    let bytes = encoded(value)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// The same file, written where nothing stands to be kept.
///
/// [`write_meta`]'s guarantee costs a second directory entry made and
/// unmade for every file written, which over a galaxy of one small file a
/// system is most of what writing one costs: measured against Spansh's
/// dump, the rename alone was an eighth of the whole read. A build raising
/// a directory from nothing overwrites nothing and is abandoned whole if it
/// fails, so it is paying for a guarantee it cannot use. See
/// [`crate::bodies::Published::raising`], which is the one caller.
pub fn raise_meta<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = encoded(value)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

/// One metadata value's bytes, for either writer.
fn encoded<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    rmp_serde::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
    /// The names table's base, `names/head.bin`
    ///
    /// Stamped by its head rather than by its sections: the head is written
    /// last and is what makes a generation live, so a moved stamp is a
    /// table that has been recompacted whole and a client re-opens it. That
    /// is rare — a cold build, or a fold of a log that has grown long.
    Names,
    /// The names table's delta log, `names/delta.bin`
    ///
    /// What moves when the feed names a system. A client holds the byte
    /// offset it has read to and takes only what is past it, so a publish
    /// of fifty arrivals costs fifty rows on both sides.
    NamesDelta,
}

/// Where the client reads cells and metadata from. One transport for both.
#[async_trait]
pub trait Source: Send + Sync {
    /// The resident tree of cell aggregates the walks plan on.
    async fn index(&self) -> io::Result<Index>;

    /// One cell's payload: its systems, positions in light years. Empty
    /// where the cell owns nothing.
    async fn payload(&self, id: CellId) -> io::Result<Vec<Point>>;

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
    ) -> io::Result<Vec<Point>> {
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
    /// client hands back the offset it holds and is given the tail.
    async fn names_delta(&self, from: u64) -> io::Result<names::Delta>;

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

    async fn payload_prefix(
        &self,
        id: CellId,
        limit: usize,
    ) -> io::Result<Vec<Point>> {
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
            kind: crate::meta::StarKind::G,
        };

        let built = crate::tree::Snapshot::build(
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

    /// A migration stopped in the body files does not go on to the cells
    ///
    /// The two halves are one open's work, and the run has already said it
    /// wants out: reading the payload directory to move nothing would be
    /// another `readdir` over a galaxy's worth of files. `cells` says which
    /// it is — [`None`] for a half that was never reached, against a
    /// [`Resharded`] that found nothing to do.
    /// A directory this build cannot read is said so, not migrated
    ///
    /// Everything the migration does is content-blind — it moves files into
    /// shards and folds chunks — so it would run happily over payloads of
    /// another layout and leave the refusal to fall out of the first cell
    /// anybody asked for, as a failed open with no remedy attached. The
    /// remedy is hours of re-encoding (`galos-index upgrade`) and not
    /// something an open can do, so the migration's job here is to name the
    /// version it met and touch nothing.
    #[test]
    fn a_directory_of_another_layout_asks_for_an_upgrade() {
        use super::{legacy_bodies_path, write_meta};

        let dir = scratch("migratestale");
        let address = 2_412_116_659_890_i64;
        write_meta(&legacy_bodies_path(&dir, address), &inside(address))
            .expect("a loose file writes");

        // An index file of a layout this build does not read.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"GIDX");
        bytes.extend_from_slice(&(crate::INDEX_VERSION - 1).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(dir.join(crate::store::INDEX_FILE), &bytes)
            .expect("an index file writes");

        let done = super::migrate(&dir, &|| false).expect("the migration runs");
        assert_eq!(
            done.upgrade,
            Some(crate::INDEX_VERSION - 1),
            "the layout met was not named",
        );
        // And nothing was moved: the loose file is still loose.
        assert_eq!(done.bodies.moved, 0);
        assert!(
            legacy_bodies_path(&dir, address).exists(),
            "the migration moved files it could not read the index of",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_migration_stopped_in_the_bodies_leaves_the_cells() {
        use super::{legacy_bodies_path, write_meta};

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
        // Atomic rather than a `Cell`: the packing deals its shards out to
        // threads, so what it asks about stopping is shared.
        let questions = std::sync::atomic::AtomicUsize::new(0);
        let stop = || {
            questions.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 2
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
