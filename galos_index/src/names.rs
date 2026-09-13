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
    /// The table over `entries`, every chunk of it needing a write. What a
    /// caller with the whole list already in hand hands over.
    pub fn from_entries(entries: Vec<NameEntry>) -> NameTable {
        let mut table = NameTable::default();
        for entry in entries {
            table.push(entry);
        }
        table
    }

    /// Put one more entry at the tail, for a caller reading them a row at a
    /// time.
    ///
    /// What a cold build uses: the names come off the same database cursor
    /// the systems do, and a galaxy of them collected into a `Vec` to be
    /// handed over is thirteen gigabytes held beside the table they are
    /// being copied into. No [`upsert`](Self::upsert), because a read of
    /// `systems` names each address once and the slot lookup would be a
    /// hash of the galaxy to prove it.
    pub fn push(&mut self, entry: NameEntry) {
        let chunk = self.append(entry);
        self.dirty.insert(chunk);
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

/// Where a build's chunks are written before its table is published.
///
/// A directory inside the one being built, holding a build directory's own
/// names path, so putting a chunk in place is a rename within a filesystem
/// and never a copy. Nothing reads it: [`read_chunks`] reads the numbered
/// files of the names directory and nothing else.
const BUILDING: &str = ".building";

/// The names table written straight to disk, a chunk at a time.
///
/// What a cold build uses in place of a [`NameTable`], which holds every
/// entry so that `upsert` can find a system's slot. A build has no use for
/// that: it names each system once, in the order it reads them, and never
/// looks one up. So entries go into a chunk, the chunk goes to disk when it
/// is full, and the memory is one chunk of [`CHUNK`] entries.
///
/// The chunks are written into [`BUILDING`] and put in place by
/// [`finish`](Self::finish). A build reads the galaxy for as long as that
/// takes and the table beneath it is served the whole time, so a build
/// [abandoned](Self::abandon) part way leaves the table that stood exactly
/// as it found it.
///
/// What lands on disk is what [`NameTable::publish`] would have written
/// from the same entries in the same order, which is what lets a watch read
/// it back and carry on appending to the tail.
pub struct Chunks {
    dir: std::path::PathBuf,
    /// Where the chunks are written until they are put in place.
    building: std::path::PathBuf,
    filling: Vec<NameEntry>,
    written: usize,
    named: usize,
}

impl Chunks {
    /// Start writing the names of a build into `dir`.
    pub fn writing(dir: &Path) -> Chunks {
        Chunks {
            dir: dir.to_owned(),
            building: dir.join(BUILDING),
            filling: Vec::with_capacity(CHUNK),
            written: 0,
            named: 0,
        }
    }

    /// Take up the chunks a stopped build staged, at the cut it recorded.
    ///
    /// `written` complete chunks stand as they are, and the part-filled
    /// tail beside them — which [`stage`](Self::stage) put there — is read
    /// back into the buffer it was written from, cut to the `tail` entries
    /// the mark stands for. Anything past that was named after the mark and
    /// is named again by the resumed read.
    ///
    /// A name is pushed for every system pushed, so the table and the
    /// spills are cut at the same place or the directory's two halves stand
    /// for different galaxies.
    pub fn resuming(
        dir: &Path,
        written: usize,
        tail: usize,
    ) -> io::Result<Chunks> {
        let building = dir.join(BUILDING);
        let mut filling = match tail {
            0 => Vec::new(),
            _ => read_meta::<Vec<NameEntry>>(&names_chunk_path(
                &building, written,
            ))?,
        };
        filling.truncate(tail);
        filling.reserve(CHUNK - filling.len().min(CHUNK));
        Ok(Chunks {
            dir: dir.to_owned(),
            building,
            named: written * CHUNK + filling.len(),
            filling,
            written,
        })
    }

    /// Put the part-filled tail on disk without closing it, and say how
    /// many entries it holds.
    ///
    /// What a build does when it marks where its caller has read to: the
    /// chunk is written where the next [`flush`](Self::flush) would write
    /// it anyway, so a resumed build reads it back and carries on filling
    /// it, and a build that is never resumed overwrites it.
    pub fn stage(&mut self) -> io::Result<usize> {
        write_meta(&names_chunk_path(&self.building, self.written), &self.filling)?;
        Ok(self.filling.len())
    }

    /// How many chunks are complete behind the one being filled.
    pub fn complete(&self) -> usize {
        self.written
    }

    /// One more system's name, in the order the build read it.
    pub fn push(&mut self, entry: NameEntry) -> io::Result<()> {
        self.filling.push(entry);
        self.named += 1;
        if self.filling.len() >= CHUNK {
            self.flush()?;
        }
        Ok(())
    }

    /// Write the part-filled tail, put every chunk in place and answer how
    /// many chunks the table came to. A build that named nothing writes no
    /// chunk at all, which reads back as the empty table it is.
    ///
    /// The one step of a build's names table that cannot be undone, and a
    /// rename apiece: a chunk is [`CHUNK`] systems, so a galaxy is a few
    /// thousand renames within one directory.
    pub fn finish(mut self) -> io::Result<usize> {
        if !self.filling.is_empty() {
            self.flush()?;
        }
        if self.written > 0 {
            std::fs::create_dir_all(names_dir(&self.dir))?;
        }
        for chunk in 0..self.written {
            std::fs::rename(
                names_chunk_path(&self.building, chunk),
                names_chunk_path(&self.dir, chunk),
            )?;
        }
        let _ = std::fs::remove_dir_all(&self.building);
        Ok(self.written)
    }

    /// Drop what has been written without publishing any of it.
    ///
    /// The table that stood in the directory is untouched: nothing of this
    /// build has been renamed over it.
    pub fn abandon(self) -> io::Result<()> {
        match std::fs::remove_dir_all(&self.building) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    /// How many systems have been named.
    pub fn named(&self) -> usize {
        self.named
    }

    fn flush(&mut self) -> io::Result<()> {
        write_meta(
            &names_chunk_path(&self.building, self.written),
            &std::mem::take(&mut self.filling),
        )?;
        self.filling = Vec::with_capacity(CHUNK);
        self.written += 1;
        Ok(())
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
