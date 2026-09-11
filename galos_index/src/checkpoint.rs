//! The builder's private resume point.
//!
//! The served index is a lossy projection: a payload [`Point`](crate::Point)
//! carries an exact position and the second a system was last updated at, but a
//! downcast `f32` magnitude, a bucketed temperature, and no age bucket, so the
//! editable [`Tree`](crate::Tree) and its aggregates cannot be rebuilt from it.
//! This can. A checkpoint holds the full-precision inputs the tree was
//! last built from and the database time they were read at, so a `--watch`
//! restart rebuilds the tree in memory and follows changes from the cursor
//! rather than re-reading the whole database and rewriting every file.
//!
//! It is server-private and never served: it belongs beside the builder, not in
//! the published directory a client reads. Written whole and atomically (a
//! sibling temp file renamed into place) after each publish, so the cursor and
//! the inputs it dates can never disagree and a torn write never replaces a good
//! checkpoint.
//!
//! It also carries what derived it. Two things write one, an event feed and
//! a read of the database, and they mean different things by a cursor — see
//! [`By`]. A resume that could not tell them apart would take one's work for
//! the other's and publish a directory nobody asked for.

use crate::System;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::Path;

/// What derived a checkpoint, and so what its cursor means.
///
/// The two derivations meant different things by the same field. The
/// database side wrote the database clock and read the rows stamped after it
/// on the next pass; the event side wrote the host's wall clock, which
/// nothing ever read. A directory written by one and resumed by the other
/// therefore either skipped every row in the gap between two clocks or
/// re-read the galaxy, and said nothing either way. Recording which wrote it
/// lets the resume refuse: see the crate's `--index` handoff, where a
/// mismatch forces a full rebuild rather than a silent wrong-sized one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum By {
    /// Live entries, from EDDN or a journal, applied as they arrived.
    Events,
    /// A read of the database, up to the clock the cursor carries.
    Database,
}

/// The inputs the tree was last built from, what derived them and the cursor
/// they date to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// The database time the inputs were read at, in UTC. Opaque here; the
    /// builder reads the changes since it. [`None`] for an event run with no
    /// database behind it, there being no clock to ask and nothing to resume
    /// against.
    pub cursor: Option<NaiveDateTime>,
    /// Which derivation wrote this, which is what makes the cursor readable.
    pub by: By,
    /// Every positioned system the tree was built from, at full precision.
    pub inputs: Vec<System>,
}

impl Checkpoint {
    /// Read a checkpoint back, MessagePack-decoded. A missing or unreadable
    /// file is an error the caller turns into a full build.
    ///
    /// A file written before the provenance existed carried `cursor` and
    /// `inputs` and nothing else. Those are all on disk now, written by the
    /// database derivation — the event side's cursor was never read by
    /// anything — so an old file decodes as [`By::Database`] with the cursor
    /// it carried, which is what its writer meant by it. Tried second, so a
    /// current file is never read through the old shape.
    pub fn read(path: &Path) -> io::Result<Checkpoint> {
        #[derive(Deserialize)]
        struct Old {
            cursor: NaiveDateTime,
            inputs: Vec<System>,
        }

        let bytes = std::fs::read(path)?;
        if let Ok(checkpoint) = rmp_serde::from_slice(&bytes) {
            return Ok(checkpoint);
        }
        rmp_serde::from_slice(&bytes)
            .map(|old: Old| Checkpoint {
                cursor: Some(old.cursor),
                by: By::Database,
                inputs: old.inputs,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Write the checkpoint atomically: serialize, write a sibling temp file,
    /// then rename it over `path`. An interrupted write leaves the previous
    /// checkpoint intact, since the rename is the only step that touches `path`.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        Checkpoint::write_from(path, self.cursor, self.by, &self.inputs)
    }

    /// The same write, for a caller that still needs its inputs.
    ///
    /// A full build holds the only copy of a hundred and twenty-nine
    /// million systems and goes on to write the metadata out of them.
    /// Handing them over to be written would mean cloning gigabytes to
    /// serialize them once, so the borrowed shape is serialized instead —
    /// the same three fields in the same order, which is the same bytes.
    pub fn write_from(
        path: &Path,
        cursor: Option<NaiveDateTime>,
        by: By,
        inputs: &[System],
    ) -> io::Result<()> {
        #[derive(Serialize)]
        struct Borrowed<'a> {
            cursor: Option<NaiveDateTime>,
            by: By,
            inputs: &'a [System],
        }

        let bytes = rmp_serde::to_vec(&Borrowed { cursor, by, inputs })
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    }
}

/// What has been published since the last whole checkpoint.
///
/// A checkpoint is every system at full precision, so it rides a timer while
/// the directory moves on every publish. A restart rebuilding from the
/// checkpoint alone therefore rebuilt the tree short of what the directory
/// already served, published the shortfall over it, and left a directory
/// whose halves stood for different systems. This is the difference, in the
/// same full precision, appended per publish and replayed at open. It costs
/// what moved where a checkpoint costs the galaxy, and a whole checkpoint
/// clears it.
///
/// Framed rather than one document, being appended to: a `u32` length ahead
/// of each MessagePack batch. A kill mid-append leaves a torn frame, which
/// [`Pending::read`] stops at — that batch was never published either.
pub struct Pending;

impl Pending {
    /// Where the log sits, which is beside the checkpoint it extends.
    pub fn path(checkpoint: &Path) -> std::path::PathBuf {
        let mut name = checkpoint.as_os_str().to_owned();
        name.push(".pending");
        std::path::PathBuf::from(name)
    }

    /// Add what a publish wrote.
    pub fn append(checkpoint: &Path, systems: &[System]) -> io::Result<()> {
        if systems.is_empty() {
            return Ok(());
        }
        let bytes = rmp_serde::to_vec(&systems)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let path = Pending::path(checkpoint);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let len = u32::try_from(bytes.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "a batch of 4 GiB")
        })?;
        file.write_all(&len.to_le_bytes())?;
        file.write_all(&bytes)?;
        file.flush()
    }

    /// Everything appended since the last whole checkpoint, in order.
    ///
    /// A missing log is nothing to replay. A torn or unreadable tail ends the
    /// replay where it began, since the rest was never published either.
    pub fn read(checkpoint: &Path) -> Vec<System> {
        let Ok(bytes) = std::fs::read(Pending::path(checkpoint)) else {
            return Vec::new();
        };
        let mut replayed = Vec::new();
        let mut at = 0;
        while at + 4 <= bytes.len() {
            let len = u32::from_le_bytes([
                bytes[at],
                bytes[at + 1],
                bytes[at + 2],
                bytes[at + 3],
            ]) as usize;
            at += 4;
            let Some(frame) = bytes.get(at..at + len) else { break };
            let Ok(batch) = rmp_serde::from_slice::<Vec<System>>(frame) else {
                break;
            };
            replayed.extend(batch);
            at += len;
        }
        replayed
    }

    /// Drop the log, the checkpoint beside it now holding what it held.
    pub fn clear(checkpoint: &Path) -> io::Result<()> {
        match std::fs::remove_file(Pending::path(checkpoint)) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            it => it,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn scratch(name: &str) -> std::path::PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("galos-ckpt-{name}-{n}"))
    }

    fn system(id: u64) -> System {
        System {
            id64: id,
            position: [id as f64, -(id as f64), 12.5],
            absolute_magnitude: 4.83 - id as f64,
            temperature: 3000.0 + id as f64,
            age_bucket: (id % 8) as usize,
            updated_at: 1_700_000_000 + id as u32,
        }
    }

    fn at(seconds: i64) -> Option<chrono::NaiveDateTime> {
        Some(chrono::DateTime::from_timestamp(seconds, 0).unwrap().naive_utc())
    }

    /// A checkpoint written and read back is the same checkpoint: cursor,
    /// what wrote it and every input intact through the MessagePack round
    /// trip.
    #[test]
    fn round_trips_through_disk() {
        let dir = scratch("round-trip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("checkpoint.bin");

        let checkpoint = Checkpoint {
            cursor: at(1_700_000_000),
            by: By::Database,
            inputs: (0..1000).map(system).collect(),
        };
        checkpoint.write(&path).unwrap();
        assert_eq!(Checkpoint::read(&path).unwrap(), checkpoint);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An event run with no database behind it keeps its cursorless shape
    /// through the round trip rather than reading back as a resume point.
    ///
    /// [`None`] is the whole of the claim: there was no clock to ask, so
    /// there is no window to catch up from, and a reader must go and build
    /// the directory afresh instead of following from a time that would be
    /// the host's wall clock and mean nothing to the database.
    #[test]
    fn an_event_run_has_no_cursor() {
        let dir = scratch("eventful");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("checkpoint.bin");

        let checkpoint = Checkpoint {
            cursor: None,
            by: By::Events,
            inputs: (0..10).map(system).collect(),
        };
        checkpoint.write(&path).unwrap();

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read, checkpoint);
        assert_eq!(read.cursor, None);
        assert_eq!(read.by, By::Events);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A checkpoint from before the provenance existed reads back as the
    /// database derivation, keeping the cursor it carried.
    ///
    /// There are such files on disk right now, written by the only thing
    /// that ever wrote a cursor anything read: a pass over the database, up
    /// to the database clock. Reading them as anything else would either
    /// throw away a week of catch-up or resume an event directory against a
    /// clock that was never the database's.
    #[test]
    fn an_old_checkpoint_reads_as_the_database() {
        #[derive(Serialize)]
        struct Old {
            cursor: chrono::NaiveDateTime,
            inputs: Vec<System>,
        }

        let dir = scratch("old-form");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("checkpoint.bin");

        let inputs: Vec<System> = (1..40).map(system).collect();
        let old = Old { cursor: at(1_700_000_000).unwrap(), inputs };
        std::fs::write(&path, rmp_serde::to_vec(&old).unwrap()).unwrap();

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read.cursor, at(1_700_000_000));
        assert_eq!(read.by, By::Database);
        assert_eq!(read.inputs, old.inputs);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The atomic write replaces a previous checkpoint in place rather than
    /// failing on an existing file, and leaves no temp file behind.
    #[test]
    fn overwrites_in_place() {
        let dir = scratch("overwrite");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("checkpoint.bin");

        let first = Checkpoint {
            cursor: at(1),
            by: By::Database,
            inputs: vec![system(1)],
        };
        let second = Checkpoint {
            cursor: at(2),
            by: By::Database,
            inputs: vec![system(1), system(2)],
        };
        first.write(&path).unwrap();
        second.write(&path).unwrap();

        assert_eq!(Checkpoint::read(&path).unwrap(), second);
        assert!(!path.with_extension("tmp").exists(), "temp file left behind");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The borrowed write is the owned one, byte for byte
    ///
    /// A full build keeps its inputs and writes them through
    /// [`Checkpoint::write_from`] rather than cloning a galaxy to hand
    /// them over; everything else reads them back through
    /// [`Checkpoint::read`]. The two are one format or a build's resume
    /// point is one nothing can resume from.
    #[test]
    fn a_borrowed_write_is_the_same_file() {
        let dir = scratch("borrowed");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("checkpoint.bin");

        let held = Checkpoint {
            cursor: at(1_700_000_000),
            by: By::Database,
            inputs: (1..40).map(system).collect(),
        };
        held.write(&path).unwrap();
        let owned = std::fs::read(&path).unwrap();

        Checkpoint::write_from(&path, held.cursor, held.by, &held.inputs)
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), owned);
        assert_eq!(Checkpoint::read(&path).unwrap(), held);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
