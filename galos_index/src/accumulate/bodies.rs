//! Where the things scanned inside a system are kept between one scan and the
//! next.
//!
//! [`Galaxy`](crate::accumulate::galaxy::Galaxy) needs a system's *whole*
//! insides every time one more body of it arrives. Two things ask for them and
//! both are whole-from-whole: the reach is the far edge over every body, star
//! and barycentre together, and the published body record is written whole. So
//! the accumulator cannot look at a scan and forget it.
//!
//! What it can do is not be the one holding them, and that is the whole of
//! this module. Two stores, and which is right depends on what is reading:
//!
//! - [`InMemory`] holds everything in memory. It is what
//!   [`Galaxy::new`](crate::accumulate::galaxy::Galaxy::new) keeps, for a
//!   galaxy with no directory behind it; every caller that writes a directory
//!   keeps an [`OnDisk`] instead.
//! - [`OnDisk`] keeps them in the index directory's own body pack, which
//!   is where they are going anyway: a system's record there is what the
//!   map reads when a click opens the system, and it is written whole. So
//!   the durable copy already exists and holding a second one in memory
//!   buys nothing.
//!
//!   [`OnDisk::raising`] is the same store for a build raising a
//!   directory from nothing, where a record not held has not been written.
//!   That is a shard's index searched and both loose layouts looked for a
//!   system less, which over a galaxy is most of what the read costs.
//!
//!   [`Shared`] is an [`OnDisk`] behind an `Arc`, for a read that builds an
//!   accumulator a line and wants one store under all of them.
//!
//! The second is what a feed needs. `galos ingest --from eddn --index DIR`
//! carries everyone's scans, and holding them all is a process that
//! grows for as long as it runs — a `records::Body` is 376 bytes before its
//! four strings, its parents and its materials, so a million of them is
//! about a gigabyte. Deriving a directory from the rows does not have the
//! problem because Postgres is its body store; this gives the database-free
//! path the same answer, with the directory standing in for the database.
//!
//! ## Why a store rather than a cache
//!
//! A cache would have to decide what to drop and would be wrong about it
//! sometimes. A store is asked and answers, and the only thing held in memory
//! is what has been changed and not yet written — which the sink flushes on
//! the same beat it publishes on. Between flushes that is the systems scanned
//! in the last few seconds, and [`OnDisk::CARRIED`] forces a flush for a
//! caller that never asks for one.
//!
//! That bound holds as long as the disk takes the writes. A forced flush the
//! disk refuses is warned and the held systems go on accumulating, the
//! alternative being to drop a commander's scans to keep a number down. So a
//! disk that has stopped taking writes is a process that grows, and that
//! warning is the only place it is said.

use crate::codec::Directory;
use crate::records::SystemBodies;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Where a system's scanned insides live.
pub trait Bodies: fmt::Debug + Send + Sync {
    /// What is on record inside the system at `address`.
    ///
    /// Empty where nothing has been scanned in it, which reads the same as a
    /// system nobody has asked about — the map cannot tell the two apart and
    /// nothing here pretends it can.
    fn read(&self, address: i64) -> Cow<'_, SystemBodies>;

    /// Change what is on record, having read it.
    ///
    /// A closure rather than a read followed by a write, so the store in
    /// memory hands out a borrow and copies nothing: a system with two
    /// hundred bodies in it is scanned two hundred times, and cloning the set
    /// on each of them is the cost this module exists to avoid.
    fn edit(&mut self, address: i64, act: &mut dyn FnMut(&mut SystemBodies));

    /// Every system with anything on record.
    ///
    /// What a whole publish walks. Ordered, so a publish written twice is
    /// written the same way twice.
    fn scanned(&self) -> Vec<i64>;

    /// Make durable whatever is being held, answering how many systems moved.
    ///
    /// Nothing for a store that is memory. For one backed by a directory this
    /// is where the files are written, and a caller on a publish beat calls
    /// it there.
    fn flush(&mut self) -> io::Result<usize> {
        Ok(0)
    }

    /// Systems written since this was last asked.
    ///
    /// Not what a flush answers: a store may force one between a caller's
    /// flushes, and a caller reporting what its publish wrote wants both.
    fn written(&mut self) -> usize {
        0
    }
}

/// Everything, in memory.
///
/// What [`Galaxy::new`](crate::accumulate::galaxy::Galaxy::new) keeps: a galaxy
/// with no directory behind it, where there is nowhere else to put them.
#[derive(Debug, Default)]
pub struct InMemory(HashMap<i64, SystemBodies>);

impl InMemory {
    /// A store holding nothing.
    pub fn new() -> InMemory {
        InMemory::default()
    }
}

impl Bodies for InMemory {
    fn read(&self, address: i64) -> Cow<'_, SystemBodies> {
        match self.0.get(&address) {
            Some(inside) => Cow::Borrowed(inside),
            None => Cow::Owned(SystemBodies::default()),
        }
    }

    fn edit(&mut self, address: i64, act: &mut dyn FnMut(&mut SystemBodies)) {
        act(self.0.entry(address).or_default());
    }

    fn scanned(&self) -> Vec<i64> {
        let mut addresses: Vec<i64> = self.0.keys().copied().collect();
        addresses.sort_unstable();
        addresses
    }
}

/// The index directory's own body pack.
///
/// A system's record in [`crate::codec::bodies`] is written whole and read
/// whole, and is what the map reads when a click opens the system. This
/// reads and writes exactly those, so the durable copy is the only copy.
///
/// What is held in memory is what has been changed and not yet written. A
/// full system scan is dozens of `Scan` events in a row about the one system,
/// and writing the record on each of them would be dozens of writes to say
/// what one says; so an edit is held and the record is written when the
/// caller flushes, which for a sink is the beat it publishes on.
#[derive(Debug)]
pub struct OnDisk {
    dir: PathBuf,
    /// Systems edited since the last flush.
    dirty: HashMap<i64, SystemBodies>,
    /// Systems written since the count was last taken.
    ///
    /// A forced flush writes between one caller's flushes, so what the last
    /// flush wrote is not what has been written since the caller last asked.
    /// A caller reporting what a publish wrote wants this.
    wrote: usize,
    /// Whether this is raising the directory rather than editing one.
    ///
    /// See [`raising`](Self::raising).
    raising: bool,
}

impl OnDisk {
    /// How many systems may be held before a flush is forced.
    ///
    /// The bound exists for a caller that never flushes — a one-shot import,
    /// which reads a whole directory and publishes once at the end. A
    /// follower flushes on its own beat and never reaches this. Sized at what
    /// a burst of a feed touches rather than at what a machine can hold: the
    /// point is that the number is bounded, not that it is large.
    pub const CARRIED: usize = 4096;

    pub fn new(dir: impl Into<PathBuf>) -> OnDisk {
        OnDisk {
            dir: dir.into(),
            dirty: HashMap::new(),
            wrote: 0,
            raising: false,
        }
    }

    /// A store onto a directory being raised from nothing.
    ///
    /// **A record not held has not been written.** An ordinary store reads
    /// the disk to find what a system already had, which is right for a
    /// feed reporting a system it has reported before. A build from nothing
    /// can only ever be told back what it has already said, and a dump names
    /// each system once, so every one of those reads is a search of a
    /// shard's index and a lookup of both loose layouts for a system none of
    /// them holds. Those are the reads that cannot be cached — a hit can be
    /// remembered and a miss cannot.
    ///
    /// The cost of being wrong about it is a system's bodies not read from a
    /// record this store then supersedes, so it is for a build raising a
    /// directory and nothing else.
    pub fn raising(dir: impl Into<PathBuf>) -> OnDisk {
        OnDisk { raising: true, ..OnDisk::new(dir) }
    }

    /// The directory being written to.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What the directory holds for `address`, empty where it holds nothing.
    ///
    /// [`bodies::read_bodies`] answers empty for a system with no record, in
    /// any of its layouts. A record that is there and will not read is warned
    /// and read as empty rather than taken as an error. It is one system's
    /// insides; refusing the whole run over it would lose the feed, and the
    /// next scan of that system writes the record afresh.
    fn on_disk(&self, address: i64) -> SystemBodies {
        if self.raising {
            // Nothing was published here, so there is nothing to read and
            // no lookup worth making. See `raising`.
            return SystemBodies::default();
        }
        match Directory::at(&self.dir).read_bodies(address) {
            Ok(inside) => inside,
            Err(err) => {
                eprintln!(
                    "unreadable body file for {address}, read as empty: {err}"
                );
                SystemBodies::default()
            }
        }
    }
}

impl Bodies for OnDisk {
    fn read(&self, address: i64) -> Cow<'_, SystemBodies> {
        match self.dirty.get(&address) {
            // Held and not yet written, which is the newer of the two.
            Some(inside) => Cow::Borrowed(inside),
            None => Cow::Owned(self.on_disk(address)),
        }
    }

    fn edit(&mut self, address: i64, act: &mut dyn FnMut(&mut SystemBodies)) {
        if self.dirty.len() >= OnDisk::CARRIED
            && !self.dirty.contains_key(&address)
        {
            if let Err(err) = self.flush() {
                eprintln!("body files could not be written: {err}");
            }
        }
        // Read before the entry is taken: `on_disk` borrows `self`, and the
        // entry holds `self.dirty` mutably for as long as it lives.
        let published =
            (!self.dirty.contains_key(&address)).then(|| self.on_disk(address));
        act(self
            .dirty
            .entry(address)
            .or_insert_with(|| published.unwrap_or_default()));
    }

    /// Every system the directory holds bodies for, and every one held
    /// unwritten.
    ///
    /// Three layouts and the held set: the packed shard indexes, the loose
    /// file a system in its shard directory, the flat unsharded file, and
    /// what this run has scanned and not yet written. Missing
    /// any of them would report a system nobody has scanned.
    fn scanned(&self) -> Vec<i64> {
        fn listed(dir: &Path, into: &mut Vec<i64>) -> Vec<PathBuf> {
            let mut shards = Vec::new();
            let Ok(read) = std::fs::read_dir(dir) else { return shards };
            for entry in read.flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|it| it.is_dir()) {
                    shards.push(path);
                    continue;
                }
                if path.extension().is_some_and(|it| it == "bin")
                    && let Some(address) = path
                        .file_stem()
                        .and_then(|it| it.to_str())
                        .and_then(|it| it.parse::<i64>().ok())
                {
                    into.push(address);
                }
            }
            shards
        }

        let mut addresses: Vec<i64> = self.dirty.keys().copied().collect();
        match Directory::at(&self.dir).body_addresses() {
            Ok(packed) => addresses.extend(packed),
            Err(err) => eprintln!("the packed bodies could not be read: {err}"),
        }
        let bodies = self.dir.join(crate::codec::layout::BODIES_DIR);
        for shard in listed(&bodies, &mut addresses) {
            listed(&shard, &mut addresses);
        }
        addresses.sort_unstable();
        addresses.dedup();
        addresses
    }

    /// Write what is held, and let go of it.
    ///
    /// Into the pack, grouped by shard: a shard is two appends however many of
    /// the held systems fell in it, and neither append is a directory
    /// operation. See [`crate::codec::bodies`] for why that is the whole of
    /// this module's cost at galaxy scale.
    ///
    /// A system whose record will not write is kept rather than dropped, so
    /// the next flush tries again and a full disk that clears costs nothing.
    /// The error is the first one met; the rest of the systems are still
    /// written.
    fn flush(&mut self) -> io::Result<usize> {
        let done = Directory::at(&self.dir)
            .write_held_bodies(std::mem::take(&mut self.dirty));
        self.dirty = done.kept;
        self.wrote += done.wrote;
        match done.failed {
            Some(err) => Err(err),
            None => Ok(done.wrote),
        }
    }

    fn written(&mut self) -> usize {
        std::mem::take(&mut self.wrote)
    }
}

/// One store, written through by several accumulators.
///
/// A cold read builds an accumulator a line — holding the galaxy one line
/// at a time is the whole point of that road — and a store built with each
/// of them is a store that can never batch: a flush a system, and a shard's
/// two files opened to append one record. This is the store behind an
/// `Arc`, so a run has one of it and each line's
/// [`Galaxy`](crate::accumulate::galaxy::Galaxy) borrows it, which is what lets
/// [`OnDisk::CARRIED`] systems pile up and go out shard by shard.
///
/// One writer still: the `Arc` is shared within a run, and a run holds the
/// directory's [`Lock`](crate::codec::lock::Lock).
#[derive(Clone, Debug)]
pub struct Shared(Arc<Mutex<OnDisk>>);

impl Shared {
    /// A shared store onto a directory being edited.
    pub fn new(dir: impl Into<PathBuf>) -> Shared {
        Shared(Arc::new(Mutex::new(OnDisk::new(dir))))
    }

    /// A shared store onto a directory being raised from nothing. See
    /// [`OnDisk::raising`].
    pub fn raising(dir: impl Into<PathBuf>) -> Shared {
        Shared(Arc::new(Mutex::new(OnDisk::raising(dir))))
    }

    /// What is held, on disk, and how many systems have been written since
    /// this was last asked.
    pub fn settle(&self) -> io::Result<usize> {
        let mut held = self.lock();
        held.flush()?;
        Ok(held.written())
    }

    /// How many systems have been written since this was last asked.
    ///
    /// The trait has the same answer and wants a `&mut`, which a store
    /// several accumulators share is never held as.
    pub fn written(&self) -> usize {
        self.lock().written()
    }

    /// The store, whatever a panicking writer left it as.
    ///
    /// A poisoned store is one a write panicked in the middle of; what is
    /// in it is still the systems the run has read, and refusing to write
    /// them would lose more than it protects.
    fn lock(&self) -> std::sync::MutexGuard<'_, OnDisk> {
        self.0.lock().unwrap_or_else(|it| it.into_inner())
    }
}

impl Bodies for Shared {
    /// Cloned rather than borrowed: what is behind the lock cannot be lent
    /// out past it, and the caller is about to merge into it anyway.
    fn read(&self, address: i64) -> Cow<'_, SystemBodies> {
        Cow::Owned(self.lock().read(address).into_owned())
    }

    fn edit(&mut self, address: i64, act: &mut dyn FnMut(&mut SystemBodies)) {
        self.lock().edit(address, act)
    }

    fn scanned(&self) -> Vec<i64> {
        self.lock().scanned()
    }

    fn flush(&mut self) -> io::Result<usize> {
        self.lock().flush()
    }

    fn written(&mut self) -> usize {
        self.lock().written()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{Barycenter, Star};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("galos_bodies_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    fn star(id: i16) -> Star {
        Star {
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
        }
    }

    /// Both stores answer the same way about what has been put in them
    ///
    /// The whole of what makes them interchangeable. Asked of both by the one
    /// test, since a store that answered differently would be a `Galaxy` that
    /// derived a different reach depending on where its bodies happened to
    /// live.
    #[test]
    fn a_store_answers_what_was_edited_into_it() {
        let dir = scratch("edited");
        let stores: [Box<dyn Bodies>; 2] =
            [Box::new(InMemory::new()), Box::new(OnDisk::new(&dir))];

        for mut store in stores {
            assert!(
                store.read(1).stars.is_empty(),
                "a store answered for a system nothing was scanned in",
            );

            store.edit(1, &mut |inside| inside.stars.push(star(0)));
            store.edit(1, &mut |inside| inside.stars.push(star(1)));
            assert_eq!(store.read(1).stars.len(), 2);
            assert_eq!(store.scanned(), vec![1]);

            store.flush().expect("what is held should write");
            assert_eq!(
                store.read(1).stars.len(),
                2,
                "a flush lost what it was holding",
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a directory store holds survives the store
    ///
    /// The point of the whole exercise: the durable copy is the only copy, so
    /// a second run reads what the first wrote rather than starting empty.
    #[test]
    fn a_published_store_reads_back_what_it_wrote() {
        let dir = scratch("published");

        let mut first = OnDisk::new(&dir);
        first.edit(7, &mut |inside| inside.stars.push(star(0)));
        first.edit(7, &mut |inside| {
            inside.barycenters.push(Barycenter {
                system_address: 7,
                id: 1,
                updated_at: "2026-08-08T12:00:00Z".parse().expect("a moment"),
                updated_by: "a test".into(),
                orbit: None,
            })
        });
        assert_eq!(first.flush().expect("the file writes"), 1);
        drop(first);

        let second = OnDisk::new(&dir);
        let inside = second.read(7);
        assert_eq!(inside.stars.len(), 1, "the star did not survive");
        assert_eq!(inside.barycenters.len(), 1);
        assert_eq!(second.scanned(), vec![7], "the listing missed the file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A raising store does not read the directory it is writing
    ///
    /// The contract of [`OnDisk::raising`] and the reason it is faster:
    /// a build from nothing can only be told back what it has already said,
    /// so a record it is not holding is one it has not written. Stated as a
    /// test because the cost of being wrong about it is a system's bodies
    /// silently replaced rather than merged — which is what a build raising
    /// a directory means to do, and what a feed must never do.
    #[test]
    fn a_raising_store_answers_for_itself_and_not_for_the_disk() {
        let dir = scratch("raising");

        let mut published = OnDisk::new(&dir);
        published.edit(11, &mut |inside| inside.stars.push(star(0)));
        published.flush().expect("the file writes");

        let mut raising = OnDisk::raising(&dir);
        assert!(
            raising.read(11).stars.is_empty(),
            "a raising store read a file it did not write",
        );

        raising.edit(11, &mut |inside| inside.stars.push(star(1)));
        raising.flush().expect("the file writes");
        let published = OnDisk::new(&dir);
        let stars = &published.read(11).stars;
        assert_eq!(stars.len(), 1, "the file was merged rather than raised");
        assert_eq!(stars[0].id, 1, "the raised file is not what was written");

        // Into the pack and nowhere else: a store writing a loose file a
        // system would be a galaxy of inodes.
        assert!(
            !crate::codec::layout::bodies_path(&dir, 11).exists(),
            "a loose body file was written",
        );
        assert!(
            matches!(
                Directory::at(&dir).find_bodies(11).expect("the pack reads"),
                crate::codec::bodies::Found::Bodies(_)
            ),
            "the pack does not hold what the store wrote",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nothing reaches the disk until the store is asked
    ///
    /// A full system scan is dozens of events about the one system, and the
    /// record is written whole. Writing on each of them would be dozens of
    /// writes to say what one says.
    #[test]
    fn an_edit_is_held_until_it_is_flushed() {
        let dir = scratch("held");
        let mut store = OnDisk::new(&dir);
        store.edit(3, &mut |inside| inside.stars.push(star(0)));

        assert_eq!(
            Directory::at(&dir).find_bodies(3).expect("the pack reads"),
            crate::codec::bodies::Found::Absent,
            "an edit reached the disk before it was asked to",
        );
        assert_eq!(store.read(3).stars.len(), 1, "the held edit was not read");

        store.flush().expect("the record writes");
        assert!(
            matches!(
                Directory::at(&dir).find_bodies(3).expect("the pack reads"),
                crate::codec::bodies::Found::Bodies(_)
            ),
            "the flush wrote nothing",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A caller that never flushes is still bounded
    ///
    /// The bound is why this is a store and not an unbounded buffer with a
    /// different name. A one-shot import publishes once at the end, and
    /// without this it would hold every system it read.
    #[test]
    fn what_is_held_is_bounded() {
        let dir = scratch("bounded");
        let mut store = OnDisk::new(&dir);
        for address in 0..(OnDisk::CARRIED as i64 + 16) {
            store.edit(address, &mut |inside| inside.stars.push(star(0)));
        }
        assert!(
            store.dirty.len() <= OnDisk::CARRIED,
            "the store held {} systems, past its own bound",
            store.dirty.len(),
        );
        // And nothing was lost by the flush that bound forced.
        assert_eq!(store.read(0).stars.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a publish wrote counts the flushes it did not ask for
    ///
    /// A dump import touches far more than [`OnDisk::CARRIED`] systems
    /// between publishes, so most systems are written by the forced flush
    /// and only the remainder by the publish's own. A count taken from the
    /// last flush alone would report one system for a hundred thousand.
    #[test]
    fn the_count_covers_a_forced_flush() {
        let dir = scratch("counted");
        let mut store = OnDisk::new(&dir);
        let touched = OnDisk::CARRIED as i64 + 16;
        for address in 1..=touched {
            store.edit(address, &mut |inside| inside.stars.push(star(0)));
        }

        // The last flush's own answer is the remainder, not the work.
        let settled = store.flush().expect("the files write");
        assert!(
            settled < touched as usize,
            "a forced flush should have written most of these already",
        );

        assert_eq!(
            store.written(),
            touched as usize,
            "every file written between publishes should be counted",
        );
        assert_eq!(store.written(), 0, "the count is taken, not repeated");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
