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

use crate::System;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::Path;

/// The inputs the tree was last built from and the cursor they date to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// The database time the inputs were read at, in UTC. Opaque here; the
    /// builder reads the changes since it.
    pub cursor: NaiveDateTime,
    /// Every positioned system the tree was built from, at full precision.
    pub inputs: Vec<System>,
}

impl Checkpoint {
    /// Read a checkpoint back, MessagePack-decoded. A missing or unreadable file
    /// is an error the caller turns into a full build.
    pub fn read(path: &Path) -> io::Result<Checkpoint> {
        let bytes = std::fs::read(path)?;
        rmp_serde::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Write the checkpoint atomically: serialize, write a sibling temp file,
    /// then rename it over `path`. An interrupted write leaves the previous
    /// checkpoint intact, since the rename is the only step that touches `path`.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        Checkpoint::write_from(path, self.cursor, &self.inputs)
    }

    /// The same write, for a caller that still needs its inputs.
    ///
    /// A full build holds the only copy of a hundred and twenty-nine
    /// million systems and goes on to write the metadata out of them.
    /// Handing them over to be written would mean cloning gigabytes to
    /// serialize them once, so the borrowed shape is serialized instead —
    /// the same two fields in the same order, which is the same bytes.
    pub fn write_from(
        path: &Path,
        cursor: NaiveDateTime,
        inputs: &[System],
    ) -> io::Result<()> {
        #[derive(Serialize)]
        struct Borrowed<'a> {
            cursor: NaiveDateTime,
            inputs: &'a [System],
        }

        let bytes = rmp_serde::to_vec(&Borrowed { cursor, inputs })
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

    /// A checkpoint written and read back is the same checkpoint, cursor and
    /// every input intact through the MessagePack round trip.
    #[test]
    fn round_trips_through_disk() {
        let dir = scratch("round-trip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("checkpoint.bin");

        let checkpoint = Checkpoint {
            cursor: chrono::DateTime::from_timestamp(1_700_000_000, 0)
                .unwrap()
                .naive_utc(),
            inputs: (0..1000).map(system).collect(),
        };
        checkpoint.write(&path).unwrap();
        assert_eq!(Checkpoint::read(&path).unwrap(), checkpoint);

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
            cursor: chrono::DateTime::from_timestamp(1, 0).unwrap().naive_utc(),
            inputs: vec![system(1)],
        };
        let second = Checkpoint {
            cursor: chrono::DateTime::from_timestamp(2, 0).unwrap().naive_utc(),
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
            cursor: chrono::DateTime::from_timestamp(1_700_000_000, 0)
                .unwrap()
                .naive_utc(),
            inputs: (1..40).map(system).collect(),
        };
        held.write(&path).unwrap();
        let owned = std::fs::read(&path).unwrap();

        Checkpoint::write_from(&path, held.cursor, &held.inputs).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), owned);
        assert_eq!(Checkpoint::read(&path).unwrap(), held);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
