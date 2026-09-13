//! A file of [`System`] records, appended once and then mapped.
//!
//! What a build that cannot hold the galaxy writes it into: a region's
//! systems are streamed out of the database into one of these and the
//! region is built from the mapping, so they are on disk and never on the
//! heap. [`crate::region`] does the building; this is only the bytes.
//!
//! The same records the resume point's base is made of: `56` bytes of
//! `repr(C)` [`System`] as the machine holds one, no header, no framing. A
//! spill is scratch — written, read once, deleted — so unlike the base it
//! carries no magic and no version.

use crate::System;
use memmap2::Mmap;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Bytes one system occupies, which is the whole of the format.
pub const RECORD: usize = std::mem::size_of::<System>();

/// The bytes of `systems`, for a write.
///
/// # Safety of the cast
///
/// `System` is `repr(C)` with no padding in it — asserted where it is
/// declared — so every byte of the slice is an initialised byte of a field,
/// and a `u8` has no alignment to violate.
pub fn as_bytes(systems: &[System]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            systems.as_ptr().cast::<u8>(),
            std::mem::size_of_val(systems),
        )
    }
}

/// `bytes` read back as the records they were written from.
///
/// Answers [`None`] where the length is not a whole number of records or
/// the bytes are not aligned to hold one, which are the two things that
/// make the cast below unsound. Every bit pattern of `System`'s `u64`,
/// `f64` and `u32` fields is a valid value of that field, so there is
/// nothing else to check.
pub fn of_bytes(bytes: &[u8]) -> Option<&[System]> {
    if bytes.len() % RECORD != 0 {
        return None;
    }
    if bytes.as_ptr().align_offset(std::mem::align_of::<System>()) != 0 {
        return None;
    }
    Some(unsafe {
        std::slice::from_raw_parts(
            bytes.as_ptr().cast::<System>(),
            bytes.len() / RECORD,
        )
    })
}

/// A spill being written, a record at a time.
pub struct Spill {
    path: PathBuf,
    out: BufWriter<File>,
    count: u64,
}

impl Spill {
    /// Open `path` for writing, replacing anything there.
    pub fn create(path: &Path) -> io::Result<Spill> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Ok(Spill {
            path: path.to_owned(),
            out: BufWriter::with_capacity(1 << 20, File::create(path)?),
            count: 0,
        })
    }

    /// One more system.
    pub fn push(&mut self, system: System) -> io::Result<()> {
        self.out.write_all(as_bytes(std::slice::from_ref(&system)))?;
        self.count += 1;
        Ok(())
    }

    /// How many have been written.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Flush and close, answering where it landed.
    pub fn finish(mut self) -> io::Result<PathBuf> {
        self.out.flush()?;
        Ok(self.path)
    }
}

/// A spill, mapped.
///
/// The systems are the mapping, so holding one of these costs a file
/// descriptor and some address space rather than the records themselves.
pub struct Spilled {
    map: Option<Mmap>,
    count: usize,
}

/// A count, not a galaxy: the records are what this keeps out of anything
/// that formats it.
impl std::fmt::Debug for Spilled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spilled").field("systems", &self.count).finish()
    }
}

impl Spilled {
    /// Map a spill. An empty file maps to no systems rather than failing,
    /// since a region with nothing in it is a thing a cut can produce.
    pub fn open(path: &Path) -> io::Result<Spilled> {
        let file = File::open(path)?;
        let len = file.metadata()?.len() as usize;
        if len == 0 {
            return Ok(Spilled { map: None, count: 0 });
        }
        if len % RECORD != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not a whole number of records", path.display()),
            ));
        }
        // SAFETY: a spill is written once by this process and read after it
        // is finished; nothing truncates one under a reader.
        let map = unsafe { Mmap::map(&file)? };
        if of_bytes(&map).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not a mapping of records", path.display()),
            ));
        }
        Ok(Spilled { map: Some(map), count: len / RECORD })
    }

    /// The systems, pointing into the mapping.
    pub fn systems(&self) -> &[System] {
        match &self.map {
            None => &[],
            // SAFETY: `open` checked the length and the alignment, and the
            // mapping outlives the slice.
            Some(map) => of_bytes(map).unwrap_or(&[]),
        }
    }

    /// How many systems it holds.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system(id: u64) -> System {
        System {
            id64: id,
            position: [id as f64, -(id as f64), 0.5],
            absolute_magnitude: 4.0 - id as f64,
            temperature: 5_000.0 + id as f64,
            age_bucket: (id % 8) as u32,
            updated_at: 1_700_000_000 + id as u32,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("galos-spill-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir.join("spill")
    }

    /// What went in comes back, in order and to the bit: a spill is the
    /// build's only copy of the systems, so a record that shifted by a byte
    /// would be a galaxy somewhere else.
    #[test]
    fn round_trips_through_the_mapping() {
        let path = scratch("round-trip");
        let written: Vec<System> = (0..5_000).map(system).collect();

        let mut spill = Spill::create(&path).unwrap();
        for &s in &written {
            spill.push(s).unwrap();
        }
        assert_eq!(spill.count(), 5_000);
        spill.finish().unwrap();

        let spilled = Spilled::open(&path).unwrap();
        assert_eq!(spilled.len(), 5_000);
        assert_eq!(spilled.systems(), written.as_slice());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A region a cut made but nothing fell into is an empty file, not an
    /// error: mapping one of length zero is what fails on most systems.
    #[test]
    fn an_empty_spill_maps_to_nothing() {
        let path = scratch("empty");
        Spill::create(&path).unwrap().finish().unwrap();

        let spilled = Spilled::open(&path).unwrap();
        assert!(spilled.is_empty());
        assert_eq!(spilled.systems(), &[]);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A file that is not a whole number of records is refused rather than
    /// read as a galaxy one byte out of step.
    #[test]
    fn a_torn_spill_is_refused() {
        let path = scratch("torn");
        let mut spill = Spill::create(&path).unwrap();
        spill.push(system(1)).unwrap();
        spill.finish().unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(RECORD - 3);
        std::fs::write(&path, bytes).unwrap();

        let err = Spilled::open(&path).expect_err("a part-record spill read");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
