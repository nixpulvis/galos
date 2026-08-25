//! The builder's private resume point.
//!
//! The served index is a lossy projection: a payload [`Point`](crate::Point)
//! carries an exact position but a downcast `f32` magnitude, a bucketed
//! temperature, and no age at all, so the editable [`Tree`](crate::Tree) and its
//! aggregates cannot be rebuilt from it. This can. A checkpoint holds the
//! full-precision inputs the tree was last built from and the database time they
//! were read at, so a `--watch` restart rebuilds the tree in memory and follows
//! changes from the cursor rather than re-reading the whole database and
//! rewriting every file.
//!
//! It is server-private and never served: it belongs beside the builder, not in
//! the published directory a client reads. Written whole and atomically (a
//! sibling temp file renamed into place) after each publish, so the cursor and
//! the inputs it dates can never disagree and a torn write never replaces a good
//! checkpoint.

use crate::System;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::io;
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
        let bytes = rmp_serde::to_vec(self)
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
}
