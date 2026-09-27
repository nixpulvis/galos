//! Weighing every shard, and sweeping the dead records out of them.
//!
//! The whole directory at once, where [`Directory::at().reclaim_body_shard()`] is one shard: what
//! `galos index verify` and `galos index sweep --bodies` ask.

use super::migrate::PACKERS;
use super::reclaim::{How, cost};
use super::{Dead, Reclaimed, Table};
use crate::format::layout::{
    BODIES_DIR, BODY_SHARDS, body_data_path, body_index_path,
};
use crate::store::Directory;
use std::io;
use std::sync::atomic::Ordering::Relaxed;

/// What the body shards hold, and what a sweep would give back.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Weighed {
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

impl Directory<'_> {
    /// Weigh a directory's body shards, writing nothing.
    ///
    /// What `galos index verify` reports, and what `galos index sweep
    /// --bodies` says before it is asked to act. One read of each shard's
    /// index and one walk of its data file's extents — a second over a
    /// galaxy — and nothing decoded at all.
    pub fn weigh_bodies(self, stop: &dyn Fn() -> bool) -> io::Result<Weighed> {
        let dir = self.root;
        let bodies = dir.join(BODIES_DIR);
        let mut held = Weighed { finished: true, ..Weighed::default() };
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
            let cost =
                cost(&body_data_path(dir, shard, table.generation), &live)?;
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
    /// **What a whole-galaxy re-import leaves behind.** A dump names each system
    /// once and [`crate::accumulate::bodies::OnDisk::raising`] writes each record
    /// without reading what is there, so a second import over a published
    /// directory appends a fresh record for every system and the one behind it is
    /// dead the moment the entry naming the new one lands. Nothing on the write
    /// path reclaims those: `append` folds when a shard's
    /// tail passes `tail_bound`, and an import leaves every
    /// tail well under it. On a re-imported galaxy that is 161 GB of the 323 GB
    /// in `bodies/`.
    ///
    /// So the reclaim is asked for rather than waited on: by
    /// [`Build::finish`](crate::build::cold::Build::finish) once its index file
    /// stands, and by `galos index sweep --bodies` for a directory nothing is
    /// about to build. [`Self::weigh_bodies`] weighs what this would do without doing any of
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
    /// Safe beside a map *reading* the directory: a reader whose data file goes out
    /// from under it reads the index again — see [`find_bodies`](crate::store::Directory::find_bodies). Not safe
    /// beside anything *writing* it, which is what [`crate::Lock`] is for.
    pub fn sweep_bodies(
        self,
        stop: &(dyn Fn() -> bool + Sync),
        said: &(dyn Fn(&Reclaimed) + Sync),
    ) -> io::Result<Reclaimed> {
        let dir = self.root;
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
                        match self.reclaim_body_shard(shard) {
                            Ok(gave) if gave.bytes == 0 => {}
                            Ok(gave) => {
                                shards.fetch_add(1, Relaxed);
                                bytes.fetch_add(gave.bytes, Relaxed);
                                punched.fetch_add(gave.punched, Relaxed);
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
}

#[cfg(test)]
mod tests {
    use super::super::Found;
    use super::super::fixtures::{found, inside, scratch};
    use super::super::reclaim::Cost;
    use super::*;
    use crate::format::layout::body_shard;
    use crate::records::SystemBodies;
    use std::collections::HashMap;
    use std::path::Path;

    /// Records large enough that a shard's dead bytes reach [`WORTH`]
    ///
    /// A galaxy reaches it with hundreds of thousands of systems in a
    /// shard; a test reaches it by making each record a big one. The
    /// length does not vary with `id`, so a system rewritten is a record
    /// replaced by one of exactly the same size — which is what an import
    /// of the same galaxy twice is, and what puts a shard *at* half dead
    /// rather than past it.
    ///
    /// [`WORTH`]: crate::store::bodies::reclaim::WORTH
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
    /// A dump names each system once and the store raising a directory
    /// writes without reading, so an import over a directory already holding
    /// the galaxy appends a fresh record for every system and leaves the one
    /// behind it dead. The write path folds when a shard's tail passes
    /// [`tail_bound`](crate::store::bodies::tail_bound) and an import leaves
    /// every tail well under it, and a rewrite of every record with one the
    /// same size lands *exactly* on half dead, so a strict `>` at the
    /// compaction's bar would never reclaim it.
    #[test]
    fn a_reimport_gives_its_dead_records_back() {
        let dir = scratch("reimport");
        let shard = body_shard(1);
        let addresses = together(shard, 200);

        let rows = |id| -> HashMap<i64, SystemBodies> {
            addresses.iter().map(|&it| (it, padded(id))).collect()
        };
        Directory::at(&dir).write_held_bodies(rows(1));
        let one = measured(&dir, shard);

        // The re-import: the same galaxy again, every record replaced.
        Directory::at(&dir).write_held_bodies(rows(2));
        let two = measured(&dir, shard);
        assert_eq!(two.length, one.length * 2, "the shard did not double");

        // Weighed, and nothing touched.
        let looked =
            Directory::at(&dir).weigh_bodies(&|| false).expect("a weighing");
        assert_eq!(looked.shards, 1);
        assert_eq!(looked.records, 200);
        assert_eq!((looked.live, looked.dead), (two.live, two.dead));
        assert_eq!(looked.dead, one.live, "the first import is what is dead");
        assert_eq!(measured(&dir, shard), two, "a weighing moved bytes");

        let swept = Directory::at(&dir)
            .sweep_bodies(&|| false, &quietly)
            .expect("a sweep");
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
            // Copied: the next generation, and the one before it gone.
            0 => {
                assert!(!body_data_path(&dir, shard, 0).exists());
                assert!(body_data_path(&dir, shard, 1).exists());
            }
            // Punched: the same generation, the same length, the same
            // offsets — and the blocks under the first import gone.
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
                found(&dir, address),
                Found::Bodies(padded(2)),
                "system {address} did not survive the reclaim",
            );
        }

        // Idempotent: a shard that costs what it holds has nothing to give.
        let again = Directory::at(&dir)
            .sweep_bodies(&|| false, &quietly)
            .expect("a second");
        assert_eq!(again, Reclaimed { finished: true, ..Reclaimed::default() });

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shard whose tail has been folded away is still reclaimed
    ///
    /// The other half of the same case. A shard whose last fold declined
    /// the rewrite — the dead third below, which the write path's bar is
    /// right to leave — has no tail left, so a fold that returned on an
    /// empty tail before weighing the data file would never reclaim it
    /// however much of it is dead.
    #[test]
    fn a_folded_shard_is_still_reclaimed() {
        let dir = scratch("folded");
        let shard = body_shard(1);
        let addresses = together(shard, 200);

        Directory::at(&dir).write_held_bodies(
            addresses
                .iter()
                .map(|&it| (it, padded(1)))
                .collect::<HashMap<_, _>>(),
        );
        // Half of them again, which leaves a third of the file dead: under
        // the write path's half.
        let rewritten = &addresses[..100];
        Directory::at(&dir).write_held_bodies(
            rewritten
                .iter()
                .map(|&it| (it, padded(2)))
                .collect::<HashMap<_, _>>(),
        );
        let before = measured(&dir, shard);

        Directory::at(&dir).fold_body_shard(shard).expect("the shard folds");
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
        // what the next append is. This is the step that has to act.
        let weighed =
            Directory::at(&dir).weigh_bodies(&|| false).expect("a weighing");
        let swept = Directory::at(&dir)
            .sweep_bodies(&|| false, &quietly)
            .expect("a sweep");
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
                found(&dir, address),
                Found::Bodies(want),
                "system {address} did not survive the reclaim",
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
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
            Directory::at(&dir).write_held_bodies(rows);
        }
        let before = measured(&dir, shard);

        let swept = Directory::at(&dir)
            .sweep_bodies(&|| true, &quietly)
            .expect("a sweep");
        assert_eq!(swept, Reclaimed::default());
        assert!(!swept.finished, "a stopped sweep called itself finished");
        assert_eq!(measured(&dir, shard), before, "a stopped sweep wrote");
        assert_eq!(found(&dir, addresses[0]), Found::Bodies(padded(2)));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
