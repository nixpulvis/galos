//! The names table: mapped, sorted by address, and never resident.
//!
//! Every positioned system's name and place is one table, and every client
//! reaches all of it: a search matches any name, a route steps between any
//! two places, a label names whatever is on screen. At 200,071,629 systems
//! that is 5.6 GB of sections. It cannot be a `Vec`.
//!
//! It was one. The table was published as MessagePack chunks and read whole
//! into `Vec<NameEntry>` plus a `HashMap<i64, usize>` beside it, which
//! measured **47 GB** at 200 M — 48 bytes of struct, a heap block per name,
//! and twenty-four more bytes a system for the address index — on a machine
//! with 24 GiB. Packing it into arrays got that to 7.9 GB and 33 s, which is
//! the same shape of answer: still every byte in memory, still a decode of
//! the galaxy before anything draws.
//!
//! So the table is a **file the client maps and never decodes**. Opening it
//! is five `mmap` calls and six length checks; what a session touches is
//! what the kernel pages in, and what it does not touch costs nothing.
//!
//! ```text
//! names/
//!   head.bin      64 B      magic, version, generation, count, name bytes
//!   <gen>/
//!     addr.bin    N x 8     i64 addresses, strictly ascending
//!     byname.bin  N x 4     u32 rows, sorted by name bytes
//!     span.bin   (N+1) x 5  u40 offsets into text.bin; equal = derived
//!     text.bin    B         the name bytes nothing can derive
//!   delta.bin               what the feed has said since (append-only)
//! ```
//!
//! **`text.bin` holds the names nothing can work out.** A procedural name is a
//! function of the system's address ([`crate::core::procedural`]), so a row
//! whose name the arithmetic spells stores no bytes at all and is marked by a
//! span of no length — a stored name is never empty, so the marker costs
//! nothing either. Measured over the 200 M table, migrating it in place:
//! **`text.bin` 3.94 GB → 128 MB**, the whole directory 9.1 GB → 5.6 GB, and
//! reads got *faster* rather than slower, a name being arithmetic where it was
//! a page fault into four gigabytes: an address lookup 489 µs → 87 µs,
//! resolving a name 2.4 ms → 109 µs, both warm.
//!
//! What is left in it is what the arithmetic will not claim: the 151,463
//! names people gave, and the 5.1 M systems under Frontier's hand-authored
//! regions, whose boxels are numbered from the region's own origin.
//!
//! Five decisions carry it, and each one is answering a measurement.
//!
//! 1. **Structure of arrays, not records.** A lookup by address walks
//!    `addr.bin` alone: 8 bytes a step, ~28 steps, and it never faults a
//!    position or a name it is not going to answer with. The router wants
//!    every position and nothing else, and gets `&[[f32; 3]]` straight off
//!    the mapping. An array-of-records layout would fault all 29 bytes a
//!    row to read any one field of it.
//! 2. **Sorted by address**, so the address index is the addresses
//!    themselves and a lookup is [`binary_search`](slice::binary_search).
//!    That is the `HashMap<i64, usize>`, 4.8 GB at 200 M, deleted rather
//!    than shrunk.
//! 3. **Sorted by name too**, in `byname.bin`. A name is
//!    [`SystemName`](crate::SystemName), upper case by construction, so
//!    names compare and sort as bytes with no fold — which is the whole
//!    reason that type exists. Resolving a route endpoint was a scan of the
//!    galaxy, 11.3 s measured, four to six times per plot; it is now a
//!    binary search over a 4-byte-a-row permutation.
//! 4. **A generation, swapped by one rename.** A build reads the galaxy for
//!    as long as that takes and the table beneath it is served the whole
//!    time. Sections are written into `names/<gen+1>/` and become live when
//!    `head.bin` is renamed over — one atomic step, after which the old
//!    generation is removed. A reader that mapped the old one keeps reading
//!    it: the mapping outlives the directory entry. A reader opening during
//!    the swap sees the old table or the new one and never half of either.
//! 5. **An append-only delta for the feed.** The feed names a few dozen
//!    systems a second and the base cannot be rewritten for that. Changed
//!    rows are appended to `delta.bin`; a client remembers the byte offset
//!    it has read and takes only the tail. A publish costs the arrivals'
//!    bytes and a refresh costs the same bytes, which is why neither side
//!    cares how long the log is — until [`Delta::worth_folding`], where a
//!    compaction folds it into the base.
//!
//! The base is immutable, so the delta is where every change goes, including
//! withdrawal: [`DeltaRow::Gone`] is the tombstone that shadows a base row.

mod delta;
mod format;
mod search;
mod table;
mod write;

pub use delta::{Delta, DeltaAnswer, DeltaRow};
pub use search::MIN_PREFIX;
pub use table::Table;
pub use write::{Writer, compact, fold_chunks, version, writes};

use crate::records::NameEntry;
use format::placed;
use std::borrow::Cow;
use std::io;
use std::path::Path;
use std::sync::Arc;

/// The names table, base and delta, as a reader or a writer holds it.
///
/// Both halves are behind an [`Arc`] because both are shared and neither is
/// copied: a client hands the base to every task that draws and swaps only
/// the delta when the feed moves ([`absorb`](Self::absorb)), and a
/// writer holds the one reference there is and mutates the delta in place
/// for free.
///
/// The precedence is the one rule of the format and it lives here: the
/// delta answers first, and a [`DeltaRow::Gone`] in it hides a base row.
#[derive(Clone, Debug, Default)]
pub struct Names {
    pub(super) base: Arc<Table>,
    pub(super) delta: Arc<Delta>,
}

impl Names {
    /// The table `dir` publishes: the base mapped, the delta read.
    ///
    /// A directory that has published none is the empty table rather than
    /// an error — that is what a directory nothing has been built into is.
    pub fn open(dir: &Path) -> io::Result<Names> {
        Ok(Names {
            base: Arc::new(Table::open(dir)?),
            delta: Arc::new(Delta::read(dir)?),
        })
    }

    /// The table these two halves make, for a caller that read them itself.
    pub fn of(base: Table, delta: Delta) -> Names {
        Names { base: Arc::new(base), delta: Arc::new(delta) }
    }

    /// Read the text a search sweeps, so the first search does not.
    ///
    /// [`Table::warm`] says why, and what it deliberately leaves cold. A
    /// client calls this once, on whatever thread opened the table: it is
    /// a streaming read of the one section a query reads end to end, and
    /// paying it at the open is the difference between a first search of
    /// seconds and one of milliseconds.
    pub fn warm(&self) -> io::Result<()> {
        self.base.warm()
    }

    /// Fold a tail of the log in, which is what a client does when the feed
    /// has appended to it.
    ///
    /// The base is untouched and the log is copied on write, so a task
    /// holding a clone of this keeps reading the table it was handed.
    pub fn absorb(&mut self, tail: Delta) {
        Arc::make_mut(&mut self.delta).absorb(tail);
    }

    /// The mapped base, for a caller that wants it without the delta: the
    /// router's positions, a bulk walk, a count.
    pub fn base(&self) -> &Table {
        &self.base
    }

    /// What the feed has said since the base was written.
    pub fn delta(&self) -> &Delta {
        &self.delta
    }

    /// How many systems the table names.
    ///
    /// The base's count plus what the log adds and less what it withdraws,
    /// floored at zero — a log may take every row of a base away.
    pub fn len(&self) -> usize {
        (self.base.len() as isize + self.delta.net(&self.base)).max(0) as usize
    }

    /// Whether it names none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What `address` is named, borrowed wherever it is held.
    ///
    /// Owned only where the name was never stored: a procedural name is
    /// spelled from the address itself ([`crate::core::procedural`]), which is
    /// 97.4 % of a galaxy and the whole reason `text.bin` is small.
    ///
    /// Upper case either way, every name in the table being a
    /// [`SystemName`](crate::SystemName) and the arithmetic spelling upper
    /// case by construction.
    pub fn name_of(&self, address: i64) -> Option<Cow<'_, str>> {
        match self.delta.said(address) {
            Some(DeltaAnswer::Named(entry)) => Some(Cow::Borrowed(&entry.name)),
            Some(DeltaAnswer::Gone) => None,
            None => self.base.index_of(address).map(|at| self.base.name_at(at)),
        }
    }

    /// `address`'s whole row, which costs the name a copy.
    ///
    /// For a caller that holds what it is given — a selection, a search
    /// result, a route's ends. Everything that only reads goes through
    /// [`name_of`](Self::name_of).
    pub fn entry_of(&self, address: i64) -> Option<NameEntry> {
        match self.delta.said(address) {
            // Through `placed`, so a row the log answers for carries the
            // same place a row the base answers for does: the log holds
            // the position a report arrived with, and handing that out
            // here would make this field mean two things.
            Some(DeltaAnswer::Named(entry)) => Some(placed(entry.clone())),
            Some(DeltaAnswer::Gone) => None,
            None => {
                self.base.index_of(address).map(|at| self.base.entry_at(at))
            }
        }
    }

    /// Which system is named exactly `name`, if one is.
    ///
    /// `name` is expected upper case, as every name in the table is. A
    /// binary search of `byname.bin` and a scan of the delta, which is the
    /// four-to-six full scans of the galaxy a route plot used to pay
    /// replaced by `O(log N)` and a few dozen bytes.
    pub fn address_of(&self, name: &str) -> Option<i64> {
        if let Some(entry) = self.delta.named_exactly(name) {
            return Some(entry.address);
        }
        let at = self.base.row_named(name)?;
        let address = self.base.address_at(at);
        // A base row the feed has since withdrawn or renamed does not
        // answer: the delta is the later word on every address in it.
        match self.delta.said(address) {
            Some(DeltaAnswer::Named(entry)) if entry.name == *name => {
                Some(address)
            }
            Some(_) => None,
            None => Some(address),
        }
    }

    /// Whether any system is named exactly `name`.
    pub fn names_exactly(&self, name: &str) -> bool {
        self.address_of(name).is_some()
    }

    /// Every address the table names, in no order a caller may rely on.
    ///
    /// What the sink's agreement check walks at open, which is why it is an
    /// iterator over a mapping and not a collection.
    pub fn addresses(&self) -> impl Iterator<Item = i64> + '_ {
        self.base
            .addresses()
            .iter()
            .copied()
            .filter(|address| self.delta.said(*address).is_none())
            .chain(self.delta.entries().map(|entry| entry.address))
    }

    /// Take `entry`, answering whether it changed anything.
    ///
    /// The compare that makes a publish cheap: an address never moves, a
    /// position is corrected about never and a name changes about never, so
    /// a system reported again matches what the table already says and
    /// nothing is appended. The comparison is against one row of a mapping
    /// — which is what the 47 GB slot map used to be for.
    pub fn name(&mut self, entry: NameEntry) -> bool {
        if self.delta.said(entry.address).is_none() && self.base.holds(&entry) {
            return false;
        }
        Arc::make_mut(&mut self.delta).named(entry)
    }

    /// Withdraw `address`, answering whether it was there to withdraw.
    ///
    /// A base row is shadowed by a tombstone; a row only the delta had is
    /// dropped from it.
    pub fn unname(&mut self, address: i64) -> bool {
        let in_base = self.base.index_of(address).is_some();
        Arc::make_mut(&mut self.delta).gone(address, in_base)
    }

    /// Append what the delta has taken to `dir`'s log, answering how many
    /// rows were written.
    ///
    /// Only the rows appended since the last publish: the log is the
    /// format's unit of change, so a pass that named fifty systems writes
    /// fifty rows and not a table.
    pub fn publish(&mut self, dir: &Path) -> io::Result<usize> {
        Arc::make_mut(&mut self.delta).append(dir)
    }

    /// Whether the delta has grown enough to be worth folding into the base
    /// — see [`Delta::worth_folding`] and [`compact`].
    pub fn worth_compacting(&self) -> bool {
        self.delta.worth_folding()
    }
}

/// What every test of the table stands on.
#[cfg(test)]
mod fixtures {
    use super::*;
    use crate::core::name::SystemName;
    use std::path::PathBuf;

    /// A scratch directory removed with the test.
    pub(super) struct Scratch(pub(super) PathBuf);

    impl Scratch {
        pub(super) fn new(name: &str) -> Scratch {
            let at = std::env::temp_dir()
                .join(format!("galos-names-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&at);
            std::fs::create_dir_all(&at).expect("a scratch directory");
            Scratch(at)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(super) fn entry(address: i64, name: &str, x: f32) -> NameEntry {
        NameEntry {
            address,
            name: SystemName::new(name),
            position: [x, 0.0, 0.0],
        }
    }

    pub(super) fn published(dir: &Path, entries: &[NameEntry]) -> usize {
        let mut writer = Writer::writing(dir).expect("a writer");
        for entry in entries {
            writer.push(entry.clone()).expect("a push");
        }
        writer.finish().expect("a finish")
    }
}
