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
//! interruptibly, and [`crate::source::read_bodies`] falls back to them until
//! it has, so a directory part way through answers for every system a
//! finished one does.

use crate::meta::SystemBodies;
use crate::source::BODIES_DIR;
use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// How many shards the addresses are spread over.
pub const SHARDS: u64 = 4096;

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

/// Which shard an address belongs to.
///
/// The top twelve bits of the address multiplied by the 64-bit golden-ratio
/// constant. The multiply mixes the high bits down: an Elite `id64` packs a
/// mass code and the boxel coordinates into its low bits, so `address % 4096`
/// leaves whole shards empty and piles the rest up.
pub fn shard_of(address: i64) -> u64 {
    (address as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 52
}

/// A shard's index file.
pub fn index_path(dir: &Path, shard: u64) -> PathBuf {
    dir.join(BODIES_DIR).join(format!("{shard:03x}.idx"))
}

/// A shard's data file of a given generation.
pub fn data_path(dir: &Path, shard: u64, generation: u16) -> PathBuf {
    dir.join(BODIES_DIR).join(format!("{shard:03x}.{generation:04x}.dat"))
}

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
/// which is what [`crate::bodies::Published`] does with a disk that is full
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

/// What an index file's header says.
struct Header {
    generation: u16,
    base: usize,
}

/// Read and check an index file's header.
fn header_of(bytes: &[u8], path: &Path) -> io::Result<Header> {
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
    let entries = (bytes.len() - HEADER) / ENTRY;
    if base > entries {
        return Err(refused(format!(
            "a base of {base} entries in a file holding {entries}"
        )));
    }
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
    let shard = shard_of(address);
    let path = index_path(dir, shard);
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

    let bytes = record(&data_path(dir, shard, generation), &entry)?;
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
                    .entry(shard_of(address))
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
            append(dir, shard_of(address), &[(address, Vec::new())])?;
            Ok(true)
        }
        Found::Withdrawn | Found::Absent => Ok(false),
    }
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
    let path = index_path(dir, shard);
    let mut index =
        OpenOptions::new().read(true).write(true).create(true).open(&path)?;
    let mut head = [0u8; HEADER];
    let length = index.metadata()?.len();
    let (generation, base) = match length >= HEADER as u64 {
        true => {
            index.read_exact(&mut head)?;
            let header = header_of(&head, &path)?;
            (header.generation, header.base)
        }
        false => {
            index.write_all(&header_bytes(0, 0))?;
            (0, 0)
        }
    };

    let data = data_path(dir, shard, generation);
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

/// Merge a shard's tail into its base, and rewrite the data file where most
/// of it is dead.
///
/// The index is written beside and renamed over, so a reader sees one whole
/// index or the other. A compaction writes the *next* generation's data file
/// and leaves the old one until the index naming the new one is in place: a
/// reader holding offsets into the old bytes has to go on being right about
/// them until it reads the index again.
pub fn fold(dir: &Path, shard: u64) -> io::Result<()> {
    let path = index_path(dir, shard);
    let table = Table::read(&path)?;
    if table.tail.is_empty() {
        return Ok(());
    }
    let live = table.live();
    let data = data_path(dir, shard, table.generation);
    let held: u64 = live.values().map(|it| 4 + it.len as u64).sum();
    let written = std::fs::metadata(&data).map(|it| it.len()).unwrap_or(0);

    // Half the file being records nothing points at is the trigger: a feed's
    // thirty systems a second leave about six gigabytes of dead records a
    // day over the galaxy, which reaches half a shard in a couple of months.
    let compacting = written > 0 && held * 2 < written;
    let generation = match compacting {
        true => table.generation.wrapping_add(1),
        false => table.generation,
    };

    let mut base = Vec::with_capacity(live.len());
    match compacting {
        true => {
            let to = data_path(dir, shard, generation);
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
        }
        false => base.extend(live.values().copied()),
    }

    let mut bytes = Vec::with_capacity(HEADER + base.len() * ENTRY);
    bytes.extend_from_slice(&header_bytes(generation, base.len()));
    for entry in &base {
        entry.onto(&mut bytes);
    }
    let tmp = path.with_extension("idx.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;

    if compacting {
        // Nothing reads this generation any more: the index naming it is
        // gone, and a reader that had already read it retries on the miss.
        let _ = std::fs::remove_file(&data);
    }
    Ok(())
}

/// Every system the pack holds bodies for, in address order.
pub fn addresses(dir: &Path) -> io::Result<Vec<i64>> {
    let mut addresses = Vec::new();
    let bodies = dir.join(BODIES_DIR);
    let Ok(entries) = std::fs::read_dir(&bodies) else {
        return Ok(addresses);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|it| it != "idx") {
            continue;
        }
        let table = Table::read(&path)?;
        addresses.extend(table.live().into_keys());
    }
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
/// [`crate::source::read_bodies`] falls back to the loose paths for whatever
/// is left, so a directory part way through answers for every system a
/// finished one does.
///
/// A system the pack already holds wins over a loose file of the same
/// address: the pack is where the newer write went, and reading the loose
/// one back over it would put a stale scan back.
pub fn pack(dir: &Path, stop: &dyn Fn() -> bool) -> io::Result<Packed> {
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
    if !take(dir, loose, &mut moved, stop)? {
        return Ok(Packed { moved, finished: false });
    }
    for shard in shards {
        let listed: Vec<PathBuf> = match std::fs::read_dir(&shard) {
            Ok(entries) => entries.flatten().map(|it| it.path()).collect(),
            Err(_) => continue,
        };
        if !take(dir, listed, &mut moved, stop)? {
            finished = false;
            break;
        }
        // Empty now, or holding something this does not recognise; either
        // way the removal is allowed to fail.
        let _ = std::fs::remove_dir(&shard);
    }
    Ok(Packed { moved, finished })
}

/// Pack a list of paths, answering whether it got through them all.
fn take(
    dir: &Path,
    paths: Vec<PathBuf>,
    moved: &mut usize,
    stop: &dyn Fn() -> bool,
) -> io::Result<bool> {
    let mut batch: HashMap<u64, Vec<(i64, PathBuf, Vec<u8>)>> = HashMap::new();
    let mut held = 0usize;
    for path in paths {
        if stop() {
            settle(dir, &mut batch, moved)?;
            return Ok(false);
        }
        if path.extension().is_none_or(|it| it != "bin") {
            continue;
        }
        let Some(address) = path
            .file_stem()
            .and_then(|it| it.to_str())
            .and_then(|it| it.parse::<i64>().ok())
        else {
            continue;
        };
        // The pack is the newer of the two wherever both exist.
        if find(dir, address)? != Found::Absent {
            std::fs::remove_file(&path)?;
            *moved += 1;
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        batch
            .entry(shard_of(address))
            .or_default()
            .push((address, path, bytes));
        held += 1;
        if held >= BATCH {
            settle(dir, &mut batch, moved)?;
            held = 0;
        }
    }
    settle(dir, &mut batch, moved)?;
    Ok(true)
}

/// Append a batch and drop the loose files it came from.
///
/// In that order: a file removed before its record was durable is a system
/// nothing holds.
fn settle(
    dir: &Path,
    batch: &mut HashMap<u64, Vec<(i64, PathBuf, Vec<u8>)>>,
    moved: &mut usize,
) -> io::Result<()> {
    for (shard, rows) in batch.drain() {
        let records: Vec<(i64, Vec<u8>)> =
            rows.iter().map(|(a, _, b)| (*a, b.clone())).collect();
        append(dir, shard, &records)?;
        for (_, path, _) in rows {
            std::fs::remove_file(&path)?;
            *moved += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::Star;

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
            .filter(|n| shard_of(*n) == shard_of(0))
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
        fold(&dir, shard_of(addresses[0])).expect("the fold");

        let table = Table::read(&index_path(&dir, shard_of(addresses[0])))
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
        let shard = shard_of(address);

        // Written enough times over that most of the data file is records
        // nothing points at any more.
        for round in 1..=8i16 {
            write(&dir, HashMap::from([(address, inside(round))]));
        }
        let before = std::fs::metadata(data_path(&dir, shard, 0))
            .expect("a data file")
            .len();

        fold(&dir, shard).expect("the fold");

        assert!(
            !data_path(&dir, shard, 0).exists(),
            "the old generation was left behind",
        );
        let after = std::fs::metadata(data_path(&dir, shard, 1))
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
            crate::source::write_meta(
                &crate::source::legacy_bodies_path(&dir, address),
                &inside(1),
            )
            .expect("a flat file writes");
        }
        for &address in &sharded {
            crate::source::write_meta(
                &crate::source::bodies_path(&dir, address),
                &inside(2),
            )
            .expect("a sharded file writes");
        }

        let some = std::cell::Cell::new(0);
        let stop = || {
            some.set(some.get() + 1);
            some.get() > 4
        };
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
                !crate::source::legacy_bodies_path(&dir, address).exists(),
                "a packed file was left loose",
            );
        }
        for &address in &sharded {
            assert_eq!(held(&dir, address), Found::Bodies(inside(2)));
            assert!(
                !crate::source::bodies_path(&dir, address).exists(),
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

    fn addresses_of(dir: &Path) -> Vec<i64> {
        addresses(dir).expect("the pack lists")
    }
}
