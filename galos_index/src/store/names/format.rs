//! The names table's bytes: the head, the sections, and how they are mapped.
//!
//! See [`super`] for the layout and why it is this one. This is the reading
//! of it: the magic and versions a head is held to, the widths a section is
//! checked against before it is trusted, and the mapped sections a
//! [`Table`](super::Table) answers out of.

use crate::format::layout::{ADDR_FILE, EXCEPTION_FILE, SPAN_FILE, TEXT_FILE};
use crate::records::NameEntry;
use memmap2::Mmap;
use std::borrow::Cow;
use std::fs::File;
use std::io;
use std::path::Path;

/// The mapped sections are cast to typed slices rather than decoded, so the
/// file's byte order has to be the host's. Every target galos builds for is
/// little-endian; a big-endian one would need the decode this exists to
/// avoid, so it is refused at compile time rather than read wrongly.
const _: () = assert!(
    cfg!(target_endian = "little"),
    "the names table is little-endian and is read by casting the mapping",
);

/// `head.bin`'s magic, in the crate's own spelling — see
/// [`crate::format::checkpoint`], whose header this mirrors.
pub(super) const MAGIC: u64 = u64::from_ne_bytes(*b"GALOSNAM");

/// The layout `head.bin` describes. A reader that does not know a version
/// refuses the table rather than reading it as this one.
///
/// **2 is a name a row may leave unwritten.** A procedural name is a function
/// of the system's address ([`crate::core::procedural`]), so a row whose name
/// the arithmetic spells stores no text at all — 97.4 % of a galaxy's rows, and
/// 3.94 GB of `text.bin` down to 128 MB. Version 2 marked such a row by giving
/// it a span of no length, which still cost the row its five bytes of
/// `span.bin`.
///
/// **3 stops paying for the rows that say nothing.** `span.bin` holds one
/// offset per *stored* name rather than per row, and `exception.bin` says
/// which rows those are — so a row absent from that list is the derived
/// marker, and the 1.00 GB of spans becomes 47 MB. See [`Text::name_at`].
///
/// **4 stops storing where a system is.** `pos.bin` was `[f32; 3]` a row,
/// 2.40 GB at a galaxy, and it duplicated the cell payload that owns the
/// system: the payloads are the router's own source and the only place a
/// place is exact. An address locates its system to within a boxel
/// ([`elite_journal::Boxel::place`], measured against every name of a
/// 200 M dump), so whoever wants an exact place asks the tree —
/// [`crate::Sky::placed`], 0.8–5 ms — and whoever wants a rough one does
/// arithmetic on the address for nothing.
///
/// Every version is read by this build: a v1 or v2 generation has a dense
/// `span.bin` and no `exception.bin`, which [`Text`] answers off the other
/// branch, and one before v4 has a `pos.bin` this simply does not map. So a
/// directory migrates whenever something rewrites its base (`galos index
/// migrate`) rather than on a deadline.
pub(super) const VERSION: u16 = 4;

/// The versions this build reads.
///
/// Writing the newest and reading the lot is what makes each change free:
/// an older build refuses a newer table, which is right — it would read a
/// derived row as nameless, or a sparse span array as a dense one — and
/// this one reads every table it ever wrote.
pub(super) const READS: [u16; 4] = [1, 2, 3, VERSION];

/// `head.bin`'s width. Everything past the fields is reserved and zero, so
/// a later revision has room that an older reader already skips.
pub(super) const HEAD: usize = 64;

/// One address, as `addr.bin` holds it.
pub(super) const ADDR: usize = 8;

/// One row number, as `byname.bin` holds it.
pub(super) const ROW: usize = 4;

/// What one page of a mapping is taken to be, for the warming read.
///
/// Sixteen kibibytes rather than four: it is what this platform faults in,
/// and a stride smaller than a page only costs touches that change nothing.
/// A stride *larger* than the real page would leave holes, so this is the
/// one constant here worth being conservative about.
pub(super) const PAGE: usize = 16 * 1024;

/// The least text worth handing a thread of its own.
///
/// A small table is swept by one hand: spawning is microseconds and the
/// scan of a scratch directory's names is nanoseconds, so the threshold is
/// there to keep a test from paying for eight threads to read sixty bytes.
pub(super) const SWEEP: usize = 4 * 1024 * 1024;

/// One offset into `text.bin`, as `span.bin` holds it.
///
/// Five bytes, not eight: a galaxy's names are ~5.0 GB, which a `u32`
/// cannot address and a `u64` wastes three bytes a row on — 600 MB at
/// 200 M. Forty bits reach a terabyte of names.
pub(super) const SPAN: usize = 5;

/// How many rows one bucket of the by-name sort holds in memory.
///
/// The by-name order cannot be had by sorting row numbers in place: the
/// comparison reads a name, and 200 M random reads into 5 GB of mapped
/// text is hours of page faults. So the sort is a radix over the name
/// bytes — buckets by first byte, then by second, until a bucket fits this
/// — and every pass is sequential. See [`emit_by_name`].
pub(super) const BUCKET_BYTES: usize = 256 * 1024 * 1024;

/// The mappings of one generation, and what `head.bin` said about them.
///
/// Apart from [`Table`] because the empty table has none: a directory that
/// has published no names maps nothing, and a zero-length mapping is not a
/// thing the platform offers.
#[derive(Debug)]
pub(super) struct Mapped {
    pub(super) byname: Mmap,
    pub(super) text: Text,
}

/// The names themselves: the bytes where a row stored any, and the address
/// they are spelled from where it stored none.
///
/// Its own type because the by-name sort needs exactly this and nothing
/// else — it runs over a generation whose `byname.bin` does not exist yet —
/// and because the addresses are no longer beside the names but *part of
/// how a name is read*: a row nothing wrote text for is one whose name
/// [`crate::core::procedural`] spells from `addr.bin`.
#[derive(Debug)]
pub(super) struct Text {
    pub(super) addr: Mmap,
    /// Which rows stored a name, ascending, as `exception.bin` holds them.
    ///
    /// [`None`] for a version 1 or 2 generation, whose `span.bin` carries
    /// an offset for every row and marks a derived one by giving it no
    /// length. Reading both is what lets a directory come forward when
    /// something rewrites its base rather than when this build lands.
    pub(super) exception: Option<Mmap>,
    /// One offset a stored name, and a terminator — or one a *row* where
    /// `exception` is [`None`].
    pub(super) span: Mmap,
    pub(super) bytes: Mmap,
    /// How many rows the generation holds.
    pub(super) count: usize,
    /// How many of them stored a name.
    ///
    /// Equal to `count` on a version 1 or 2 generation, where the spans are
    /// dense whether or not a row's name was written.
    pub(super) stored: usize,
}

impl Mapped {
    /// How many systems the generation names.
    pub(super) fn count(&self) -> usize {
        self.text.count
    }

    /// The `at`th name, off the mapping or off the address.
    pub(super) fn name_at(&self, at: usize) -> Cow<'_, str> {
        self.text.name_at(at)
    }

    /// Where the `at`th name starts in the text.
    pub(super) fn start(&self, at: usize) -> usize {
        self.text.start(at)
    }
}

impl Text {
    /// Map a generation's addresses, names and offsets.
    ///
    /// `stored` is how many rows wrote a name, which is what sizes the
    /// spans and the exception list — or [`None`] for a version 1 or 2
    /// generation, whose spans are one a *row* and whose exception list
    /// does not exist.
    ///
    /// Refused where the offsets do not span the bytes exactly: the first
    /// must be zero and the last must be the length, which is what makes
    /// every name a span of the file rather than of whatever is next to
    /// it. What is *not* checked here is that the exception list ascends
    /// and stays inside the table — that is proportional to it, so it
    /// belongs to [`Table::audit`] and to whatever wrote the file.
    pub(super) fn open(
        at: &Path,
        count: usize,
        bytes: usize,
        stored: Option<usize>,
    ) -> io::Result<Text> {
        let spans = stored.unwrap_or(count);
        let text = Text {
            addr: map(&at.join(ADDR_FILE), count * ADDR)?,
            exception: match stored {
                Some(stored) => {
                    Some(map(&at.join(EXCEPTION_FILE), stored * ROW)?)
                }
                None => None,
            },
            span: map(&at.join(SPAN_FILE), (spans + 1) * SPAN)?,
            bytes: map(&at.join(TEXT_FILE), bytes)?,
            count,
            stored: spans,
        };
        if text.start(0) != 0 || text.start(spans) != bytes {
            return Err(refused("names whose spans do not span them"));
        }
        Ok(text)
    }

    /// Every address, ascending.
    pub(super) fn addresses(&self) -> &[i64] {
        // SAFETY: `addr.bin` is `count * 8` bytes, checked at open; a
        // mapping begins on a page boundary so the slice is aligned; and
        // any eight bytes are a valid `i64`.
        unsafe {
            std::slice::from_raw_parts(
                self.addr.as_ptr().cast::<i64>(),
                self.count,
            )
        }
    }

    /// Which rows stored a name, ascending — empty where the spans are
    /// dense.
    pub(super) fn exceptions(&self) -> &[u32] {
        match &self.exception {
            // SAFETY: `exception.bin` is `stored * 4` bytes, checked at
            // open; a mapping begins on a page boundary so the slice is
            // aligned; and any four bytes are a valid `u32`.
            Some(held) => unsafe {
                std::slice::from_raw_parts(
                    held.as_ptr().cast::<u32>(),
                    self.stored,
                )
            },
            None => &[],
        }
    }

    /// Where the `at`th *stored* name sits in the spans, or [`None`] where
    /// the row stored none.
    ///
    /// One binary search of `exception.bin`, ~23 probes over 21 MB at a
    /// galaxy — and the position it answers with *is* the index into the
    /// spans, which is why the list is both "which rows have text" and
    /// "where each one's offset is". On a dense generation the row is its
    /// own index and an empty span is the marker instead.
    pub(super) fn spanned(&self, at: usize) -> Option<usize> {
        match self.exception.is_some() {
            true => self.exceptions().binary_search(&(at as u32)).ok(),
            false => (self.start(at) != self.start(at + 1)).then_some(at),
        }
    }

    /// Which *span* holds the byte at `off` of the text.
    ///
    /// The first span that *ends* past `off`, which is the owner. Sparse,
    /// that search is over the stored names alone — 5.26 M rather than 200
    /// M at a galaxy. Dense, spans ascend with equal runs where rows are
    /// derived, so asking for the last one starting at or before `off`
    /// would answer with whichever empty span sits beside it.
    ///
    /// The span rather than the row, and they are not the same number: a
    /// span is one of the names actually stored and a row is one of the
    /// table's, which since version 3 is mostly rows storing nothing. A
    /// caller that wants to know where a name begins ([`Self::start`]) is
    /// asking about the span; one that wants to answer with the system is
    /// asking about the row. Reading the span array at a row's number was
    /// this table's one crash: over a galaxy's 5.2 M stored names in 200 M
    /// rows, a sweep that found a hit in the tail of the text indexed the
    /// span array 26,473,335 spans in and it is 5,257,783 long.
    pub(super) fn span_at(&self, off: usize) -> usize {
        let mut lo = 0usize;
        let mut hi = self.stored;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.start(mid + 1) <= off {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Which row the `at`th stored name belongs to.
    pub(super) fn row_of(&self, at: usize) -> usize {
        match self.exception.is_some() {
            true => self.exceptions().get(at).map_or(0, |row| *row as usize),
            false => at,
        }
    }

    /// Where the `at`th span starts in the text. `at == stored` is the end
    /// of the last, which is what makes a length array unnecessary.
    pub(super) fn start(&self, at: usize) -> usize {
        let b = &self.span[at * SPAN..at * SPAN + SPAN];
        u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], 0, 0, 0]) as usize
    }

    /// The `at`th name: the bytes where the row stored any, and the name
    /// its address spells where it stored none.
    ///
    /// **A row absent from the exception list is the derived marker.** It
    /// costs nothing to say so — the list is needed anyway to find a
    /// stored name's offset — where version 2 spent five bytes of
    /// `span.bin` on every derived row to say the same thing, which over a
    /// galaxy was 974 MB of "nothing here".
    ///
    /// A derived row whose sector the dictionary does not know reads as
    /// empty rather than panicking a client, which is the same thing this
    /// does with text that is not UTF-8. Nothing writes such a row: a name
    /// is dropped only where the arithmetic has just spelled it.
    pub(super) fn name_at(&self, at: usize) -> Cow<'_, str> {
        let Some(span) = self.spanned(at) else {
            return self
                .addresses()
                .get(at)
                .and_then(|address| crate::core::procedural::name_of(*address))
                .map_or(Cow::Borrowed(""), |held| Cow::Owned(held.into()));
        };
        let (from, to) = (self.start(span), self.start(span + 1));
        // Written from `str`s and checked by `audit`, so the bytes between
        // two starts are one. A table that says otherwise reads as empty
        // rather than panicking a client.
        Cow::Borrowed(
            std::str::from_utf8(&self.bytes[from..to]).unwrap_or_default(),
        )
    }
}

/// Map `path`, refusing it where it is not exactly `want` bytes.
pub(super) fn map(path: &Path, want: usize) -> io::Result<Mmap> {
    let file = File::open(path)?;
    let found = file.metadata()?.len();
    if found != want as u64 {
        return Err(refused(&format!(
            "{}: {found} bytes, not {want}",
            path.display()
        )));
    }
    // SAFETY: a generation's files are written once, renamed into place and
    // never modified after; the generation a reader holds is unlinked, not
    // rewritten, so the bytes under the mapping do not change.
    let map = unsafe { Mmap::map(&file)? };
    if map.as_ptr() as usize % ADDR != 0 {
        return Err(refused("a mapping that is not eight-byte aligned"));
    }
    Ok(map)
}

/// The middle of the boxel `address` names, as a published row carries a
/// place.
///
/// **This table stopped holding positions at version 4** — `pos.bin` was
/// 2.40 GB of a place a row, duplicating the cell payload that owns the
/// system — so what a row answers with is what the *address* implies: the
/// middle of its boxel, within half a boxel of the truth, which is ten
/// light years across at the class most systems are. That is what a search
/// result ranked by distance from the camera wants, and it is free, being
/// arithmetic on the address.
///
/// Every read path goes through this, the log's rows included, and that is
/// the point rather than a detail: the log carries the place a report
/// arrived with, so answering it there and a boxel here would make one
/// field mean two things depending on which half of the table replied. The
/// oracle caught exactly that (`tests/derivations_agree.rs`) within an hour
/// of it existing.
///
/// Whoever needs the exact place asks the galaxy, where it is exact:
/// [`crate::Sky::placed`], which the address locates to within this same
/// boxel and which a router's endpoints use.
pub(super) fn placed(entry: NameEntry) -> NameEntry {
    NameEntry { position: boxel_middle(entry.address), ..entry }
}

/// The middle of the boxel `address` names, in light years.
pub(super) fn boxel_middle(address: i64) -> [f32; 3] {
    let (at, _) = elite_journal::Boxel::of(address).place();
    [at[0] as f32, at[1] as f32, at[2] as f32]
}

/// The one error kind this format refuses with.
pub(super) fn refused(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_owned())
}
