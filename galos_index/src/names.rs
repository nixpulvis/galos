//! The names table, held resident and published in chunks.
//!
//! Every positioned system's name and place is one table, and the client reads
//! all of it: a search reaches any name and a route steps between any two
//! places. A hundred megabytes of it. The feed changes a few dozen systems a
//! second, so the table cannot be re-derived and rewritten whole each time the
//! index publishes — that is a full read of `systems` and a full rewrite of the
//! table to say that fifty systems arrived.
//!
//! So the table is held here, in memory, in fixed-size chunks, and a publish
//! writes only the chunks that changed. Two facts make that pay. A system's
//! entry is *almost* immutable: an address never moves, a position is corrected
//! about never, and a name changes about never, so [`upsert`](NameTable::upsert)
//! compares before it dirties and a system reported again costs nothing. And
//! new systems land at the tail, so the arrivals of a pass dirty the one chunk
//! they are appended to. A pass writes one chunk, not the galaxy.
//!
//! The chunk a system sits in is wherever it was first appended; there is no
//! ordering to keep, since the client reads the whole table and indexes it by
//! address itself. Chunk files are numbered from zero with no gaps, which is
//! what lets a reader find them all without a manifest to keep in step.

use crate::meta::NameEntry;
use crate::source::{names_chunk_path, names_dir, read_meta, write_meta};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

/// How many entries one chunk file holds.
///
/// The trade is publish cost against read cost: a chunk is the unit a change
/// rewrites, and the whole table is as many files as it takes. At 64Ki entries
/// a chunk is about three megabytes and a galaxy is a few dozen files.
const CHUNK: usize = 64 * 1024;

/// The names table as the builder holds it: chunked, indexed by address, and
/// tracking which chunks a publish must write.
#[derive(Clone, Debug, Default)]
pub struct NameTable {
    /// The chunks in file order. All but the last are full to [`CHUNK`].
    chunks: Vec<Vec<NameEntry>>,
    /// Where each address sits: which chunk, and where in it.
    slot: HashMap<i64, (usize, usize)>,
    /// Chunks changed since the last publish.
    dirty: HashSet<usize>,
    /// How many chunk files the last publish left on disk, so a table that has
    /// shrunk can delete the ones it no longer fills.
    on_disk: usize,
}

impl NameTable {
    /// The table over `entries`, every chunk of it needing a write. What a full
    /// build hands over.
    pub fn from_entries(entries: Vec<NameEntry>) -> NameTable {
        let mut table = NameTable::default();
        for entry in entries {
            table.append(entry);
        }
        table.dirty = (0..table.chunks.len()).collect();
        table
    }

    /// The table as `dir` holds it, chunk boundaries and all, with nothing to
    /// write. What a `--watch` restart resumes onto: the chunks read back are
    /// the chunks the next publish writes into, so resuming leaves the same
    /// files a run that never stopped would have.
    pub fn read(dir: &Path) -> io::Result<NameTable> {
        let chunks = read_chunks(dir)?;
        let mut slot = HashMap::with_capacity(
            chunks.iter().map(Vec::len).sum::<usize>() + 1,
        );
        for (c, chunk) in chunks.iter().enumerate() {
            for (i, entry) in chunk.iter().enumerate() {
                slot.insert(entry.address, (c, i));
            }
        }
        let on_disk = chunks.len();
        Ok(NameTable { chunks, slot, dirty: HashSet::new(), on_disk })
    }

    /// How many systems the table names.
    ///
    /// No `is_empty` beside it. There was one, and nothing ever called it; a
    /// table with no names in it is not a case any caller asks about, since
    /// what they want to know is how far the publish has to write.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.slot.len()
    }

    /// Every address the table names.
    ///
    /// For a publisher reconciling the table against the cell tree beside
    /// it: the two stand for the same systems or the directory does not
    /// reopen, and this is the cheap side of that comparison.
    pub fn addresses(&self) -> impl Iterator<Item = i64> + '_ {
        self.slot.keys().copied()
    }

    /// Put `entry` in the table, and say whether anything changed.
    ///
    /// An entry equal to the one on record is not a change: the feed reports
    /// the same system over and over, and rewriting its chunk to store the
    /// bytes already there is the whole cost this table exists to avoid.
    pub fn upsert(&mut self, entry: NameEntry) -> bool {
        match self.slot.get(&entry.address) {
            Some(&(c, i)) => {
                if self.chunks[c][i] == entry {
                    return false;
                }
                self.chunks[c][i] = entry;
                self.dirty.insert(c);
                true
            }
            None => {
                let c = self.append(entry);
                self.dirty.insert(c);
                true
            }
        }
    }

    /// Take `address` out of the table, and say whether it was there.
    ///
    /// For a system that stopped being one the table carries: its position
    /// withdrawn, or the row gone. The entries after it in its chunk close the
    /// gap, which is why the slots of that one chunk are re-indexed; every
    /// other chunk is untouched and stays out of the publish.
    pub fn remove(&mut self, address: i64) -> bool {
        let Some((c, i)) = self.slot.remove(&address) else {
            return false;
        };
        self.chunks[c].remove(i);
        for (j, entry) in self.chunks[c].iter().enumerate().skip(i) {
            self.slot.insert(entry.address, (c, j));
        }
        self.dirty.insert(c);
        // A tail chunk emptied is a file that should go, not an empty one left
        // to be read back. An interior chunk cannot empty short of the table
        // being cleared, and an empty one read back costs a file and nothing
        // else, so only the tail is trimmed.
        while self.chunks.last().is_some_and(Vec::is_empty) {
            let gone = self.chunks.len() - 1;
            self.chunks.pop();
            self.dirty.remove(&gone);
        }
        true
    }

    /// Write every chunk that changed since the last publish, and delete the
    /// files of any the table has shrunk out of. Returns how many were written.
    pub fn publish(&mut self, dir: &Path) -> io::Result<usize> {
        if self.dirty.is_empty() && self.on_disk == self.chunks.len() {
            return Ok(0);
        }
        std::fs::create_dir_all(names_dir(dir))?;
        let written = self.dirty.len();
        for &c in &self.dirty {
            write_meta(&names_chunk_path(dir, c), &self.chunks[c])?;
        }
        for c in self.chunks.len()..self.on_disk {
            let path = names_chunk_path(dir, c);
            if path.exists() {
                std::fs::remove_file(path)?;
            }
        }
        self.dirty.clear();
        self.on_disk = self.chunks.len();
        Ok(written)
    }

    /// Append to the tail chunk, opening a new one where it is full, and record
    /// the slot. Returns the chunk the entry landed in.
    fn append(&mut self, entry: NameEntry) -> usize {
        if self.chunks.last().is_none_or(|tail| tail.len() >= CHUNK) {
            self.chunks.push(Vec::new());
        }
        let c = self.chunks.len() - 1;
        let i = self.chunks[c].len();
        self.slot.insert(entry.address, (c, i));
        self.chunks[c].push(entry);
        c
    }
}

/// The chunk files of `dir`, in order, as they were written.
///
/// Chunks are numbered from zero with no gaps, so the first number missing is
/// the end of the table. A directory that holds none reads as an empty table
/// rather than an error, which is what a directory nothing has been published
/// to yet is.
pub(crate) fn read_chunks(dir: &Path) -> io::Result<Vec<Vec<NameEntry>>> {
    let mut chunks = Vec::new();
    for c in 0.. {
        let path = names_chunk_path(dir, c);
        if !path.exists() {
            break;
        }
        chunks.push(read_meta(&path)?);
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn scratch(name: &str) -> std::path::PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("galos_names_{}_{name}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(address: i64) -> NameEntry {
        NameEntry {
            address,
            name: format!("SYSTEM {address}"),
            position: [address as f32, 0., 0.],
        }
    }

    /// A table published and read back is the same table, in the same chunks,
    /// so a restart appends where the run before it left off rather than
    /// re-chunking what is already on disk.
    #[test]
    fn round_trips_through_disk() {
        let dir = scratch("round_trip");
        let entries: Vec<NameEntry> =
            (0..CHUNK as i64 + 7).map(entry).collect();
        let mut table = NameTable::from_entries(entries.clone());
        assert_eq!(table.chunks.len(), 2);
        assert_eq!(table.publish(&dir).unwrap(), 2);

        let read = NameTable::read(&dir).unwrap();
        assert_eq!(read.len(), entries.len());
        assert_eq!(read.chunks.len(), 2);
        assert_eq!(read.chunks[0].len(), CHUNK);
        assert_eq!(read.chunks[1].len(), 7);
        assert_eq!(crate::source::read_names(&dir).unwrap(), entries);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The point of the whole arrangement: arrivals dirty the tail chunk alone,
    /// and a system reported again with nothing new to say dirties nothing.
    #[test]
    fn a_pass_writes_only_the_chunks_it_touched() {
        let dir = scratch("dirty");
        let mut table =
            NameTable::from_entries((0..3 * CHUNK as i64).map(entry).collect());
        assert_eq!(table.chunks.len(), 3);
        table.publish(&dir).unwrap();

        // The feed re-reports systems from every chunk, unchanged.
        for address in [0, CHUNK as i64, 2 * CHUNK as i64 + 5] {
            assert!(!table.upsert(entry(address)));
        }
        assert_eq!(table.publish(&dir).unwrap(), 0);

        // New systems arrive: the tail chunk, and only it, is written.
        for address in 3 * CHUNK as i64..3 * CHUNK as i64 + 50 {
            assert!(table.upsert(entry(address)));
        }
        assert_eq!(table.publish(&dir).unwrap(), 1);

        // A system that really did change writes its own chunk, no others.
        let mut moved = entry(7);
        moved.position = [1., 2., 3.];
        assert!(table.upsert(moved.clone()));
        assert_eq!(table.publish(&dir).unwrap(), 1);

        let read = crate::source::read_names(&dir).unwrap();
        assert_eq!(read.len(), 3 * CHUNK + 50);
        assert_eq!(read.iter().find(|e| e.address == 7), Some(&moved));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A withdrawn system leaves the table and the file, its chunk closing the
    /// gap, and a tail chunk emptied of the last of them stops being a file at
    /// all.
    #[test]
    fn a_removed_system_leaves_the_published_table() {
        let dir = scratch("remove");
        let mut table =
            NameTable::from_entries((0..CHUNK as i64 + 2).map(entry).collect());
        table.publish(&dir).unwrap();

        assert!(table.remove(3));
        assert!(!table.remove(3));
        table.publish(&dir).unwrap();
        let read = crate::source::read_names(&dir).unwrap();
        assert_eq!(read.len(), CHUNK + 1);
        assert!(!read.iter().any(|e| e.address == 3));

        // The two entries of the tail chunk withdrawn: the file goes with them.
        assert!(table.remove(CHUNK as i64));
        assert!(table.remove(CHUNK as i64 + 1));
        table.publish(&dir).unwrap();
        assert_eq!(table.chunks.len(), 1);
        assert!(!names_chunk_path(&dir, 1).exists());
        assert_eq!(crate::source::read_names(&dir).unwrap().len(), CHUNK - 1);

        // What is left still answers by address, the slots of the chunk that
        // closed its gap having been re-indexed.
        assert!(!table.upsert(entry(4)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
