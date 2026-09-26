//! The delta log: what the feed has said since the base was written.
//!
//! Append-only, so a publish costs the arrivals' bytes and a client reading
//! the tail costs the same. A withdrawal is a row in it like a naming,
//! which is what lets a replay reach the table the writer has.

use super::table::Table;
use crate::format::layout::{names_delta_path, names_dir};
use crate::records::NameEntry;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// What the delta says about one address.
#[derive(Clone, Debug, PartialEq)]
pub enum DeltaAnswer<'a> {
    /// Named, and this is the row.
    Named(&'a NameEntry),
    /// Withdrawn: the base's row, if it has one, does not answer.
    Gone,
}

/// One row of the delta log.
///
/// An enum rather than a nullable row so that withdrawal is a *statement*
/// the log carries in order beside the namings, which is what lets a client
/// replay the tail and reach the same table the writer has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeltaRow {
    /// This system is named so, and sits there.
    Named(NameEntry),
    /// This system no longer has a place, so the table no longer names it.
    Gone(i64),
}

/// How many addresses the log may mention before folding it into the base
/// is worth the rewrite.
///
/// The trade is a publish against an open. Every client reads the whole log
/// at startup and holds it, so the log is the one part of the table that is
/// still resident — at this many addresses about 60 MB held and 16 MB on
/// disk, against 5.8 GB of base that is neither. A fold's cost is the base
/// rewrite and so barely depends on the log's length, which argues for a
/// long log; what argues back is that 60 MB.
///
/// The live feed names ~60 k systems a week that the table did not already
/// name, so this is reached about monthly. An unchanged report appends
/// nothing at all — [`Names::name`] compares against the base row first —
/// which is why the log tracks the systems that are new, not the messages
/// that arrive.
pub(super) const FOLD_ROWS: usize = 256 * 1024;

/// How many bytes the log may reach before the same thing is true.
///
/// [`FOLD_ROWS`] counts addresses and the file grows by rows, and the two
/// are not the same thing: an address the log already mentions that changes
/// again appends a second row and leaves the count where it was. Renames
/// and position corrections are near-never, so this is not the trigger that
/// fires in practice — but without it a feed that never ends has a way to
/// grow a file that is never folded, and "near-never" is not a bound.
pub(super) const FOLD_BYTES: u64 = 64 * 1024 * 1024;

/// What the feed has said since the base was written.
///
/// Small, resident, and read whole at startup. Held as a map for the
/// answering and as a byte offset for the reading: a client that has
/// consumed the first `read` bytes of the log asks only for what is past
/// them, so a refresh costs the arrivals and not the log.
#[derive(Clone, Debug, Default)]
pub struct Delta {
    /// What each address the log mentions has come to.
    pub(super) said: HashMap<i64, DeltaRow>,
    /// Addresses whose last word was [`DeltaRow::Gone`], kept apart so
    /// [`entries`](Self::entries) can walk the named without testing each.
    pub(super) gone: HashSet<i64>,
    /// How many bytes of the log this is, which is where the next read
    /// starts.
    pub(super) read: u64,
    /// Rows taken since the last [`append`](Self::append), in the order
    /// they were taken.
    pub(super) pending: Vec<DeltaRow>,
}

impl Delta {
    /// The whole of `dir`'s log.
    ///
    /// A log whose tail is a half-written row is truncated to its last
    /// whole one: a row is appended in one write and a writer killed
    /// part way through one never marked it, so the bytes past the last
    /// whole row stand for nothing and must not be appended after.
    pub fn read(dir: &Path) -> io::Result<Delta> {
        let mut delta = Delta::default();
        delta.take_from(dir, 0)?;
        let path = names_delta_path(dir);
        if let Ok(found) = std::fs::metadata(&path)
            && found.len() > delta.read
        {
            let file = File::options().write(true).open(&path)?;
            file.set_len(delta.read)?;
        }
        Ok(delta)
    }

    /// The log's rows past `from`, which is what a client that already
    /// holds the head of it asks for.
    pub fn since(dir: &Path, from: u64) -> io::Result<Delta> {
        let mut delta = Delta::default();
        delta.take_from(dir, from)?;
        Ok(delta)
    }

    /// The log these rows make, with no file behind it.
    ///
    /// A table that is all overlay and no base: what a caller with rows
    /// already in hand builds, and the one road to a [`Names`](super::Names) that does
    /// not touch a directory.
    pub fn of(entries: impl IntoIterator<Item = NameEntry>) -> Delta {
        let mut delta = Delta::default();
        for entry in entries {
            delta.apply(DeltaRow::Named(entry));
        }
        delta.pending.clear();
        delta
    }

    /// Fold `tail`'s words in over what this already says.
    ///
    /// How a client applies what it read past its offset. A word is kept
    /// per address, so the tail's word on an address is the later one
    /// whatever order they are folded in, and the offset moves to the
    /// tail's.
    pub fn absorb(&mut self, tail: Delta) {
        for said in tail.said.into_values() {
            self.apply(said);
        }
        self.pending.clear();
        self.read = self.read.max(tail.read);
    }

    /// How far into the log this has read.
    pub fn read_to(&self) -> u64 {
        self.read
    }

    /// What the log's last word on `address` is, if it has one.
    pub fn said(&self, address: i64) -> Option<DeltaAnswer<'_>> {
        match self.said.get(&address)? {
            DeltaRow::Named(entry) => Some(DeltaAnswer::Named(entry)),
            DeltaRow::Gone(_) => Some(DeltaAnswer::Gone),
        }
    }

    /// Every row the log still names, in no order a caller may rely on.
    pub fn entries(&self) -> impl Iterator<Item = &NameEntry> + '_ {
        self.said.values().filter_map(|said| match said {
            DeltaRow::Named(entry) => Some(entry),
            DeltaRow::Gone(_) => None,
        })
    }

    /// Every address the log has withdrawn.
    pub fn withdrawn(&self) -> impl Iterator<Item = i64> + '_ {
        self.gone.iter().copied()
    }

    /// How many addresses the log mentions at all.
    pub fn len(&self) -> usize {
        self.said.len()
    }

    /// Whether it mentions none.
    pub fn is_empty(&self) -> bool {
        self.said.is_empty()
    }

    /// Whether the log has grown enough that folding it into the base is
    /// worth the rewrite: either threshold, since one bounds what a client
    /// holds ([`FOLD_ROWS`]) and the other bounds the file itself
    /// ([`FOLD_BYTES`]).
    pub fn worth_folding(&self) -> bool {
        self.said.len() >= FOLD_ROWS || self.read >= FOLD_BYTES
    }

    /// What this adds to `base`'s count, which may be negative.
    ///
    /// So that a count of the two together is neither short nor doubled: a
    /// row the log names that the base also holds is one system, not two,
    /// and a tombstone over a base row takes one away that the base's own
    /// count still holds. Signed rather than clamped here, because a log
    /// that withdraws more than it adds has to be able to say so — clamping
    /// it at zero made a base of one row under one tombstone count as one.
    pub(super) fn net(&self, base: &Table) -> isize {
        let mut net = 0isize;
        for said in self.said.values() {
            match said {
                DeltaRow::Named(entry) => {
                    if base.index_of(entry.address).is_none() {
                        net += 1;
                    }
                }
                DeltaRow::Gone(address) => {
                    if base.index_of(*address).is_some() {
                        net -= 1;
                    }
                }
            }
        }
        net
    }

    /// The row named exactly `name`, if the log holds one.
    pub(super) fn named_exactly(&self, name: &str) -> Option<&NameEntry> {
        self.entries().find(|entry| entry.name == *name)
    }

    /// Take a naming, answering whether it changed what the log says.
    pub(super) fn named(&mut self, entry: NameEntry) -> bool {
        if let Some(DeltaRow::Named(held)) = self.said.get(&entry.address)
            && *held == entry
        {
            return false;
        }
        let said = DeltaRow::Named(entry);
        self.pending.push(said.clone());
        self.apply(said);
        true
    }

    /// Take a withdrawal, answering whether it changed what the log says.
    ///
    /// `in_base` is whether the base names the address: one it does not and
    /// the log has not named is not there to withdraw, and one the base
    /// names needs the tombstone written even where the log never mentioned
    /// it.
    pub(super) fn gone(&mut self, address: i64, in_base: bool) -> bool {
        match self.said.get(&address) {
            Some(DeltaRow::Gone(_)) => return false,
            None if !in_base => return false,
            _ => {}
        }
        let said = DeltaRow::Gone(address);
        self.pending.push(said.clone());
        self.apply(said);
        true
    }

    /// One row's effect on what the log says.
    pub(super) fn apply(&mut self, said: DeltaRow) {
        match &said {
            DeltaRow::Named(entry) => {
                self.gone.remove(&entry.address);
                self.said.insert(entry.address, said);
            }
            DeltaRow::Gone(address) => {
                let address = *address;
                self.gone.insert(address);
                self.said.insert(address, said);
            }
        }
    }

    /// Append what has been taken since the last append.
    ///
    /// Opened for append and written in one go, so a reader holding an
    /// offset into the log never sees a row twice and never sees one
    /// half. The offset this now stands at is the file's length, which is
    /// what the next client to read the tail is told.
    pub(super) fn append(&mut self, dir: &Path) -> io::Result<usize> {
        if self.pending.is_empty() {
            return Ok(0);
        }
        std::fs::create_dir_all(names_dir(dir))?;
        let path = names_delta_path(dir);
        let file = File::options().create(true).append(true).open(&path)?;
        let mut out = BufWriter::new(file);
        let mut bytes = 0u64;
        for said in &self.pending {
            let row = rmp_serde::to_vec(said)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            out.write_all(&(row.len() as u32).to_le_bytes())?;
            out.write_all(&row)?;
            bytes += row.len() as u64 + 4;
        }
        out.flush()?;
        let written = self.pending.len();
        self.pending.clear();
        self.read += bytes;
        Ok(written)
    }

    /// Read the log from `from` to its end into what this says.
    pub(super) fn take_from(
        &mut self,
        dir: &Path,
        from: u64,
    ) -> io::Result<()> {
        let path = names_delta_path(dir);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                self.read = from;
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        file.seek(SeekFrom::Start(from))?;
        let mut inner = io::BufReader::new(file);
        let mut head = [0u8; 4];
        let mut buf = Vec::new();
        let mut at = from;
        loop {
            match inner.read_exact(&mut head) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(err) => return Err(err),
            }
            let len = u32::from_le_bytes(head) as usize;
            buf.resize(len, 0);
            match inner.read_exact(&mut buf) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(err) => return Err(err),
            }
            let said: DeltaRow = rmp_serde::from_slice(&buf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            self.pending.push(said.clone());
            self.apply(said);
            at += len as u64 + 4;
        }
        // Read rather than taken: a client replaying the log has nothing to
        // append, and a writer resuming onto a directory has already
        // written every row it just read.
        self.pending.clear();
        self.read = at;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::names::Names;
    use crate::store::names::fixtures::{Scratch, entry, published};

    /// The delta answers over the base, a withdrawal hides a base row, and
    /// a report of what the base already says appends nothing.
    #[test]
    fn the_delta_is_the_later_word() {
        let dir = Scratch::new("delta");
        published(&dir.0, &[entry(1, "SOL", 1.0), entry(2, "ACRUX", 2.0)]);

        let mut names = Names::open(&dir.0).expect("the table opens");
        assert_eq!(names.len(), 2);

        // Already said: nothing changes and nothing is written.
        assert!(!names.name(entry(1, "SOL", 1.0)));
        assert_eq!(names.publish(&dir.0).expect("a publish"), 0);

        assert!(names.name(entry(1, "SOL RENAMED", 1.0)));
        assert!(names.name(entry(3, "NEW", 3.0)));
        assert!(names.unname(2));
        assert!(!names.unname(99));
        assert_eq!(names.publish(&dir.0).expect("a publish"), 3);

        assert_eq!(names.name_of(1).as_deref(), Some("SOL RENAMED"));
        assert_eq!(names.name_of(2), None);
        assert_eq!(names.name_of(3).as_deref(), Some("NEW"));
        assert_eq!(names.address_of("SOL"), None);
        assert_eq!(names.address_of("SOL RENAMED"), Some(1));
        assert_eq!(names.address_of("ACRUX"), None);
        assert_eq!(names.address_of("NEW"), Some(3));
        assert_eq!(names.len(), 2);

        // And the same table comes back off disk.
        let read = Names::open(&dir.0).expect("the table re-opens");
        assert_eq!(read.name_of(1).as_deref(), Some("SOL RENAMED"));
        assert_eq!(read.name_of(2), None);
        assert_eq!(read.name_of(3).as_deref(), Some("NEW"));
        assert_eq!(read.len(), 2);
    }

    /// A client that holds the head of the log reads only the tail.
    #[test]
    fn a_client_reads_only_the_tail_of_the_log() {
        let dir = Scratch::new("tail");
        published(&dir.0, &[entry(1, "SOL", 1.0)]);

        let mut names = Names::open(&dir.0).expect("the table opens");
        names.name(entry(2, "FIRST", 2.0));
        names.publish(&dir.0).expect("a publish");

        let held = Delta::read(&dir.0).expect("the log reads");
        assert_eq!(held.len(), 1);
        let at = held.read_to();
        assert!(at > 0);

        names.name(entry(3, "SECOND", 3.0));
        names.publish(&dir.0).expect("a second publish");

        let tail = Delta::since(&dir.0, at).expect("the tail reads");
        assert_eq!(tail.len(), 1);
        assert!(tail.entries().any(|it| it.name == *"SECOND"));
        assert!(tail.read_to() > at);
    }

    /// A log that withdraws every row of the base leaves a table that
    /// names nothing.
    ///
    /// The count is the base's plus what the log adds less what it takes,
    /// and the middle term can be negative. Clamping it before the
    /// addition made a base of one row under one tombstone count as one,
    /// which the map showed in its diagnostics.
    #[test]
    fn a_log_can_withdraw_the_whole_base() {
        let dir = Scratch::new("emptied");
        published(&dir.0, &[entry(1, "SOL", 1.0), entry(2, "ACRUX", 2.0)]);

        let mut names = Names::open(&dir.0).expect("the table opens");
        assert!(names.unname(1));
        assert_eq!(names.len(), 1);
        assert!(names.unname(2));
        assert_eq!(names.len(), 0);
        assert!(names.is_empty(), "a table whose every row was withdrawn");
        assert_eq!(names.len(), 0);
        assert!(names.addresses().next().is_none());
    }

    /// A log whose tail is half a row reads as the rows before it, and the
    /// half row is cut so the next append is not written after it.
    #[test]
    fn a_half_written_row_is_the_end_of_the_log() {
        let dir = Scratch::new("torn");
        published(&dir.0, &[entry(1, "SOL", 1.0)]);
        let mut names = Names::open(&dir.0).expect("the table opens");
        names.name(entry(2, "FIRST", 2.0));
        names.publish(&dir.0).expect("a publish");

        let path = names_delta_path(&dir.0);
        let whole = std::fs::metadata(&path).expect("the log").len();
        {
            let file =
                File::options().append(true).open(&path).expect("the log");
            let mut out = BufWriter::new(file);
            out.write_all(&[64, 0, 0, 0, 1, 2, 3]).expect("half a row");
            out.flush().expect("a flush");
        }

        let held = Delta::read(&dir.0).expect("the log reads");
        assert_eq!(held.len(), 1);
        assert_eq!(
            std::fs::metadata(&path).expect("the log").len(),
            whole,
            "the half row is cut"
        );

        // And an append after it lands where a reader will find it.
        let mut names = Names::open(&dir.0).expect("the table re-opens");
        names.name(entry(3, "SECOND", 3.0));
        names.publish(&dir.0).expect("an append");
        let read = Names::open(&dir.0).expect("the table re-opens again");
        assert_eq!(read.name_of(2).as_deref(), Some("FIRST"));
        assert_eq!(read.name_of(3).as_deref(), Some("SECOND"));
    }
}
