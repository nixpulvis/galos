//! The bodies of every system, packed into files a shard rather than a file
//! a system.
//!
//! A system's insides are 2.4 KB of MessagePack, written whole and read
//! whole, and they used to be a file each: `bodies/{shard:03x}/{address}.bin`.
//! At a hundred and eighty-eight million of them that is 188 M inodes and
//! **~830 GB** of a 4.4 KB allocation apiece, and — measured with `sample`
//! against a live import — **91 %** of the wall clock, in `open`, `rename`
//! and the directory insert behind them. Nothing about the write path fixes
//! that; the file count is the cost.
//!
//! So a shard is two files:
//!
//! ```text
//! bodies/{shard:03x}.idx            header, a sorted base, an unsorted tail
//! bodies/{shard:03x}.{gen:04x}.dat  the records, appended
//! ```
//!
//! - **A write is two appends**, neither of them a directory operation: the
//!   record (`[u32 len][MessagePack]`) onto the data file, and the entry
//!   (`[i64 address][u64 offset][u32 len]`, 20 B) onto the index. A length of
//!   zero is a tombstone, which is what a withdrawal is — it has to *beat*
//!   the entry behind it rather than be an absence.
//! - **A read** binary-searches the base of the mapped index and scans the
//!   tail newest-first, then reads the record at the offset. Nothing is
//!   resident.
//! - **A fold** merges the tail into the base and writes the whole index
//!   beside the old one and renames it over, so a reader sees one file or the
//!   other and never half of each. The tail is bounded at
//!   `(base / 8).clamp(8 Ki, 52 Ki)` entries: linear in the writes, and a
//!   megabyte of scanning at worst.
//! - **A compaction** is the same fold with the data file rewritten, when
//!   more than half of it is records nothing points at. It writes the *next
//!   generation's* file rather than rewriting in place, because a reader
//!   holding offsets into the old bytes must not be handed new ones; the
//!   index naming the new generation is renamed over in the same step, and a
//!   reader that finds its data file gone reads the index again. That retry
//!   is the whole of the concurrency, there being one writer (the
//!   directory's [`Lock`](crate::Lock)) and any number of readers.
//! - **A sweep** is a compaction of every shard, asked for rather than
//!   waited on: what a whole-galaxy re-import leaves behind, which no
//!   append was ever going to reach. See [`sweep_bodies`].
//!
//! At 200 M systems that is 4,096 index files and a handful of data files
//! rather than 188 M of them, **~450 GB rather than ~830 GB** — the
//! difference is the block a small file rounds up to — and an import whose
//! writes are sequential.
//!
//! ## What is still loose
//!
//! Two older layouts exist and are read, never written: `bodies/{address}.bin`
//! from before the sharding, and `bodies/{shard:03x}/{address}.bin` from
//! before this. [`pack`] walks both into the shards, one batch at a time and
//! interruptibly, and [`crate::store::bodies::read_bodies`] falls back to them until
//! it has, so a directory part way through answers for every system a
//! finished one does.

use crate::format::layout::{
    BODIES_DIR, BODY_SHARDS, bodies_path, body_data_path, body_index_path,
    body_shard, legacy_bodies_path,
};
use crate::records::SystemBodies;
use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;

/// What an index file starts with, so a file that is not one is not read as
/// one.
const MAGIC: [u8; 4] = *b"GPAK";

/// The layout this build reads and writes. A directory in another one is
/// refused rather than guessed at.
const VERSION: u16 = 1;

/// Magic, version, generation, base count, and four bytes spare.
const HEADER: usize = 16;

/// One entry: address, offset, length.
const ENTRY: usize = 20;

/// What the pack has to say about one system.
///
/// Three answers rather than two: a pack that has been told a system's
/// bodies were withdrawn must say so, or a reader would fall back to a loose
/// file this layout replaced and read the withdrawn scan as published.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// The pack holds these bodies.
    Bodies(SystemBodies),
    /// The pack holds a tombstone: this system's bodies were withdrawn.
    Withdrawn,
    /// The pack has never been told about this system.
    Absent,
}

/// What a write of many systems came to.
///
/// A shard is written as one batch, so a batch that fails leaves its systems
/// unwritten and they come back in `kept` for the caller to try again with —
/// which is what [`crate::accumulate::bodies::Published`] does with a disk that is full
/// and then is not.
#[derive(Debug, Default)]
pub struct Wrote {
    /// Systems written.
    pub wrote: usize,
    /// Systems that could not be, and are still the caller's.
    pub kept: HashMap<i64, SystemBodies>,
    /// The first thing that went wrong, where anything did.
    pub failed: Option<io::Error>,
}

/// How far a packing got: what it moved, and whether anything is left.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Packed {
    /// Loose files that are no longer loose.
    pub moved: usize,
    /// Whether nothing loose was left behind. A stop part way answers
    /// `false`; a directory with nothing loose answers `true`.
    pub finished: bool,
}

/// What a sweep of the shards gave back.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Reclaimed {
    /// Shards that gave anything back.
    pub shards: usize,
    /// Bytes the disk no longer holds.
    pub bytes: u64,
    /// Of those, the bytes punched out in place. The rest were copied —
    /// see [`How`], which is where the two are chosen between.
    pub punched: u64,
    /// Whether every shard was looked at. A stop part way answers `false`,
    /// as does a shard that would not reclaim.
    pub finished: bool,
}

/// One entry of a shard's index.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Entry {
    address: i64,
    /// Where the record's frame starts in the data file.
    offset: u64,
    /// The record's bytes, the frame's own four not counted. Zero is a
    /// tombstone.
    len: u32,
}

impl Entry {
    /// The entry `bytes` starts with, which is 20 bytes of it.
    fn of(bytes: &[u8]) -> Entry {
        let address =
            i64::from_le_bytes(bytes[0..8].try_into().expect("eight bytes"));
        let offset =
            u64::from_le_bytes(bytes[8..16].try_into().expect("eight bytes"));
        let len =
            u32::from_le_bytes(bytes[16..20].try_into().expect("four bytes"));
        Entry { address, offset, len }
    }

    /// The same, onto the end of `out`.
    fn onto(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.address.to_le_bytes());
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&self.len.to_le_bytes());
    }

    /// Whether this says the system's bodies were withdrawn.
    fn withdrawn(&self) -> bool {
        self.len == 0
    }
}

/// A shard's index, read whole.
///
/// What the writer works on. A reader answering one system maps the file and
/// searches it instead: at 200 M systems a shard's index is a couple of
/// megabytes, which is nothing to map and too much to read for one click.
#[derive(Debug, Default)]
struct Table {
    generation: u16,
    /// Sorted by address, and the older half of the truth.
    base: Vec<Entry>,
    /// Append order, newest last, and the newer half.
    tail: Vec<Entry>,
}

impl Table {
    /// Read a shard's index, or answer an empty one where there is no file.
    fn read(path: &Path) -> io::Result<Table> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Table::default());
            }
            Err(err) => return Err(err),
        };
        let header = header_of(&bytes, path)?;
        let mut entries = bytes[HEADER..].chunks_exact(ENTRY).map(Entry::of);
        let base = entries.by_ref().take(header.base).collect();
        Table { generation: header.generation, base, tail: entries.collect() }
            .checked(path)
    }

    /// A table whose base is not sorted is a file something else wrote.
    fn checked(self, path: &Path) -> io::Result<Table> {
        match self.base.windows(2).all(|it| it[0].address <= it[1].address) {
            true => Ok(self),
            false => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{}: the index base is not in address order",
                    path.display()
                ),
            )),
        }
    }

    /// Every address the shard stands for, tombstones dropped, newest entry
    /// each.
    fn live(&self) -> BTreeMap<i64, Entry> {
        let mut live = BTreeMap::new();
        for entry in self.base.iter().chain(&self.tail) {
            match entry.withdrawn() {
                true => live.remove(&entry.address),
                false => live.insert(entry.address, *entry),
            };
        }
        live
    }
}

/// How long a shard's tail may grow before it is folded into the base.
///
/// A share of the base, so folding is linear in the writes rather than
/// quadratic, bounded either side so a small shard is not folded on every
/// write and a large one is never scanned for more than a megabyte.
fn tail_bound(base: usize) -> usize {
    (base / 8).clamp(8 * 1024, 52 * 1024)
}

/// How much of a data file has to be dead before a fold rewrites it.
///
/// Dead is what the index points at none of: a record a later write
/// replaced, or one a tombstone withdrew. Reclaiming it is the shard's
/// live bytes written out again, so the bar is what that write is worth.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Dead {
    /// Half the file. What the write path keeps: a feed appending to a
    /// shard it has appended to for months pays the rewrite once the file
    /// holds twice the bytes it needs, and no sooner.
    Half,
    /// A tenth of the file, and at least [`WORTH`] of it. What a sweep
    /// asks by — an operator, or a build that has just rewritten the
    /// galaxy, is paying for the space rather than for the next append.
    Worth,
}

impl Dead {
    /// Whether what [`cost`] measured has reached this bar.
    fn reached(self, cost: &Cost) -> bool {
        match self {
            // `>=`, and not `>`: a re-import replaces every record with
            // one of very nearly the same length, which leaves a shard
            // *at* half rather than past it. The strict test made a
            // re-imported galaxy the one case the rule was written for
            // and the one case it refused.
            Dead::Half => cost.dead >= cost.live,
            // A tenth of what the file holds is `dead * 10 >= dead +
            // live`, which is this.
            Dead::Worth => cost.dead >= WORTH && cost.dead * 9 >= cost.live,
        }
    }
}

/// The least dead bytes worth rewriting a shard for.
///
/// A mebibyte, which is about 400 records. Below that the rewrite costs
/// more in writes than the file gives back in blocks.
const WORTH: u64 = 1 << 20;

/// What an index file's header says.
struct Header {
    generation: u16,
    base: usize,
}

/// Read and check an index file's header, against the whole file
///
/// `bytes` is the file — the mapping or the read of it — because the check
/// that a base of `n` entries has `n` entries behind it can only be made
/// where those bytes are in hand. See [`header_fields`] for the caller
/// that holds the header alone.
fn header_of(bytes: &[u8], path: &Path) -> io::Result<Header> {
    let header = header_fields(bytes, path)?;
    let entries = (bytes.len() - HEADER) / ENTRY;
    if header.base > entries {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{}: a base of {} entries in a file holding {entries}",
                path.display(),
                header.base,
            ),
        ));
    }
    Ok(header)
}

/// What the first sixteen bytes say, and nothing about what follows them
///
/// **Split out because [`append`] holds only those sixteen.** It reads the
/// header off the front of the file to learn the generation and the base,
/// and handing that buffer to [`header_of`] asked it whether a base of
/// 8,218 entries fitted in sixteen bytes — which it does not, so every
/// append to a shard that had ever been folded failed with "a base of 8218
/// entries in a file holding 0". Reported from a real directory, where it
/// stopped the packing of every loose body file the moment it reached one.
fn header_fields(bytes: &[u8], path: &Path) -> io::Result<Header> {
    let refused = |said: String| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {said}", path.display()),
        )
    };
    if bytes.len() < HEADER || bytes[0..4] != MAGIC {
        return Err(refused("not a packed body index".to_string()));
    }
    let version =
        u16::from_le_bytes(bytes[4..6].try_into().expect("two bytes"));
    if version != VERSION {
        return Err(refused(format!(
            "body pack version {version}, this build reads {VERSION}: \
             rebuild the directory"
        )));
    }
    let generation =
        u16::from_le_bytes(bytes[6..8].try_into().expect("two bytes"));
    let base = u32::from_le_bytes(bytes[8..12].try_into().expect("four bytes"))
        as usize;
    Ok(Header { generation, base })
}

/// The sixteen bytes an index file starts with.
fn header_bytes(generation: u16, base: usize) -> [u8; HEADER] {
    let mut head = [0u8; HEADER];
    head[0..4].copy_from_slice(&MAGIC);
    head[4..6].copy_from_slice(&VERSION.to_le_bytes());
    head[6..8].copy_from_slice(&generation.to_le_bytes());
    head[8..12].copy_from_slice(&(base as u32).to_le_bytes());
    head
}

/// What the pack holds for `address`.
///
/// The index is mapped rather than read: the base is binary-searched and the
/// tail scanned newest-first, so a click costs a page or two of a file that
/// may be megabytes. A data file that has gone out from under the read is a
/// compaction landing, and the answer is to read the index again — the new
/// one names the generation that exists.
pub fn find(dir: &Path, address: i64) -> io::Result<Found> {
    match found(dir, address) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            found(dir, address)
        }
        answer => answer,
    }
}

/// One attempt at [`find`].
fn found(dir: &Path, address: i64) -> io::Result<Found> {
    let shard = body_shard(address);
    let path = body_index_path(dir, shard);
    let file = match File::open(&path) {
        Ok(file) => file,
        // No index file is a shard nothing has been packed into, which is
        // not the same as a data file disappearing mid-read: this one is
        // the caller's answer, not a retry.
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(Found::Absent);
        }
        Err(err) => return Err(err),
    };
    // Safety: the file is opened read-only and the mapping is dropped before
    // this returns. A writer only ever appends to it or renames another file
    // over it, so the bytes under the mapping are not rewritten in place.
    let mapped = unsafe { memmap2::Mmap::map(&file)? };
    let header = header_of(&mapped, &path)?;
    let entries = &mapped[HEADER..];
    let at = |n: usize| Entry::of(&entries[n * ENTRY..]);
    let count = entries.len() / ENTRY;

    // The tail first and backwards: it is the newer half, and the last thing
    // said about a system is what the pack holds.
    let mut found = None;
    for n in (header.base..count).rev() {
        let entry = at(n);
        if entry.address == address {
            found = Some(entry);
            break;
        }
    }
    if found.is_none() {
        // The base is sorted, so the rest is a binary search. A system
        // written twice before a fold leaves one entry here, the fold having
        // kept the newer.
        let mut lo = 0usize;
        let mut hi = header.base;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let entry = at(mid);
            match entry.address.cmp(&address) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    found = Some(entry);
                    break;
                }
            }
        }
    }
    let Some(entry) = found else { return Ok(Found::Absent) };
    if entry.withdrawn() {
        return Ok(Found::Withdrawn);
    }
    let generation = header.generation;
    drop(mapped);

    let bytes = record(&body_data_path(dir, shard, generation), &entry)?;
    let inside = rmp_serde::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Found::Bodies(inside))
}

/// One record's bytes out of a data file.
fn record(path: &Path, entry: &Entry) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(entry.offset))?;
    let mut framed = [0u8; 4];
    file.read_exact(&mut framed)?;
    let len = u32::from_le_bytes(framed);
    if len != entry.len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{}: the index says {} bytes at {} and the record says {len}",
                path.display(),
                entry.len,
                entry.offset,
            ),
        ));
    }
    let mut bytes = vec![0u8; len as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Group rows by shard, encoding each record on the way.
///
/// A body that will not encode is not a body a retry fixes, so it is
/// reported rather than batched; the systems either side of it still go.
fn batches<'a>(
    rows: impl IntoIterator<Item = (i64, &'a SystemBodies)>,
) -> (HashMap<u64, Vec<(i64, Vec<u8>)>>, Option<io::Error>) {
    let mut batches: HashMap<u64, Vec<(i64, Vec<u8>)>> = HashMap::new();
    let mut failed = None;
    for (address, inside) in rows {
        match rmp_serde::to_vec(inside) {
            Ok(bytes) => {
                batches
                    .entry(body_shard(address))
                    .or_default()
                    .push((address, bytes));
            }
            Err(err) => {
                failed.get_or_insert(io::Error::new(
                    io::ErrorKind::InvalidData,
                    err,
                ));
            }
        }
    }
    (batches, failed)
}

/// Write what a store is holding into the pack.
///
/// Grouped by shard, so a shard is one open of each of its two files and
/// one append to each however many of the held systems fell in it. A shard
/// that cannot be written leaves its systems with the caller rather than
/// dropping them.
pub fn write(dir: &Path, mut rows: HashMap<i64, SystemBodies>) -> Wrote {
    let (batches, failed) = batches(rows.iter().map(|(a, b)| (*a, b)));
    let mut done = Wrote { failed, ..Wrote::default() };
    for (shard, batch) in batches {
        match append(dir, shard, &batch) {
            Ok(()) => {
                done.wrote += batch.len();
                for (address, _) in &batch {
                    rows.remove(address);
                }
            }
            Err(err) => {
                done.failed.get_or_insert(err);
            }
        }
    }
    done.kept = rows;
    done
}

/// The same, for a caller with nothing to keep: a write that fails is the
/// caller's error rather than a batch handed back.
pub fn write_each<'a>(
    dir: &Path,
    rows: impl IntoIterator<Item = (i64, &'a SystemBodies)>,
) -> io::Result<usize> {
    let (batches, failed) = batches(rows);
    if let Some(err) = failed {
        return Err(err);
    }
    let mut wrote = 0;
    for (shard, batch) in batches {
        append(dir, shard, &batch)?;
        wrote += batch.len();
    }
    Ok(wrote)
}

/// Withdraw a system's bodies, answering whether the pack held any.
///
/// A tombstone rather than an erasure: the entry it hides may be in the
/// base, and an absence would read straight through to it.
pub fn remove(dir: &Path, address: i64) -> io::Result<bool> {
    match find(dir, address)? {
        Found::Bodies(_) => {
            append(dir, body_shard(address), &[(address, Vec::new())])?;
            Ok(true)
        }
        Found::Withdrawn | Found::Absent => Ok(false),
    }
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
    match crate::store::bodies::find(dir, address)? {
        crate::store::bodies::Found::Bodies(inside) => return Ok(inside),
        crate::store::bodies::Found::Withdrawn => {
            return Ok(SystemBodies::default());
        }
        crate::store::bodies::Found::Absent => {}
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
    let mut removed = crate::store::bodies::remove(dir, address)?;
    for path in [bodies_path(dir, address), legacy_bodies_path(dir, address)] {
        match std::fs::remove_file(&path) {
            Ok(()) => removed = true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(removed)
}

/// Append a batch of records to one shard, and fold if the tail has grown.
///
/// The record before the entry that names it, always: a reader that meets an
/// entry must find the bytes it points at, where a record nothing points at
/// is only a few bytes of a data file waiting to be compacted away.
fn append(dir: &Path, shard: u64, batch: &[(i64, Vec<u8>)]) -> io::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir.join(BODIES_DIR))?;
    let path = body_index_path(dir, shard);
    let mut index =
        OpenOptions::new().read(true).write(true).create(true).open(&path)?;
    let mut head = [0u8; HEADER];
    let length = index.metadata()?.len();
    let (generation, base) = match length >= HEADER as u64 {
        true => {
            index.read_exact(&mut head)?;
            // The header alone: what follows it is the file, which this has
            // not read and [`header_fields`] does not ask about.
            let header = header_fields(&head, &path)?;
            let entries = (length as usize - HEADER) / ENTRY;
            if header.base > entries {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{}: a base of {} entries in a file holding {entries}",
                        path.display(),
                        header.base,
                    ),
                ));
            }
            (header.generation, header.base)
        }
        false => {
            index.write_all(&header_bytes(0, 0))?;
            (0, 0)
        }
    };

    let data = body_data_path(dir, shard, generation);
    let mut file = OpenOptions::new().append(true).create(true).open(&data)?;
    let mut at = file.metadata()?.len();
    let mut records = Vec::new();
    let mut entries = Vec::with_capacity(batch.len() * ENTRY);
    for (address, bytes) in batch {
        let len = bytes.len() as u32;
        let entry = match len {
            // A tombstone points nowhere: there is no record to point at.
            0 => Entry { address: *address, offset: 0, len: 0 },
            _ => {
                records.extend_from_slice(&len.to_le_bytes());
                records.extend_from_slice(bytes);
                let entry = Entry { address: *address, offset: at, len };
                at += 4 + bytes.len() as u64;
                entry
            }
        };
        entry.onto(&mut entries);
    }
    file.write_all(&records)?;
    file.flush()?;

    index.seek(SeekFrom::End(0))?;
    index.write_all(&entries)?;
    index.flush()?;

    let entries = (index.metadata()?.len() as usize - HEADER) / ENTRY;
    let tail = entries - base;
    match tail > tail_bound(base) {
        true => fold(dir, shard),
        false => Ok(()),
    }
}

/// Merge a shard's tail into its base, and reclaim the data file where
/// enough of it is dead.
///
/// The index is written beside and renamed over, so a reader sees one whole
/// index or the other. A compaction writes the *next* generation's data file
/// and leaves the old one until the index naming the new one is in place: a
/// reader holding offsets into the old bytes has to go on being right about
/// them until it reads the index again.
///
/// At [`Dead::Half`], which is the write path's bar. [`reclaim`] is the
/// same fold at a sweep's.
pub fn fold(dir: &Path, shard: u64) -> io::Result<()> {
    folded(dir, shard, Dead::Half).map(|_| ())
}

/// Give one shard's dead bytes back, where a sweep's bar is reached.
pub fn reclaim(dir: &Path, shard: u64) -> io::Result<Gave> {
    folded(dir, shard, Dead::Worth)
}

/// What one shard gave back, and how.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Gave {
    /// Bytes the disk no longer holds for this shard.
    pub bytes: u64,
    /// Whether they were punched out in place rather than copied away.
    pub punched: bool,
}

/// One fold, at whichever bar the caller keeps.
fn folded(dir: &Path, shard: u64, bar: Dead) -> io::Result<Gave> {
    let path = body_index_path(dir, shard);
    let table = Table::read(&path)?;
    let live = table.live();
    let data = body_data_path(dir, shard, table.generation);
    let cost = cost(&data, &live)?;

    // Half the file being records nothing points at is the write path's
    // trigger: a feed's thirty systems a second leave about six gigabytes
    // of dead records a day over the galaxy, which reaches half a shard in
    // a couple of months.
    let reclaiming = bar.reached(&cost);

    // Nothing to merge and nothing to reclaim.
    //
    // **The tail being empty was once the whole of this test**, which put
    // a compaction out of reach of the shard that most needs one: a folded
    // shard whose data file is half dead is exactly what a re-import
    // leaves, and no later write folds it because the tail it left is
    // under [`tail_bound`]. Measured on a re-imported galaxy — 4,096
    // shards, 49.8 % of their bytes live, and not one of them foldable.
    if table.tail.is_empty() && !reclaiming {
        return Ok(Gave::default());
    }

    // Punched where the dead bytes are whole blocks and the file has not
    // outgrown what it holds, copied where they are not. See [`How`].
    let mut how = match reclaiming {
        true => How::of(&cost),
        false => How::Nothing,
    };
    let mut gave = Gave::default();

    // **Before the index, and the copy after it.** The two are not the
    // same operation: a punch takes bytes nothing points at and leaves
    // every live offset where it was, so a reader mapping the index at
    // any point either side of it reads the same records. The copy moves
    // them, which is why it has to hand the index over first.
    //
    // A filesystem that will not punch says so here — no hole is opened
    // by a failed `fcntl` — and the shard takes the copy instead.
    if how == How::Punch {
        match punched(&data, &cost) {
            Ok(()) => gave = Gave { bytes: cost.punchable, punched: true },
            Err(_) => how = How::Copy,
        }
    }

    let generation = match how {
        How::Copy => table.generation.wrapping_add(1),
        How::Punch | How::Nothing => table.generation,
    };
    let mut base = Vec::with_capacity(live.len());
    match how {
        How::Copy => {
            let to = body_data_path(dir, shard, generation);
            let mut out = io::BufWriter::new(File::create(&to)?);
            let mut from = File::open(&data)?;
            let mut at = 0u64;
            for entry in live.values() {
                from.seek(SeekFrom::Start(entry.offset))?;
                let mut framed = vec![0u8; 4 + entry.len as usize];
                from.read_exact(&mut framed)?;
                out.write_all(&framed)?;
                base.push(Entry { offset: at, ..*entry });
                at += framed.len() as u64;
            }
            out.flush()?;
            gave = Gave { bytes: cost.dead, punched: false };
        }
        // In place, so the entries are the entries: every live offset
        // still names the byte it named before.
        How::Punch | How::Nothing => base.extend(live.values().copied()),
    }

    let mut bytes = Vec::with_capacity(HEADER + base.len() * ENTRY);
    bytes.extend_from_slice(&header_bytes(generation, base.len()));
    for entry in &base {
        entry.onto(&mut bytes);
    }
    let tmp = path.with_extension("idx.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;

    if how == How::Copy {
        // Nothing reads this generation any more: the index naming it is
        // gone, and a reader that had already read it retries on the miss.
        let _ = std::fs::remove_file(&data);
    }
    Ok(gave)
}

/// Punch every whole block of a shard's data file that nothing points at.
///
/// All or nothing as far as the caller is concerned: a filesystem that
/// refuses the first hole has opened none of them, and one that refuses a
/// later one has opened holes only in runs that were already dead. Either
/// way the error sends the shard down the copy, which reclaims whatever
/// is left.
fn punched(data: &Path, cost: &Cost) -> io::Result<()> {
    let file = OpenOptions::new().write(true).open(data)?;
    for &(at, len) in &cost.holes {
        punch(&file, at, len)?;
    }
    Ok(())
}

/// How a shard gives its dead bytes back.
///
/// **The copy is the expensive one, and was the only one.** It reads every
/// live record and writes it into the next generation — 160 GB moved to
/// free 161 GB, measured over a re-imported galaxy — because a reader
/// holding an offset into the old bytes must not be handed new ones.
///
/// A hole moves nothing and hands nobody anything: the live records stay
/// at the offsets the index already names, and the blocks under the dead
/// runs go back to the filesystem. Which is the shape a re-import leaves —
/// the first import's records are one run ahead of the second's, so a
/// shard is a single punch of about half the file.
///
/// It is not always available and not always enough:
///
/// - A filesystem that cannot punch answers an error, and the copy is what
///   the sweep falls back to.
/// - Blocks are the granularity, so dead records finely interleaved with
///   live ones — what a feed leaves — free nothing. Under [`PUNCHED`] of
///   the dead bytes, the copy is the honest answer.
/// - A punched file keeps its length, and appends go on past it, so a
///   shard punched for ever is a file whose length grows without bound —
///   and a `cp` or an `rsync` that does not understand holes copies the
///   length rather than the blocks. [`BLOAT`] is where that stops: past
///   it the shard is copied, which sets the length back to what it holds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum How {
    /// Punch the dead runs; the file keeps its offsets and its length.
    Punch,
    /// Write the live records into the next generation.
    Copy,
    /// Fold the index and leave the data file alone.
    Nothing,
}

/// What share of a shard's dead bytes a punch has to reach to be worth
/// preferring to the copy, as the divisor of `(n-1)/n`.
///
/// Four, so three quarters: below that the file is left mostly dead and
/// the copy is the thing that actually reclaims it.
const PUNCHED: u64 = 4;

/// How far a data file's length may run past the bytes it holds before a
/// shard is copied rather than punched again.
///
/// Four, which is three re-imports of a galaxy before a shard is rewritten
/// once. The length is what a tool that does not understand holes copies.
const BLOAT: u64 = 4;

impl How {
    /// Which way this shard gives its dead bytes back.
    fn of(cost: &Cost) -> How {
        match cost.punchable * PUNCHED >= cost.dead * (PUNCHED - 1)
            && cost.length <= cost.live.saturating_mul(BLOAT)
        {
            true => How::Punch,
            false => How::Copy,
        }
    }
}

/// What a shard's data file costs, weighed against the index over it.
///
/// **Measured against the file's own extents, not against its size.**
/// Neither number is the truth on its own: a punched file keeps the
/// length it grew to, and an appended file is over-allocated past its end
/// — 84.6 MB of blocks behind a 79.1 MB shard, measured on APFS. Weighing
/// by either makes a shard that has just been swept look worth sweeping
/// again, for ever. What is dead is the bytes that are *in* the file, are
/// backed by blocks, and have no entry pointing at them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Cost {
    /// Bytes of record the index points at.
    live: u64,
    /// Bytes the file holds that it points at none of.
    dead: u64,
    /// Of the dead, what punching [`Cost::holes`] would free: whole
    /// blocks, and only the ones still backed by data.
    punchable: u64,
    /// The file's length, which after a punch is more than it costs.
    length: u64,
    /// What the disk holds for it, which is what `du` reports.
    allocated: u64,
    /// The block-aligned dead runs, ready to punch.
    holes: Vec<(u64, u64)>,
}

/// Weigh a data file against the live entries of the index over it.
fn cost(data: &Path, live: &BTreeMap<i64, Entry>) -> io::Result<Cost> {
    let Ok(meta) = std::fs::metadata(data) else {
        return Ok(Cost::default());
    };
    #[cfg(unix)]
    let (length, allocated, block) = {
        use std::os::unix::fs::MetadataExt;
        (meta.len(), meta.blocks() * 512, meta.blksize().max(512))
    };
    #[cfg(not(unix))]
    let (length, allocated, block) = (meta.len(), meta.len(), 4096);

    let mut cost = Cost { length, allocated, ..Cost::default() };
    if length == 0 {
        return Ok(cost);
    }
    cost.live = live.values().map(|it| 4 + it.len as u64).sum();

    let file = File::open(data)?;
    let data = extents(&file, length);
    for (from, to) in gaps(live, length) {
        cost.dead += backed(&data, from, to);
        // Pulled *in* to the blocks inside the run: a hole is whole
        // blocks or it is nothing, and a range that is not block-aligned
        // is refused outright — `EINVAL` from `F_PUNCHHOLE`, measured —
        // rather than rounded for you.
        let (from, to) = (from.div_ceil(block) * block, to / block * block);
        if to > from {
            let backed = backed(&data, from, to);
            if backed > 0 {
                cost.punchable += backed;
                cost.holes.push((from, to - from));
            }
        }
    }
    Ok(cost)
}

/// The runs of a data file no live entry covers, in file order.
fn gaps(live: &BTreeMap<i64, Entry>, length: u64) -> Vec<(u64, u64)> {
    let mut spans: Vec<(u64, u64)> = live
        .values()
        .map(|it| (it.offset, it.offset + 4 + it.len as u64))
        .collect();
    spans.sort_unstable();

    let mut at = 0u64;
    let mut runs = Vec::new();
    for (from, to) in spans {
        if from > at {
            runs.push((at, from));
        }
        at = at.max(to);
    }
    if length > at {
        runs.push((at, length));
    }
    runs
}

/// Where a file's bytes actually are.
///
/// `SEEK_DATA` and `SEEK_HOLE`, walked once: a file that has never been
/// punched answers one extent and a swept one answers a handful. This is
/// what keeps the weighing honest across a sweep — the runs a previous
/// sweep punched are holes, and a hole is not dead weight, it is nothing
/// at all.
///
/// A filesystem that does not answer these is taken at its length, which
/// is what every filesystem looked like before holes.
fn extents(file: &File, length: u64) -> Vec<(u64, u64)> {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "linux",
        target_os = "android"
    ))]
    {
        use std::os::fd::AsRawFd;
        let fd = file.as_raw_fd();
        // SAFETY: two seeks on a live descriptor, neither of which moves
        // anything this process reads through.
        let seek = |from: u64, whence: libc::c_int| -> Option<u64> {
            match unsafe { libc::lseek(fd, from as libc::off_t, whence) } {
                -1 => None,
                at => Some(at as u64),
            }
        };
        let mut found = Vec::new();
        let mut at = 0u64;
        while at < length {
            let Some(from) = seek(at, libc::SEEK_DATA) else { break };
            let to = seek(from, libc::SEEK_HOLE).unwrap_or(length).min(length);
            if to <= from {
                break;
            }
            found.push((from, to));
            at = to;
        }
        // An answer of nothing is a file that is all hole; an error on
        // the first seek is a filesystem that does not answer, and it is
        // told apart by whether anything was found before the break.
        if !found.is_empty() || seek(0, libc::SEEK_DATA).is_none() {
            return found;
        }
    }
    let _ = file;
    vec![(0, length)]
}

/// How much of `[from, to)` is backed by bytes rather than by hole.
fn backed(extents: &[(u64, u64)], from: u64, to: u64) -> u64 {
    extents
        .iter()
        .map(|&(at, end)| end.min(to).saturating_sub(at.max(from)))
        .sum()
}

/// Give the blocks under `[at, at + len)` back to the filesystem.
///
/// The file keeps its length and the range reads as zeroes. Offset and
/// length must both be block-aligned; [`holes`] is the only caller and is
/// where they are aligned.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn punch(file: &File, at: u64, len: u64) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    /// `fpunchhole_t`, whose two spare words must be zero.
    #[repr(C)]
    struct Punchhole {
        flags: u32,
        reserved: u32,
        offset: libc::off_t,
        length: libc::off_t,
    }
    let hole = Punchhole {
        flags: 0,
        reserved: 0,
        offset: at as libc::off_t,
        length: len as libc::off_t,
    };
    // SAFETY: `F_PUNCHHOLE` reads one `fpunchhole_t` through the pointer,
    // which is live for the call, as is the descriptor.
    match unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PUNCHHOLE, &hole) } {
        -1 => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

/// The same, where the hole is `fallocate`'s to punch.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn punch(file: &File, at: u64, len: u64) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: a syscall against a live descriptor; nothing is read through
    // a pointer.
    let done = unsafe {
        libc::fallocate(
            file.as_raw_fd(),
            libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE,
            at as libc::off_t,
            len as libc::off_t,
        )
    };
    match done {
        -1 => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

/// And where it is nobody's: the copy is the whole of the reclaim.
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android"
)))]
fn punch(_file: &File, _at: u64, _len: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this platform cannot punch a hole in a file",
    ))
}

/// What the body shards hold, and what a sweep would give back.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Held {
    /// Shards with an index file.
    pub shards: usize,
    /// Systems the pack answers for.
    pub records: u64,
    /// Bytes of record those systems are.
    pub live: u64,
    /// Bytes the disk holds for the data files, live and dead together.
    pub allocated: u64,
    /// Of those, bytes nothing points at.
    pub dead: u64,
    /// Of the dead, what a sweep would actually give back: a shard under
    /// the bar is left alone, and a punch frees whole blocks or nothing.
    pub reclaimable: u64,
    /// Body files still loose in the older layouts, which `pack` moves.
    pub loose: u64,
    /// Whether every shard was looked at. A stop part way answers `false`.
    pub finished: bool,
}

/// Weigh a directory's body shards, writing nothing.
///
/// What `galos index verify` reports, and what `galos index sweep
/// --bodies` says before it is asked to act. One read of each shard's
/// index and one walk of its data file's extents — a second over a
/// galaxy — and nothing decoded at all.
pub fn weigh(dir: &Path, stop: &dyn Fn() -> bool) -> io::Result<Held> {
    let bodies = dir.join(BODIES_DIR);
    let mut held = Held { finished: true, ..Held::default() };
    let Ok(entries) = std::fs::read_dir(&bodies) else {
        return Ok(held);
    };
    // The loose files of both older layouts, counted on the way past: a
    // directory part way through its packing has systems the shards do not
    // answer for, and a count that did not say so would read as a galaxy
    // with holes in it.
    for entry in entries.flatten() {
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                held.loose += std::fs::read_dir(entry.path())
                    .map(|it| it.flatten().count() as u64)
                    .unwrap_or(0);
            }
            Ok(_) => {
                let name = entry.file_name();
                held.loose +=
                    u64::from(name.to_string_lossy().ends_with(".bin"));
            }
            Err(_) => {}
        }
    }

    for shard in 0..BODY_SHARDS {
        if stop() {
            held.finished = false;
            return Ok(held);
        }
        let path = body_index_path(dir, shard);
        if !path.exists() {
            continue;
        }
        let table = Table::read(&path)?;
        let live = table.live();
        let cost = cost(&body_data_path(dir, shard, table.generation), &live)?;
        held.shards += 1;
        held.records += live.len() as u64;
        held.live += cost.live;
        held.allocated += cost.allocated;
        held.dead += cost.dead;
        if Dead::Worth.reached(&cost) {
            held.reclaimable += match How::of(&cost) {
                How::Punch => cost.punchable,
                How::Copy | How::Nothing => cost.dead,
            };
        }
    }
    Ok(held)
}

/// Give a directory's dead body records back, shard by shard.
///
/// **What a whole-galaxy re-import leaves behind.** A dump names each
/// system once and [`crate::accumulate::bodies::Published::raising`] writes each record
/// without reading what stood there, so a second import over a published
/// directory appends a fresh record for every system and the one behind it
/// is dead the moment the entry naming the new one lands. Nothing on the
/// write path reclaims those: [`append`] folds when a shard's tail passes
/// [`tail_bound`], and an import leaves every tail well under it. Measured
/// on a re-imported galaxy: `bodies/` at 323 GB, 49.8 % of it live, and
/// 161 GB of dead record no append was going to reach.
///
/// So the reclaim is asked for rather than waited on: by
/// [`Build::finish`](crate::build::cold::Build::finish) once its index file
/// stands, and by `galos index sweep --bodies` for a directory nothing is
/// about to build. [`held`] weighs what this would do without doing any of
/// it, which is what that command reports before it is asked to act.
///
/// `said` is handed the running total as each shard lands. A galaxy is
/// minutes of this, and minutes of silence is a run nobody can tell from a
/// hang; it is called from the worker threads, and from several at once.
///
/// **Nothing a stop or a kill can spoil, and no system at risk.** A shard
/// is reclaimed whole — punched behind an index that no longer names the
/// dead runs, or copied into the next generation with the old one unlinked
/// only once the index naming the new one is in place — and every live
/// record is in hand throughout. What an interruption leaves is a
/// directory part way through the sweep, which is to say one that is
/// merely larger. That is the difference between this and clearing
/// `bodies/` before a re-import, which would take with it every system the
/// new read does not reach.
///
/// Safe beside a map *reading* the directory: a reader whose data file goes
/// out from under it reads the index again — see [`find`]. Not safe beside
/// anything *writing* it, which is what [`crate::Lock`] is for.
pub fn sweep_bodies(
    dir: &Path,
    stop: &(dyn Fn() -> bool + Sync),
    said: &(dyn Fn(&Reclaimed) + Sync),
) -> io::Result<Reclaimed> {
    if !dir.join(BODIES_DIR).is_dir() {
        return Ok(Reclaimed { finished: true, ..Reclaimed::default() });
    }
    // A shard at a time, several shards at once, for the reason [`pack`]
    // deals its directories out to threads: two shards share nothing but
    // the disk, and the disk will take more in flight than one thread asks
    // of it. Sequential inside a shard, the index being appended to and
    // renamed over.
    let hands = std::thread::available_parallelism()
        .map(|it| it.get().min(PACKERS))
        .unwrap_or(1);
    let next = std::sync::atomic::AtomicU64::new(0);
    let shards = std::sync::atomic::AtomicUsize::new(0);
    let punched = std::sync::atomic::AtomicU64::new(0);
    let bytes = std::sync::atomic::AtomicU64::new(0);
    let done = std::sync::atomic::AtomicBool::new(true);
    let failed = std::sync::Mutex::<Option<io::Error>>::new(None);
    let running = || Reclaimed {
        shards: shards.load(Relaxed),
        bytes: bytes.load(Relaxed),
        punched: punched.load(Relaxed),
        finished: done.load(Relaxed),
    };
    std::thread::scope(|threads| {
        for _ in 0..hands {
            threads.spawn(|| {
                loop {
                    if stop() {
                        done.store(false, Relaxed);
                        break;
                    }
                    let shard = next.fetch_add(1, Relaxed);
                    if shard >= BODY_SHARDS {
                        break;
                    }
                    match reclaim(dir, shard) {
                        Ok(gave) if gave.bytes == 0 => {}
                        Ok(gave) => {
                            shards.fetch_add(1, Relaxed);
                            bytes.fetch_add(gave.bytes, Relaxed);
                            if gave.punched {
                                punched.fetch_add(gave.bytes, Relaxed);
                            }
                            said(&running());
                        }
                        // The first failure is the answer rather than a
                        // number folded into a total: a shard that will
                        // not reclaim is a file to go and look at, and
                        // what has been given back already stands.
                        Err(err) => {
                            if let Ok(mut failed) = failed.lock() {
                                failed.get_or_insert(err);
                            }
                            done.store(false, Relaxed);
                            break;
                        }
                    }
                }
            });
        }
    });
    if let Some(err) = failed.into_inner().unwrap_or(None) {
        return Err(err);
    }
    Ok(running())
}

/// Every system the pack holds bodies for, in address order.
/// What every packed system's arrival star is, shard by shard
///
/// **Sequential on purpose.** [`find`] maps a shard's index, searches it and
/// seeks the data file, which is right for one system and wrong for ninety
/// five million: a sweep that asked it per address would map the same index
/// a thousand times a shard and seek at random through gigabytes. This maps
/// each shard once and walks its live entries in the order they were
/// written.
///
/// `each` is handed the address and the class of the star a ship arrives
/// at — [`crate::records::derive::arrival_class`]'s answer, which is the rule the
/// boost table and the map's own panels read by. Systems with nothing
/// scanned are not offered at all.
///
/// Interruptible, a galaxy of scans being minutes of them, and what it
/// abandons costs nothing: the caller is filling in a column it can fill
/// again.
pub fn each_arrival_class(
    dir: &Path,
    stop: &dyn Fn() -> bool,
    each: &mut dyn FnMut(i64, &str),
) -> io::Result<u64> {
    let mut swept = 0u64;
    let bodies = dir.join(BODIES_DIR);
    let Ok(entries) = std::fs::read_dir(&bodies) else {
        return Ok(swept);
    };
    for shard in entries.flatten() {
        if stop() {
            return Ok(swept);
        }
        let path = shard.path();
        if path.extension().is_none_or(|it| it != "idx") {
            continue;
        }
        let table = Table::read(&path)?;
        let live = table.live();
        if live.is_empty() {
            continue;
        }
        let Some(shard) = path
            .file_stem()
            .and_then(|it| it.to_str())
            .and_then(|it| u64::from_str_radix(it, 16).ok())
        else {
            continue;
        };
        let data = body_data_path(dir, shard, table.generation);
        let Ok(file) = File::open(&data) else { continue };
        // SAFETY: a shard's data file is appended to and never rewritten in
        // place, and the mapping is dropped before the next shard.
        let mapped = unsafe { memmap2::Mmap::map(&file)? };

        // In the order the records were written rather than by address: a
        // sweep is a sequential read of the file and the addresses are
        // whatever order that gives.
        let mut rows: Vec<(i64, u64, u32)> = live
            .into_iter()
            .map(|(address, entry)| (address, entry.offset, entry.len))
            .collect();
        rows.sort_unstable_by_key(|&(_, offset, _)| offset);

        for (address, offset, len) in rows {
            if stop() {
                return Ok(swept);
            }
            let from = offset as usize + 4;
            let Some(bytes) = mapped.get(from..from + len as usize) else {
                continue;
            };
            let Ok(inside) =
                rmp_serde::from_slice::<crate::records::SystemBodies>(bytes)
            else {
                continue;
            };
            if let Some(class) = crate::records::derive::arrival_class(&inside)
            {
                each(address, class);
                swept += 1;
            }
        }
    }
    Ok(swept)
}

/// Every address the pack answers for, one shard's index at a time.
///
/// The cheap walk: the indexes are read and the data files are not
/// touched at all, which is the difference between this and
/// [`each_arrival_class`]. A caller weighing a galaxy's bodies against the
/// tree that names them wants exactly this and nothing decoded.
///
/// Interruptible, and what it abandons costs nothing: the caller is
/// counting, and a count cut short says so.
pub fn each_address(
    dir: &Path,
    stop: &dyn Fn() -> bool,
    each: &mut dyn FnMut(i64),
) -> io::Result<bool> {
    for shard in 0..BODY_SHARDS {
        if stop() {
            return Ok(false);
        }
        let path = body_index_path(dir, shard);
        if !path.exists() {
            continue;
        }
        for address in Table::read(&path)?.live().into_keys() {
            each(address);
        }
    }
    Ok(true)
}

/// Every address the pack answers for, in address order.
///
/// Held whole, which at a galaxy's scale is 76 million of them and 609 MB:
/// a caller that only wants to count them should take
/// [`each_address`] instead.
pub fn addresses(dir: &Path) -> io::Result<Vec<i64>> {
    let mut addresses = Vec::new();
    each_address(dir, &|| false, &mut |address| addresses.push(address))?;
    addresses.sort_unstable();
    Ok(addresses)
}

/// How many loose files are packed between one look at the stop flag.
///
/// A batch is held in memory — 2.4 KB a system — and is what one shard's
/// two appends cover, so the flag is asked often enough for a Ctrl-C to be
/// quick and rarely enough that the appends are worth making.
const BATCH: usize = 512;

/// Walk a directory's loose body files into the shards.
///
/// Both older layouts at once: `bodies/{address}.bin` from before the
/// sharding and `bodies/{shard:03x}/{address}.bin` from before this. A
/// file's bytes are already the record the pack stores, so nothing is
/// decoded on the way through.
///
/// Interruptible, because a galaxy of loose files is hours of them and a run
/// asked to stop must not wait. What it abandons the next open takes up: a
/// loose file is removed only once the pack has its record, and
/// [`crate::store::bodies::read_bodies`] falls back to the loose paths for whatever
/// is left, so a directory part way through answers for every system a
/// finished one does.
///
/// A system the pack already holds wins over a loose file of the same
/// address: the pack is where the newer write went, and reading the loose
/// one back over it would put a stale scan back.
pub fn pack(
    dir: &Path,
    stop: &(dyn Fn() -> bool + Sync),
) -> io::Result<Packed> {
    let bodies = dir.join(BODIES_DIR);
    let mut moved = 0;
    let mut loose: Vec<PathBuf> = Vec::new();
    let mut shards: Vec<PathBuf> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&bodies) else {
        return Ok(Packed { moved, finished: true });
    };
    for entry in entries.flatten() {
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => shards.push(entry.path()),
            Ok(_) => loose.push(entry.path()),
            Err(_) => {}
        }
    }

    let mut finished = true;
    // The loose files at the top level are of every shard at once, so they
    // are removed as they are taken: there is no directory to drop.
    if take(dir, loose, &mut moved, stop, Removal::Eager)? == Took::Stopped {
        return Ok(Packed { moved, finished: false });
    }
    // **A shard at a time, several shards at once.** Each directory's
    // files belong to one shard, and a shard is its own index and its own
    // data file, so two of them share nothing but the disk. What the work
    // is bound by is small reads and metadata — measured at 2,100 files a
    // second on one thread, where the drive will take several times that
    // in flight — so the directories are dealt out to a few threads and
    // each keeps its own `Holds` and its own batches.
    //
    // Sequential inside a shard all the same: its index is appended to and
    // folded, and two threads doing that to one file is a corrupt shard.
    let hands = std::thread::available_parallelism()
        .map(|it| it.get().min(PACKERS))
        .unwrap_or(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let packed = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicBool::new(true);
    let shards = &shards;
    std::thread::scope(|threads| {
        for _ in 0..hands {
            threads.spawn(|| {
                let mut mine = 0usize;
                loop {
                    let at = next.fetch_add(1, Relaxed);
                    let Some(shard) = shards.get(at) else { break };
                    match one_shard(dir, shard, &mut mine, stop) {
                        Ok(true) => {}
                        // Stopped, or a shard that would not pack: either
                        // way the run is not finished and the rest of the
                        // list is left for the next one.
                        Ok(false) | Err(_) => {
                            done.store(false, Relaxed);
                            break;
                        }
                    }
                }
                packed.fetch_add(mine, Relaxed);
            });
        }
    });
    moved += packed.load(Relaxed);
    if !done.load(Relaxed) {
        finished = false;
    }
    Ok(Packed { moved, finished })
}

/// How many shards are packed at once
///
/// A few, not a core each: the work is the disk's and a queue of thirty-two
/// readers deep is no faster than eight. Bounded so a pack running beside a
/// map leaves it some.
const PACKERS: usize = 8;

/// Pack one shard's directory, answering whether it got through it
///
/// Its own function because a thread wants it whole: the directory's files
/// are all one shard's, so the appends, the fold they may trigger and the
/// removal are one shard's business and no other thread's.
fn one_shard(
    dir: &Path,
    shard: &Path,
    moved: &mut usize,
    stop: &(dyn Fn() -> bool + Sync),
) -> io::Result<bool> {
    {
        let listed: Vec<PathBuf> = match std::fs::read_dir(shard) {
            Ok(entries) => entries.flatten().map(|it| it.path()).collect(),
            Err(_) => return Ok(true),
        };
        // **The directory goes in one call, not a file at a time.** Fifty
        // million `unlink`s is what a galaxy of loose files costs, and on a
        // directory of thirteen thousand entries each one walks its
        // metadata: measured at 670 files a second, and 1,350 once the
        // membership question stopped being a `find` apiece. A shard's
        // files are all one shard's, so they are appended in batches and
        // the directory taken away whole once its records are durable.
        //
        // Sound where a file at a time was sound, and for the same reason:
        // the records go in before anything is removed, and a run cut
        // short leaves files the next run recognises as already held and
        // drops. Only where *everything* in it was taken — a directory
        // holding something this does not understand keeps that thing, and
        // the files are then removed one by one as before.
        match take(dir, listed, moved, stop, Removal::Deferred)? {
            Took::Stopped => return Ok(false),
            Took::Every(count) => {
                std::fs::remove_dir_all(shard)?;
                *moved += count;
            }
            Took::Some => {
                // Something unrecognised stands in it; whatever this took
                // has already been removed a file at a time.
                let _ = std::fs::remove_dir(shard);
            }
        }
    }
    Ok(true)
}

/// What the pack already holds, one shard's worth at a time
///
/// **Why a pack of fifty million files took twenty hours.** The question
/// asked of every loose file is whether the pack already holds that
/// system — the pack being the newer of the two wherever both exist — and
/// it used to be asked with [`find`], which maps the shard's index, scans
/// its tail backwards, binary searches its base and reads the data file.
/// Fifty million times over, that is the whole of the cost: measured at
/// 670 files a second, where the reads and unlinks alone are thousands.
///
/// One entry, not a map of every shard: the walk takes a shard's directory
/// at a time, so the answer wanted is nearly always the one already
/// loaded, and a galaxy's worth of live sets held at once would be
/// hundreds of megabytes for nothing.
struct Holds {
    shard: Option<u64>,
    live: std::collections::HashSet<i64>,
}

impl Holds {
    fn new() -> Holds {
        Holds { shard: None, live: std::collections::HashSet::new() }
    }

    /// Whether the pack holds `address` already.
    fn holds(&mut self, dir: &Path, address: i64) -> io::Result<bool> {
        let shard = body_shard(address);
        if self.shard != Some(shard) {
            let table = Table::read(&body_index_path(dir, shard))?;
            self.live = table.live().into_keys().collect();
            self.shard = Some(shard);
        }
        Ok(self.live.contains(&address))
    }

    /// And what this run has just put there, so a second loose file of the
    /// same address is dropped rather than appended twice — which is what
    /// asking [`find`] afresh would have concluded.
    fn took(&mut self, address: i64) {
        if self.shard == Some(body_shard(address)) {
            self.live.insert(address);
        }
    }
}

/// Whether a packed file is removed as it goes or left for its directory
#[derive(Copy, Clone, PartialEq)]
enum Removal {
    /// Removed one at a time, there being no directory to take away.
    Eager,
    /// Left where it is: the caller drops the whole directory, which is
    /// one call against thirteen thousand.
    Deferred,
}

/// What a pass over a list of paths came to.
#[derive(Copy, Clone, PartialEq, Debug)]
enum Took {
    /// Every path, and how many — so a caller dropping the directory whole
    /// can still say what it moved.
    Every(usize),
    /// All it could; something in the list was not a loose body file.
    Some,
    /// Asked to stop part way.
    Stopped,
}

/// Pack a list of paths, answering what it got through.
fn take(
    dir: &Path,
    paths: Vec<PathBuf>,
    moved: &mut usize,
    stop: &(dyn Fn() -> bool + Sync),
    removal: Removal,
) -> io::Result<Took> {
    let mut batch: HashMap<u64, Vec<(i64, PathBuf, Vec<u8>)>> = HashMap::new();
    let mut holds = Holds::new();
    let mut held = 0usize;
    // What this took, against what stood there: a directory is only taken
    // away whole where the two agree.
    let mut taken = 0usize;
    let mut every = true;
    // What was packed and not yet removed, where the caller meant to drop
    // the whole directory. See the `Took::Some` arm below.
    let mut deferred: Vec<PathBuf> = Vec::new();
    for path in paths {
        if stop() {
            settle(dir, &mut batch, moved, &mut holds, removal, &mut deferred)?;
            return Ok(Took::Stopped);
        }
        if path.extension().is_none_or(|it| it != "bin") {
            every = false;
            continue;
        }
        let Some(address) = path
            .file_stem()
            .and_then(|it| it.to_str())
            .and_then(|it| it.parse::<i64>().ok())
        else {
            every = false;
            continue;
        };
        taken += 1;
        // The pack is the newer of the two wherever both exist.
        if holds.holds(dir, address)? {
            match removal {
                Removal::Eager => {
                    std::fs::remove_file(&path)?;
                    *moved += 1;
                }
                Removal::Deferred => deferred.push(path),
            }
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        batch
            .entry(body_shard(address))
            .or_default()
            .push((address, path, bytes));
        held += 1;
        if held >= BATCH {
            settle(dir, &mut batch, moved, &mut holds, removal, &mut deferred)?;
            held = 0;
        }
    }
    settle(dir, &mut batch, moved, &mut holds, removal, &mut deferred)?;
    if every {
        // The caller drops the directory, which takes these with it.
        return Ok(Took::Every(taken));
    }

    // Something in there is not a loose body file, so the directory stays
    // and what was packed out of it goes a file at a time after all.
    for path in deferred {
        std::fs::remove_file(&path)?;
        *moved += 1;
    }
    Ok(Took::Some)
}

/// Append a batch and drop the loose files it came from.
///
/// In that order: a file removed before its record was durable is a system
/// nothing holds.
fn settle(
    dir: &Path,
    batch: &mut HashMap<u64, Vec<(i64, PathBuf, Vec<u8>)>>,
    moved: &mut usize,
    holds: &mut Holds,
    removal: Removal,
    deferred: &mut Vec<PathBuf>,
) -> io::Result<()> {
    for (shard, rows) in batch.drain() {
        // The paths kept aside and the bytes handed over: a batch of five
        // hundred records is a megabyte, and copying it to hand it on was
        // a hundred and twenty gigabytes of `memcpy` over a galaxy.
        let mut paths = Vec::with_capacity(rows.len());
        let mut records = Vec::with_capacity(rows.len());
        for (address, path, bytes) in rows {
            paths.push((address, path));
            records.push((address, bytes));
        }
        append(dir, shard, &records)?;
        for (address, path) in paths {
            holds.took(address);
            match removal {
                Removal::Eager => {
                    std::fs::remove_file(&path)?;
                    *moved += 1;
                }
                // Kept, in case the directory turns out to hold something
                // this does not understand and cannot be dropped whole.
                Removal::Deferred => deferred.push(path),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{Barycenter, Star};

    fn scratch(name: &str) -> PathBuf {
        let at = std::env::temp_dir()
            .join(format!("galos-pack-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&at);
        std::fs::create_dir_all(at.join(BODIES_DIR)).expect("a directory");
        at
    }

    fn held(dir: &Path, address: i64) -> Found {
        find(dir, address).expect("the pack reads")
    }

    fn inside(id: i16) -> SystemBodies {
        SystemBodies {
            stars: vec![Star {
                system_address: 1,
                id,
                name: format!("Star {id}"),
                parents: Vec::new(),
                updated_at: "2026-08-08T12:00:00Z".parse().expect("a moment"),
                updated_by: "a test".into(),
                absolute_magnitude: 4.83,
                age_my: 4600,
                distance_from_arrival_ls: 0.0,
                luminosity: "V".into(),
                star_class: "G".into(),
                stellar_mass: 1.0,
                subclass: 2,
                orbit: None,
                spin: elite_journal::body::Spin { period: 1.0, tilt: 0.0 },
                radius: 696_000_000.0,
                temperature: 5778.0,
                mapped: false,
                discovered_at: None,
            }],
            ..SystemBodies::default()
        }
    }

    /// A shard directory is taken away whole, unless it holds something else
    ///
    /// The removal is one call against thirteen thousand `unlink`s, which is
    /// what a galaxy of loose files costs — but only where everything in
    /// the directory was a loose body file this understood. A directory
    /// holding anything else keeps that thing, and its body files go one at
    /// a time as they always did.
    #[test]
    fn a_shard_directory_keeps_what_the_pack_does_not_understand() {
        let dir = scratch("stray");
        let address = 7_700_017_i64;
        crate::format::msgpack::write_meta(
            &crate::format::layout::bodies_path(&dir, address),
            &inside(1),
        )
        .expect("a sharded file writes");

        // Something the pack has no idea about, beside it.
        let shard = crate::format::layout::bodies_path(&dir, address)
            .parent()
            .expect("a shard directory")
            .to_path_buf();
        let stray = shard.join("notes.txt");
        std::fs::write(&stray, b"nothing to do with bodies")
            .expect("a stray file writes");

        let done = pack(&dir, &|| false).expect("the pack runs");
        assert!(done.finished);
        assert_eq!(done.moved, 1, "the body file was not counted");
        assert!(matches!(held(&dir, address), Found::Bodies(_)));
        assert!(
            !crate::format::layout::bodies_path(&dir, address).exists(),
            "a packed file was left loose",
        );
        assert!(
            stray.exists(),
            "the pack removed a file it did not understand",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shard that has been folded still takes an append
    ///
    /// **The bug this is here for.** [`append`] reads the sixteen header
    /// bytes to learn the generation and the base, and handed that buffer to
    /// the header check it asked whether a base of *n* entries fitted in
    /// sixteen bytes. It does not, so every append to a shard that had ever
    /// been folded failed — `a base of 8218 entries in a file holding 0` —
    /// and with it every pack of a loose body file into that shard.
    ///
    /// Reported off a real directory, where the packing stopped at the
    /// first folded shard and the upgrade that called it stopped with it.
    /// Nothing in the suite caught it because nothing appended to a folded
    /// shard: a fold happens when a tail grows past thousands of entries,
    /// which no test had reached. This one folds by hand instead.
    #[test]
    fn a_folded_shard_still_takes_an_append() {
        let dir = scratch("foldappend");
        let address = 4_611_686_020_061_657_985_i64;
        let shard = body_shard(address);

        write(&dir, HashMap::from([(address, inside(1))]));
        // Into the base, which is what a fold does with a tail.
        fold(&dir, shard).expect("the shard folds");
        let folded =
            Table::read(&body_index_path(&dir, shard)).expect("a table");
        assert_eq!(folded.base.len(), 1, "the fold left nothing in the base");
        assert!(folded.tail.is_empty());

        // And now another system into the same shard, which is the step
        // that used to fail. The shard is a hash of the address, so the
        // next one is looked for rather than guessed at.
        let next = (1..10_000)
            .map(|n| address + n)
            .find(|&it| body_shard(it) == shard)
            .expect("another address in the same shard");
        let wrote = write(&dir, HashMap::from([(next, inside(2))]));
        assert!(wrote.failed.is_none(), "{:?}", wrote.failed);

        // Both readable, the folded one and the appended one.
        assert!(matches!(held(&dir, address), Found::Bodies(_)));
        assert!(matches!(held(&dir, next), Found::Bodies(_)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What was written is what is read, and the newer write is what is read
    ///
    /// The two halves of the layout at once: a record is found through the
    /// tail, and a system written twice reads as the second of them. A pack
    /// that answered the older entry would serve a scan the commander has
    /// already replaced.
    #[test]
    fn a_packed_system_reads_back_as_it_was_last_written() {
        let dir = scratch("roundtrip");
        let address = 4_611_686_020_061_657_985_i64;

        assert_eq!(held(&dir, address), Found::Absent);

        let wrote = write(&dir, HashMap::from([(address, inside(1))]));
        assert_eq!(wrote.wrote, 1);
        assert!(wrote.failed.is_none(), "{:?}", wrote.failed);
        assert_eq!(held(&dir, address), Found::Bodies(inside(1)));

        write(&dir, HashMap::from([(address, inside(2))]));
        assert_eq!(
            held(&dir, address),
            Found::Bodies(inside(2)),
            "the older record was read over the newer one",
        );

        assert!(remove(&dir, address).expect("the withdrawal"));
        assert_eq!(
            held(&dir, address),
            Found::Withdrawn,
            "a withdrawn system read as published",
        );
        assert!(
            !remove(&dir, address).expect("the second withdrawal"),
            "withdrawing nothing said it had withdrawn something",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folded shard answers exactly what the tail answered
    ///
    /// The fold is where the pack rewrites what it serves from, so it is
    /// where a system can be lost: a tombstone dropped too early puts a
    /// withdrawn scan back, and an entry merged the wrong way round serves
    /// the older of two writes.
    #[test]
    fn folding_keeps_what_the_tail_said() {
        let dir = scratch("fold");
        let shard = 1u64;
        let addresses: Vec<i64> = (0..64)
            .map(|n| n)
            .filter(|n| body_shard(*n) == body_shard(0))
            .collect();
        // One shard's worth, written a few times each, so the fold has
        // duplicates to merge.
        let mut want: HashMap<i64, SystemBodies> = HashMap::new();
        for round in 1..=3i16 {
            for &address in &addresses {
                let inside = inside(round);
                write(&dir, HashMap::from([(address, inside.clone())]));
                want.insert(address, inside);
            }
        }
        let withdrawn = addresses[0];
        remove(&dir, withdrawn).expect("the withdrawal");
        want.remove(&withdrawn);

        let _ = shard;
        fold(&dir, body_shard(addresses[0])).expect("the fold");

        let table =
            Table::read(&body_index_path(&dir, body_shard(addresses[0])))
                .expect("the index reads");
        assert!(table.tail.is_empty(), "the fold left a tail behind");
        assert_eq!(table.base.len(), want.len(), "the base is the wrong size");

        for (&address, inside) in &want {
            assert_eq!(
                held(&dir, address),
                Found::Bodies(inside.clone()),
                "a folded system reads as something else",
            );
        }
        assert_eq!(
            held(&dir, withdrawn),
            Found::Absent,
            "a fold kept a tombstone's system",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A compaction moves the records and nothing else
    ///
    /// Rewriting the data file is the one operation that invalidates every
    /// offset a reader might be holding, which is why it writes the next
    /// generation rather than the same file. What must survive it is what
    /// the pack answers.
    #[test]
    fn compaction_rewrites_the_records_and_keeps_the_answers() {
        let dir = scratch("compact");
        let address = 12_345_678_i64;
        let shard = body_shard(address);

        // Written enough times over that most of the data file is records
        // nothing points at any more.
        for round in 1..=8i16 {
            write(&dir, HashMap::from([(address, inside(round))]));
        }
        let before = std::fs::metadata(body_data_path(&dir, shard, 0))
            .expect("a data file")
            .len();

        fold(&dir, shard).expect("the fold");

        assert!(
            !body_data_path(&dir, shard, 0).exists(),
            "the old generation was left behind",
        );
        let after = std::fs::metadata(body_data_path(&dir, shard, 1))
            .expect("the next generation")
            .len();
        assert!(
            after < before / 2,
            "the compaction kept the dead records: {after} of {before}",
        );
        assert_eq!(
            held(&dir, address),
            Found::Bodies(inside(8)),
            "the compaction lost the live record",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Loose files become packed ones, once, and a stop leaves the rest
    ///
    /// The migration is the only thing that touches a directory an older
    /// build wrote, and the way it goes wrong is a file removed before its
    /// record is durable. A stop is the test for that: what it has moved
    /// reads out of the pack, and what it has not still reads off the disk.
    #[test]
    fn packing_moves_every_loose_file_once() {
        let dir = scratch("migrate");
        let flat: Vec<i64> = (1..=6).map(|n| n * 1_000_003).collect();
        let sharded: Vec<i64> = (1..=6).map(|n| n * 7_700_017).collect();
        for &address in &flat {
            crate::format::msgpack::write_meta(
                &crate::format::layout::legacy_bodies_path(&dir, address),
                &inside(1),
            )
            .expect("a flat file writes");
        }
        for &address in &sharded {
            crate::format::msgpack::write_meta(
                &crate::format::layout::bodies_path(&dir, address),
                &inside(2),
            )
            .expect("a sharded file writes");
        }

        // Atomic rather than a `Cell`: the pack deals shards out to
        // threads now, so what it asks about stopping is shared.
        let some = std::sync::atomic::AtomicUsize::new(0);
        let stop = || some.fetch_add(1, Relaxed) > 4;
        let part = pack(&dir, &stop).expect("the migration runs");
        assert!(!part.finished, "an abandoned migration claimed to be done");

        let rest = pack(&dir, &|| false).expect("the migration runs again");
        assert!(rest.finished, "a migration nobody stopped did not finish");
        assert_eq!(
            part.moved + rest.moved,
            flat.len() + sharded.len(),
            "the two passes did not cover the directory between them",
        );

        for &address in &flat {
            assert_eq!(held(&dir, address), Found::Bodies(inside(1)));
            assert!(
                !crate::format::layout::legacy_bodies_path(&dir, address)
                    .exists(),
                "a packed file was left loose",
            );
        }
        for &address in &sharded {
            assert_eq!(held(&dir, address), Found::Bodies(inside(2)));
            assert!(
                !crate::format::layout::bodies_path(&dir, address).exists(),
                "a packed file was left loose",
            );
        }

        let again = pack(&dir, &|| false).expect("a third pass");
        assert_eq!(again.moved, 0, "a second pass moved what was packed");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every address, and only the ones still standing
    #[test]
    fn the_pack_says_which_systems_it_holds() {
        let dir = scratch("addresses");
        let addresses: Vec<i64> = (1..=32).map(|n| n * 99_991).collect();
        write(
            &dir,
            addresses
                .iter()
                .map(|&it| (it, inside(1)))
                .collect::<HashMap<_, _>>(),
        );
        remove(&dir, addresses[3]).expect("the withdrawal");

        let mut want: Vec<i64> = addresses
            .iter()
            .copied()
            .filter(|it| *it != addresses[3])
            .collect();
        want.sort_unstable();
        assert_eq!(addresses_of(&dir), want);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shard weighs by what is in it, not by how large the file is
    ///
    /// Two readings that used to be taken off the file's size, and are
    /// wrong from either end of it. Exactly half dead is the case rather
    /// than a corner of it — a re-import replaces every record with one
    /// of very nearly the same length, so a galaxy imported twice sits
    /// *at* half, which `dead * 2 > written` refused. And a file is not
    /// its length or its blocks: a punched shard keeps a length it no
    /// longer costs, and an appended one is over-allocated past its end
    /// (84.6 MB of blocks behind a 79.1 MB shard, measured), either of
    /// which makes a swept shard look worth sweeping again for ever.
    #[test]
    fn a_shard_weighs_by_what_is_in_it() {
        let at = |live: u64, dead: u64| Cost {
            live,
            dead,
            // The length and the blocks disagree with both, and with
            // each other, and neither is asked.
            length: 1 << 40,
            allocated: 0,
            ..Cost::default()
        };
        assert!(Dead::Half.reached(&at(50, 50)));
        assert!(!Dead::Half.reached(&at(51, 49)));

        // Nothing dead is nothing to rewrite, at either bar.
        assert!(!Dead::Half.reached(&at(100, 0)));
        assert!(!Dead::Worth.reached(&at(100, 0)));

        // The sweep asks a tenth of what the file holds, and never fewer
        // than [`WORTH`] bytes of it however large that tenth's share.
        assert!(Dead::Worth.reached(&at(9 * WORTH, WORTH)));
        assert!(!Dead::Worth.reached(&at(9 * WORTH + 1, WORTH - 1)));
        assert!(!Dead::Worth.reached(&at(90, 10)));
    }

    /// Records large enough that a shard's dead bytes reach [`WORTH`]
    ///
    /// A galaxy reaches it with hundreds of thousands of systems in a
    /// shard; a test reaches it by making each record a big one. The
    /// length does not vary with `id`, so a system rewritten is a record
    /// replaced by one of exactly the same size — which is what an import
    /// of the same galaxy twice is, and what puts a shard *at* half dead
    /// rather than past it.
    fn padded(id: i16) -> SystemBodies {
        let mut bodies = inside(id);
        bodies.stars[0].name = format!("Star {id} {}", "x".repeat(16 * 1024));
        bodies
    }

    /// Addresses in one shard, so their dead bytes pile up in one file
    fn together(shard: u64, count: usize) -> Vec<i64> {
        (1i64..).filter(|&it| body_shard(it) == shard).take(count).collect()
    }

    /// What the data file the index names costs, and what it holds.
    fn measured(dir: &Path, shard: u64) -> Cost {
        let table = Table::read(&body_index_path(dir, shard)).expect("a table");
        let live = table.live();
        cost(&body_data_path(dir, shard, table.generation), &live)
            .expect("a data file")
    }

    /// Nothing watching, for a sweep a test is not reading progress off.
    fn quietly(_: &Reclaimed) {}

    /// A re-imported galaxy gives its dead records back
    ///
    /// **The bug this is here for.** A dump names each system once and the
    /// store raising a directory writes without reading, so an import over
    /// a directory already holding the galaxy appends a fresh record for
    /// every system and leaves the one behind it dead. Nothing reclaimed
    /// them: the write path folds when a shard's tail passes
    /// [`tail_bound`] and an import leaves every tail well under it, and
    /// the compaction it would have reached was `dead * 2 > written` —
    /// which a rewrite of every record with one the same size lands
    /// exactly on and so failed. Reported off a real directory: `bodies/`
    /// at 323 GB, 49.8 % of it live, 161 GB unreachable.
    #[test]
    fn a_reimport_gives_its_dead_records_back() {
        let dir = scratch("reimport");
        let shard = body_shard(1);
        let addresses = together(shard, 200);

        let rows = |id| -> HashMap<i64, SystemBodies> {
            addresses.iter().map(|&it| (it, padded(id))).collect()
        };
        write(&dir, rows(1));
        let one = measured(&dir, shard);

        // The re-import: the same galaxy again, every record replaced.
        write(&dir, rows(2));
        let two = measured(&dir, shard);
        assert_eq!(two.length, one.length * 2, "the shard did not double");

        // Weighed, and nothing touched.
        let looked = weigh(&dir, &|| false).expect("a weighing");
        assert_eq!(looked.shards, 1);
        assert_eq!(looked.records, 200);
        assert_eq!((looked.live, looked.dead), (two.live, two.dead));
        assert_eq!(looked.dead, one.live, "the first import is what is dead");
        assert_eq!(measured(&dir, shard), two, "a weighing moved bytes");

        let swept = sweep_bodies(&dir, &|| false, &quietly).expect("a sweep");
        assert_eq!(swept.shards, 1);
        assert_eq!(swept.bytes, looked.reclaimable, "{swept:?}");
        assert!(swept.finished);
        assert!(
            looked.reclaimable * 4 >= looked.dead * 3,
            "most of the dead bytes were left where they were: {looked:?}",
        );

        // What it says it gave back is what the disk gave back, and what
        // is left costs what it holds.
        let after = measured(&dir, shard);
        assert!(
            after.allocated + swept.bytes <= two.allocated + 4096
                && after.allocated + swept.bytes + 4096 >= two.allocated,
            "the report and the disk disagree: {after:?} {swept:?}",
        );
        // Where there is a hole punch, it is the road a re-import takes:
        // the dead records are one run and nothing is moved to free them.
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert!(swept.punched > 0, "the punch was not taken: {swept:?}");
        match swept.punched {
            // Punched: the same generation, the same length, the same
            // offsets — and the blocks under the first import gone.
            0 => {
                assert!(!body_data_path(&dir, shard, 0).exists());
                assert!(body_data_path(&dir, shard, 1).exists());
            }
            punched => {
                assert_eq!(punched, swept.bytes);
                assert!(body_data_path(&dir, shard, 0).exists());
                assert_eq!(after.length, two.length, "a punch moved records");
            }
        }

        // The half that matters: every system reads back as the re-import
        // wrote it, and not as the import it replaced did.
        for &address in &addresses {
            assert_eq!(
                held(&dir, address),
                Found::Bodies(padded(2)),
                "system {address} did not survive the reclaim",
            );
        }

        // Idempotent: a shard that costs what it holds has nothing to give.
        let again = sweep_bodies(&dir, &|| false, &quietly).expect("a second");
        assert_eq!(again, Reclaimed { finished: true, ..Reclaimed::default() });

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shard whose tail has been folded away is still reclaimed
    ///
    /// The other half of the same bug. [`fold`] returned on an empty tail
    /// before it weighed the data file at all, so a shard whose last fold
    /// declined the rewrite — the dead third below, which the write path's
    /// bar is right to leave — could never be reclaimed again however much
    /// of it was dead: there was no tail left to carry it back in.
    #[test]
    fn a_folded_shard_is_still_reclaimed() {
        let dir = scratch("folded");
        let shard = body_shard(1);
        let addresses = together(shard, 200);

        write(
            &dir,
            addresses
                .iter()
                .map(|&it| (it, padded(1)))
                .collect::<HashMap<_, _>>(),
        );
        // A third of them again, which is under the write path's half.
        let rewritten = &addresses[..100];
        write(
            &dir,
            rewritten
                .iter()
                .map(|&it| (it, padded(2)))
                .collect::<HashMap<_, _>>(),
        );
        let before = measured(&dir, shard);

        fold(&dir, shard).expect("the shard folds");
        let table =
            Table::read(&body_index_path(&dir, shard)).expect("a table");
        assert!(table.tail.is_empty(), "the fold left a tail");
        assert_eq!(table.base.len(), 200);
        assert_eq!(
            measured(&dir, shard),
            before,
            "the write path's bar reclaimed a file only a third dead",
        );

        // And the sweep, which asks what the space is worth rather than
        // what the next append is. This is the step that did nothing.
        let weighed = weigh(&dir, &|| false).expect("a weighing");
        let swept = sweep_bodies(&dir, &|| false, &quietly).expect("a sweep");
        assert!(
            measured(&dir, shard).allocated + swept.bytes
                <= before.allocated + 4096,
            "the report and the disk disagree",
        );
        assert!(
            swept.shards == 1 && swept.bytes == weighed.reclaimable,
            "a folded shard was passed over: {swept:?} {weighed:?}",
        );

        for (at, &address) in addresses.iter().enumerate() {
            let want = match at < rewritten.len() {
                true => padded(2),
                false => padded(1),
            };
            assert_eq!(
                held(&dir, address),
                Found::Bodies(want),
                "system {address} did not survive the reclaim",
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A dead run gives up its whole blocks, and a swept file says so
    ///
    /// Two claims about the weighing, and the second is what keeps a
    /// sweep from running for ever. The alignment: `F_PUNCHHOLE` refuses
    /// a range that is not a multiple of the block size outright —
    /// `EINVAL`, measured — so a run is pulled *in* to the blocks inside
    /// it and a run with no whole block in it is not a hole at all. And
    /// the read-back: the bytes a punch gave away are gone from the next
    /// weighing, because what is dead is measured against where the
    /// file's data actually is and not against its length.
    #[test]
    fn a_hole_is_the_whole_blocks_of_a_dead_run() {
        let dir = scratch("holes");
        let data = dir.join("probe.dat");
        std::fs::write(&data, vec![7u8; 40_960]).expect("a data file");
        let at = |address: i64, offset: u64, len: u32| {
            (address, Entry { address, offset, len })
        };
        // Live at [0, 100) and [20_000, 20_100), so the dead runs are
        // [100, 20_000) and [20_100, 40_960).
        let live = BTreeMap::from([at(1, 0, 96), at(2, 20_000, 96)]);
        let weighed = cost(&data, &live).expect("a weighing");
        assert_eq!(weighed.live, 200);
        assert_eq!(weighed.dead, 19_900 + 20_860);
        assert_eq!(weighed.holes, vec![(4096, 12_288), (20_480, 20_480)]);
        assert_eq!(weighed.punchable, 12_288 + 20_480);

        if punched(&data, &weighed).is_err() {
            // A filesystem with no holes in it; the copy is its road and
            // the rest of this is about holes.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let after = cost(&data, &live).expect("a second weighing");
        assert_eq!(after.length, weighed.length, "a punch moved the end");
        assert_eq!(
            after.allocated,
            weighed.allocated - weighed.punchable,
            "the disk did not give the blocks back",
        );
        // What is left dead is the edges of the runs the blocks did not
        // cover — 3,996 and 3,616 bytes either side of the first hole,
        // 380 before the second — and there is nothing left to punch.
        assert_eq!(after.dead, 3_996 + 3_616 + 380);
        assert_eq!(after.punchable, 0);
        assert!(after.holes.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The copy is what reclaims a file a punch cannot
    ///
    /// Three shapes, and the road each is owed. A re-import is the
    /// first: one run of dead records ahead of the live ones, which is a
    /// single punch of half the file. A feed leaves the second: records
    /// smaller than a block, dead ones between live ones, where punching
    /// frees almost nothing and only the copy reclaims it. The third is
    /// the one that stops a punched shard's length growing for ever.
    #[test]
    fn the_copy_takes_what_a_punch_cannot() {
        let half = 4u64 << 20;
        let reimported = Cost {
            live: half,
            dead: half,
            punchable: half - 8192,
            length: half * 2,
            allocated: half * 2,
            holes: Vec::new(),
        };
        assert_eq!(How::of(&reimported), How::Punch);

        let fed = Cost { punchable: 128 << 10, ..reimported.clone() };
        assert_eq!(How::of(&fed), How::Copy);

        let bloated = Cost { length: half * (BLOAT + 1), ..reimported.clone() };
        assert_eq!(How::of(&bloated), How::Copy);
    }

    /// A sweep asked to stop gives nothing back and says so
    ///
    /// The flag is asked between shards, so what a stop leaves is a
    /// directory some of whose shards have been reclaimed and the rest
    /// of which stand exactly as they were — which is why `finished` is
    /// part of the answer and not an aside.
    #[test]
    fn a_stopped_sweep_says_so() {
        let dir = scratch("stopped");
        let shard = body_shard(1);
        let addresses = together(shard, 8);
        for id in 1..=2 {
            let rows: HashMap<i64, SystemBodies> =
                addresses.iter().map(|&it| (it, padded(id))).collect();
            write(&dir, rows);
        }
        let before = measured(&dir, shard);

        let swept = sweep_bodies(&dir, &|| true, &quietly).expect("a sweep");
        assert_eq!(swept, Reclaimed::default());
        assert!(!swept.finished, "a stopped sweep called itself finished");
        assert_eq!(measured(&dir, shard), before, "a stopped sweep wrote");
        assert_eq!(held(&dir, addresses[0]), Found::Bodies(padded(2)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn addresses_of(dir: &Path) -> Vec<i64> {
        addresses(dir).expect("the pack lists")
    }

    /// An empty scratch directory named after the test using it.
    fn loose_scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("galos_bodies_loose_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A system's insides, told apart by the address they belong to.
    fn loose_inside(address: i64) -> SystemBodies {
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

    /// A withdrawn scan loses its flat file as well as its sharded one
    ///
    /// The removal is what makes a system whose last scan was taken back read
    /// as unscanned. Clearing only the sharded file would leave the
    /// pre-sharding one for the read to fall back onto.
    #[test]
    fn removing_a_body_clears_both_layouts() {
        use crate::format::msgpack::write_meta;

        let dir = loose_scratch("remove");
        let address = 2_412_116_659_890_i64;
        write_meta(&bodies_path(&dir, address), &loose_inside(address))
            .expect("the sharded file writes");
        write_meta(&legacy_bodies_path(&dir, address), &loose_inside(address))
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
