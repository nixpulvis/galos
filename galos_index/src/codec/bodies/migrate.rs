//! Packing the loose per-system files of the two older layouts into the
//! shards.
//!
//! See [`super`]'s "What is still loose". Read, never written: this is the
//! only thing that moves them, and [`read_bodies`](super::read_bodies) falls
//! back to them until it has.

use super::Table;
use super::write::append;
use crate::codec::Directory;
use crate::codec::layout::{BODIES_DIR, body_index_path, body_shard};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;

/// How far a packing got: what it moved, and whether anything is left.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Packed {
    /// Loose files packed into the shards.
    pub moved: usize,
    /// Whether nothing loose was left behind. A stop part way answers
    /// `false`; a directory with nothing loose answers `true`.
    pub finished: bool,
}

/// How many loose files are packed between one look at the stop flag.
///
/// A batch is held in memory — 2.4 KB a system — and is what one shard's
/// two appends cover, so the flag is asked often enough for a Ctrl-C to be
/// quick and rarely enough that the appends are worth making.
const BATCH: usize = 512;

impl Directory<'_> {
    /// Walk a directory's loose body files into the shards.
    ///
    /// Both older layouts at once: the unsharded `bodies/{address}.bin` and
    /// `bodies/{shard:03x}/{address}.bin`. A file's bytes are already the record
    /// the pack stores, so nothing is decoded on the way through.
    ///
    /// Interruptible, because a galaxy of loose files is hours of them and a run
    /// asked to stop must not wait. What it abandons the next open takes up: a
    /// loose file is removed only once the pack has its record, and
    /// [`crate::codec::Directory::read_bodies`] falls back to the loose paths for
    /// whatever is left, so a directory part way through answers for every system a
    /// finished one does.
    ///
    /// A system the pack already holds wins over a loose file of the same
    /// address: the pack is where the newer write went, and reading the loose
    /// one back over it would put a stale scan back.
    pub fn pack_bodies(
        self,
        stop: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<Packed> {
        let dir = self.root;
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
        if take(dir, loose, &mut moved, stop, Removal::Eager)? == Took::Stopped
        {
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
}

/// How many shards are packed at once
///
/// A few, not a core each: the work is the disk's and a queue of thirty-two
/// readers deep is no faster than eight. Bounded so a pack running beside a
/// map leaves it some.
pub(super) const PACKERS: usize = 8;

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
        // metadata. A shard's files are all one shard's, so they are
        // appended in batches and the directory taken away whole once its
        // records are durable.
        //
        // Sound for the same reason a file at a time is: the records go in
        // before anything is removed, and a run cut short leaves files the
        // next run recognises as already held and drops. Only where
        // *everything* in it was taken — a directory holding something
        // this does not understand keeps that thing, and the files are then
        // removed one by one.
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
/// **One shard's live set, not a lookup apiece.** The question asked of
/// every loose file is whether the pack already holds that system — the pack
/// being the newer of the two wherever both exist — and asking it with
/// [`find`](super::find), which maps the shard's index, scans its tail
/// backwards, binary searches its base and reads the data file, fifty million
/// times over would be the whole of the cost, where the reads and unlinks
/// alone are thousands of files a second.
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

    /// And what this run has just put there, so a second loose file of the same
    /// address is dropped rather than appended twice — which is what asking
    /// [`find`](super::find) afresh would conclude.
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
        // hundred records is a megabyte, and copying it to hand it on would
        // be a hundred and twenty gigabytes of `memcpy` over a galaxy.
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
    use super::super::Found;
    use super::super::fixtures::{found, inside, scratch};
    use super::*;

    /// A shard directory is taken away whole, unless it holds something else
    ///
    /// The removal is one call against thirteen thousand `unlink`s, which is
    /// what a galaxy of loose files costs — but only where everything in
    /// the directory was a loose body file this understood. A directory
    /// holding anything else keeps that thing, and its body files go one at
    /// a time.
    #[test]
    fn a_shard_directory_keeps_what_the_pack_does_not_understand() {
        let dir = scratch("stray");
        let address = 7_700_017_i64;
        crate::codec::tables::msgpack::write_meta(
            &crate::codec::layout::bodies_path(&dir, address),
            &inside(1),
        )
        .expect("a sharded file writes");

        // Something the pack has no idea about, beside it.
        let shard = crate::codec::layout::bodies_path(&dir, address)
            .parent()
            .expect("a shard directory")
            .to_path_buf();
        let stray = shard.join("notes.txt");
        std::fs::write(&stray, b"nothing to do with bodies")
            .expect("a stray file writes");

        let done =
            Directory::at(&dir).pack_bodies(&|| false).expect("the pack runs");
        assert!(done.finished);
        assert_eq!(done.moved, 1, "the body file was not counted");
        assert!(matches!(found(&dir, address), Found::Bodies(_)));
        assert!(
            !crate::codec::layout::bodies_path(&dir, address).exists(),
            "a packed file was left loose",
        );
        assert!(
            stray.exists(),
            "the pack removed a file it did not understand",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Loose files become packed ones, once, and a stop leaves the rest
    ///
    /// The migration is the only thing that touches a directory in an older
    /// layout, and the way it goes wrong is a file removed before its
    /// record is durable. A stop is the test for that: what it has moved
    /// reads out of the pack, and what it has not still reads off the disk.
    #[test]
    fn packing_moves_every_loose_file_once() {
        let dir = scratch("migrate");
        let flat: Vec<i64> = (1..=6).map(|n| n * 1_000_003).collect();
        let sharded: Vec<i64> = (1..=6).map(|n| n * 7_700_017).collect();
        for &address in &flat {
            crate::codec::tables::msgpack::write_meta(
                &crate::codec::layout::legacy_bodies_path(&dir, address),
                &inside(1),
            )
            .expect("a flat file writes");
        }
        for &address in &sharded {
            crate::codec::tables::msgpack::write_meta(
                &crate::codec::layout::bodies_path(&dir, address),
                &inside(2),
            )
            .expect("a sharded file writes");
        }

        // Atomic rather than a `Cell`: the pack deals shards out to
        // threads, so what it asks about stopping is shared.
        let some = std::sync::atomic::AtomicUsize::new(0);
        let stop = || some.fetch_add(1, Relaxed) > 4;
        let part =
            Directory::at(&dir).pack_bodies(&stop).expect("the migration runs");
        assert!(!part.finished, "an abandoned migration claimed to be done");

        let rest = Directory::at(&dir)
            .pack_bodies(&|| false)
            .expect("the migration runs again");
        assert!(rest.finished, "a migration nobody stopped did not finish");
        assert_eq!(
            part.moved + rest.moved,
            flat.len() + sharded.len(),
            "the two passes did not cover the directory between them",
        );

        for &address in &flat {
            assert_eq!(found(&dir, address), Found::Bodies(inside(1)));
            assert!(
                !crate::codec::layout::legacy_bodies_path(&dir, address)
                    .exists(),
                "a packed file was left loose",
            );
        }
        for &address in &sharded {
            assert_eq!(found(&dir, address), Found::Bodies(inside(2)));
            assert!(
                !crate::codec::layout::bodies_path(&dir, address).exists(),
                "a packed file was left loose",
            );
        }

        let again =
            Directory::at(&dir).pack_bodies(&|| false).expect("a third pass");
        assert_eq!(again.moved, 0, "a second pass moved what was packed");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
