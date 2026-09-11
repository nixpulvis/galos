//! One writer to a directory, and how to get the directory back.
//!
//! An index directory is published whole: every pass rewrites files under it
//! and the client reads them with no coordination beyond the writes landing
//! atomically. Two builders over one directory therefore interleave two
//! galaxies into it — each writing a cell tree the other's metadata does not
//! describe — and neither notices, since neither reads what the other wrote.
//! Nothing stopped that, and `galos-sync --index DIR` run twice in two
//! terminals is an easy mistake to make.
//!
//! So a builder takes `<dir>.lock` for as long as it holds the directory. The
//! file sits *beside* the directory rather than inside it: the directory is
//! served to clients whole, and a runtime file in it is a file a client asks
//! for, a mirror copies and a checksum covers. It is created with `O_EXCL`,
//! which is one atomic step against a file system rather than a check and a
//! create with a race between them, and it carries the pid and start time of
//! whoever holds it so the refusal can say who to go and look at.
//!
//! # A stale lock
//!
//! The lock is removed on [`Drop`], which covers a normal exit and a panic
//! that unwinds. It does not cover `SIGKILL`, a host that lost power, or a
//! container killed for its memory — after any of those the file outlives
//! the process that made it and the next run is refused by a pid that is
//! gone. That is the right default: a lock that cleared itself on a guess
//! would clear itself exactly when a long build was still running and slow to
//! answer. Check the pid the refusal names, and if nothing is running under
//! it, clear the lock with [`Lock::force`].

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// A held index directory, released when this is dropped.
///
/// Take it once, for as long as the directory is being written, and let it
/// fall with the writer. It is deliberately not [`Clone`]: two of them would
/// be two writers, which is the thing being prevented.
#[derive(Debug)]
pub struct Lock {
    path: PathBuf,
}

impl Lock {
    /// Where a directory's lock sits, which is beside it rather than in it.
    ///
    /// The directory is served whole, so nothing that belongs to the running
    /// process may live under it.
    pub fn path(dir: &Path) -> PathBuf {
        let mut name = dir.as_os_str().to_owned();
        name.push(".lock");
        PathBuf::from(name)
    }

    /// Take the directory, or fail naming who holds it.
    ///
    /// `create_new` is `O_EXCL`: the file is made or the call fails, with no
    /// window between asking whether it exists and making it. An existing
    /// lock is read back so the error can quote the pid and the time it was
    /// taken, that being what a person needs to decide whether the holder is
    /// alive — see the module header on a stale lock.
    pub fn take(dir: &Path) -> io::Result<Lock> {
        let path = Lock::path(dir);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                // Best effort: a lock whose contents did not land is still a
                // lock, and failing the take over it would be refusing a
                // directory nothing holds.
                let _ = writeln!(file, "pid {}", std::process::id());
                let _ = writeln!(file, "since {}", chrono::Utc::now());
                Ok(Lock { path })
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                let held = std::fs::read_to_string(&path).unwrap_or_default();
                let held = held.trim().replace('\n', ", ");
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "{} is already being written: {} holds {}. \
                         If that process is gone, remove the lock.",
                        dir.display(),
                        if held.is_empty() {
                            "an unnamed writer"
                        } else {
                            held.as_str()
                        },
                        path.display(),
                    ),
                ))
            }
            Err(err) => Err(err),
        }
    }

    /// Take the directory, clearing a lock that is already there.
    ///
    /// The documented way out of a stale lock: a killed builder leaves its
    /// file behind and no amount of waiting will clear it. It is a separate
    /// call rather than a retry inside [`take`](Self::take) because the
    /// judgement it needs — that nothing is running under the pid the file
    /// names — is one only the person reading the refusal can make.
    pub fn force(dir: &Path) -> io::Result<Lock> {
        let path = Lock::path(dir);
        match std::fs::remove_file(&path) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => {
                return Err(err);
            }
            _ => {}
        }
        Lock::take(dir)
    }
}

impl Drop for Lock {
    /// Release the directory. A removal that fails leaves a stale lock, which
    /// is the case the module header covers, so there is nothing useful to
    /// report from here and nothing that may panic during an unwind.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn scratch(name: &str) -> PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("galos-lock-{name}-{n}"))
    }

    /// A second writer is refused while the first holds the directory, and
    /// the refusal names the pid to go and look at.
    ///
    /// This is the whole point of the file: two builders over one directory
    /// interleave two galaxies into it and neither can tell.
    #[test]
    fn a_second_take_is_refused() {
        let dir = scratch("contended");
        std::fs::create_dir_all(&dir).unwrap();
        let held = Lock::take(&dir).unwrap();

        let refused = Lock::take(&dir).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
        let said = refused.to_string();
        let pid = std::process::id();
        assert!(said.contains(&format!("pid {pid}")), "{said}");

        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The lock goes with the writer that took it, and the directory can then
    /// be taken again.
    ///
    /// A lock left behind by an orderly exit would refuse every later run of
    /// a daemon that is restarted for a living.
    #[test]
    fn the_lock_is_released_on_drop() {
        let dir = scratch("released");
        std::fs::create_dir_all(&dir).unwrap();

        let held = Lock::take(&dir).unwrap();
        assert!(Lock::path(&dir).exists());
        assert!(!dir.join(".lock").exists(), "it belongs beside, not inside");

        drop(held);
        assert!(!Lock::path(&dir).exists(), "left behind after a clean exit");

        let again = Lock::take(&dir).unwrap();
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A lock with nothing behind it — what a killed builder leaves — is
    /// cleared by [`Lock::force`], and the forced lock is a real one that
    /// refuses the next taker in its turn.
    #[test]
    fn force_clears_a_stale_lock() {
        let dir = scratch("stale");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Lock::path(&dir), "pid 999999\n").unwrap();

        assert!(Lock::take(&dir).is_err(), "a stale lock still refuses");
        let forced = Lock::force(&dir).unwrap();
        assert!(Lock::take(&dir).is_err(), "and the forced one holds");

        drop(forced);
        assert!(!Lock::path(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
