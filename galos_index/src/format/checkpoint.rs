//! The builder's private resume point: a base compacted rarely, and the
//! deltas since.
//!
//! The served index is lossy — a payload [`Point`](crate::Point) downcasts
//! the magnitude, buckets the temperature and drops the age — so the
//! editable [`Tree`](crate::Tree) cannot be rebuilt from it. This holds the
//! full-precision inputs the tree was last built from and the database time
//! they were read at, so a `--watch` restart rebuilds the tree in memory
//! and follows changes from the cursor. It is server-private and never
//! served, and it carries what derived it, an event feed and a database
//! read meaning different things by a cursor — see [`By`].
//!
//! ## Two files
//!
//! **The log is what a publish writes**: one [`Pending`] frame, costing
//! what moved. **The base is the compaction**, written when the log has
//! grown against it — see [`Pending::append`]. A restart maps the base,
//! applies the log over it, and follows the last cursor either carries.
//!
//! ## Why the base is fixed-width and this machine's
//!
//! A 64-byte header and then nothing but [`System`] records, 56 bytes each
//! as this machine holds one. [`Checkpoint::read`] maps the file and hands
//! [`Tree::build`](crate::Tree::build) a `&[System]` pointing into the
//! mapping: mapped, not decoded, so the inputs cost no heap. Decoding *is*
//! the allocation.
//!
//! The cost is that the file is one machine's, native order and native
//! layout, and one this build cannot read is refused rather than misread:
//! the magic is a native `u64`, and the record width and a format version
//! are in the header. Refusal costs a rebuild and nothing else.

use crate::System;
use crate::format::layout::pending_path;
use chrono::NaiveDateTime;
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// What derived a checkpoint, and so what its cursor means.
///
/// A database pass writes the database clock; an event run has no clock the
/// database could read. A directory written by one and resumed by the other
/// would either skip every row in the gap between two clocks or re-read the
/// galaxy, so a mismatch forces a full rebuild — see the crate's `--index`
/// handoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum By {
    /// Live entries, from EDDN or a journal, applied as they arrived.
    Events,
    /// A read of the database, up to the clock the cursor carries.
    Database,
}

impl By {
    /// How the header spells it. A reader that meets a number it has no
    /// name for refuses the file rather than guessing at provenance.
    fn code(self) -> u32 {
        match self {
            By::Events => 0,
            By::Database => 1,
        }
    }

    /// The header's spelling read back, or [`None`] for a number this build
    /// has no name for.
    fn of_code(code: u32) -> Option<By> {
        match code {
            0 => Some(By::Events),
            1 => Some(By::Database),
            _ => None,
        }
    }
}

/// Bytes one system occupies in the base and in the log alike.
const RECORD: usize = std::mem::size_of::<System>();

/// Bytes of header ahead of the base's first record. A multiple of eight,
/// so the records behind it are aligned in the mapping.
const HEADER: usize = 64;

/// What a base file says it is, read as a native `u64` so that a file written
/// by a machine of the other byte order fails to be one.
const MAGIC: u64 = u64::from_ne_bytes(*b"GALOSCKP");

/// The format the records and the header are in. A base that says anything
/// else is refused.
const VERSION: u32 = 2;

/// What stands in a header or a frame for "no cursor": an event run has no
/// database clock to record, and a publish that could not read one leaves
/// the last cursor standing rather than replacing it with a guess.
const NO_CURSOR: i64 = i64::MIN;

/// Bytes ahead of a log frame's records: how many of them, and the cursor
/// that holds once they are applied.
const FRAME: usize = 16;

/// The share of the base the log may reach before a publish folds it in.
///
/// A sixteenth, which is what bounds the write amplification: the base is
/// rewritten once per sixteenth of a galaxy published.
const FOLD_AT: u64 = 16;

/// The smallest log worth folding in. A sixteenth of a small base is a
/// compaction every few publishes for no gain.
const FOLD_FLOOR: u64 = 4 << 20;

/// The largest log worth carrying. A restart applies every record in the
/// log one at a time, and this bounds that replay however large the galaxy
/// gets.
const FOLD_CEILING: u64 = 256 << 20;

/// A resume point as a reader has it: what derived it, the cursor it is
/// current as of, the base and the deltas published since.
///
/// The two halves are handed over separately rather than merged: a tree is
/// built from the base in one batch and the deltas applied over it with
/// [`Tree::apply`](crate::Tree::apply). Merged here it would be a `HashMap`
/// of the galaxy.
pub struct Checkpoint {
    /// The database time the inputs were read at, in UTC. Opaque here; the
    /// builder reads the changes since it. [`None`] for an event run with no
    /// database behind it, there being no clock to ask and nothing to resume
    /// against.
    pub cursor: Option<NaiveDateTime>,
    /// Which derivation wrote this, which is what makes the cursor readable.
    pub by: By,
    /// The base file, mapped. A resume point in the old encoding is
    /// rewritten in this one as it is read, so the base is a file of
    /// records by the time anything holds a `Checkpoint`.
    map: Mmap,
    /// The header's count, checked against the file's length when it was
    /// read.
    count: usize,
    deltas: Vec<System>,
}

/// Counts rather than contents: a resume point's contents are a galaxy.
impl std::fmt::Debug for Checkpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checkpoint")
            .field("cursor", &self.cursor)
            .field("by", &self.by)
            .field("base", &self.base().len())
            .field("deltas", &self.deltas.len())
            .finish()
    }
}

impl Checkpoint {
    /// Read a resume point back: the base mapped, the log replayed behind
    /// it. A missing or unreadable file is an error the caller turns into a
    /// full build.
    ///
    /// The cursor answered is the last one anything carries — the newest
    /// frame in the log that holds one, or the base's where the log holds
    /// none — since a frame is appended after the publish it describes.
    ///
    /// A file written before this format is MessagePack and is decoded
    /// whole, as is the log beside it in the framing of its own day; the
    /// read upgrades both in place, and the first compaction writes them
    /// out in this format.
    pub fn read(path: &Path) -> io::Result<Checkpoint> {
        let file = File::open(path)?;
        // SAFETY: the base is only ever replaced by a rename, so the inode
        // this maps is never written again and the mapping cannot be torn
        // under the reader. Truncation in place is the failure mode a map
        // has and nothing here truncates.
        let map = unsafe { Mmap::map(&file)? };

        if map.len() < HEADER || read_u64(&map, 0) != MAGIC {
            return Checkpoint::legacy(path, &map);
        }

        let version = read_u32(&map, 8);
        if version != VERSION {
            return Err(invalid(format!(
                "a resume point of format {version}, and this build reads \
                 {VERSION}"
            )));
        }
        let record = read_u32(&map, 12) as usize;
        if record != RECORD {
            return Err(invalid(format!(
                "a resume point of {record}-byte records, and this build's \
                 are {RECORD}"
            )));
        }
        let by = By::of_code(read_u32(&map, 16)).ok_or_else(|| {
            invalid(
                "a resume point derived by something this build has no \
                     name for",
            )
        })?;
        let count = read_u64(&map, 32) as usize;
        let held = map.len() - HEADER;
        if held != count * RECORD {
            return Err(invalid(format!(
                "a resume point that says {count} systems and holds {held} \
                 bytes of them"
            )));
        }
        if !aligned(&map[HEADER..]) {
            return Err(invalid("a mapping the records are not aligned in"));
        }

        let (deltas, framed) = read_frames(path);
        Ok(Checkpoint {
            cursor: framed.or_else(|| stamped(read_i64(&map, 24))),
            by,
            map,
            count,
            deltas,
        })
    }

    /// Everything the base holds, in the order it was compacted in.
    pub fn base(&self) -> &[System] {
        // SAFETY: `read` is the only thing that builds a `Checkpoint`, and
        // it checks all four of what this needs before it does: the
        // header's magic (so the file is this machine's and this format's),
        // its record width against `RECORD`, its count against the file's
        // length, and the alignment of the mapping past the header.
        // `System` is `repr(C)` with no padding and every bit pattern of
        // its `u64`, `f64` and `u32` fields is a valid value of that field,
        // so any 56 aligned bytes are one.
        unsafe {
            std::slice::from_raw_parts(
                self.map.as_ptr().add(HEADER).cast::<System>(),
                self.count,
            )
        }
    }

    /// What the log holds, in the order it was published in: later wins,
    /// which is what applying them over a tree built from [`base`](Self::base)
    /// does.
    pub fn deltas(&self) -> &[System] {
        &self.deltas
    }

    /// Fold everything into a new base and drop the log.
    ///
    /// [`Compaction`] with the whole of it already in hand. Answers how many
    /// records it wrote.
    pub fn compact(
        path: &Path,
        cursor: Option<NaiveDateTime>,
        by: By,
        systems: impl IntoIterator<Item = System>,
    ) -> io::Result<u64> {
        let mut writing = Compaction::begin(path)?;
        for system in systems {
            writing.push(system)?;
        }
        writing.finish(cursor, by)
    }

    /// A resume point from before the fixed-width base: upgraded in place,
    /// then read as one of the current ones.
    ///
    /// Two shapes are decoded, newest first: the three-field one, and the
    /// two-field one from before the provenance existed. The older reads as
    /// [`By::Database`], a database pass having been the only thing that
    /// ever wrote a cursor anything read.
    ///
    /// Then it *writes*: the old form is replaced with the new one and the
    /// log beside it re-framed. It has to. A publish appends a current
    /// frame, and a log half in one framing and half in the other reads as
    /// the older one and stops at the join, silently dropping every system
    /// published since — the one failure this log exists to prevent. The
    /// reader is where the old form is already in hand, and the file is the
    /// builder's private business, so nothing else opens one.
    fn legacy(path: &Path, bytes: &[u8]) -> io::Result<Checkpoint> {
        #[derive(Deserialize)]
        struct Whole {
            cursor: Option<NaiveDateTime>,
            by: By,
            inputs: Vec<System>,
        }

        #[derive(Deserialize)]
        struct Old {
            cursor: NaiveDateTime,
            inputs: Vec<System>,
        }

        let (cursor, by, inputs) = if let Ok(it) =
            rmp_serde::from_slice::<Whole>(bytes)
        {
            (it.cursor, it.by, it.inputs)
        } else {
            let it: Old = rmp_serde::from_slice(bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            (Some(it.cursor), By::Database, it.inputs)
        };

        let deltas = read_legacy_frames(path);
        Checkpoint::compact(path, cursor, by, inputs)?;
        if !deltas.is_empty() {
            Pending::append(path, None, &deltas)?;
        }
        Checkpoint::read(path)
    }
}

/// A base being written, one record at a time.
///
/// What a cold build spills into. The systems come off a database cursor a
/// row at a time and go straight here, and the tree is then built from the
/// mapping of what this wrote, so the galaxy is never a `Vec` at all.
///
/// Written to a sibling temp file and renamed over the base at
/// [`finish`](Self::finish), so an interrupted build leaves the resume point
/// that stood before it whole. [`abandon`](Self::abandon) takes the temp
/// file with it; a `Compaction` merely dropped leaves it behind and nothing
/// else, and the next one truncates it.
pub(crate) struct Compaction {
    path: PathBuf,
    tmp: PathBuf,
    out: BufWriter<File>,
    count: u64,
}

impl Compaction {
    /// Open a new base beside `path`, with room for the header the count
    /// goes in once it is known.
    pub fn begin(path: &Path) -> io::Result<Compaction> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let tmp = path.with_extension("tmp");
        let mut out = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
        out.write_all(&[0u8; HEADER])?;
        Ok(Compaction { path: path.to_owned(), tmp, out, count: 0 })
    }

    /// One more system.
    pub fn push(&mut self, system: System) -> io::Result<()> {
        self.out.write_all(as_bytes(std::slice::from_ref(&system)))?;
        self.count += 1;
        Ok(())
    }

    /// Stamp the header, put the file in place and drop the log it
    /// supersedes. Answers how many records it wrote.
    ///
    /// The log goes *after* the rename and not before: a crash in between
    /// replays a log the base already holds, which is a handful of
    /// idempotent upserts, where the other order would lose the publishes
    /// the log was the only record of.
    pub fn finish(
        self,
        cursor: Option<NaiveDateTime>,
        by: By,
    ) -> io::Result<u64> {
        let Compaction { path, tmp, out, count } = self;
        let mut file = out.into_inner().map_err(io::Error::other)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header(by, cursor, count))?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)?;
        Pending::clear(&path)?;
        Ok(count)
    }

    /// Drop a base that will not be finished, and its temp file with it.
    ///
    /// For a build asked to stop part way: the temp file is the galaxy's
    /// size and nothing will read it, since the resume point it was to
    /// become is the one thing that says a directory can be followed.
    pub fn abandon(self) -> io::Result<()> {
        let Compaction { tmp, out, .. } = self;
        drop(out);
        match std::fs::remove_file(&tmp) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }
}

/// What has been published since the last compaction.
///
/// A frame per publish: how many systems it moved, the cursor that holds
/// once they are applied, and the systems themselves at full precision. It
/// costs what moved where a base costs the galaxy.
///
/// A frame's cursor is [`None`] where the publish had none to record, and
/// the cursor a restart follows is the newest one that is not, so a log of
/// cursorless frames leaves the base's cursor standing.
///
/// A kill mid-append leaves a torn frame, which the replay stops at: that
/// publish either never finished or is in the directory and will be
/// published again from the cursor behind it.
pub struct Pending;

impl Pending {
    /// Add what a publish wrote, and answer whether the base should now be
    /// compacted.
    ///
    /// The answer is two `stat` calls against [`FOLD_AT`], clamped by
    /// [`FOLD_FLOOR`] and [`FOLD_CEILING`], and it is `true` whenever there
    /// is no base at all: the first publish into a fresh directory writes
    /// one rather than logging against nothing.
    ///
    /// An empty `systems` is not nothing: it is how a pass that published
    /// through earlier frames records the cursor those frames are current
    /// as of, in sixteen bytes.
    pub fn append(
        checkpoint: &Path,
        cursor: Option<NaiveDateTime>,
        systems: &[System],
    ) -> io::Result<bool> {
        let path = pending_path(checkpoint);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut file =
            OpenOptions::new().create(true).append(true).open(&path)?;
        let mut frame = [0u8; FRAME];
        frame[0..8].copy_from_slice(&(systems.len() as u64).to_ne_bytes());
        frame[8..16].copy_from_slice(&stamp(cursor).to_ne_bytes());
        file.write_all(&frame)?;
        file.write_all(as_bytes(systems))?;
        file.flush()?;

        let base = std::fs::metadata(checkpoint).map_or(0, |it| it.len());
        let log = file.metadata()?.len();
        Ok(base == 0 || log > (base / FOLD_AT).clamp(FOLD_FLOOR, FOLD_CEILING))
    }

    /// Drop the log, the base beside it now holding what it held.
    pub fn clear(checkpoint: &Path) -> io::Result<()> {
        match std::fs::remove_file(pending_path(checkpoint)) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            it => it,
        }
    }
}

/// The base's header: what it is, what wrote it, when, and how much.
fn header(by: By, cursor: Option<NaiveDateTime>, count: u64) -> [u8; HEADER] {
    let mut header = [0u8; HEADER];
    header[0..8].copy_from_slice(&MAGIC.to_ne_bytes());
    header[8..12].copy_from_slice(&VERSION.to_ne_bytes());
    header[12..16].copy_from_slice(&(RECORD as u32).to_ne_bytes());
    header[16..20].copy_from_slice(&by.code().to_ne_bytes());
    header[24..32].copy_from_slice(&stamp(cursor).to_ne_bytes());
    header[32..40].copy_from_slice(&count.to_ne_bytes());
    header
}

/// A cursor as the format spells it: microseconds, or [`NO_CURSOR`].
fn stamp(cursor: Option<NaiveDateTime>) -> i64 {
    cursor
        .and_then(|at| at.and_utc().timestamp_micros().into())
        .unwrap_or(NO_CURSOR)
}

/// The format's spelling read back.
fn stamped(micros: i64) -> Option<NaiveDateTime> {
    if micros == NO_CURSOR {
        return None;
    }
    chrono::DateTime::from_timestamp_micros(micros).map(|at| at.naive_utc())
}

/// Every frame of the log, in the order they were appended, and the newest
/// cursor any of them carries.
///
/// A missing log is nothing to replay. A frame that runs past the end of the
/// file is the torn tail of a kill and ends the replay there.
fn read_frames(checkpoint: &Path) -> (Vec<System>, Option<NaiveDateTime>) {
    let Ok(bytes) = std::fs::read(pending_path(checkpoint)) else {
        return (Vec::new(), None);
    };
    let mut replayed = Vec::new();
    let mut cursor = None;
    let mut at = 0;
    while at + FRAME <= bytes.len() {
        let count = read_u64(&bytes, at) as usize;
        let stamped_at = stamped(read_i64(&bytes, at + 8));
        let Some(end) = count
            .checked_mul(RECORD)
            .and_then(|held| (at + FRAME).checked_add(held))
        else {
            break;
        };
        if end > bytes.len() {
            break;
        }
        for record in bytes[at + FRAME..end].chunks_exact(RECORD) {
            // SAFETY: 56 initialised bytes read unaligned as the `repr(C)`
            // record they were written from, every bit pattern of whose
            // fields is a valid value of that field.
            replayed.push(unsafe {
                record.as_ptr().cast::<System>().read_unaligned()
            });
        }
        cursor = stamped_at.or(cursor);
        at = end;
    }
    (replayed, cursor)
}

/// The log as it was framed before the fixed-width format: a `u32` length
/// ahead of a MessagePack batch, and no cursor anywhere in it. Read once,
/// by the upgrade, and written back in the current framing.
fn read_legacy_frames(checkpoint: &Path) -> Vec<System> {
    let Ok(bytes) = std::fs::read(pending_path(checkpoint)) else {
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

/// The bytes of `systems`, for a write.
fn as_bytes(systems: &[System]) -> &[u8] {
    // SAFETY: `System` is `repr(C)` with no padding in it — asserted where it
    // is declared — so every byte of the slice is an initialised byte of a
    // field, and a `u8` has no alignment to violate.
    unsafe {
        std::slice::from_raw_parts(
            systems.as_ptr().cast::<u8>(),
            std::mem::size_of_val(systems),
        )
    }
}

/// Whether records may be read where this mapping's do.
fn aligned(bytes: &[u8]) -> bool {
    bytes.as_ptr().align_offset(std::mem::align_of::<System>()) == 0
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_ne_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

fn read_i64(bytes: &[u8], at: usize) -> i64 {
    i64::from_ne_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

/// A file this build will not read, said in the one place that says it.
fn invalid(
    what: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn scratch(name: &str) -> std::path::PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("galos-ckpt-{name}-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    fn system(id: u64) -> System {
        System {
            id64: id,
            position: [id as f64, -(id as f64), 12.5],
            absolute_magnitude: 4.83 - id as f64,
            temperature: 3000.0 + id as f64,
            age_bucket: (id % 8) as u32,
            updated_at: 1_700_000_000 + id as u32,
            kind: crate::core::record::StarKind::G,
        }
    }

    fn at(seconds: i64) -> Option<chrono::NaiveDateTime> {
        Some(chrono::DateTime::from_timestamp(seconds, 0).unwrap().naive_utc())
    }

    /// A base written and read back is the same base: cursor, what wrote it,
    /// and every input intact through the mapping.
    #[test]
    fn round_trips_through_disk() {
        let dir = scratch("round-trip");
        let path = dir.join("checkpoint");

        let inputs: Vec<System> = (0..1000).map(system).collect();
        let wrote = Checkpoint::compact(
            &path,
            at(1_700_000_000),
            By::Database,
            inputs.iter().copied(),
        )
        .unwrap();
        assert_eq!(wrote, 1000);

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read.cursor, at(1_700_000_000));
        assert_eq!(read.by, By::Database);
        assert_eq!(read.base(), inputs.as_slice());
        assert!(read.deltas().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An event run with no database behind it keeps its cursorless shape
    /// through the round trip.
    ///
    /// There was no clock to ask, so there is no window to catch up from
    /// and a reader must build the directory afresh.
    #[test]
    fn an_event_run_has_no_cursor() {
        let dir = scratch("eventful");
        let path = dir.join("checkpoint");

        let inputs: Vec<System> = (0..10).map(system).collect();
        Checkpoint::compact(&path, None, By::Events, inputs.iter().copied())
            .unwrap();

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read.cursor, None);
        assert_eq!(read.by, By::Events);
        assert_eq!(read.base(), inputs.as_slice());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a publish appends is what a restart reads, and the cursor it
    /// follows from is the newest frame's rather than the base's: the base
    /// is old and the log is what has happened since.
    #[test]
    fn the_log_carries_the_publishes_and_the_cursor() {
        let dir = scratch("logged");
        let path = dir.join("checkpoint");

        let base: Vec<System> = (0..10).map(system).collect();
        Checkpoint::compact(&path, at(100), By::Database, base.iter().copied())
            .unwrap();

        Pending::append(&path, None, &[system(10), system(11)]).unwrap();
        Pending::append(&path, at(200), &[system(12)]).unwrap();

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read.base(), base.as_slice());
        assert_eq!(read.deltas(), &[system(10), system(11), system(12)]);
        assert_eq!(read.cursor, at(200), "the base's cursor was followed");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A frame with nothing in it moves the cursor and nothing else, which is
    /// how a pass records where it got to without rewriting a galaxy.
    #[test]
    fn an_empty_frame_is_a_cursor() {
        let dir = scratch("cursor-only");
        let path = dir.join("checkpoint");

        Checkpoint::compact(&path, at(100), By::Database, [system(1)]).unwrap();
        Pending::append(&path, at(500), &[]).unwrap();

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read.cursor, at(500));
        assert!(read.deltas().is_empty());
        assert_eq!(read.base().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A torn frame ends the replay where it began, and everything whole
    /// ahead of it still stands. The publish that frame described either
    /// never landed or will be read again from the frame before it.
    #[test]
    fn a_torn_tail_stops_the_replay() {
        let dir = scratch("torn");
        let path = dir.join("checkpoint");

        Checkpoint::compact(&path, at(100), By::Database, [system(1)]).unwrap();
        Pending::append(&path, at(200), &[system(2)]).unwrap();
        Pending::append(&path, at(300), &[system(3), system(4)]).unwrap();

        // Half of the last frame's records, as a kill between two writes
        // leaves them.
        let log = pending_path(&path);
        let bytes = std::fs::read(&log).unwrap();
        std::fs::write(&log, &bytes[..bytes.len() - RECORD - 1]).unwrap();

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read.deltas(), &[system(2)]);
        assert_eq!(read.cursor, at(200));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A compaction supersedes the log and drops it, so nothing is replayed
    /// twice over a base that already holds it.
    #[test]
    fn compacting_drops_the_log() {
        let dir = scratch("folded");
        let path = dir.join("checkpoint");

        Checkpoint::compact(&path, at(100), By::Database, [system(1)]).unwrap();
        Pending::append(&path, at(200), &[system(2)]).unwrap();
        assert!(pending_path(&path).exists());

        Checkpoint::compact(
            &path,
            at(200),
            By::Database,
            [system(1), system(2)],
        )
        .unwrap();
        assert!(!pending_path(&path).exists(), "the log outlived the base");

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read.base().len(), 2);
        assert!(read.deltas().is_empty());
        assert_eq!(read.cursor, at(200));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The first publish into a directory with no base asks for one, and a
    /// log that is still small against its base does not.
    #[test]
    fn the_fold_waits_for_the_log_to_be_worth_it() {
        let dir = scratch("fold");
        let path = dir.join("checkpoint");

        assert!(
            Pending::append(&path, None, &[system(1)]).unwrap(),
            "a log with no base to extend was left standing",
        );

        Checkpoint::compact(&path, at(1), By::Database, [system(1)]).unwrap();
        assert!(
            !Pending::append(&path, at(2), &[system(2)]).unwrap(),
            "one system asked for a galaxy to be rewritten",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A resume point from before the fixed-width format still reads, and
    /// so does the log beside it in the framing of its own day.
    #[test]
    fn a_checkpoint_from_before_the_format_still_reads() {
        #[derive(Serialize)]
        struct Whole {
            cursor: Option<chrono::NaiveDateTime>,
            by: By,
            inputs: Vec<System>,
        }

        let dir = scratch("legacy");
        let path = dir.join("checkpoint");

        let inputs: Vec<System> = (1..40).map(system).collect();
        let old = Whole {
            cursor: at(1_700_000_000),
            by: By::Database,
            inputs: inputs.clone(),
        };
        std::fs::write(&path, rmp_serde::to_vec(&old).unwrap()).unwrap();

        let batch = vec![system(100), system(101)];
        let encoded = rmp_serde::to_vec(&batch).unwrap();
        let mut log = (encoded.len() as u32).to_le_bytes().to_vec();
        log.extend_from_slice(&encoded);
        std::fs::write(pending_path(&path), log).unwrap();

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read.cursor, at(1_700_000_000));
        assert_eq!(read.by, By::Database);
        assert_eq!(read.base(), inputs.as_slice());
        assert_eq!(read.deltas(), batch.as_slice());

        // And the read upgraded both files, which is why it writes: a log
        // half in each framing reads as the older one and stops at the
        // join.
        let upgraded = std::fs::read(&path).unwrap();
        assert_eq!(
            read_u64(&upgraded, 0),
            MAGIC,
            "the old form was left on disk for the next publish to append to",
        );
        Pending::append(&path, at(1_700_000_100), &[system(102)]).unwrap();
        let again = Checkpoint::read(&path).unwrap();
        assert_eq!(
            again.deltas(),
            &[system(100), system(101), system(102)],
            "what the old log held was dropped by a publish behind it",
        );
        assert_eq!(again.cursor, at(1_700_000_100));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A resume point from before the provenance existed reads as the
    /// database derivation, keeping the cursor it carried: a database pass
    /// was the only thing that ever wrote a cursor anything read.
    #[test]
    fn an_old_checkpoint_reads_as_the_database() {
        #[derive(Serialize)]
        struct Old {
            cursor: chrono::NaiveDateTime,
            inputs: Vec<System>,
        }

        let dir = scratch("old-form");
        let path = dir.join("checkpoint");

        let inputs: Vec<System> = (1..40).map(system).collect();
        let old =
            Old { cursor: at(1_700_000_000).unwrap(), inputs: inputs.clone() };
        std::fs::write(&path, rmp_serde::to_vec(&old).unwrap()).unwrap();

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read.cursor, at(1_700_000_000));
        assert_eq!(read.by, By::Database);
        assert_eq!(read.base(), inputs.as_slice());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A base written by a build whose record is a different width is
    /// refused, not read as a galaxy of nonsense.
    #[test]
    fn a_foreign_record_width_is_refused() {
        let dir = scratch("width");
        let path = dir.join("checkpoint");

        Checkpoint::compact(&path, at(1), By::Database, [system(1)]).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[12..16].copy_from_slice(&48u32.to_ne_bytes());
        std::fs::write(&path, bytes).unwrap();

        let err = Checkpoint::read(&path).expect_err("a 48-byte record read");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The compaction replaces a base in place rather than failing on an
    /// existing file, and leaves no temp file behind.
    #[test]
    fn overwrites_in_place() {
        let dir = scratch("overwrite");
        let path = dir.join("checkpoint");

        Checkpoint::compact(&path, at(1), By::Database, [system(1)]).unwrap();
        Checkpoint::compact(&path, at(2), By::Database, [system(1), system(2)])
            .unwrap();

        let read = Checkpoint::read(&path).unwrap();
        assert_eq!(read.cursor, at(2));
        assert_eq!(read.base().len(), 2);
        assert!(!path.with_extension("tmp").exists(), "temp file left behind");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
