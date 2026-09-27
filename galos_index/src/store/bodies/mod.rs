//! The bodies of every system, packed into files a shard rather than a file
//! a system.
//!
//! A system's insides are 2.4 KB of MessagePack, written whole and read
//! whole. A file each, at a hundred and eighty-eight million of them, is
//! 188 M inodes and **~830 GB** of a 4.4 KB allocation apiece, and —
//! measured with `sample` against a live import — **91 %** of the wall
//! clock, in `open`, `rename` and the directory insert behind them. Nothing
//! about the write path fixes that; the file count is the cost.
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
//!   directory's [`Lock`](crate::format::lock::Lock)) and any number of
//!   readers.
//! - **A sweep** is a compaction of every shard, asked for rather than
//!   waited on: what a whole-galaxy re-import leaves behind, which no
//!   append reaches. See [`crate::store::Directory::sweep_bodies`].
//!
//! At 200 M systems that is 4,096 index files and a handful of data files
//! rather than 188 M of them, **~450 GB rather than ~830 GB** — the
//! difference is the block a small file rounds up to — and an import whose
//! writes are sequential.
//!
//! ## What is still loose
//!
//! Two older layouts, a file a system, are read and never written:
//! `bodies/{address}.bin` and `bodies/{shard:03x}/{address}.bin`. [`crate::store::Directory::pack_bodies`]
//! walks both into the shards, one batch at a time and interruptibly, and
//! [`crate::store::Directory::read_bodies`] falls back to them for whatever is
//! left, so a directory part way through answers for every system a
//! finished one does.

mod iter;
mod migrate;
mod read;
mod reclaim;
mod sweep;
mod write;

pub use migrate::Packed;
pub use reclaim::{Dead, Reclaimed};
pub use sweep::Weighed;
pub use write::Wrote;

use crate::records::SystemBodies;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

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
/// file of an older layout and read the withdrawn scan as published.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// The pack holds these bodies.
    Bodies(SystemBodies),
    /// The pack holds a tombstone: this system's bodies were withdrawn.
    Withdrawn,
    /// The pack has never been told about this system.
    Absent,
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
/// **Split out because [`append`](write::append) holds only those
/// sixteen.** It reads the header off the front of the file to learn the
/// generation and the base, and [`header_of`] would check a base of
/// thousands of entries against those sixteen bytes and refuse every
/// append to a shard that has ever been folded.
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

/// What the tests of every part of the pack build on.
#[cfg(test)]
mod fixtures {
    use super::Found;
    use crate::format::layout::BODIES_DIR;
    use crate::records::{Star, SystemBodies};
    use crate::store::Directory;
    use std::path::{Path, PathBuf};

    pub(super) fn scratch(name: &str) -> PathBuf {
        let at = std::env::temp_dir()
            .join(format!("galos-pack-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&at);
        std::fs::create_dir_all(at.join(BODIES_DIR)).expect("a directory");
        at
    }

    pub(super) fn found(dir: &Path, address: i64) -> Found {
        Directory::at(dir).find_bodies(address).expect("the pack reads")
    }

    pub(super) fn inside(id: i16) -> SystemBodies {
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
}

#[cfg(test)]
mod tests {
    use super::fixtures::{found, inside, scratch};
    use super::*;
    use crate::store::Directory;
    use std::collections::HashMap;

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

        assert_eq!(found(&dir, address), Found::Absent);

        let wrote = Directory::at(&dir)
            .write_held_bodies(HashMap::from([(address, inside(1))]));
        assert_eq!(wrote.wrote, 1);
        assert!(wrote.failed.is_none(), "{:?}", wrote.failed);
        assert_eq!(found(&dir, address), Found::Bodies(inside(1)));

        Directory::at(&dir)
            .write_held_bodies(HashMap::from([(address, inside(2))]));
        assert_eq!(
            found(&dir, address),
            Found::Bodies(inside(2)),
            "the older record was read over the newer one",
        );

        assert!(
            Directory::at(&dir)
                .tombstone_bodies(address)
                .expect("the withdrawal")
        );
        assert_eq!(
            found(&dir, address),
            Found::Withdrawn,
            "a withdrawn system read as published",
        );
        assert!(
            !Directory::at(&dir)
                .tombstone_bodies(address)
                .expect("the second withdrawal"),
            "withdrawing nothing said it had withdrawn something",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
