//! Where the things scanned inside a system are kept between one scan and the
//! next.
//!
//! [`Galaxy`](crate::Galaxy) needs a system's *whole* insides every time one
//! more body of it arrives. Two things ask for them and both are
//! whole-from-whole: the reach is the far edge over every body, star and
//! barycentre together, and the published body file is written whole. So the
//! accumulator cannot look at a scan and forget it.
//!
//! What it can do is not be the one holding them, and that is the whole of
//! this module. Two stores, and which is right depends on what is reading:
//!
//! - [`Kept`] holds everything in memory. Right for
//!   [`JournalSource`](crate::JournalSource), which has no directory to keep
//!   them in: the map builds one straight out of the `.log` files and nothing
//!   publishes an index unless somebody asks for one. It is also what that
//!   type's shape wants — it rebuilds its tables whole on every poll, and
//!   `Galaxy::reaches` walks every system with anything scanned, so a store
//!   behind a disk would be a file read per scanned system per second where
//!   a sink asks about the handful a pass touched.
//!
//!   Not for the sake of a click: a click is answered by `Source::bodies`, and
//!   the map already reads the published `bodies/<address>.bin` off the disk
//!   for that.
//! - [`Published`] keeps them in the index directory's own body files, which
//!   is where they were going anyway: `bodies/<address>.bin` is what the map
//!   fetches when a click opens a system, and it is written whole. So the
//!   durable copy already exists and holding a second one in memory bought
//!   nothing.
//!
//! The second is what a feed needs. `galos-sync eddn --to index` carries
//! everyone's scans, and holding them all is a process that grows for as long
//! as it runs — a `meta::Body` is 376 bytes before its four strings, its
//! parents and its materials, so a million of them is about a gigabyte.
//! `galos-sync db --to index` never had the problem because Postgres is its
//! body store; this gives the database-free path the same answer, with the
//! directory standing in for the database.
//!
//! ## Why a store rather than a cache
//!
//! A cache would have to decide what to drop and would be wrong about it
//! sometimes. A store is asked and answers, and the only thing held in memory
//! is what has been changed and not yet written — which the sink flushes on
//! the same beat it publishes on. Between flushes that is the systems scanned
//! in the last few seconds, and [`Published::CARRIED`] forces a flush for a
//! caller that never asks for one.
//!
//! That bound holds as long as the disk takes the writes. A forced flush the
//! disk refuses is warned and the held systems go on accumulating, the
//! alternative being to drop a commander's scans to keep a number down. So a
//! disk that has stopped taking writes is a process that grows, and that
//! warning is the only place it is said.

use galos_index::meta::SystemBodies;
use galos_index::source::{bodies_path, read_meta, write_meta};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Where a system's scanned insides live.
///
/// `Sync` as well as `Send`: a [`JournalSource`](crate::JournalSource) is a
/// [`galos_index::Source`], which the map holds behind an `Arc` and reads
/// from its task pool.
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
}

/// Everything, in memory.
///
/// What a commander's own journal wants: it is megabytes, the map holds one
/// and answers a click off it, and there is no directory in the arrangement
/// at all.
#[derive(Debug, Default)]
pub struct Kept(HashMap<i64, SystemBodies>);

impl Kept {
    /// A store holding nothing.
    pub fn new() -> Kept {
        Kept::default()
    }
}

impl Bodies for Kept {
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

/// The index directory's own body files.
///
/// `bodies/<address>.bin` is written whole and read whole, one file per
/// system, and is what the map fetches when a click opens one. This reads and
/// writes exactly those, so the durable copy is the only copy.
///
/// What is held in memory is what has been changed and not yet written. A
/// full system scan is dozens of `Scan` events in a row about the one system,
/// and writing the file on each of them would be dozens of writes to say what
/// one says; so an edit is held and the file is written when the caller
/// flushes, which for a sink is the beat it publishes on.
#[derive(Debug)]
pub struct Published {
    dir: PathBuf,
    /// Systems edited since the last flush.
    dirty: HashMap<i64, SystemBodies>,
}

impl Published {
    /// How many systems may be held before a flush is forced.
    ///
    /// The bound exists for a caller that never flushes — a one-shot import,
    /// which reads a whole directory and publishes once at the end. A
    /// follower flushes on its own beat and never reaches this. Sized at what
    /// a burst of a feed touches rather than at what a machine can hold: the
    /// point is that the number is bounded, not that it is large.
    pub const CARRIED: usize = 4096;

    /// A store over the body files of the index directory at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Published {
        Published { dir: dir.into(), dirty: HashMap::new() }
    }

    /// The directory being written to.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What the file for `address` holds, empty where there is none.
    ///
    /// A file that will not decode is warned and read as empty rather than
    /// taken as an error. It is one system's insides; refusing the whole run
    /// over it would lose the feed, and the next scan of that system writes
    /// the file afresh.
    fn on_disk(&self, address: i64) -> SystemBodies {
        match read_meta(&bodies_path(&self.dir, address)) {
            Ok(inside) => inside,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                SystemBodies::default()
            }
            Err(err) => {
                warn!(
                    address = address,
                    error = %err,
                    "unreadable body file, read as empty",
                );
                SystemBodies::default()
            }
        }
    }
}

impl Bodies for Published {
    fn read(&self, address: i64) -> Cow<'_, SystemBodies> {
        match self.dirty.get(&address) {
            // Held and not yet written, which is the newer of the two.
            Some(inside) => Cow::Borrowed(inside),
            None => Cow::Owned(self.on_disk(address)),
        }
    }

    fn edit(&mut self, address: i64, act: &mut dyn FnMut(&mut SystemBodies)) {
        if self.dirty.len() >= Published::CARRIED
            && !self.dirty.contains_key(&address)
        {
            if let Err(err) = self.flush() {
                warn!(error = %err, "body files could not be written");
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

    /// Every system with a body file, and every one held unwritten.
    ///
    /// The directory listing is the answer for a store that has been running
    /// across restarts, and the held set covers what this run has scanned and
    /// not yet written.
    fn scanned(&self) -> Vec<i64> {
        let mut addresses: Vec<i64> = self.dirty.keys().copied().collect();
        if let Ok(read) =
            std::fs::read_dir(self.dir.join(galos_index::source::BODIES_DIR))
        {
            addresses.extend(read.filter_map(|entry| {
                entry.ok()?.path().file_stem()?.to_str()?.parse::<i64>().ok()
            }));
        }
        addresses.sort_unstable();
        addresses.dedup();
        addresses
    }

    /// Write what is held, and let go of it.
    ///
    /// A system whose file will not write is kept rather than dropped, so the
    /// next flush tries again and a full disk that clears costs nothing. The
    /// error is the first one met; the rest of the systems are still written.
    fn flush(&mut self) -> io::Result<usize> {
        let mut wrote = 0;
        let mut failed = None;
        let mut kept = HashMap::new();
        for (address, inside) in self.dirty.drain() {
            match write_meta(&bodies_path(&self.dir, address), &inside) {
                Ok(()) => wrote += 1,
                Err(err) => {
                    failed.get_or_insert(err);
                    kept.insert(address, inside);
                }
            }
        }
        self.dirty = kept;
        match failed {
            Some(err) => Err(err),
            None => Ok(wrote),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_index::meta::{Barycenter, Star};

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
            [Box::new(Kept::new()), Box::new(Published::new(&dir))];

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

        let mut first = Published::new(&dir);
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

        let second = Published::new(&dir);
        let inside = second.read(7);
        assert_eq!(inside.stars.len(), 1, "the star did not survive");
        assert_eq!(inside.barycenters.len(), 1);
        assert_eq!(second.scanned(), vec![7], "the listing missed the file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nothing reaches the disk until the store is asked
    ///
    /// A full system scan is dozens of events about the one system, and the
    /// file is written whole. Writing on each of them would be dozens of
    /// writes to say what one says.
    #[test]
    fn an_edit_is_held_until_it_is_flushed() {
        let dir = scratch("held");
        let mut store = Published::new(&dir);
        store.edit(3, &mut |inside| inside.stars.push(star(0)));

        assert!(
            !bodies_path(&dir, 3).exists(),
            "an edit reached the disk before it was asked to",
        );
        assert_eq!(store.read(3).stars.len(), 1, "the held edit was not read");

        store.flush().expect("the file writes");
        assert!(bodies_path(&dir, 3).exists(), "the flush wrote nothing");

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
        let mut store = Published::new(&dir);
        for address in 0..(Published::CARRIED as i64 + 16) {
            store.edit(address, &mut |inside| inside.stars.push(star(0)));
        }
        assert!(
            store.dirty.len() <= Published::CARRIED,
            "the store held {} systems, past its own bound",
            store.dirty.len(),
        );
        // And nothing was lost by the flush that bound forced.
        assert_eq!(store.read(0).stars.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
