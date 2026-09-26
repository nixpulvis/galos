//! Copying a directory whole: what a backup is, and a restore the other way
//! round.
//!
//! One operation run in two directions. `galos index backup -i DIR --to
//! DEST` takes the served directory somewhere else; `galos index restore
//! --from SRC -i DIR` brings it back. Nothing about the two is asymmetric,
//! so there is one [`copy`] and the verbs differ only in which path they
//! hand it first.
//!
//! Nothing here knows the layout. The walk is generic — every file under
//! the directory is carried, whatever it is called — so a sidecar added to
//! [`crate::read::source`] tomorrow is backed up without this module being
//! touched. The only served name it knows is [`INDEX_FILE`], and it knows
//! that one because of the order.
//!
//! ## The resume point travels with the directory
//!
//! A build directory is served; the resume point is the builder's private
//! business and sits *beside* it, three files: the base `<dir>.checkpoint`, the
//! log `<dir>.checkpoint.pending` and the dump mark `<dir>.checkpoint.mark`.
//! They are what says the directory can be followed — the served tree is lossy
//! and no tree can be rebuilt from it (see [`crate::format::checkpoint`]) — so
//! a copy that took the directory alone would be a galaxy that cannot be
//! resumed by `--watch` and cannot be carried on by a cold build. It would have
//! to be reimported from nothing to be useful again, which is the day the
//! backup was taken against. So [`copy`] carries all four things and
//! [`Copied::siblings`] says how many of the three were there.
//!
//! ## The order, which is the whole of the argument
//!
//! A backup is taken of a directory a writer is publishing into. Nothing is
//! frozen, so the order files are taken in decides what an inconsistent
//! copy is inconsistent *by*, and there is a right answer:
//!
//! - **Everything but `index.bin` first, and `index.bin` last.** A cell the
//!   tree names with no payload under it is a hole — a galaxy quietly missing a
//!   piece, and nothing reports it. A payload file the tree does not name is an
//!   orphan, which costs disk and nothing else and which `galos index sweep`
//!   gives back. Taking the payloads before the index means a copy caught
//!   across a publish holds orphans and can hold no holes, because every cell
//!   the copied index names was on disk before the index was read. This is the
//!   same argument [`sweep_payloads`](crate::store::cells::sweep_payloads)
//!   makes for running after the index is written, in the other direction.
//! - **The log before the base.** `Compaction::finish` renames the new base
//!   into place and *then* clears the log, so a copy that takes the log
//!   first and the base second holds at worst a base newer than its log —
//!   and replaying a frame the base already carries is a handful of
//!   idempotent upserts. The other order can catch the old base and the
//!   cleared log, which loses every publish the log was the only record of.
//! - **Never `<dir>.lock`.** A lock is a live process's claim on a live
//!   directory ([`crate::Lock`]). Copied, it hands the destination a pid
//!   that has never heard of it, and the next writer there is refused by a
//!   ghost until somebody runs `--force-lock`.
//! - **Not the scratch.**
//!   [`names::scratch_dir`](crate::format::layout::scratch_dir) —
//!   `names/.building/` — is a fold in progress, and
//!   [`cold::spill_dir`](crate::format::layout::spill_dir) —
//!   `<dir>.checkpoint.regions/` — is a cold build's per-region spill; both run
//!   to gigabytes and neither means anything away from the run that made them.
//!   The first is inside the directory and is skipped by name. The second, like
//!   `<dir>.checkpoint.tmp`, is beside it, and only the three named siblings
//!   beside a directory are ever looked at.
//!
//! ## Why [`std::fs::copy`] and not a read/write loop
//!
//! Because the standard library's copy is the one that talks to the
//! filesystem. On macOS it tries `fclonefileat` first, so on APFS a backup
//! of a galaxy is a copy-on-write clone: seconds, and no bytes moved. On
//! Linux it goes through `copy_file_range`, which reflinks on btrfs and XFS
//! and which preserves holes everywhere else.
//!
//! Holes are not a nicety here. A packed body shard is sparse by
//! construction — [`crate::store::bodies`] punches the dead runs out of
//! `bodies/<shard>.<gen>.dat` and leaves the file's length alone, which is
//! why `BLOAT` exists at all — and a hand-rolled `read`/`write` loop reads
//! zeroes out of the holes and writes them down, inflating the backup to
//! the apparent length. That is the exact failure `pack`'s own note about
//! `cp` and `rsync` warns of, and there is no reason to reimplement it.
//!
//! What is measured and reported is therefore the **apparent** size, what
//! `metadata().len()` says, because that is the only number that can be
//! counted without asking the filesystem what it really allocated. A cloned
//! or reflinked copy costs less than that on disk, and [`Copied`]'s line
//! says so rather than letting an operator read the number as disk spent.
//!
//! ## Stopping
//!
//! `stop` is asked between files, not between bytes: a single file is
//! bounded and a clone of one is instant, and a galaxy's worth of them is
//! where the minutes are. An interrupted copy leaves a partial destination,
//! and a partial destination is safe in both the ways that matter. It reads
//! as nothing at all, because `index.bin` is written last and a directory
//! without one is not a directory any reader will draw from. And it is safe
//! to run the copy again straight over it, because every step is a write of
//! the same bytes to the same place.

use crate::format::layout::{
    INDEX_FILE, checkpoint_beside, mark_path, pending_path, scratch_dir,
};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How many files pass between two readings of the clock.
///
/// The rate limit is a second, but asking the clock per file over a galaxy
/// is millions of reads for a line of output nobody sees. Every 256 files
/// the clock is read, and a report goes out if a second has gone by.
const SPOKEN: u64 = 256;

/// What a copy moved.
///
/// `bytes` is the **apparent** size of everything copied, directory and
/// siblings together — the sum of the file lengths, which on a filesystem
/// that clones or reflinks is more than the copy actually costs. See the
/// module header.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Copied {
    /// Files carried from inside the directory.
    pub files: u64,
    /// Their apparent size, with the siblings', in bytes.
    pub bytes: u64,
    /// How many of the three resume-point files beside the directory were
    /// there to be carried. Fewer than three is ordinary: a directory that
    /// has never been compacted has no base, and one never built from a
    /// dump has no mark.
    pub siblings: u64,
}

impl fmt::Display for Copied {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} files and {} of the resume point beside them, \
             {} apparent (a clone costs less than that on disk)",
            self.files,
            self.siblings,
            size(self.bytes),
        )
    }
}

/// Copy the served directory `from` to `to`, and the resume point beside it.
///
/// Four things move: everything under `from/`, then
/// `<from>.checkpoint.pending`, `<from>.checkpoint` and
/// `<from>.checkpoint.mark` to the same three names beside `to`, and
/// `index.bin` last of all. The order is the correctness argument and the
/// module header is where it is made; in short, a copy taken across a live
/// publish holds orphans and never holes, a copy taken across a compaction
/// replays frames it already has rather than losing them, and a copy that
/// stopped part way reads as nothing rather than as a galaxy with a piece
/// missing.
///
/// `to` and every directory under it are made as needed, so the destination
/// may be empty or may not exist. It may also be a destination a previous
/// run left part copied: every write is the same bytes to the same path, so
/// running again over one finishes it.
///
/// Nothing at the destination is ever removed. This writes over, it does
/// not mirror, so a file standing at `to` that `from` has no counterpart
/// for survives the copy — which is why a destination is weighed by
/// [`occupied`] and cleared by [`discard`] before a backup is taken, and
/// not here.
///
/// A missing source file is not an error. A directory with no `index.bin`
/// is one being built, and a resume point with no mark or no base is the
/// ordinary case — what is there is copied and what is not is counted as
/// not there.
///
/// `stop` is asked before each directory entry and before each sibling, and
/// abandoning leaves a destination that is safe to re-run over. `said` is
/// called at most once a second while the walk runs, and once more when the
/// copy ends however it ends, so a galaxy-sized copy is not a silent
/// terminal for minutes.
///
/// `<from>.lock` is never copied, and neither is the build scratch.
pub fn copy(
    from: &Path,
    to: &Path,
    stop: &dyn Fn() -> bool,
    said: &mut dyn FnMut(&Copied),
) -> io::Result<Copied> {
    let mut copied = Copied::default();
    let mut clock = Instant::now();
    let scratch = scratch_dir(from);

    // Everything under the directory but the index. A stack of relative
    // directories rather than recursion: the depth is two and the breadth
    // is four thousand shards, so what is held is the directories and never
    // the files, of which a galaxy has millions.
    let mut walking = vec![PathBuf::new()];
    while let Some(rel) = walking.pop() {
        let (here, there) = (from.join(&rel), to.join(&rel));
        let entries = fs::read_dir(&here)?;
        fs::create_dir_all(&there)?;
        for entry in entries {
            if stop() {
                said(&copied);
                return Ok(copied);
            }
            let entry = entry?;
            let name = entry.file_name();
            if entry.file_type()?.is_dir() {
                if entry.path() != scratch {
                    walking.push(rel.join(&name));
                }
                continue;
            }
            // Last, and only once the payloads under it are all here.
            if rel.as_os_str().is_empty() && name == INDEX_FILE {
                continue;
            }
            copied.bytes += fs::copy(entry.path(), there.join(&name))?;
            copied.files += 1;
            if copied.files % SPOKEN == 0
                && clock.elapsed() >= Duration::from_secs(1)
            {
                clock = Instant::now();
                said(&copied);
            }
        }
    }

    // The resume point, log before base — see the module header.
    for (src, dst) in siblings(from).into_iter().zip(siblings(to)) {
        if stop() {
            said(&copied);
            return Ok(copied);
        }
        if let Some(bytes) = carried(&src, &dst)? {
            copied.bytes += bytes;
            copied.siblings += 1;
        }
    }

    // And the index, which is what makes the destination a directory at all.
    if stop() {
        said(&copied);
        return Ok(copied);
    }
    if let Some(bytes) = carried(&from.join(INDEX_FILE), &to.join(INDEX_FILE))?
    {
        copied.bytes += bytes;
        copied.files += 1;
    }

    said(&copied);
    Ok(copied)
}

/// Whether anything already stands at `dir` or at any of its three
/// resume-point siblings.
///
/// What the CLI asks before it copies, so `galos index backup --to DEST`
/// refuses a destination already holding something rather than mixing two
/// galaxies into one directory. The siblings count: a destination with no
/// directory but a checkpoint beside it is the wreckage of a run that
/// stopped, and writing a fresh tree in front of a stale resume point would
/// hand `--watch` a cursor for a galaxy that is no longer there.
///
/// `<dir>.lock` is not consulted. It says a writer is running, which is a
/// different refusal with a different remedy, and it is the caller's own
/// lock as often as not.
pub fn occupied(dir: &Path) -> bool {
    [dir.to_owned()]
        .into_iter()
        .chain(siblings(dir))
        .any(|path| fs::symlink_metadata(path).is_ok())
}

/// Remove `dir` and its three resume-point siblings.
///
/// What `--force` runs before a copy, and the only supported way to clear a
/// destination: removing the directory by hand and leaving the checkpoint
/// beside it is exactly the half-cleared state [`occupied`] exists to
/// refuse.
///
/// `<dir>.lock` is deliberately left alone. The caller is holding it — it
/// took the directory in order to be allowed to clear it — and removing it
/// here would drop that claim in the middle of the write it is guarding.
///
/// Nothing there is not an error; this is asked of destinations that may be
/// empty.
pub fn discard(dir: &Path) -> io::Result<()> {
    removed(dir)?;
    for sibling in siblings(dir) {
        removed(&sibling)?;
    }
    Ok(())
}

/// The three files beside a directory that a copy carries, **in the order
/// they must be taken in**: the log, then the base it extends, then the
/// mark. Why that order and not the obvious one is the module header.
///
/// The base is [`checkpoint_beside`], which is the workspace's one
/// spelling of the suffix — `galos::sink::index` re-exports
/// [`checkpoint::SUFFIX`] and publishes into the same name — and the two
/// past it are named by the modules that write them, so a copy and a
/// builder cannot come to disagree about which files the resume point is.
fn siblings(dir: &Path) -> [PathBuf; 3] {
    let base = checkpoint_beside(dir);
    let log = pending_path(&base);
    let mark = mark_path(&base);
    [log, base, mark]
}

/// Copy one file, answering its apparent size, or [`None`] where there was
/// no such file. A source that is not there is not a failure — see
/// [`copy`].
fn carried(src: &Path, dst: &Path) -> io::Result<Option<u64>> {
    match fs::copy(src, dst) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Remove whatever is at a path, directory or file, and say nothing about a
/// path with nothing at it.
///
/// `symlink_metadata` rather than `metadata`, so a symlink standing where a
/// directory is expected is unlinked rather than followed and its target
/// emptied.
fn removed(path: &Path) -> io::Result<()> {
    let found = match fs::symlink_metadata(path) {
        Ok(found) => found,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    match found.is_dir() {
        true => fs::remove_dir_all(path),
        false => fs::remove_file(path),
    }
}

/// Bytes in the unit a person would have said them in.
fn size(bytes: u64) -> String {
    let bytes = bytes as f64;
    match bytes {
        b if b >= 1e9 => format!("{:.1} GB", b / 1e9),
        b if b >= 1e6 => format!("{:.1} MB", b / 1e6),
        b if b >= 1e3 => format!("{:.1} kB", b / 1e3),
        b => format!("{b:.0} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Lock;
    use crate::build::snapshot::{BuildParams, Snapshot};
    use crate::core::record::System;
    use crate::format::checkpoint::{Checkpoint, Provenance, pending};
    use crate::format::layout::{PAYLOAD_DIR, lock_path};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A scratch root unique to this run, removed when the guard drops.
    /// Everything a test makes lives under it, siblings included, so the
    /// drop takes the lot.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            static SEQ: AtomicU32 = AtomicU32::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let at = std::env::temp_dir()
                .join(format!("galos-copy-{name}-{}-{n}", std::process::id(),));
            let _ = fs::remove_dir_all(&at);
            fs::create_dir_all(&at).expect("a scratch root");
            Scratch(at)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A cube lattice of systems well inside the root cube, enough of them
    /// to fill a few hundred cells across a few hundred shard directories —
    /// which is what makes the walk a walk rather than one `readdir`.
    fn systems(n: usize) -> Vec<System> {
        let side = (n as f64).cbrt().ceil() as usize;
        let step = 80.0;
        let span = (side.saturating_sub(1)) as f64 * step;
        let base = [-span / 2.0, 900.0 - span / 2.0, 24400.0 - span / 2.0];
        let mut out = Vec::new();
        let mut id = 1u64;
        'lattice: for x in 0..side {
            for y in 0..side {
                for z in 0..side {
                    if out.len() >= n {
                        break 'lattice;
                    }
                    out.push(System {
                        id64: id,
                        position: [
                            base[0] + x as f64 * step,
                            base[1] + y as f64 * step,
                            base[2] + z as f64 * step,
                        ],
                        absolute_magnitude: id as f64 * 0.001 - 3.0,
                        temperature: 4000.0 + (id % 5000) as f64,
                        age_bucket: (id % 8) as u32,
                        updated_at: 1_700_000_000 + id as u32,
                        kind: crate::core::record::StarKind::G,
                    });
                    id += 1;
                }
            }
        }
        out
    }

    /// A served directory with a tree in it, a compacted base and a log
    /// beside it, and a mark.
    ///
    /// The mark's bytes are made up. Nothing in a copy decodes it — it is a
    /// file beside a directory and that is all this module knows about it —
    /// and writing a real one would need a cold build behind it.
    fn built(dir: &Path) -> Snapshot {
        let rows = systems(9000);
        let built = Snapshot::build(&rows, &BuildParams::default());
        built.write(dir).unwrap();

        let base = checkpoint_beside(dir);
        Checkpoint::compact(
            &base,
            None,
            Provenance::Events,
            rows.iter().copied(),
        )
        .unwrap();
        pending::append(&base, None, &rows[..4]).unwrap();
        fs::write(mark_path(&base), b"a dump read this far").unwrap();
        built
    }

    /// Every file under a directory, relative and sorted.
    fn listing(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut walking = vec![PathBuf::new()];
        while let Some(rel) = walking.pop() {
            for entry in fs::read_dir(dir.join(&rel)).unwrap() {
                let entry = entry.unwrap();
                let at = rel.join(entry.file_name());
                match entry.file_type().unwrap().is_dir() {
                    true => walking.push(at),
                    false => out.push(at),
                }
            }
        }
        out.sort();
        out
    }

    /// Two directories are the same file set, byte for byte.
    fn assert_same(a: &Path, b: &Path) {
        let (na, nb) = (listing(a), listing(b));
        assert_eq!(na, nb, "file sets differ");
        for name in na {
            assert_eq!(
                fs::read(a.join(&name)).unwrap(),
                fs::read(b.join(&name)).unwrap(),
                "{} differs",
                name.display(),
            );
        }
    }

    /// A directory and its resume point copied is the same directory and
    /// the same resume point, to the byte.
    ///
    /// The whole promise of a backup: what comes back is what was there.
    #[test]
    fn a_copy_is_the_same_directory() {
        let at = Scratch::new("same");
        let (from, to) = (at.0.join("live"), at.0.join("backup"));
        built(&from);

        let mut heard = 0;
        let copied = copy(&from, &to, &|| false, &mut |_| heard += 1).unwrap();

        assert_same(&from, &to);
        assert_eq!(copied.siblings, 3, "the resume point did not travel");
        for (src, dst) in siblings(&from).into_iter().zip(siblings(&to)) {
            assert_eq!(
                fs::read(&src).unwrap(),
                fs::read(&dst).unwrap(),
                "{} differs",
                src.display(),
            );
        }
        assert_eq!(copied.files as usize, listing(&to).len());
        assert!(copied.bytes > 0);
        assert!(heard >= 1, "a copy that ended said nothing");
    }

    /// The index is the last file written, so a copy cut short is a
    /// destination that reads as nothing rather than as a galaxy with cells
    /// whose payloads never arrived — and running the copy again over it
    /// finishes the job.
    #[test]
    fn the_index_is_written_last_and_the_rest_re_runs() {
        let at = Scratch::new("last");
        let (from, to) = (at.0.join("live"), at.0.join("backup"));
        built(&from);

        // True on the first ask after a payload has landed, so the copy
        // stops with the directory part built and the index still to come.
        let landed = Cell::new(false);
        let stop = || {
            if landed.get() {
                return true;
            }
            landed.set(any_file(&to.join(PAYLOAD_DIR)));
            false
        };
        let cut = copy(&from, &to, &stop, &mut |_| {}).unwrap();

        assert!(cut.files > 0, "nothing was copied at all");
        assert!(any_file(&to.join(PAYLOAD_DIR)), "no payloads landed");
        assert!(
            !to.join(INDEX_FILE).exists(),
            "the index landed before the payloads under it",
        );
        assert_eq!(cut.siblings, 0, "the siblings went before the index");

        copy(&from, &to, &|| false, &mut |_| {}).unwrap();
        assert_same(&from, &to);
    }

    /// Whether any file at all sits under a directory.
    fn any_file(dir: &Path) -> bool {
        let mut walking = vec![dir.to_owned()];
        while let Some(at) = walking.pop() {
            let Ok(entries) = fs::read_dir(&at) else {
                continue;
            };
            for entry in entries.flatten() {
                match entry.file_type().map(|it| it.is_dir()) {
                    Ok(true) => walking.push(entry.path()),
                    Ok(false) => return true,
                    Err(_) => {}
                }
            }
        }
        false
    }

    /// The lock does not travel.
    ///
    /// It is one process's claim on one directory. Carried into a backup it
    /// becomes a pid nothing is running under, and the first writer to the
    /// restored directory is refused by a ghost.
    #[test]
    fn the_lock_is_not_copied() {
        let at = Scratch::new("lock");
        let (from, to) = (at.0.join("live"), at.0.join("backup"));
        built(&from);
        let held = Lock::take(&from).unwrap();

        copy(&from, &to, &|| false, &mut |_| {}).unwrap();

        assert!(lock_path(&from).exists(), "the writer still holds it");
        assert!(!lock_path(&to).exists(), "the copy took the claim with it");
        assert!(Lock::take(&to).is_ok(), "the copy cannot be written");
        drop(held);
    }

    /// The build scratch does not travel either: it is a fold in progress
    /// and means nothing away from the run that started it.
    #[test]
    fn the_build_scratch_is_not_copied() {
        let at = Scratch::new("scratch");
        let (from, to) = (at.0.join("live"), at.0.join("backup"));
        built(&from);
        let building = scratch_dir(&from);
        fs::create_dir_all(&building).unwrap();
        fs::write(building.join("run.000"), vec![7u8; 4096]).unwrap();

        let copied = copy(&from, &to, &|| false, &mut |_| {}).unwrap();

        assert!(!scratch_dir(&to).exists());
        assert_eq!(copied.files as usize, listing(&to).len());
    }

    /// A destination with nothing but a checkpoint beside it is occupied.
    ///
    /// That is the wreckage a stopped run leaves, and it is the case a bare
    /// `dir.exists()` misses: a fresh tree written in front of a stale
    /// resume point hands `--watch` a cursor for a galaxy that is gone.
    #[test]
    fn occupied_sees_a_sibling_with_no_directory() {
        let at = Scratch::new("occupied");
        let dir = at.0.join("backup");
        assert!(!occupied(&dir));

        fs::write(checkpoint_beside(&dir), b"a base, no tree").unwrap();
        assert!(!dir.exists(), "the directory itself is still absent");
        assert!(occupied(&dir), "a stale resume point was not noticed");

        // And a lock is not what this asks about.
        removed(&checkpoint_beside(&dir)).unwrap();
        let held = Lock::take(&dir).unwrap();
        assert!(!occupied(&dir));
        drop(held);
    }

    /// `--force` clears the siblings as well as the directory, and leaves
    /// the caller's lock where it is.
    #[test]
    fn discard_takes_the_siblings_and_leaves_the_lock() {
        let at = Scratch::new("discard");
        let dir = at.0.join("stale");
        built(&dir);
        let held = Lock::take(&dir).unwrap();
        assert!(occupied(&dir));

        discard(&dir).unwrap();

        assert!(!dir.exists());
        for sibling in siblings(&dir) {
            assert!(!sibling.exists(), "{} survived", sibling.display());
        }
        assert!(!occupied(&dir));
        assert!(lock_path(&dir).exists(), "the caller's claim was dropped");
        drop(held);

        // Asked of a destination with nothing at it, it is content.
        discard(&dir).unwrap();
    }

    /// The line an operator reads says the counts and warns that the
    /// apparent size is not what the copy cost.
    #[test]
    fn the_report_reads_as_a_sentence() {
        let said = Copied { files: 4212, bytes: 38_200_000_000, siblings: 3 }
            .to_string();
        assert!(said.contains("4212 files"), "{said}");
        assert!(said.contains("38.2 GB"), "{said}");
        assert!(said.contains("clone"), "{said}");
    }
}
