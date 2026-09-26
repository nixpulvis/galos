//! One generation of the table, mapped: the base every client reads.
//!
//! Opening it is five `mmap` calls and six length checks, and nothing is
//! decoded: a row is read out of the sections when it is asked for.

use super::format::{
    HEAD, MAGIC, Mapped, PAGE, READS, ROW, Text, boxel_middle, map, refused,
};
use crate::core::name::SystemName;
use crate::format::layout::{BYNAME_FILE, generation_dir, names_head_path};
use crate::records::NameEntry;
use std::borrow::Cow;
use std::io;
use std::path::Path;

/// The base: the sorted, mapped, immutable half of the table.
///
/// Five mappings and two numbers. Nothing in it is decoded, allocated or
/// copied at open; a slice of it is a slice of the file.
#[derive(Debug, Default)]
pub struct Table {
    pub(super) held: Option<Mapped>,
}

impl Table {
    /// Map the base `dir` publishes, or the empty table where it publishes
    /// none.
    ///
    /// Refused, rather than read wrongly: a magic or version that is not
    /// this format's, a section whose length disagrees with the count, a
    /// `span.bin` that does not end at `text.bin`'s length. Those are the
    /// checks that cost nothing — one `metadata` a section. What is *not*
    /// checked is anything proportional to the table: that the addresses
    /// ascend, that `byname.bin` is a permutation in name order. A galaxy's
    /// worth of that at every open is the read this format exists to
    /// delete; it belongs to whatever wrote the file, and
    /// [`Table::audit`] is it.
    pub fn open(dir: &Path) -> io::Result<Table> {
        let head = match std::fs::read(names_head_path(dir)) {
            Ok(head) => head,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Table::default());
            }
            Err(err) => return Err(err),
        };
        if head.len() < HEAD
            || u64::from_ne_bytes(head[0..8].try_into().unwrap()) != MAGIC
        {
            return Err(refused("not a names table"));
        }
        let version = u16::from_le_bytes(head[8..10].try_into().unwrap());
        if !READS.contains(&version) {
            return Err(refused(&format!(
                "a names table of version {version}, not one of {READS:?}"
            )));
        }
        let generation = u64::from_le_bytes(head[16..24].try_into().unwrap());
        let count =
            u64::from_le_bytes(head[24..32].try_into().unwrap()) as usize;
        let bytes =
            u64::from_le_bytes(head[32..40].try_into().unwrap()) as usize;
        // How many rows stored a name, which is what sizes `span.bin` and
        // `exception.bin`. Version 1 and 2 wrote a span a row and the field
        // is reserved zero in their heads, so the count stands in for it
        // and the sections read dense.
        let stored = match version >= 3 {
            true => {
                u64::from_le_bytes(head[40..48].try_into().unwrap()) as usize
            }
            false => count,
        };
        if count == 0 {
            return Ok(Table::default());
        }

        let at = generation_dir(dir, generation);
        let held = Mapped {
            byname: map(&at.join(BYNAME_FILE), count * ROW)?,
            text: Text::open(
                &at,
                count,
                bytes,
                (version >= 3).then_some(stored),
            )?,
        };
        Ok(Table { held: Some(held) })
    }

    /// How many systems the base names.
    pub fn len(&self) -> usize {
        self.held.as_ref().map_or(0, |held| held.text.count)
    }

    /// Whether it names none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every address, ascending: the base's own index, straight off the
    /// mapping.
    pub fn addresses(&self) -> &[i64] {
        self.held.as_ref().map_or(&[], |held| held.text.addresses())
    }

    /// Where `address` sits in the table, if it is in it. A binary search
    /// over [`addresses`](Self::addresses).
    pub fn index_of(&self, address: i64) -> Option<usize> {
        self.addresses().binary_search(&address).ok()
    }

    /// The address of the `at`th system.
    pub fn address_at(&self, at: usize) -> i64 {
        self.addresses()[at]
    }

    /// The name of the `at`th system: the mapping's bytes, or the name its
    /// address spells where the row stored none.
    ///
    /// Upper case either way — the table is written from
    /// [`SystemName`](crate::SystemName)s and
    /// [`crate::core::procedural`] spells upper case by construction — and
    /// borrowed wherever there is something to borrow, which is every row
    /// of a version 1 table and every stored exception of a version 2 one.
    pub fn name_at(&self, at: usize) -> Cow<'_, str> {
        self.held.as_ref().map_or(Cow::Borrowed(""), |held| held.name_at(at))
    }

    /// The `at`th system as a row, which costs the name a copy.
    pub fn entry_at(&self, at: usize) -> NameEntry {
        NameEntry {
            address: self.address_at(at),
            name: SystemName::new(self.name_at(at)),
            position: boxel_middle(self.address_at(at)),
        }
    }

    /// Whether the base already says exactly this, which is the compare
    /// that keeps an unchanged report from being appended to the log.
    pub fn holds(&self, entry: &NameEntry) -> bool {
        match self.index_of(entry.address) {
            // The name alone: the base holds no position to compare
            // against since version 4, and a report that agrees about the
            // name is one the log has nothing to add about. A system that
            // really moved is a system the galaxy renamed or re-placed,
            // and the payload is what says so.
            Some(at) => entry.name == *self.name_at(at),
            None => false,
        }
    }

    /// Read the text the search sweeps, so the first search does not.
    ///
    /// **Reported: the first search of a session took about four seconds.**
    /// A search that underfills its limit sweeps `text.bin`
    /// ([`rows_holding`](Self::rows_holding)), and on a cold page cache
    /// that is not a 128 MB read — it is 128 MB *faulted in a page at a
    /// time*, thousands of round trips to the disk with no read-ahead,
    /// because a mapping the kernel has not been told about is read
    /// wherever the code happens to touch it. Warm, the same sweep is
    /// milliseconds; every measurement of it was taken over a file that
    /// had just been written and was therefore already resident, which is
    /// exactly the measurement that hides this.
    ///
    /// So the pages are asked for **once, in order, off the load** — an
    /// advice the kernel may read ahead on, and then a byte a page, which
    /// it may not ignore. Sequential, so it is one streaming read rather
    /// than thousands of waits.
    ///
    /// **Only the text.** `byname.bin` is 800 MB and a prefix search
    /// touches ~28 pages of it, and `addr.bin` is 1.6 GB and a lookup
    /// touches ~28. Those are the sections the format exists to *not*
    /// read, and pulling them in would trade a slow first search for a
    /// slow open and 2.4 GB of page cache the drawing wants. The text is
    /// the one section a single query reads end to end.
    pub fn warm(&self) -> io::Result<()> {
        let Some(held) = self.held.as_ref() else {
            return Ok(());
        };
        // The list a stored name is found through as well as the text
        // itself: 21 MB at a galaxy, searched ~23 probes deep by every
        // name read, so faulting it a page at a time is the same mistake
        // one level down.
        if let Some(exception) = held.text.exception.as_ref() {
            exception.advise(memmap2::Advice::WillNeed)?;
            let mut seen = 0u64;
            for page in exception.chunks(PAGE) {
                seen += u64::from(page[0]);
            }
            std::hint::black_box(seen);
        }
        let bytes = &held.text.bytes;
        bytes.advise(memmap2::Advice::WillNeed)?;
        // A byte a page, which is what makes the read happen rather than
        // merely be suggested. Summed and handed to `black_box` so the
        // loop is not taken for dead code and deleted.
        let mut seen = 0u64;
        for page in bytes.chunks(PAGE) {
            seen += u64::from(page[0]);
        }
        std::hint::black_box(seen);
        Ok(())
    }

    /// Everything an open does not check, checked: that the addresses
    /// ascend and are unique, that `byname.bin` is a permutation of the
    /// rows in name order, and that every name is UTF-8 within its span.
    ///
    /// A galaxy-sized scan, for whatever wrote the table and for a test.
    /// No client open runs it.
    pub fn audit(&self) -> Result<(), String> {
        let Some(held) = self.held.as_ref() else {
            return Ok(());
        };
        let addresses = self.addresses();
        for pair in addresses.windows(2) {
            if pair[0] >= pair[1] {
                return Err(format!(
                    "addresses {} and {} are out of order",
                    pair[0], pair[1]
                ));
            }
        }
        // The exception list: ascending, inside the table, and one entry a
        // span. Ascending is what the binary search that finds a stored
        // name depends on, and nothing at open can afford to check it.
        let exceptions = held.text.exceptions();
        for pair in exceptions.windows(2) {
            if pair[0] >= pair[1] {
                return Err(format!(
                    "exceptions {} and {} are out of order",
                    pair[0], pair[1]
                ));
            }
        }
        if let Some(last) = exceptions.last() {
            if *last as usize >= held.count() {
                return Err(format!(
                    "exception row {last} of {} rows",
                    held.count()
                ));
            }
        }
        // A sparse span covers a name, so it has length; a dense one may
        // be empty, that being how a version 1 or 2 generation says the
        // row's name was derived.
        let empty = held.text.exception.is_none();
        let mut ends = 0usize;
        for at in 0..held.text.stored {
            let (from, to) = (held.start(at), held.start(at + 1));
            if from > to || (from == to && !empty) || to > held.text.bytes.len()
            {
                return Err(format!("span {at} covers {from}..{to}"));
            }
            if std::str::from_utf8(&held.text.bytes[from..to]).is_err() {
                return Err(format!("span {at} is not UTF-8"));
            }
            ends = to;
        }
        if ends != held.text.bytes.len() {
            return Err(format!(
                "the names end at {ends} of {}",
                held.text.bytes.len()
            ));
        }
        let rows = self.by_name();
        let mut seen = vec![false; held.count()];
        for (order, &row) in rows.iter().enumerate() {
            let row = row as usize;
            if row >= held.count() {
                return Err(format!("byname holds row {row}"));
            }
            if std::mem::replace(&mut seen[row], true) {
                return Err(format!("byname holds row {row} twice"));
            }
            if order > 0 {
                let before = rows[order - 1] as usize;
                let (a, b) = (held.name_at(before), held.name_at(row));
                if (a.as_ref(), before) > (b.as_ref(), row) {
                    return Err(format!("byname has {a} before {b}"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::layout::{
        ADDR_FILE, EXCEPTION_FILE, SPAN_FILE, TEXT_FILE,
    };
    use crate::store::names::Names;
    use crate::store::names::fixtures::{Scratch, entry, published};
    use crate::store::names::format::{ADDR, SPAN, VERSION};
    use crate::store::names::write::{
        compact, live_generation, span_bytes, version,
    };
    use std::fs::File;

    /// A name its address spells is not written down, and reads back anyway
    ///
    /// The whole of what version 2 is: `text.bin` holds the exceptions and
    /// nothing else, and a row whose span has no length is answered by
    /// [`crate::core::procedural`]. Measured over a real galaxy, that is 97.4 %
    /// of rows and 3.94 GB of text against 133 MB — so the assertion here
    /// is the *bytes*, since a table that stored them all would read back
    /// identically and save nothing.
    #[test]
    fn a_name_its_address_spells_is_not_stored() {
        let dir = Scratch::new("derived");
        // Two procedural systems and one name somebody gave. The pairs are
        // real, off `.index/full`.
        let entries = vec![
            entry(96_076_086, "SIDGIO AA-A G1", 1.0),
            entry(1_038_034_644, "PRUE EAEWSY NR-W E1-0", 2.0),
            entry(10, "SOL", 3.0),
        ];
        assert_eq!(published(&dir.0, &entries), 3);

        // Only the given name is in the text, and the derived rows are what
        // is left over: three rows, one name's worth of bytes.
        let at =
            generation_dir(&dir.0, live_generation(&dir.0).unwrap().unwrap());
        let text = std::fs::metadata(at.join(TEXT_FILE)).unwrap().len();
        assert_eq!(text, "SOL".len() as u64, "the derived names were stored");
        assert_eq!(version(&dir.0).unwrap(), Some(VERSION));

        // **And a derived row costs no span either**, which is version 3:
        // one offset a *stored* name plus a terminator, and one row number
        // beside it, against the five bytes a row version 2 spent saying
        // "derived". Three rows, one stored name: ten bytes of span and
        // four of exception, where a dense array would be twenty.
        let span = std::fs::metadata(at.join(SPAN_FILE)).unwrap().len();
        let exception =
            std::fs::metadata(at.join(EXCEPTION_FILE)).unwrap().len();
        assert_eq!(span, 2 * SPAN as u64, "a span a row was written");
        assert_eq!(exception, ROW as u64, "the exception list is one row");

        // And the table answers with all three, in address order, through
        // the same reads a client makes.
        let table = Table::open(&dir.0).expect("the table opens");
        table.audit().expect("a table a build just wrote");
        assert_eq!(table.name_at(0), "SOL");
        assert_eq!(table.name_at(1), "SIDGIO AA-A G1");
        assert_eq!(table.name_at(2), "PRUE EAEWSY NR-W E1-0");
        assert_eq!(table.row_named("SIDGIO AA-A G1"), Some(1));
        assert_eq!(
            table.entry_at(2).name,
            SystemName::new("PRUE EAEWSY NR-W E1-0"),
        );
        // The by-name order is over the names as read, derived ones
        // included: a search has to find them.
        let found = table.rows_starting("PRUE", 25);
        assert_eq!(found, vec![2]);

        // And the log still answers over a derived row, which is what a
        // system being renamed later comes to.
        let mut names = Names::open(&dir.0).expect("the table opens");
        assert_eq!(
            names.name_of(96_076_086).as_deref(),
            Some("SIDGIO AA-A G1")
        );
        assert!(names.name(entry(96_076_086, "SIDGIO PRIME", 1.0)));
        assert_eq!(names.name_of(96_076_086).as_deref(), Some("SIDGIO PRIME"));
    }

    /// A directory that has published no names is the empty table, not an
    /// error.
    #[test]
    fn an_unpublished_directory_is_the_empty_table() {
        let dir = Scratch::new("empty");
        let names = Names::open(&dir.0).expect("an empty table opens");
        assert!(names.is_empty());
        assert_eq!(names.name_of(1), None);
        assert_eq!(names.address_of("SOL"), None);
        assert!(names.matching("SOL", 25).is_empty());
        assert_eq!(names.len(), 0);

        // And a build that names nothing publishes that, readably.
        assert_eq!(published(&dir.0, &[]), 0);
        let names = Names::open(&dir.0).expect("a table of no rows opens");
        assert!(names.is_empty());
        names.base().audit().expect("an empty base");
    }

    /// A table whose sections disagree with its head is refused rather than
    /// read.
    #[test]
    fn a_truncated_section_is_refused() {
        let dir = Scratch::new("truncated");
        published(&dir.0, &[entry(1, "SOL", 1.0), entry(2, "ACRUX", 2.0)]);

        let addr = generation_dir(&dir.0, 0).join(ADDR_FILE);
        let file =
            File::options().write(true).open(&addr).expect("the addresses");
        file.set_len(ADDR as u64).expect("a truncation");

        let err = Table::open(&dir.0).expect_err("a truncated section");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// A version 2 generation still reads, dense spans and all
    ///
    /// The rule this format keeps: no change may need a 610 GB import to
    /// be run again, so every version this build ever wrote it also reads
    /// and a directory comes forward when something rewrites its base.
    /// Version 2 wrote a span for every row and marked a derived one by
    /// giving it no length; version 3 writes a span a *stored* name and
    /// names the rows in `exception.bin`. Both are the same table.
    ///
    /// Built by hand, because nothing writes version 2 any more.
    #[test]
    fn a_version_two_generation_still_reads() {
        let dir = Scratch::new("version-two");
        let entries = vec![
            entry(10, "SOL", 3.0),
            entry(96_076_086, "SIDGIO AA-A G1", 1.0),
            entry(1_038_034_644, "PRUE EAEWSY NR-W E1-0", 2.0),
        ];
        // Address order, which is what the base is written in.
        published(&dir.0, &entries);
        let at =
            generation_dir(&dir.0, live_generation(&dir.0).unwrap().unwrap());

        // A dense span array over the same text: nought for `SOL`'s row,
        // then three, and the two derived rows repeat it.
        let mut span = Vec::new();
        for offset in [0usize, 3, 3, 3] {
            span.extend_from_slice(&span_bytes(offset));
        }
        std::fs::write(at.join(SPAN_FILE), &span).expect("dense spans");
        std::fs::remove_file(at.join(EXCEPTION_FILE)).expect("no list");

        // And a version 2 head: the same fields, with the stored count
        // reserved zero.
        let path = names_head_path(&dir.0);
        let mut head = std::fs::read(&path).expect("the head");
        head[8..10].copy_from_slice(&2u16.to_le_bytes());
        head[40..48].copy_from_slice(&0u64.to_le_bytes());
        std::fs::write(&path, &head).expect("a version 2 head");

        assert_eq!(version(&dir.0).unwrap(), Some(2));
        let table = Table::open(&dir.0).expect("a version 2 table opens");
        table.audit().expect("a version 2 table is sound");
        assert_eq!(table.len(), 3);
        assert_eq!(table.name_at(0), "SOL");
        assert_eq!(table.name_at(1), "SIDGIO AA-A G1");
        assert_eq!(table.name_at(2), "PRUE EAEWSY NR-W E1-0");
        assert_eq!(table.matching("A*", 25), Vec::<usize>::new());
        assert_eq!(table.rows_starting("SIDGIO", 25), vec![1]);
        // The text sweep maps an offset back to a row through the dense
        // spans, which is the other branch of the same question.
        assert_eq!(table.rows_holding("SOL", 25), vec![0]);

        // And a rewrite brings it forward without reading anything but the
        // table itself.
        assert_eq!(compact(&dir.0).expect("a fold"), 3);
        assert_eq!(version(&dir.0).unwrap(), Some(VERSION));
        let table = Table::open(&dir.0).expect("the table reopens");
        table.audit().expect("a table the fold just wrote");
        assert_eq!(table.name_at(0), "SOL");
        assert_eq!(table.name_at(2), "PRUE EAEWSY NR-W E1-0");
    }
}
