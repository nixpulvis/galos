//! Where everything lives: every file and directory name the format uses.
//!
//! The file-layout contract the builder writes to and every reader reads
//! from, named once here so the two cannot drift. A served directory holds
//! the cells (the index file and a payload a cell), the metadata tables, the
//! names table's generations and the packed bodies; beside it — never inside
//! it, since the directory is served whole — stand the files that belong to
//! whoever is writing it: the lock, the resume point and the logs and
//! scratch that hang off the resume point's name.
//!
//! Only names and paths. What is *in* each file is the module that reads
//! and writes it.

use crate::core::geometry::CellId;
use std::path::{Path, PathBuf};

// --- The served directory -------------------------------------------------

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
/// [`LEAF_CAP`]: crate::build::snapshot::LEAF_CAP
pub fn payload_path(dir: &Path, id: CellId) -> PathBuf {
    let morton = id.morton();
    dir.join(PAYLOAD_DIR)
        .join(format!("{:03x}", morton & 0xfff))
        .join(format!("{:02}-{morton:016x}.bin", id.level))
}

/// Where a cell's payload was written before the sharding: `cells/` flat.
/// Read where the sharded path is absent, never written.
pub fn legacy_payload_path(dir: &Path, id: CellId) -> PathBuf {
    dir.join(PAYLOAD_DIR).join(format!(
        "{:02}-{:016x}.bin",
        id.level,
        id.morton()
    ))
}

/// The populated-systems table, resident once and read for every color.
pub const POPULATED_FILE: &str = "populated.bin";

/// How far each scanned system reaches, resident once and read for every
/// system the map draws.
pub const REACHES_FILE: &str = "reaches.bin";

/// The faction id-to-name table, small and read whole.
pub const FACTIONS_FILE: &str = "factions.bin";

/// Which systems can supercharge a drive, resident for the router.
pub const BOOSTS_FILE: &str = "boosts.bin";

/// The populated table's path within a build directory.
pub fn populated_path(dir: &Path) -> PathBuf {
    dir.join(POPULATED_FILE)
}

/// The reaches table's path within a build directory.
pub fn reaches_path(dir: &Path) -> PathBuf {
    dir.join(REACHES_FILE)
}

/// The factions table's path within a build directory.
pub fn factions_path(dir: &Path) -> PathBuf {
    dir.join(FACTIONS_FILE)
}

/// The supercharge table's path within a build directory.
pub fn boosts_path(dir: &Path) -> PathBuf {
    dir.join(BOOSTS_FILE)
}

// --- The names table --------------------------------------------------------

/// The subdirectory the names table's sections and log live in.
pub const NAMES_DIR: &str = "names";

/// `head.bin`'s name within the names directory.
pub const HEAD_FILE: &str = "head.bin";

/// The addresses, within a generation directory.
pub const ADDR_FILE: &str = "addr.bin";

/// The rows in name order, within a generation directory.
pub const BYNAME_FILE: &str = "byname.bin";

/// The offsets into the text, one a stored name, within a generation
/// directory.
pub const SPAN_FILE: &str = "span.bin";

/// Which rows stored a name, ascending, within a generation directory.
///
/// Version 3 and after: the rows the arithmetic could not spell. A row
/// absent from it is one [`crate::core::procedural`] answers for.
pub const EXCEPTION_FILE: &str = "exception.bin";

/// The name bytes, within a generation directory.
pub const TEXT_FILE: &str = "text.bin";

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
    names_dir(dir).join(HEAD_FILE)
}

/// The names table's delta log, within the names directory.
pub const DELTA_FILE: &str = "delta.bin";

/// The names table's delta log, which the feed appends to.
pub fn names_delta_path(dir: &Path) -> PathBuf {
    names_dir(dir).join(DELTA_FILE)
}

/// One generation's directory within the names directory.
pub fn generation_dir(dir: &Path, generation: u64) -> PathBuf {
    names_dir(dir).join(format!("{generation:03}"))
}

/// Where a build's rows and runs live: inside the names directory, so the
/// sections it writes are renamed within one filesystem and never copied.
///
/// Public because a copy of a directory has to skip it: a build's scratch
/// is gigabytes of sort runs that mean nothing anywhere else, and one
/// spelling of `.building` is what keeps the copy and the writer agreeing
/// about which directory that is. See [`crate::ops::copy`].
pub fn scratch_dir(dir: &Path) -> PathBuf {
    names_dir(dir).join(".building")
}

// --- The bodies -----------------------------------------------------------

/// The subdirectory of per-system body files.
pub const BODIES_DIR: &str = "bodies";

/// How many shards the addresses are spread over.
pub const BODY_SHARDS: u64 = 4096;

/// Which shard an address belongs to.
///
/// The top twelve bits of the address multiplied by the 64-bit golden-ratio
/// constant. The multiply mixes the high bits down: an Elite `id64` packs a
/// mass code and the boxel coordinates into its low bits, so `address % 4096`
/// leaves whole shards empty and piles the rest up.
pub fn body_shard(address: i64) -> u64 {
    (address as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 52
}

/// A body shard's index file.
pub fn body_index_path(dir: &Path, shard: u64) -> PathBuf {
    dir.join(BODIES_DIR).join(format!("{shard:03x}.idx"))
}

/// A body shard's data file of a given generation.
pub fn body_data_path(dir: &Path, shard: u64, generation: u16) -> PathBuf {
    dir.join(BODIES_DIR).join(format!("{shard:03x}.{generation:04x}.dat"))
}

/// A system's body file within a build directory, keyed by address.
///
/// The layout before [`crate::store::bodies`]: one file a system, sharded over
/// 4,096 subdirectories, `bodies/{shard:03x}/{address}.bin`. Read and never
/// written — [`crate::store::bodies::pack`] walks these into the shard files on
/// the first open, and until it has,
/// [`read_bodies`](crate::store::bodies::read_bodies) falls back to this path.
pub fn bodies_path(dir: &Path, address: i64) -> PathBuf {
    let shard = body_shard(address);
    dir.join(BODIES_DIR)
        .join(format!("{shard:03x}"))
        .join(format!("{address}.bin"))
}

/// Where a body file sat before the sharding, `bodies/{address}.bin`.
///
/// What a directory published by an older builder holds. Read and never
/// written: [`pack`](crate::store::bodies::pack) moves these into the shard
/// files on the first open, and until it has,
/// [`read_bodies`](crate::store::bodies::read_bodies) falls back to this
/// path.
pub fn legacy_bodies_path(dir: &Path, address: i64) -> PathBuf {
    dir.join(BODIES_DIR).join(format!("{address}.bin"))
}

// --- Beside the directory ---------------------------------------------------

/// Where a directory's lock sits, which is beside it rather than in it.
///
/// The directory is served whole, so nothing that belongs to the running
/// process may live under it.
pub fn lock_path(dir: &Path) -> PathBuf {
    let mut name = dir.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// What a resume point is named beside the directory it resumes.
///
/// Beside the directory rather than inside it: the file holds every
/// system at full precision, which no client should be served. One
/// spelling, here, because four things need it: [`pending_path`],
/// [`mark_path`] and [`spill_dir`] hang their own suffixes off it,
/// [`crate::ops::copy`] has to carry the whole family across, and
/// `galos::sink::index` re-exports this rather than spelling it again.
pub const CHECKPOINT_SUFFIX: &str = ".checkpoint";

/// The resume point that stands beside `dir`.
///
/// What a directory's resume point is called where nothing has said
/// otherwise — `galos ingest --checkpoint PATH` is the caller that says
/// otherwise, and `galos::sink::index::Index::checkpoint` is where that
/// choice is made.
pub fn checkpoint_beside(dir: &Path) -> PathBuf {
    let mut name = dir.as_os_str().to_owned();
    name.push(CHECKPOINT_SUFFIX);
    PathBuf::from(name)
}

/// Where the log sits, which is beside the base it extends.
pub fn pending_path(checkpoint: &Path) -> PathBuf {
    let mut name = checkpoint.as_os_str().to_owned();
    name.push(".pending");
    PathBuf::from(name)
}

/// Where the mark sits: beside the resume point, which is what it stands
/// with. The build's scratch is cleared by the publish that writes this,
/// so it cannot live there.
///
/// Public because a copy of a directory has to carry it — see
/// [`crate::ops::copy`] — and one spelling of `.mark` is the only way that
/// copy and this build agree about which file it is.
pub fn mark_path(checkpoint: &Path) -> PathBuf {
    let mut name = checkpoint.as_os_str().to_owned();
    name.push(".mark");
    PathBuf::from(name)
}

/// Where the systems of each region go while they are being read.
///
/// Beside the resume point rather than in the served directory: scratch,
/// the size of the galaxy, and no client may see them.
///
/// Public for the same reason [`mark_path`] is: a copy of a directory
/// has to know this is scratch so that it skips it rather than carrying
/// a galaxy of spills nobody will read — see [`crate::ops::copy`].
pub fn spill_dir(checkpoint: &Path) -> PathBuf {
    let mut name = checkpoint.as_os_str().to_owned();
    name.push(".regions");
    PathBuf::from(name)
}

/// Where a cell's systems are spilled, within a build's spill directory
/// ([`spill_dir`]).
///
/// Level first, so a region's file cannot collide with the buckets or the
/// pieces that a split of it produces, which are at other levels.
pub fn spill_path(dir: &Path, cell: CellId) -> PathBuf {
    dir.join(format!("{:02}-{:016x}.bin", cell.level, cell.morton()))
}
