//! Writing and withdrawing: records appended to a shard, and the
//! tombstones that beat the entries behind them.
//!
//! A write is two appends and no directory operation; a tail grown past its
//! bound is folded on the way out, which is [`fold`]'s.

use super::{
    ENTRY, Entry, Found, HEADER, find, fold, header_bytes, header_fields,
    tail_bound,
};
use crate::format::layout::{
    BODIES_DIR, bodies_path, body_data_path, body_index_path, body_shard,
    legacy_bodies_path,
};
use crate::records::SystemBodies;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// What a write of many systems came to.
///
/// A shard is written as one batch, so a batch that fails leaves its systems
/// unwritten and they come back in `kept` for the caller to try again with —
/// which is what [`crate::accumulate::bodies::OnDisk`] does with a disk that is
/// full and then is not.
#[derive(Debug, Default)]
pub struct Wrote {
    /// Systems written.
    pub wrote: usize,
    /// Systems that could not be, and are still the caller's.
    pub kept: HashMap<i64, SystemBodies>,
    /// The first thing that went wrong, where anything did.
    pub failed: Option<io::Error>,
}

/// Group rows by shard, encoding each record on the way.
///
/// A body that will not encode is not a body a retry fixes, so it is
/// reported rather than batched; the systems either side of it still go.
fn batches<'a>(
    rows: impl IntoIterator<Item = (i64, &'a SystemBodies)>,
) -> (HashMap<u64, Vec<(i64, Vec<u8>)>>, Option<io::Error>) {
    let mut batches: HashMap<u64, Vec<(i64, Vec<u8>)>> = HashMap::new();
    let mut failed = None;
    for (address, inside) in rows {
        match rmp_serde::to_vec(inside) {
            Ok(bytes) => {
                batches
                    .entry(body_shard(address))
                    .or_default()
                    .push((address, bytes));
            }
            Err(err) => {
                failed.get_or_insert(io::Error::new(
                    io::ErrorKind::InvalidData,
                    err,
                ));
            }
        }
    }
    (batches, failed)
}

/// Write what a store is holding into the pack.
///
/// Grouped by shard, so a shard is one open of each of its two files and
/// one append to each however many of the held systems fell in it. A shard
/// that cannot be written leaves its systems with the caller rather than
/// dropping them.
pub fn write(dir: &Path, mut rows: HashMap<i64, SystemBodies>) -> Wrote {
    let (batches, failed) = batches(rows.iter().map(|(a, b)| (*a, b)));
    let mut done = Wrote { failed, ..Wrote::default() };
    for (shard, batch) in batches {
        match append(dir, shard, &batch) {
            Ok(()) => {
                done.wrote += batch.len();
                for (address, _) in &batch {
                    rows.remove(address);
                }
            }
            Err(err) => {
                done.failed.get_or_insert(err);
            }
        }
    }
    done.kept = rows;
    done
}

/// The same, for a caller with nothing to keep: a write that fails is the
/// caller's error rather than a batch handed back.
pub fn write_each<'a>(
    dir: &Path,
    rows: impl IntoIterator<Item = (i64, &'a SystemBodies)>,
) -> io::Result<usize> {
    let (batches, failed) = batches(rows);
    if let Some(err) = failed {
        return Err(err);
    }
    let mut wrote = 0;
    for (shard, batch) in batches {
        append(dir, shard, &batch)?;
        wrote += batch.len();
    }
    Ok(wrote)
}

/// Withdraw a system's bodies, answering whether the pack held any.
///
/// A tombstone rather than an erasure: the entry it hides may be in the
/// base, and an absence would read straight through to it.
pub fn remove(dir: &Path, address: i64) -> io::Result<bool> {
    match find(dir, address)? {
        Found::Bodies(_) => {
            append(dir, body_shard(address), &[(address, Vec::new())])?;
            Ok(true)
        }
        Found::Withdrawn | Found::Absent => Ok(false),
    }
}

/// Withdraw the bodies of `address`, answering whether there were any.
///
/// All three layouts: a tombstone in the pack, and the two loose files
/// removed. Leaving either loose file behind would leave the withdrawn scan
/// for a fallback to read, and leaving out the tombstone would leave it in
/// the pack.
pub fn remove_bodies(dir: &Path, address: i64) -> io::Result<bool> {
    let mut removed = crate::store::bodies::remove(dir, address)?;
    for path in [bodies_path(dir, address), legacy_bodies_path(dir, address)] {
        match std::fs::remove_file(&path) {
            Ok(()) => removed = true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(removed)
}

/// Append a batch of records to one shard, and fold if the tail has grown.
///
/// The record before the entry that names it, always: a reader that meets an
/// entry must find the bytes it points at, where a record nothing points at
/// is only a few bytes of a data file waiting to be compacted away.
pub(super) fn append(
    dir: &Path,
    shard: u64,
    batch: &[(i64, Vec<u8>)],
) -> io::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir.join(BODIES_DIR))?;
    let path = body_index_path(dir, shard);
    let mut index =
        OpenOptions::new().read(true).write(true).create(true).open(&path)?;
    let mut head = [0u8; HEADER];
    let length = index.metadata()?.len();
    let (generation, base) = match length >= HEADER as u64 {
        true => {
            index.read_exact(&mut head)?;
            // The header alone: what follows it is the file, which this has
            // not read and [`header_fields`] does not ask about.
            let header = header_fields(&head, &path)?;
            let entries = (length as usize - HEADER) / ENTRY;
            if header.base > entries {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{}: a base of {} entries in a file holding {entries}",
                        path.display(),
                        header.base,
                    ),
                ));
            }
            (header.generation, header.base)
        }
        false => {
            index.write_all(&header_bytes(0, 0))?;
            (0, 0)
        }
    };

    let data = body_data_path(dir, shard, generation);
    let mut file = OpenOptions::new().append(true).create(true).open(&data)?;
    let mut at = file.metadata()?.len();
    let mut records = Vec::new();
    let mut entries = Vec::with_capacity(batch.len() * ENTRY);
    for (address, bytes) in batch {
        let len = bytes.len() as u32;
        let entry = match len {
            // A tombstone points nowhere: there is no record to point at.
            0 => Entry { address: *address, offset: 0, len: 0 },
            _ => {
                records.extend_from_slice(&len.to_le_bytes());
                records.extend_from_slice(bytes);
                let entry = Entry { address: *address, offset: at, len };
                at += 4 + bytes.len() as u64;
                entry
            }
        };
        entry.onto(&mut entries);
    }
    file.write_all(&records)?;
    file.flush()?;

    index.seek(SeekFrom::End(0))?;
    index.write_all(&entries)?;
    index.flush()?;

    let entries = (index.metadata()?.len() as usize - HEADER) / ENTRY;
    let tail = entries - base;
    match tail > tail_bound(base) {
        true => fold(dir, shard),
        false => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::Table;
    use super::super::fixtures::{held, inside, scratch};
    use super::super::read_bodies;
    use super::*;
    use crate::records::Barycenter;
    use std::path::PathBuf;

    /// A shard that has been folded still takes an append
    ///
    /// [`append`] reads the sixteen header bytes to learn the generation
    /// and the base, and those sixteen bytes cannot hold a base of *n*
    /// entries, so checking them as the whole file would refuse every
    /// append to a folded shard — and with it every pack of a loose body
    /// file into that shard. A fold happens when a tail grows past
    /// thousands of entries, which no other test reaches, so this one folds
    /// by hand.
    #[test]
    fn a_folded_shard_still_takes_an_append() {
        let dir = scratch("foldappend");
        let address = 4_611_686_020_061_657_985_i64;
        let shard = body_shard(address);

        write(&dir, HashMap::from([(address, inside(1))]));
        // Into the base, which is what a fold does with a tail.
        fold(&dir, shard).expect("the shard folds");
        let folded =
            Table::read(&body_index_path(&dir, shard)).expect("a table");
        assert_eq!(folded.base.len(), 1, "the fold left nothing in the base");
        assert!(folded.tail.is_empty());

        // And now another system into the same shard. The shard is a hash
        // of the address, so the next one is looked for rather than
        // guessed at.
        let next = (1..10_000)
            .map(|n| address + n)
            .find(|&it| body_shard(it) == shard)
            .expect("another address in the same shard");
        let wrote = write(&dir, HashMap::from([(next, inside(2))]));
        assert!(wrote.failed.is_none(), "{:?}", wrote.failed);

        // Both readable, the folded one and the appended one.
        assert!(matches!(held(&dir, address), Found::Bodies(_)));
        assert!(matches!(held(&dir, next), Found::Bodies(_)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An empty scratch directory named after the test using it.
    fn loose_scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("galos_bodies_loose_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A system's insides, told apart by the address they belong to.
    fn loose_inside(address: i64) -> SystemBodies {
        SystemBodies {
            barycenters: vec![Barycenter {
                system_address: address,
                id: 0,
                updated_at: "2026-08-08T12:00:00Z".parse().expect("a moment"),
                updated_by: "a test".into(),
                orbit: None,
            }],
            ..SystemBodies::default()
        }
    }

    /// A withdrawn scan loses its flat file as well as its sharded one
    ///
    /// The removal is what makes a system whose last scan was taken back read
    /// as unscanned. Clearing only the sharded file would leave the flat,
    /// unsharded one for the read to fall back onto.
    #[test]
    fn removing_a_body_clears_both_layouts() {
        use crate::format::msgpack::write_meta;

        let dir = loose_scratch("remove");
        let address = 2_412_116_659_890_i64;
        write_meta(&bodies_path(&dir, address), &loose_inside(address))
            .expect("the sharded file writes");
        write_meta(&legacy_bodies_path(&dir, address), &loose_inside(address))
            .expect("the flat file writes");

        assert!(
            remove_bodies(&dir, address).expect("the removal"),
            "the removal said there was nothing to remove",
        );
        assert!(!bodies_path(&dir, address).exists());
        assert!(!legacy_bodies_path(&dir, address).exists());
        assert_eq!(
            read_bodies(&dir, address).expect("a read"),
            SystemBodies::default(),
            "a withdrawn scan still reads as one that stands",
        );

        assert!(
            !remove_bodies(&dir, address).expect("a second removal"),
            "removing nothing was reported as removing something",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
