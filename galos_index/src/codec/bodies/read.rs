//! Reading a system's bodies: the pack first, then the loose files of the
//! older layouts.
//!
//! See [`super`] for the layout. A reader maps the index, searches it, and
//! reads one record; nothing is resident.

use super::{ENTRY, Entry, Found, HEADER, header_of};
use crate::codec::Directory;
use crate::codec::layout::{
    bodies_path, body_data_path, body_index_path, body_shard,
    legacy_bodies_path,
};
use crate::records::SystemBodies;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

impl Directory<'_> {
    /// What the pack holds for `address`.
    ///
    /// The index is mapped rather than read: the base is binary-searched and the
    /// tail scanned newest-first, so a click costs a page or two of a file that
    /// may be megabytes. A data file that has gone out from under the read is a
    /// compaction landing, and the answer is to read the index again — the new
    /// one names the generation that exists.
    pub fn find_bodies(self, address: i64) -> io::Result<Found> {
        let dir = self.root;
        match found(dir, address) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                found(dir, address)
            }
            answer => answer,
        }
    }
}

/// One attempt at [`find`].
fn found(dir: &Path, address: i64) -> io::Result<Found> {
    let shard = body_shard(address);
    let path = body_index_path(dir, shard);
    let file = match File::open(&path) {
        Ok(file) => file,
        // No index file is a shard nothing has been packed into, which is
        // not the same as a data file disappearing mid-read: this one is
        // the caller's answer, not a retry.
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(Found::Absent);
        }
        Err(err) => return Err(err),
    };
    // Safety: the file is opened read-only and the mapping is dropped before
    // this returns. A writer only ever appends to it or renames another file
    // over it, so the bytes under the mapping are not rewritten in place.
    let mapped = unsafe { memmap2::Mmap::map(&file)? };
    let header = header_of(&mapped, &path)?;
    let entries = &mapped[HEADER..];
    let at = |n: usize| Entry::of(&entries[n * ENTRY..]);
    let count = entries.len() / ENTRY;

    // The tail first and backwards: it is the newer half, and the last thing
    // said about a system is what the pack holds.
    let mut found = None;
    for n in (header.base..count).rev() {
        let entry = at(n);
        if entry.address == address {
            found = Some(entry);
            break;
        }
    }
    if found.is_none() {
        // The base is sorted, so the rest is a binary search. A system
        // written twice before a fold leaves one entry here, the fold having
        // kept the newer.
        let mut lo = 0usize;
        let mut hi = header.base;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let entry = at(mid);
            match entry.address.cmp(&address) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    found = Some(entry);
                    break;
                }
            }
        }
    }
    let Some(entry) = found else { return Ok(Found::Absent) };
    if entry.withdrawn() {
        return Ok(Found::Withdrawn);
    }
    let generation = header.generation;
    drop(mapped);

    let bytes = record(&body_data_path(dir, shard, generation), &entry)?;
    let inside = rmp_serde::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Found::Bodies(inside))
}

/// One record's bytes out of a data file.
fn record(path: &Path, entry: &Entry) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(entry.offset))?;
    let mut framed = [0u8; 4];
    file.read_exact(&mut framed)?;
    let len = u32::from_le_bytes(framed);
    if len != entry.len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{}: the index says {} bytes at {} and the record says {len}",
                path.display(),
                entry.len,
                entry.offset,
            ),
        ));
    }
    let mut bytes = vec![0u8; len as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

impl Directory<'_> {
    /// What the bodies of `address` are, empty where nothing has scanned it.
    ///
    /// Three layouts, newest first: the packed shard files, then the loose file
    /// a system in its shard directory, then the flat, unsharded one. A
    /// directory part way through a packing answers out of whichever holds the
    /// system, and a system the pack says was *withdrawn* is empty rather than
    /// whatever a loose file for it still says.
    ///
    /// A system nothing has scanned is [`SystemBodies::default`] rather than an
    /// error.
    pub fn read_bodies(self, address: i64) -> io::Result<SystemBodies> {
        let dir = self.root;
        match self.find_bodies(address)? {
            crate::codec::bodies::Found::Bodies(inside) => return Ok(inside),
            crate::codec::bodies::Found::Withdrawn => {
                return Ok(SystemBodies::default());
            }
            crate::codec::bodies::Found::Absent => {}
        }
        let bytes = match std::fs::read(bodies_path(dir, address)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                match std::fs::read(legacy_bodies_path(dir, address)) {
                    Ok(bytes) => bytes,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        return Ok(SystemBodies::default());
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };
        rmp_serde::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}
