//! Rows written to a file and sorted without the table ever being in
//! memory.
//!
//! Every table this crate publishes is one address-ordered array, and every
//! derivation of one produces its rows in the order the galaxy happened to
//! be read rather than in address order. The gap between those two is a
//! sort, and at 200 M systems it is a sort of gigabytes: the names alone are
//! ~5.8 GB of rows, which was the last structure on this road that the whole
//! sky had to fit in.
//!
//! So the rows go to a file as they are derived ([`Sheet`]), and the sort is
//! external: runs of [`RUN_BYTES`] are read back, sorted and written out,
//! and the runs are merged ([`sorted`]). What a table costs to write is one
//! run.
//!
//! Two rules hold across the split, and both callers lean on them:
//!
//! - **The last row an address has wins.** A run is a stretch of the row
//!   file, so every row in one is older than every row in the next, and the
//!   sort inside a run is stable. A build carrying on from a published table
//!   pushes the table's rows in first and its own over them.
//! - **A row half written is the end of the file**, not a row read out of
//!   the wrong bytes: rows are length-framed, and [`Framed::next`] reads a
//!   short frame as the end of what the file stands for. A build killed
//!   mid-push leaves a file its next open can still read.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// How many bytes of rows one sorted run holds.
///
/// What the sort costs in memory, and the only dial it has. A galaxy's
/// names are ~5.8 GB of rows, which is tens of runs at this size — few
/// enough that [`merge`] can scan their heads rather than heap them.
pub(crate) const RUN_BYTES: usize = 128 * 1024 * 1024;

/// One table's rows, length-framed so a row half written is the end of what
/// the file stands for rather than a row read out of the wrong bytes.
pub(crate) struct Sheet {
    path: PathBuf,
    out: BufWriter<File>,
}

impl Sheet {
    /// Open `path`, empty.
    pub(crate) fn open(path: PathBuf) -> io::Result<Sheet> {
        let file = File::create(&path)?;
        Ok(Sheet { path, out: BufWriter::new(file) })
    }

    /// Where the rows are being written.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// One row, its length ahead of it.
    pub(crate) fn push<T: Serialize>(&mut self, row: &T) -> io::Result<()> {
        let bytes = rmp_serde::to_vec(row)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.out.write_all(&(bytes.len() as u32).to_le_bytes())?;
        self.out.write_all(&bytes)
    }

    /// Everything pushed, on disk.
    pub(crate) fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// A row file read a row at a time.
///
/// Nothing reads a row file twice, so the rows go past rather than in: the
/// whole point of writing them to a file was not to hold them.
pub(crate) struct Framed {
    inner: BufReader<File>,
    buf: Vec<u8>,
}

impl Framed {
    /// Open a row file, or answer [`None`] where there is not one.
    pub(crate) fn open(path: &Path) -> io::Result<Option<Framed>> {
        match File::open(path) {
            Ok(file) => Ok(Some(Framed {
                inner: BufReader::new(file),
                buf: Vec::new(),
            })),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The next row and what it took on disk, or the end of the file.
    ///
    /// A row half written is a row the build never marked, so a short read
    /// is the end of what the file stands for rather than a failure.
    pub(crate) fn next<T: DeserializeOwned>(
        &mut self,
    ) -> io::Result<Option<(T, usize)>> {
        let mut head = [0u8; 4];
        match self.inner.read_exact(&mut head) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                return Ok(None);
            }
            Err(err) => return Err(err),
        }
        let len = u32::from_le_bytes(head) as usize;
        self.buf.resize(len, 0);
        match self.inner.read_exact(&mut self.buf) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                return Ok(None);
            }
            Err(err) => return Err(err),
        }
        let row = rmp_serde::from_slice(&self.buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some((row, len + 4)))
    }
}

/// A row file sorted by address, and the scratch files that took it there.
///
/// Held rather than returned as a path so the runs and the merged file are
/// removed when the caller is done with them, whichever way it leaves: a
/// galaxy's sort is gigabytes of scratch, and a build that failed part way
/// through writing its table must not leave them behind.
pub(crate) struct Sorted {
    merged: PathBuf,
    count: usize,
    runs: Vec<PathBuf>,
}

impl Sorted {
    /// How many addresses the sort came to, each once.
    pub(crate) fn count(&self) -> usize {
        self.count
    }

    /// The sorted rows, to be read once. Empty where nothing was pushed.
    pub(crate) fn rows(&self) -> io::Result<Option<Framed>> {
        Framed::open(&self.merged)
    }
}

impl Drop for Sorted {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.merged);
        for run in &self.runs {
            let _ = std::fs::remove_file(run);
        }
    }
}

/// Sort a row file into address order, the last row an address has winning.
///
/// `name` names the scratch files, so two sorts in one directory do not
/// collide. `budget` is how many bytes of rows a run holds, which is
/// [`RUN_BYTES`] everywhere but a test that wants several runs out of a
/// handful of rows.
pub(crate) fn sorted<T: Serialize + DeserializeOwned>(
    rows: &Path,
    scratch: &Path,
    name: &str,
    key: &impl Fn(&T) -> i64,
    budget: usize,
) -> io::Result<Sorted> {
    let runs = spill_runs::<T>(rows, scratch, name, key, budget)?;
    let merged = scratch.join(format!("{name}.sorted"));
    let count = merge::<T>(&runs, &merged, key)?;
    Ok(Sorted { merged, count, runs })
}

/// Read the rows a run at a time, sort each run, and answer the runs.
fn spill_runs<T: Serialize + DeserializeOwned>(
    rows: &Path,
    scratch: &Path,
    name: &str,
    key: &impl Fn(&T) -> i64,
    budget: usize,
) -> io::Result<Vec<PathBuf>> {
    let mut runs = Vec::new();
    let Some(mut framed) = Framed::open(rows)? else {
        return Ok(runs);
    };
    let mut held: Vec<T> = Vec::new();
    let mut bytes = 0usize;
    let mut ended = false;
    while !ended {
        match framed.next::<T>()? {
            Some((row, width)) => {
                held.push(row);
                bytes += width;
            }
            None => ended = true,
        }
        if held.is_empty() || (!ended && bytes < budget) {
            continue;
        }
        // Stable, so the rows an address has keep the order they were
        // written in and the last of them is still the last.
        held.sort_by_key(|it| key(it));
        let path = scratch.join(format!("{name}.run{:04}", runs.len()));
        let mut run = Sheet::open(path.clone())?;
        for row in held.drain(..) {
            run.push(&row)?;
        }
        run.flush()?;
        runs.push(path);
        bytes = 0;
    }
    Ok(runs)
}

/// Merge sorted runs into one file in address order, the last row an
/// address has winning.
///
/// A scan over the runs' heads rather than a heap: a run is [`RUN_BYTES`]
/// and a galaxy's rows are gigabytes, so there are tens of runs and the
/// scan costs less than the code a heap would.
fn merge<T: Serialize + DeserializeOwned>(
    runs: &[PathBuf],
    out: &Path,
    key: &impl Fn(&T) -> i64,
) -> io::Result<usize> {
    let mut readers = Vec::new();
    let mut heads: Vec<Option<T>> = Vec::new();
    for run in runs {
        let mut framed = Framed::open(run)?.expect("a run just written");
        heads.push(framed.next::<T>()?.map(|(row, _)| row));
        readers.push(framed);
    }

    let mut sorted = Sheet::open(out.to_owned())?;
    let mut count = 0usize;
    loop {
        let Some(address) = heads.iter().flatten().map(key).min() else {
            break;
        };
        // The runs in order, so a later run's row is taken over an earlier
        // one's, and inside a run the last of a stretch over the first:
        // both are the one rule, that the last row written wins.
        let mut best: Option<T> = None;
        for (at, head) in heads.iter_mut().enumerate() {
            while head.as_ref().is_some_and(|it| key(it) == address) {
                best = head.take();
                *head = readers[at].next::<T>()?.map(|(row, _)| row);
            }
        }
        sorted.push(&best.expect("the address came off a head"))?;
        count += 1;
    }
    sorted.flush()?;
    Ok(count)
}
