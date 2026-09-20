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
//! **`text.bin` holds the names nothing can work out.** A procedural name
//! is a function of the system's address ([`crate::procedural`]), so a row
//! whose name the arithmetic spells stores no bytes at all and is marked by
//! a span of no length — a stored name is never empty, so the marker costs
//! nothing either. Measured over the 200 M table, migrating it in place:
//! **`text.bin` 3.94 GB → 128 MB**, the whole directory 9.1 GB → 5.6 GB,
//! and reads got *faster* rather than slower, a name being arithmetic where
//! it was a page fault into four gigabytes: an address lookup 489 µs → 87
//! µs, resolving a name 2.4 ms → 109 µs, both warm.
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
//! withdrawal: [`Said::Gone`] is the tombstone that shadows a base row.

use crate::meta::NameEntry;
use crate::name::SystemName;
use crate::rows::{self, Sheet};
use crate::source::{names_delta_path, names_dir, names_head_path};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The mapped sections are cast to typed slices rather than decoded, so the
/// file's byte order has to be the host's. Every target galos builds for is
/// little-endian; a big-endian one would need the decode this exists to
/// avoid, so it is refused at compile time rather than read wrongly.
const _: () = assert!(
    cfg!(target_endian = "little"),
    "the names table is little-endian and is read by casting the mapping",
);

/// `head.bin`'s magic, in the crate's own spelling — see
/// [`crate::checkpoint`], whose header this mirrors.
const MAGIC: u64 = u64::from_ne_bytes(*b"GALOSNAM");

/// The layout `head.bin` describes. A reader that does not know a version
/// refuses the table rather than reading it as this one.
///
/// **2 is a name a row may leave unwritten.** A procedural name is a
/// function of the system's address ([`crate::procedural`]), so a row whose
/// name the arithmetic spells stores no text at all — 97.4 % of a galaxy's
/// rows, and 3.94 GB of `text.bin` down to 128 MB. Version 2 marked such a
/// row by giving it a span of no length, which still cost the row its five
/// bytes of `span.bin`.
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
const VERSION: u16 = 4;

/// The versions this build reads.
///
/// Writing the newest and reading the lot is what makes each change free:
/// an older build refuses a newer table, which is right — it would read a
/// derived row as nameless, or a sparse span array as a dense one — and
/// this one reads every table it ever wrote.
const READS: [u16; 4] = [1, 2, 3, VERSION];

/// `head.bin`'s width. Everything past the fields is reserved and zero, so
/// a later revision has room that an older reader already skips.
const HEAD: usize = 64;

/// One address, as `addr.bin` holds it.
const ADDR: usize = 8;

/// One row number, as `byname.bin` holds it.
const ROW: usize = 4;

/// What one page of a mapping is taken to be, for the warming read.
///
/// Sixteen kibibytes rather than four: it is what this platform faults in,
/// and a stride smaller than a page only costs touches that change nothing.
/// A stride *larger* than the real page would leave holes, so this is the
/// one constant here worth being conservative about.
const PAGE: usize = 16 * 1024;

/// The least text worth handing a thread of its own.
///
/// A small table is swept by one hand: spawning is microseconds and the
/// scan of a scratch directory's names is nanoseconds, so the threshold is
/// there to keep a test from paying for eight threads to read sixty bytes.
const SWEEP: usize = 4 * 1024 * 1024;

/// One offset into `text.bin`, as `span.bin` holds it.
///
/// Five bytes, not eight: a galaxy's names are ~5.0 GB, which a `u32`
/// cannot address and a `u64` wastes three bytes a row on — 600 MB at
/// 200 M. Forty bits reach a terabyte of names.
const SPAN: usize = 5;

/// What one row of the base costs on disk, beside its name.
///
/// 8 + 12 + 4 + 5 = 29 bytes, and a name averages ~25 more: 5.8 GB at
/// 200,071,629 systems, against 8.7 GB of MessagePack chunks for the same
/// table and 7.9 GB resident to read them.
pub const ROW_BYTES: usize = ADDR + ROW + SPAN;

/// How many rows one bucket of the by-name sort holds in memory.
///
/// The by-name order cannot be had by sorting row numbers in place: the
/// comparison reads a name, and 200 M random reads into 5 GB of mapped
/// text is hours of page faults. So the sort is a radix over the name
/// bytes — buckets by first byte, then by second, until a bucket fits this
/// — and every pass is sequential. See [`emit_by_name`].
const BUCKET_BYTES: usize = 256 * 1024 * 1024;

/// The shortest prefix a search is answered for.
///
/// One character of a galaxy-wide index is every system starting with that
/// letter, of which there are millions, and the 25 that come back are the
/// first 25 in name order — noise dressed as an answer, since name order
/// near `S` says nothing about where the commander is or what they meant.
/// Two characters is EDDA's floor for the same reason.
///
/// It is a floor on *searching*, not on naming: [`Names::address_of`] and
/// [`Table::rows_starting`] answer whatever they are asked, so a system
/// really named `S` is still resolvable as a route endpoint.
pub const MIN_PREFIX: usize = 2;

/// The names table, base and delta, as a reader or a writer holds it.
///
/// Both halves are behind an [`Arc`] because both are shared and neither is
/// copied: a client hands the base to every task that draws and swaps only
/// the delta when the feed moves ([`with_delta`](Self::with_delta)), and a
/// writer holds the one reference there is and mutates the delta in place
/// for free.
///
/// The precedence is the one rule of the format and it lives here: the
/// delta answers first, and a [`Said::Gone`] in it hides a base row.
#[derive(Clone, Debug, Default)]
pub struct Names {
    base: Arc<Table>,
    delta: Arc<Delta>,
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
    /// spelled from the address itself ([`crate::procedural`]), which is
    /// 97.4 % of a galaxy and the whole reason `text.bin` is small.
    ///
    /// Upper case either way, every name in the table being a
    /// [`SystemName`](crate::SystemName) and the arithmetic spelling upper
    /// case by construction.
    pub fn name_of(&self, address: i64) -> Option<Cow<'_, str>> {
        match self.delta.said(address) {
            Some(Held::Named(entry)) => Some(Cow::Borrowed(&entry.name)),
            Some(Held::Gone) => None,
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
            Some(Held::Named(entry)) => Some(placed(entry.clone())),
            Some(Held::Gone) => None,
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
            Some(Held::Named(entry)) if entry.name == *name => Some(address),
            Some(_) => None,
            None => Some(address),
        }
    }

    /// Whether any system is named exactly `name`.
    pub fn names_exactly(&self, name: &str) -> bool {
        self.address_of(name).is_some()
    }

    /// Which systems' names hold `needle`, at most `limit` of them.
    ///
    /// `needle` is expected upper case, and shorter than [`MIN_PREFIX`] is
    /// answered with nothing — see that constant for why an answer to one
    /// character would be worse than none.
    ///
    /// Three roads, in the order a reader wants them, and the first that
    /// fills the limit ends it:
    ///
    /// 1. **The delta**, which is small and is the later word.
    /// 2. **The base by name** — the prefix, and then the stored names
    ///    holding the query at a word start ([`Table::matching`]). That
    ///    second half is a scan and is affordable only because the names
    ///    are derived: it reads the 128 MB of exceptions rather than 3.94
    ///    GB of every name.
    /// 3. **The sector dictionary**, for the 97.4 % of names nothing
    ///    stored. A derived name's words are a sector and a boxel code, so
    ///    a query matching a sector *mid-name* — `EUQ` for `PRAEA EUQ
    ///    YE-Q D5-0` — is answered by asking
    ///    [`procedural::sectors_holding`] which sectors hold that word and
    ///    walking each one's run of the by-name order. 11,662 sectors and
    ///    192 KB, compiled in, so finding the sector costs microseconds
    ///    and the rows come back through the same prefix search as ever.
    pub fn matching(&self, needle: &str, limit: usize) -> Vec<NameEntry> {
        self.matching_near(needle, None, limit)
    }

    /// The same, ranked from where the reader is looking
    ///
    /// **Which sectors are offered is the whole of what this changes.** A
    /// word like `EUQ` names sixty-odd of them and no search can offer
    /// them all, so without a place to measure from the answer is whichever
    /// the vocabulary begins with — alphabetically, which is nowhere in
    /// particular. `near` is the camera's own centre, and a sector's place
    /// is arithmetic on its key.
    pub fn matching_near(
        &self,
        needle: &str,
        near: Option<[f64; 3]>,
        limit: usize,
    ) -> Vec<NameEntry> {
        if needle.chars().count() < MIN_PREFIX {
            return Vec::new();
        }
        // **The words of the query, and they need not be in order.** A
        // reader holds a name in pieces — the sector of one they have been
        // to and the boxel code off a screenshot, or the two words of a
        // sector the wrong way round — and a search that only matched a run
        // of bytes answered `EUQ PRAEA` with nothing while holding two
        // hundred million names beginning `PRAEA EUQ`.
        //
        // One word is the old question exactly: the roads below take the
        // longest of them, which is the most selective, and every candidate
        // is then checked against the *whole* query. So the roads are
        // unchanged and what is new is a sieve behind them.
        let words = words_of(needle);
        let probe = words
            .iter()
            .max_by_key(|word| word.len())
            .copied()
            .unwrap_or(needle);
        // Room for the sieve to throw candidates away. A road asked for
        // `limit` rows and filtered would answer a two-word query with
        // almost nothing; asked for this many it has something to filter.
        // Bounded, because the roads are a scan and a sector's run is
        // millions of rows: a query whose words are spread thinly through
        // one sector is the case [`crate::procedural`]'s coordinate search
        // is for, and is not this.
        let reach = match words.len() {
            0 | 1 => limit,
            _ => limit.saturating_mul(SIFT).min(SIFTED),
        };
        let holds = |name: &SystemName| holds_words(name.as_str(), &words);
        let mut found: Vec<NameEntry> = self
            .delta
            .entries()
            .filter(|entry| holds(&entry.name))
            .take(limit)
            .cloned()
            .map(placed)
            .collect();
        let take = |at: usize, found: &mut Vec<NameEntry>| {
            let address = self.base.address_at(at);
            if self.delta.said(address).is_some()
                || found.iter().any(|held| held.address == address)
            {
                return;
            }
            let entry = self.base.entry_at(at);
            if holds(&entry.name) {
                found.push(entry);
            }
        };
        // **A share each, because a road that answers cannot be allowed to
        // starve one that answers differently.** The by-name road ran
        // first and filled the whole limit, so `EUQ` came back as three
        // systems of `EUQAIPPY` — the sector `PRAEA EUQ` holds a hundred
        // thousand and not one of them was offered, the road that knows
        // about them never having been reached. Precedence was the bug:
        // the two roads answer *different questions* about the same query,
        // "named this" and "in a sector called this", and which of them a
        // reader meant is not something the order of a match can say.
        //
        // So each takes half, and whatever half one leaves is the other's.
        // The by-name road goes first with its share and again at the end
        // with the remainder, which keeps a query nothing procedural
        // matches reading exactly as it did.
        let share = (limit - found.len()).div_ceil(2);
        let mut by_name = self.base.matching(probe, reach).into_iter();
        for at in by_name.by_ref().take(share) {
            if found.len() >= limit {
                return found;
            }
            take(at, &mut found);
        }
        // The sectors the query's words name, most words matched first: a
        // derived name is a sector and then coordinates, so this is the
        // only road that reaches the 97.4 % of names nothing stored.
        // **A word that is coordinates is constructed, not matched.** `EUQ
        // YE-Q` used to answer nothing: the sector road walks a sector's
        // rows in name order and the `YE-Q` boxels sit a hundred thousand
        // rows down them, past any window worth reading. But `YE-Q` is not
        // a name at all — it is the boxel's ordinal in base 26 — so the
        // address it names in a given sector is arithmetic, and all the
        // table is asked is whether it holds it. See
        // [`crate::procedural::addresses_in`].
        if let Some(coded) = crate::procedural::coded(&words) {
            // The words that are not coordinates name the sector; where
            // there are none, the query is a boxel of *every* sector and
            // only where the reader is looking can say which to try.
            let named: Vec<&str> = words
                .iter()
                .copied()
                .filter(|word| !crate::procedural::is_coordinate(word))
                .collect();
            let sectors: Vec<u32> = match (named.is_empty(), near) {
                (true, Some(near)) => crate::procedural::sectors_near(
                    near,
                    crate::procedural::TRIED_SECTORS,
                ),
                (true, None) => Vec::new(),
                _ => {
                    crate::procedural::sectors_holding_all(&named, near, limit)
                        .into_iter()
                        .filter_map(|(_, sector)| {
                            crate::procedural::sector_named(sector)
                                .map(crate::procedural::sector_key)
                        })
                        .collect()
                }
            };
            let each = (limit - found.len()).div_ceil(sectors.len().max(1));
            for key in sectors {
                let mut kept = 0;
                for address in crate::procedural::addresses_in(
                    key,
                    &coded,
                    crate::procedural::TRIED,
                ) {
                    if found.len() >= limit || kept >= each {
                        break;
                    }
                    if self.delta.said(address).is_some()
                        || found.iter().any(|held| held.address == address)
                    {
                        continue;
                    }
                    if let Some(at) = self.base.index_of(address) {
                        found.push(self.base.entry_at(at));
                        kept += 1;
                    }
                }
            }
            if found.len() >= limit {
                return found;
            }
        }
        // **Spelled right before spelled nearly, whichever road answers.**
        // A sector merely near what was typed is a worse answer than a
        // stored name holding the word exactly, so the sector road is two:
        // the sectors spelled right run here and the ones only nearly
        // spelled run behind the text sweep below. Without that, `COLONA`
        // answered with five systems of `COJOA`, two edits out, and never
        // reached `COLONIA`.
        //
        // A few rows from each sector, for the same reason the roads take
        // a share: a sector holds a hundred thousand systems whose names
        // differ in the coordinates, so its first dozen rows are the least
        // useful dozen answers there are. `EUQ` offered six of `BLAEA EUQ
        // AA-A` and nothing of the other sectors named `EUQ`.
        let sectors =
            crate::procedural::sectors_holding_all(&words, near, limit);
        let mut nearly: Vec<&str> = Vec::new();
        let each = (limit - found.len()).div_ceil(sectors.len().max(1));
        for (exactly, sector) in sectors {
            if !exactly {
                nearly.push(sector);
                continue;
            }
            for at in
                self.base.rows_starting(sector, reach).into_iter().take(each)
            {
                if found.len() >= limit {
                    return found;
                }
                take(at, &mut found);
            }
        }
        for at in by_name {
            if found.len() >= limit {
                return found;
            }
            take(at, &mut found);
        }
        // **And last, the stored names nearly spelled.** The procedural
        // ones are fuzzed off the dictionary by every road above; a given
        // name has no vocabulary to fuzz against, so the only answer is to
        // walk the text a word at a time ([`Table::rows_near`]).
        // Only where nothing was spelled right. A query that found real
        // names is a query a reader spelled, and near misses beside them
        // are noise; and this is the one road that reads all 128 MB of the
        // text, which measured 20–90 ms against the 1–6 ms the roads above
        // cost. `SOL` pays none of it.
        if found.is_empty() {
            for at in self.base.rows_near(probe, limit) {
                if found.len() >= limit {
                    return found;
                }
                take(at, &mut found);
            }
        }
        // And the sectors only nearly spelled, last of all.
        let each = (limit - found.len()).div_ceil(nearly.len().max(1));
        for sector in nearly {
            for at in
                self.base.rows_starting(sector, reach).into_iter().take(each)
            {
                if found.len() >= limit {
                    return found;
                }
                take(at, &mut found);
            }
        }
        found
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

/// How many candidates a road is asked for per row wanted, where the query
/// has more than one word
///
/// The roads match one word and the sieve checks the rest, so a road asked
/// for exactly what the caller wants would answer a two-word query with
/// almost nothing. Sixty-four is enough for a query whose words sit
/// together — which is what a name is — and the cap below is what keeps a
/// query whose words do not from reading a sector's whole run.
const SIFT: usize = 64;

/// And at most this many candidates however large the limit.
const SIFTED: usize = 4_096;

/// The words of a query, upper case and in the order typed.
fn words_of(needle: &str) -> Vec<&str> {
    needle.split(' ').filter(|word| !word.is_empty()).collect()
}

/// Whether `name` holds every one of `words`, in any order
///
/// The order they were typed in is not the order they have to appear in,
/// which is the whole point; a word start each, for
/// [`Table::rows_holding`]'s reason — `SOL` answering every `RESOLUTE` is
/// noise dressed as an answer.
///
/// A word matches at the start of one of the name's words, or near enough
/// to it — [`crate::procedural::matches_word`], which is the same rule the
/// sector vocabulary is searched by, so the sieve cannot throw away a row
/// the road in front of it just found.
fn holds_words(name: &str, words: &[&str]) -> bool {
    words.iter().all(|word| {
        name.split(' ').any(|held| crate::procedural::matches_word(word, held))
    })
}

/// What the delta says about one address.
#[derive(Clone, Debug, PartialEq)]
pub enum Held<'a> {
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
pub enum Said {
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
const FOLD_ROWS: usize = 256 * 1024;

/// How many bytes the log may reach before the same thing is true.
///
/// [`FOLD_ROWS`] counts addresses and the file grows by rows, and the two
/// are not the same thing: an address the log already mentions that changes
/// again appends a second row and leaves the count where it was. Renames
/// and position corrections are near-never, so this is not the trigger that
/// fires in practice — but without it a feed that never ends has a way to
/// grow a file that is never folded, and "near-never" is not a bound.
const FOLD_BYTES: u64 = 64 * 1024 * 1024;

/// What the feed has said since the base was written.
///
/// Small, resident, and read whole at startup. Held as a map for the
/// answering and as a byte offset for the reading: a client that has
/// consumed the first `read` bytes of the log asks only for what is past
/// them, so a refresh costs the arrivals and not the log.
#[derive(Clone, Debug, Default)]
pub struct Delta {
    /// What each address the log mentions has come to.
    said: HashMap<i64, Said>,
    /// Addresses whose last word was [`Said::Gone`], kept apart so
    /// [`entries`](Self::entries) can walk the named without testing each.
    gone: HashSet<i64>,
    /// How many bytes of the log this is, which is where the next read
    /// starts.
    read: u64,
    /// Rows taken since the last [`append`](Self::append), in the order
    /// they were taken.
    pending: Vec<Said>,
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
    /// already in hand builds, and the one road to a [`Names`] that does
    /// not touch a directory.
    pub fn of(entries: impl IntoIterator<Item = NameEntry>) -> Delta {
        let mut delta = Delta::default();
        for entry in entries {
            delta.apply(Said::Named(entry));
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
    pub fn said(&self, address: i64) -> Option<Held<'_>> {
        match self.said.get(&address)? {
            Said::Named(entry) => Some(Held::Named(entry)),
            Said::Gone(_) => Some(Held::Gone),
        }
    }

    /// Every row the log still names, in no order a caller may rely on.
    pub fn entries(&self) -> impl Iterator<Item = &NameEntry> + '_ {
        self.said.values().filter_map(|said| match said {
            Said::Named(entry) => Some(entry),
            Said::Gone(_) => None,
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
    fn net(&self, base: &Table) -> isize {
        let mut net = 0isize;
        for said in self.said.values() {
            match said {
                Said::Named(entry) => {
                    if base.index_of(entry.address).is_none() {
                        net += 1;
                    }
                }
                Said::Gone(address) => {
                    if base.index_of(*address).is_some() {
                        net -= 1;
                    }
                }
            }
        }
        net
    }

    /// The row named exactly `name`, if the log holds one.
    fn named_exactly(&self, name: &str) -> Option<&NameEntry> {
        self.entries().find(|entry| entry.name == *name)
    }

    /// Take a naming, answering whether it changed what the log says.
    fn named(&mut self, entry: NameEntry) -> bool {
        if let Some(Said::Named(held)) = self.said.get(&entry.address)
            && *held == entry
        {
            return false;
        }
        let said = Said::Named(entry);
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
    fn gone(&mut self, address: i64, in_base: bool) -> bool {
        match self.said.get(&address) {
            Some(Said::Gone(_)) => return false,
            None if !in_base => return false,
            _ => {}
        }
        let said = Said::Gone(address);
        self.pending.push(said.clone());
        self.apply(said);
        true
    }

    /// One row's effect on what the log says.
    fn apply(&mut self, said: Said) {
        match &said {
            Said::Named(entry) => {
                self.gone.remove(&entry.address);
                self.said.insert(entry.address, said);
            }
            Said::Gone(address) => {
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
    fn append(&mut self, dir: &Path) -> io::Result<usize> {
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
    fn take_from(&mut self, dir: &Path, from: u64) -> io::Result<()> {
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
            let said: Said = rmp_serde::from_slice(&buf)
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

/// The base: the sorted, mapped, immutable half of the table.
///
/// Five mappings and two numbers. Nothing in it is decoded, allocated or
/// copied at open; a slice of it is a slice of the file.
#[derive(Debug, Default)]
pub struct Table {
    held: Option<Mapped>,
}

/// The mappings of one generation, and what `head.bin` said about them.
///
/// Apart from [`Table`] because the empty table has none: a directory that
/// has published no names maps nothing, and a zero-length mapping is not a
/// thing the platform offers.
#[derive(Debug)]
struct Mapped {
    byname: Mmap,
    text: Text,
}

/// The names themselves: the bytes where a row stored any, and the address
/// they are spelled from where it stored none.
///
/// Its own type because the by-name sort needs exactly this and nothing
/// else — it runs over a generation whose `byname.bin` does not exist yet —
/// and because the addresses are no longer beside the names but *part of
/// how a name is read*: a row nothing wrote text for is one whose name
/// [`crate::procedural`] spells from `addr.bin`.
#[derive(Debug)]
struct Text {
    addr: Mmap,
    /// Which rows stored a name, ascending, as `exception.bin` holds them.
    ///
    /// [`None`] for a version 1 or 2 generation, whose `span.bin` carries
    /// an offset for every row and marks a derived one by giving it no
    /// length. Reading both is what lets a directory come forward when
    /// something rewrites its base rather than when this build lands.
    exception: Option<Mmap>,
    /// One offset a stored name, and a terminator — or one a *row* where
    /// `exception` is [`None`].
    span: Mmap,
    bytes: Mmap,
    /// How many rows the generation holds.
    count: usize,
    /// How many of them stored a name.
    ///
    /// Equal to `count` on a version 1 or 2 generation, where the spans are
    /// dense whether or not a row's name was written.
    stored: usize,
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
    /// [`crate::procedural`] spells upper case by construction — and
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

    /// Which system is named exactly `name`, as a row number.
    ///
    /// A binary search of `byname.bin`, comparing bytes: `name` is expected
    /// upper case, as every name in the table is. Where two systems share a
    /// name the search answers with one of them.
    pub fn row_named(&self, name: &str) -> Option<usize> {
        let held = self.held.as_ref()?;
        let rows = self.by_name();
        let mut lo = 0usize;
        let mut hi = rows.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            let row = rows[mid] as usize;
            match held.name_at(row).as_ref().cmp(name) {
                std::cmp::Ordering::Equal => return Some(row),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    /// Which systems' names begin with `prefix`, at most `limit` of them,
    /// in name order.
    ///
    /// The lower bound by binary search, then a walk forward while the
    /// names still begin with it: the work is the answer's size and not the
    /// galaxy's.
    pub fn rows_starting(&self, prefix: &str, limit: usize) -> Vec<usize> {
        let mut found = Vec::new();
        let Some(held) = self.held.as_ref() else {
            return found;
        };
        if prefix.is_empty() || limit == 0 {
            return found;
        }
        let rows = self.by_name();
        let mut lo = 0usize;
        let mut hi = rows.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if held.name_at(rows[mid] as usize).as_ref() < prefix {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        for &row in &rows[lo..] {
            if !held.name_at(row as usize).starts_with(prefix) {
                break;
            }
            found.push(row as usize);
            if found.len() >= limit {
                break;
            }
        }
        found
    }

    /// Which systems' names hold `needle` at the start of a word, at most
    /// `limit` of them.
    ///
    /// **This is a scan, and it is affordable because the names are
    /// derived.** It used to read every name in the table — 3.94 GB at
    /// 200 M, measured at 564 ms warm and 5.7 s cold, and worse than the
    /// wait it faulted the whole blob in and evicted the cell payloads the
    /// map draws from. What it reads now is `text.bin`, which holds only
    /// the names no address spells: **128 MB of a 200 M galaxy**, thirty
    /// times less, and none of it is a page the drawing wants.
    ///
    /// **Swept in parallel**, because a query with one answer still reads
    /// all of it: 128 MB single-threaded measured 40–66 ms, which is ten
    /// times the whole budget a prefix search costs. The text is cut into
    /// one run a thread, each overlapping the next by `needle - 1` bytes
    /// so a match lying across a cut is found exactly once, and the runs'
    /// answers are concatenated in order, which keeps the result the same
    /// on every machine.
    ///
    /// A word start rather than any offset: `A*` answers `SAGITTARIUS A*`
    /// and `SOL` answers `NEW SOL`, where matching mid-word would answer
    /// `SOL` with every `SOLATI` *and* every `RESOLUTE`, which is noise
    /// dressed as an answer. The start of a name counts as a word start,
    /// so this is a superset of the prefix search rather than a different
    /// question.
    ///
    /// The names a *derived* row bears are not in here to be scanned —
    /// they are the sector dictionary and a boxel code — so a query
    /// matching a sector's word is [`Names::matching`]'s business, which
    /// asks [`crate::procedural`] and expands through
    /// [`rows_starting`](Self::rows_starting).
    pub fn rows_holding(&self, needle: &str, limit: usize) -> Vec<usize> {
        let Some(held) = self.held.as_ref() else {
            return Vec::new();
        };
        if needle.is_empty() || limit == 0 {
            return Vec::new();
        }
        let text = &held.text;
        let bytes = &text.bytes[..];
        let needle = needle.as_bytes();
        let hands = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(bytes.len().div_ceil(SWEEP).max(1));
        let run = bytes.len().div_ceil(hands);

        let mut found = Vec::new();
        std::thread::scope(|scope| {
            let mut hands = Vec::with_capacity(hands);
            let mut from = 0usize;
            while from < bytes.len() {
                // Overlapped by the needle less one, so a match straddling
                // the cut is whole in the run before it — and only there,
                // since a run reports a match by its start.
                let upto = (from + run + needle.len() - 1).min(bytes.len());
                let (at, slice) = (from, &bytes[from..upto]);
                hands.push(scope.spawn(move || {
                    let mut hits = Vec::new();
                    let mut off = 0usize;
                    while let Some(found) = find(&slice[off..], needle) {
                        let start = at + off + found;
                        // A hit that begins in the overlap belongs to the
                        // next run, which will begin inside it.
                        if start >= at + run {
                            break;
                        }
                        hits.push(start);
                        off += found + 1;
                    }
                    hits
                }));
                from += run;
            }
            for hand in hands {
                for off in hand.join().expect("a sweep") {
                    // The span the hit landed in, and then the row that
                    // span's name belongs to: the word-start test is about
                    // where the *name* begins, and a row's number is no
                    // index into the span array. See [`Text::span_at`].
                    let span = text.span_at(off);
                    let row = text.row_of(span);
                    // The start of a name or the byte after a space.
                    // Without the name's own start a needle straddling two
                    // names would match: the text has no separators, so
                    // `SOL` and `ACRUX` end to end hold `LACR` between
                    // them.
                    let word =
                        off == text.start(span) || bytes[off - 1] == b' ';
                    if word && !found.contains(&row) {
                        found.push(row);
                        if found.len() >= limit {
                            return;
                        }
                    }
                }
            }
        });
        found
    }

    /// Which systems' *stored* names hold a word near enough to `word`, at
    /// most `limit` of them
    ///
    /// **The fuzzy road for the names nothing derives.** The procedural
    /// ones are fuzzed against a compiled-in dictionary — 11,662 sectors
    /// and 192 KB — because everything else about them is coordinates; the
    /// given ones have no such vocabulary, so the only place a misspelling
    /// can be answered from is the text itself.
    ///
    /// Which is affordable for the same reason the word-start sweep is:
    /// `text.bin` holds only the names no address spells, 128 MB of a 200 M
    /// galaxy. This walks it a *word* at a time rather than looking for a
    /// run of bytes — a banded edit distance has nowhere to start from in a
    /// byte search — and it is swept in parallel, a run of spans a thread.
    ///
    /// The bound is the word's own ([`crate::procedural::matches_word`]),
    /// so a coordinate is never fuzzed and three letters are matched
    /// exactly.
    pub fn rows_near(&self, word: &str, limit: usize) -> Vec<usize> {
        let Some(held) = self.held.as_ref() else {
            return Vec::new();
        };
        if word.is_empty() || limit == 0 {
            return Vec::new();
        }
        let text = &held.text;
        let hands = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(text.bytes.len().div_ceil(SWEEP).max(1));
        let run = text.stored.div_ceil(hands.max(1));

        let mut found = Vec::new();
        std::thread::scope(|scope| {
            let mut hands = Vec::new();
            let mut from = 0usize;
            while from < text.stored {
                let upto = (from + run).min(text.stored);
                let (at, end) = (from, upto);
                hands.push(scope.spawn(move || {
                    let mut hits = Vec::new();
                    for span in at..end {
                        let name =
                            &text.bytes[text.start(span)..text.start(span + 1)];
                        let Ok(name) = std::str::from_utf8(name) else {
                            continue;
                        };
                        // The nearest of the name's words, a name of
                        // several being as near as its best one.
                        let edits = name
                            .split(' ')
                            .filter_map(|part| {
                                crate::procedural::edits_to(word, part)
                            })
                            .min();
                        if let Some(edits) = edits {
                            // How much name there is around the word that
                            // matched, which is the tie-break: `COLONA` is
                            // one edit from `COLONIA` and one from the
                            // `CORONA` of `CORONA AUSTR. DARK REGION FG-Y
                            // E12`, and a name that is nearly the query is
                            // a better answer than a region label holding
                            // a word that is.
                            let held = name.split(' ').count();
                            hits.push((edits, held, text.row_of(span)));
                        }
                    }
                    hits
                }));
                from = upto;
            }
            let mut hits: Vec<(usize, usize, usize)> = Vec::new();
            for hand in hands {
                hits.extend(hand.join().expect("a sweep"));
            }
            // **The nearest first, not the first swept.** A loose query
            // matches thousands of names and there is room for a
            // screenful: `COLONA` reached `R CORONAE AUSTRINI` before
            // `COLONIA` — two edits against one — and filled the answer
            // with it. Ties by row, so the answer is the same on every
            // machine.
            hits.sort_unstable();
            for (.., row) in hits {
                if !found.contains(&row) {
                    found.push(row);
                    if found.len() >= limit {
                        return;
                    }
                }
            }
        });
        found
    }

    /// Which systems' names the client's search should answer with: the
    /// prefix first, then the names holding the query at a word start.
    pub fn matching(&self, needle: &str, limit: usize) -> Vec<usize> {
        let mut found = self.rows_starting(needle, limit);
        if found.len() < limit {
            for row in self.rows_holding(needle, limit) {
                if !found.contains(&row) {
                    found.push(row);
                }
                if found.len() >= limit {
                    break;
                }
            }
        }
        found
    }

    /// The rows in name order.
    fn by_name(&self) -> &[u32] {
        match &self.held {
            // SAFETY: `byname.bin` is `count * 4` bytes, checked at open; a
            // mapping begins on a page boundary so the slice is aligned;
            // and any four bytes are a valid `u32`.
            Some(held) => unsafe {
                std::slice::from_raw_parts(
                    held.byname.as_ptr().cast::<u32>(),
                    held.count(),
                )
            },
            None => &[],
        }
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

impl Mapped {
    /// How many systems the generation names.
    fn count(&self) -> usize {
        self.text.count
    }

    /// The `at`th name, off the mapping or off the address.
    fn name_at(&self, at: usize) -> Cow<'_, str> {
        self.text.name_at(at)
    }

    /// Where the `at`th name starts in the text.
    fn start(&self, at: usize) -> usize {
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
    fn open(
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
    fn addresses(&self) -> &[i64] {
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
    fn exceptions(&self) -> &[u32] {
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
    fn spanned(&self, at: usize) -> Option<usize> {
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
    fn span_at(&self, off: usize) -> usize {
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
    fn row_of(&self, at: usize) -> usize {
        match self.exception.is_some() {
            true => self.exceptions().get(at).map_or(0, |row| *row as usize),
            false => at,
        }
    }

    /// Where the `at`th span starts in the text. `at == stored` is the end
    /// of the last, which is what makes a length array unnecessary.
    fn start(&self, at: usize) -> usize {
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
    fn name_at(&self, at: usize) -> Cow<'_, str> {
        let Some(span) = self.spanned(at) else {
            return self
                .addresses()
                .get(at)
                .and_then(|address| crate::procedural::name_of(*address))
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

/// `head.bin`'s name within the names directory.
pub const HEAD_FILE: &str = "head.bin";
/// The addresses, within a generation directory.
pub const ADDR_FILE: &str = "addr.bin";
/// The rows in name order, within a generation directory.
pub const BYNAME_FILE: &str = "byname.bin";
/// The offsets into the text, one a stored name, within a generation
/// directory.
pub const SPAN_FILE: &str = "span.bin";
/// Which rows stored a name, ascending, within a generation directory.
///
/// Version 3 and after: the rows the arithmetic could not spell. A row
/// absent from it is one [`crate::procedural`] answers for.
pub const EXCEPTION_FILE: &str = "exception.bin";
/// The name bytes, within a generation directory.
pub const TEXT_FILE: &str = "text.bin";

/// One generation's directory within the names directory.
pub fn generation_dir(dir: &Path, generation: u64) -> PathBuf {
    names_dir(dir).join(format!("{generation:03}"))
}

/// Map `path`, refusing it where it is not exactly `want` bytes.
fn map(path: &Path, want: usize) -> io::Result<Mmap> {
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
fn placed(entry: NameEntry) -> NameEntry {
    NameEntry { position: boxel_middle(entry.address), ..entry }
}

/// The middle of the boxel `address` names, in light years.
fn boxel_middle(address: i64) -> [f32; 3] {
    let (at, _) = elite_journal::Boxel::of(address).place();
    [at[0] as f32, at[1] as f32, at[2] as f32]
}

/// Where `needle` first sits in `hay`, or [`None`].
///
/// Hand-rolled rather than a dependency: the sweep it serves reads 128 MB
/// of a real table, so what matters is that the inner loop is a byte
/// compare over a mapping and that a miss on the first byte costs one.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let last = hay.len() - needle.len();
    let mut at = 0usize;
    while at <= last {
        match hay[at..=last].iter().position(|byte| *byte == first) {
            Some(off) => {
                let from = at + off;
                if &hay[from..from + needle.len()] == needle {
                    return Some(from);
                }
                at = from + 1;
            }
            None => return None,
        }
    }
    None
}

/// The one error kind this format refuses with.
fn refused(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_owned())
}

/// The names table written straight to disk, sorted, without ever being in
/// memory.
///
/// What a build uses in place of [`Names`]: it names each system once, in
/// whatever order it reads them, and never looks one up. The rows go to a
/// file as they arrive, the file is sorted by address externally
/// ([`crate::rows`]), and the sections are written from the sorted rows in
/// one pass.
///
/// The sort is the price of the format and it is worth saying why it is
/// paid here. Two of the four sources arrive sorted — the database reads
/// `ORDER BY address` — and two do not: a Spansh dump is in the dump's own
/// order and a feed is in no order at all. A table sorted by address is
/// what makes the address index be the addresses; there is no version of
/// this format that is both unsorted and `O(log N)`.
///
/// A build reads the galaxy for as long as that takes and the table beneath
/// it is served the whole time, so a build [abandoned](Self::abandon) part
/// way leaves the table that stood exactly as it found it, and a build that
/// [finishes](Self::finish) swaps it in with one rename.
pub struct Writer {
    dir: PathBuf,
    scratch: PathBuf,
    rows: Sheet,
    named: usize,
}

impl Writer {
    /// Write a table into `dir`, from nothing.
    pub fn writing(dir: &Path) -> io::Result<Writer> {
        let scratch = scratch_dir(dir);
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch)?;
        Ok(Writer {
            dir: dir.to_owned(),
            rows: Sheet::open(scratch.join("names.rows"))?,
            scratch,
            named: 0,
        })
    }

    /// The same, seeded with what `dir` already publishes.
    ///
    /// What a build carrying on from a read a stop published starts with,
    /// and what a [`compact`] folds a log into a base with. The table's own
    /// rows go in first and this read's rows go over them, which is
    /// [`crate::rows`]'s one rule: the last row an address has wins.
    ///
    /// Seeded from the table as it *answers*, base under log, so a row the
    /// log renamed is carried at its new name and one it withdrew is not
    /// carried at all. Read off the mapping a row at a time, so carrying on
    /// costs a row and not a table.
    ///
    /// A directory published before this format has its table in
    /// MessagePack chunks and no base at all, and seeding from the base
    /// would carry *nothing* — a build that then published would take every
    /// name the directory served away. So the chunks are the seed where
    /// there is no base, which makes a resumed build onto an unmigrated
    /// directory carry its names whether or not anything called
    /// [`crate::migrate`] first.
    pub fn onto(dir: &Path) -> io::Result<Writer> {
        let mut writer = Writer::writing(dir)?;
        let held = Names::open(dir)?;
        if held.is_empty() {
            writer.take_chunks(dir)?;
        }
        for address in held.addresses() {
            if let Some(entry) = held.entry_of(address) {
                writer.push(entry)?;
            }
        }
        Ok(writer)
    }

    /// Take every row of the MessagePack chunks a directory published
    /// before this format, answering how many there were.
    ///
    /// Read a chunk at a time and pushed straight to the row file, so a
    /// galaxy's worth costs one chunk rather than one table.
    fn take_chunks(&mut self, dir: &Path) -> io::Result<usize> {
        let mut taken = 0;
        for chunk in legacy_chunks(dir)? {
            let entries: Vec<NameEntry> = crate::source::read_meta(&chunk)?;
            for entry in entries {
                self.push(entry)?;
                taken += 1;
            }
        }
        Ok(taken)
    }

    /// Take one system's name and place.
    pub fn push(&mut self, entry: NameEntry) -> io::Result<()> {
        self.rows.push(&entry)?;
        self.named += 1;
        Ok(())
    }

    /// How many rows have been taken, which counts a system named twice
    /// twice.
    pub fn named(&self) -> usize {
        self.named
    }

    /// Sort the rows, write the sections, and swap the table in.
    ///
    /// Answers how many systems the published table names, which is the
    /// rows taken less whatever was named more than once.
    pub fn finish(mut self) -> io::Result<usize> {
        self.rows.flush()?;
        let count = write_base(
            &self.dir,
            &self.scratch,
            self.rows.path(),
            rows::RUN_BYTES,
        )?;
        let _ = std::fs::remove_dir_all(&self.scratch);
        Ok(count)
    }

    /// Leave the directory exactly as it was found.
    pub fn abandon(self) -> io::Result<()> {
        match std::fs::remove_dir_all(&self.scratch) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }
}

/// Where a build's rows and runs live: inside the names directory, so the
/// sections it writes are renamed within one filesystem and never copied.
fn scratch_dir(dir: &Path) -> PathBuf {
    names_dir(dir).join(".building")
}

/// Fold `dir`'s delta into its base, answering how many systems the table
/// now names.
///
/// The same road a build takes, from the table that stands: every base row
/// and every row the log named, sorted and written as a new generation. The
/// log is removed after the swap, so a reader that sees the new base and the
/// old log reads rows the base already holds — which is the same table —
/// and one that sees the old base still has the log it needs.
pub fn compact(dir: &Path) -> io::Result<usize> {
    let writer = Writer::onto(dir)?;
    writer.finish()
}

/// The version this build writes, which is what a migration compares
/// against.
pub fn writes() -> u16 {
    VERSION
}

/// What version the table `dir` publishes is, or [`None`] where it
/// publishes none.
///
/// Read off `head.bin` alone — sixty-four bytes, no sections mapped — so a
/// caller deciding whether a rewrite is owed pays nothing to ask. A head
/// that is not this format's is an error rather than a version, which is
/// the same refusal [`Table::open`] makes.
pub fn version(dir: &Path) -> io::Result<Option<u16>> {
    let head = match std::fs::read(names_head_path(dir)) {
        Ok(head) => head,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if head.len() < HEAD
        || u64::from_ne_bytes(head[0..8].try_into().unwrap()) != MAGIC
    {
        return Err(refused("not a names table"));
    }
    Ok(Some(u16::from_le_bytes(head[8..10].try_into().unwrap())))
}

/// Write the base from sorted rows and swap it in.
///
/// `budget` is the sort's run size, which only a test sets.
fn write_base(
    dir: &Path,
    scratch: &Path,
    rows_path: &Path,
    budget: usize,
) -> io::Result<usize> {
    let sorted = rows::sorted::<NameEntry>(
        rows_path,
        scratch,
        "names",
        &|it| it.address,
        budget,
    )?;
    let count = sorted.count();

    let next = live_generation(dir)?.map_or(0, |live| live + 1);
    let at = generation_dir(dir, next);
    std::fs::create_dir_all(&at)?;

    let (bytes, stored) = write_sections(&at, &sorted)?;
    if count > 0 {
        write_by_name(&at, scratch, count, bytes, stored)?;
    }
    drop(sorted);

    write_head(dir, next, count, bytes, stored)?;
    let _ = std::fs::remove_file(names_delta_path(dir));
    sweep_generations(dir, next)?;
    // A published base retires the chunks of the format before it, and
    // that is an invariant rather than a step of the migration: leaving
    // them would let a later fold read them *over* a base that already
    // holds their rows and everything published since, which would take
    // the newer rows away.
    for chunk in legacy_chunks(dir)? {
        let _ = std::fs::remove_file(chunk);
    }
    Ok(count)
}

/// Write `addr.bin`, `exception.bin`, `span.bin` and `text.bin` from the
/// sorted rows, answering how many bytes of names they came to and
/// how many rows stored one.
///
/// One pass, four sequential streams. A position is not among them: the
/// cell payload that owns a system is where its place is exact, and the
/// address says which boxel to look in ([`crate::Sky::placed`]). The sections are separate files
/// exactly so that this is possible: one file with the sections laid end to
/// end would need the counts before the first byte of it could be placed,
/// or a second pass to concatenate gigabytes.
fn write_sections(
    at: &Path,
    sorted: &rows::Sorted,
) -> io::Result<(usize, usize)> {
    let mut addr = BufWriter::new(File::create(at.join(ADDR_FILE))?);

    let mut exception = BufWriter::new(File::create(at.join(EXCEPTION_FILE))?);
    let mut span = BufWriter::new(File::create(at.join(SPAN_FILE))?);
    let mut text = BufWriter::new(File::create(at.join(TEXT_FILE))?);

    let mut bytes = 0usize;
    let mut stored = 0usize;
    let mut row = 0u32;
    span.write_all(&span_bytes(0))?;
    if let Some(mut rows) = sorted.rows()? {
        while let Some((entry, _)) = rows.next::<NameEntry>()? {
            addr.write_all(&entry.address.to_le_bytes())?;
            // **The name is written only where the address does not spell
            // it**, and only such a row costs a span and a place in the
            // exception list. Over a real galaxy that is 2.6 % of them:
            // 3.94 GB of text against 128 MB, and 1.00 GB of spans against
            // 47 MB. The exceptions are the names people gave and
            // Frontier's hand-authored regions, both of which the
            // arithmetic deliberately does not claim.
            if !crate::procedural::spells(entry.address, &entry.name) {
                text.write_all(entry.name.as_bytes())?;
                bytes += entry.name.len();
                exception.write_all(&row.to_le_bytes())?;
                span.write_all(&span_bytes(bytes))?;
                stored += 1;
            }
            row += 1;
        }
    }
    addr.flush()?;
    exception.flush()?;
    span.flush()?;
    text.flush()?;
    Ok((bytes, stored))
}

/// One `span.bin` offset: forty bits, little-endian.
fn span_bytes(at: usize) -> [u8; SPAN] {
    let bytes = (at as u64).to_le_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4]]
}

/// Write `byname.bin`: every row, in name order.
///
/// Not by sorting row numbers with a comparator that reads the mapping —
/// that is 200 M random reads into 5 GB of text and hours of page faults.
/// A radix over the name bytes instead: rows are bucketed by their name's
/// first byte, a bucket too big for [`BUCKET_BYTES`] is bucketed again by
/// the next, and a bucket that fits is sorted in memory. Every pass is
/// sequential, and because a bucket's key is a prefix of its names, writing
/// the buckets in key order writes the rows in name order.
fn write_by_name(
    at: &Path,
    scratch: &Path,
    count: usize,
    bytes: usize,
    stored: usize,
) -> io::Result<()> {
    let text = Text::open(at, count, bytes, Some(stored))?;

    let buckets = scratch.join("byname");
    let _ = std::fs::remove_dir_all(&buckets);
    std::fs::create_dir_all(&buckets)?;

    let root = buckets.join("all");
    {
        let mut out = BufWriter::new(File::create(&root)?);
        for row in 0..count {
            write_pair(&mut out, text.name_at(row).as_bytes(), row as u32)?;
        }
        out.flush()?;
    }

    let mut byname = BufWriter::new(File::create(at.join(BYNAME_FILE))?);
    emit_by_name(&root, 0, &buckets, &mut byname)?;
    byname.flush()?;
    let _ = std::fs::remove_dir_all(&buckets);
    Ok(())
}

/// One `(name, row)` pair of the by-name sort, length-framed.
fn write_pair(
    out: &mut BufWriter<File>,
    name: &[u8],
    row: u32,
) -> io::Result<()> {
    out.write_all(&(name.len() as u16).to_le_bytes())?;
    out.write_all(name)?;
    out.write_all(&row.to_le_bytes())
}

/// Write the rows of one bucket in name order, splitting it first where it
/// is too big to sort in memory.
///
/// Every name in the bucket shares its first `depth` bytes, so the order
/// within it is decided by what follows — and a name that *ends* at `depth`
/// sorts before every longer one that shares its prefix, which is bucket
/// zero.
fn emit_by_name(
    path: &Path,
    depth: usize,
    scratch: &Path,
    out: &mut BufWriter<File>,
) -> io::Result<()> {
    if std::fs::metadata(path)?.len() as usize <= BUCKET_BYTES {
        let (text, mut held) = read_pairs(path)?;
        held.sort_unstable_by(|a, b| {
            let (left, right) = (&text[a.span()], &text[b.span()]);
            left.cmp(right).then(a.row.cmp(&b.row))
        });
        for pair in held {
            out.write_all(&pair.row.to_le_bytes())?;
        }
        return Ok(());
    }

    let mut parts: Vec<Option<BufWriter<File>>> =
        (0..257).map(|_| None).collect();
    let names: Vec<PathBuf> = (0..257)
        .map(|bucket| scratch.join(format!("d{depth}b{bucket:03}")))
        .collect();
    {
        let (text, held) = read_pairs(path)?;
        for pair in &held {
            let name = &text[pair.span()];
            let bucket = name.get(depth).map_or(0, |byte| *byte as usize + 1);
            let part = match &mut parts[bucket] {
                Some(part) => part,
                slot => {
                    slot.insert(BufWriter::new(File::create(&names[bucket])?))
                }
            };
            write_pair(part, name, pair.row)?;
        }
        for part in parts.iter_mut().flatten() {
            part.flush()?;
        }
    }

    for (bucket, part) in parts.iter().enumerate() {
        if part.is_none() {
            continue;
        }
        emit_by_name(&names[bucket], depth + 1, scratch, out)?;
        let _ = std::fs::remove_file(&names[bucket]);
    }
    Ok(())
}

/// One `(name, row)` pair as the sort holds it: the name is a span of the
/// bucket's own bytes rather than a `String`, so a bucket of eight million
/// rows is one allocation and not eight million.
struct Pair {
    from: u32,
    to: u32,
    row: u32,
}

impl Pair {
    fn span(&self) -> std::ops::Range<usize> {
        self.from as usize..self.to as usize
    }
}

/// Read a bucket: its name bytes in one block, and a pair per row.
fn read_pairs(path: &Path) -> io::Result<(Vec<u8>, Vec<Pair>)> {
    let bytes = std::fs::read(path)?;
    let mut text = Vec::with_capacity(bytes.len());
    let mut pairs = Vec::new();
    let mut cur = &bytes[..];
    while cur.len() >= 2 {
        let len = u16::from_le_bytes([cur[0], cur[1]]) as usize;
        if cur.len() < 2 + len + 4 {
            break;
        }
        let from = text.len() as u32;
        text.extend_from_slice(&cur[2..2 + len]);
        let row =
            u32::from_le_bytes(cur[2 + len..2 + len + 4].try_into().unwrap());
        pairs.push(Pair { from, to: text.len() as u32, row });
        cur = &cur[2 + len + 4..];
    }
    Ok((text, pairs))
}

/// Write `head.bin`, which is the step that makes a generation live.
///
/// Beside the file and renamed over it, so the swap is one atomic step: a
/// reader sees the generation that was live or the one that is, never half
/// of either.
fn write_head(
    dir: &Path,
    generation: u64,
    count: usize,
    bytes: usize,
    stored: usize,
) -> io::Result<()> {
    let mut head = vec![0u8; HEAD];
    head[0..8].copy_from_slice(&MAGIC.to_ne_bytes());
    head[8..10].copy_from_slice(&VERSION.to_le_bytes());
    head[16..24].copy_from_slice(&generation.to_le_bytes());
    head[24..32].copy_from_slice(&(count as u64).to_le_bytes());
    head[32..40].copy_from_slice(&(bytes as u64).to_le_bytes());
    // How many rows stored a name, which is what sizes `span.bin` and
    // `exception.bin`. Reserved zero before version 3, where the spans
    // were one a row and the count stood in for this.
    head[40..48].copy_from_slice(&(stored as u64).to_le_bytes());

    std::fs::create_dir_all(names_dir(dir))?;
    let path = names_head_path(dir);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &head)?;
    std::fs::rename(&tmp, &path)
}

/// Which generation `head.bin` names, if it names one.
fn live_generation(dir: &Path) -> io::Result<Option<u64>> {
    let head = match std::fs::read(names_head_path(dir)) {
        Ok(head) => head,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if head.len() < HEAD {
        return Ok(None);
    }
    Ok(Some(u64::from_le_bytes(head[16..24].try_into().unwrap())))
}

/// Remove every generation but the live one.
///
/// The old one, whose readers keep their mappings — a mapping outlives the
/// directory entry — and whatever a build that died between writing a
/// generation and renaming `head.bin` over left behind.
fn sweep_generations(dir: &Path, live: u64) -> io::Result<()> {
    let held = match std::fs::read_dir(names_dir(dir)) {
        Ok(held) => held,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for found in held {
        let path = found?.path();
        if !path.is_dir() {
            continue;
        }
        let stale = path
            .file_name()
            .and_then(|it| it.to_str())
            .and_then(|it| it.parse::<u64>().ok())
            .is_some_and(|generation| generation != live);
        if stale {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
    Ok(())
}

/// The chunk files a directory published before this format, folded into a
/// base — or [`None`] where there are none.
///
/// The one migration this format has. A galaxy's worth of MessagePack
/// chunks took an afternoon to derive and nothing is going to derive it
/// again to change how it is stored, so the chunks are read once, a row at
/// a time, sorted, and written as a generation. They are removed after the
/// swap.
pub fn fold_chunks(dir: &Path) -> io::Result<Option<usize>> {
    let chunks = legacy_chunks(dir)?;
    if chunks.is_empty() {
        return Ok(None);
    }
    let mut writer = Writer::writing(dir)?;
    for chunk in &chunks {
        let entries: Vec<NameEntry> = crate::source::read_meta(chunk)?;
        for entry in entries {
            writer.push(entry)?;
        }
    }
    let count = writer.finish()?;
    for chunk in &chunks {
        let _ = std::fs::remove_file(chunk);
    }
    Ok(Some(count))
}

/// The `names/NNNNN.bin` files of the format before this one, in order.
///
/// Numbered from zero with no gaps, so the first number missing is the end
/// of the table — which is what let a reader find them all without a
/// manifest, and what lets this find them all to be rid of them.
fn legacy_chunks(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut chunks = Vec::new();
    for chunk in 0.. {
        let path = names_dir(dir).join(format!("{chunk:05}.bin"));
        if !path.exists() {
            break;
        }
        chunks.push(path);
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory removed with the test.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
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

    fn entry(address: i64, name: &str, x: f32) -> NameEntry {
        NameEntry {
            address,
            name: SystemName::new(name),
            position: [x, 0.0, 0.0],
        }
    }

    fn published(dir: &Path, entries: &[NameEntry]) -> usize {
        let mut writer = Writer::writing(dir).expect("a writer");
        for entry in entries {
            writer.push(entry.clone()).expect("a push");
        }
        writer.finish().expect("a finish")
    }

    /// A name its address spells is not written down, and reads back anyway
    ///
    /// The whole of what version 2 is: `text.bin` holds the exceptions and
    /// nothing else, and a row whose span has no length is answered by
    /// [`crate::procedural`]. Measured over a real galaxy, that is 97.4 %
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

    /// The table a build wrote is the table a client opens: every row, in
    /// address order, whatever order it was pushed in.
    #[test]
    fn a_build_writes_the_table_a_client_reads() {
        let dir = Scratch::new("round-trip");
        let entries = vec![
            entry(30, "COL 285 SECTOR AB-C D1", 3.0),
            entry(10, "SOL", 1.0),
            entry(20, "ALPHA CENTAURI", 2.0),
        ];
        assert_eq!(published(&dir.0, &entries), 3);

        let table = Table::open(&dir.0).expect("the table opens");
        table.audit().expect("a table a build just wrote");
        assert_eq!(table.len(), 3);
        assert_eq!(table.addresses(), [10, 20, 30]);
        assert_eq!(table.name_at(0), "SOL");
        assert_eq!(table.name_at(1), "ALPHA CENTAURI");
        assert_eq!(table.name_at(2), "COL 285 SECTOR AB-C D1");
        assert_eq!(table.index_of(20), Some(1));
        assert_eq!(table.index_of(11), None);
    }

    /// The by-name index answers an exact name without a scan, and the
    /// order it is written in is the order names sort in.
    #[test]
    fn a_name_is_found_by_binary_search() {
        let dir = Scratch::new("by-name");
        let entries = vec![
            entry(1, "SOL", 0.0),
            entry(2, "SOLATI", 0.0),
            entry(3, "ALPHA CENTAURI", 0.0),
            entry(4, "SOLA", 0.0),
        ];
        published(&dir.0, &entries);

        let table = Table::open(&dir.0).expect("the table opens");
        table.audit().expect("a table a build just wrote");
        assert_eq!(
            table.row_named("SOL").map(|at| table.address_at(at)),
            Some(1)
        );
        assert_eq!(
            table.row_named("SOLA").map(|at| table.address_at(at)),
            Some(4)
        );
        assert_eq!(
            table.row_named("SOLATI").map(|at| table.address_at(at)),
            Some(2)
        );
        assert_eq!(table.row_named("SO"), None);
        assert_eq!(table.row_named("SOLATIX"), None);

        // A shorter name sorts before a longer one that begins with it.
        let order: Vec<String> = table
            .by_name()
            .iter()
            .map(|&row| table.name_at(row as usize).to_string())
            .collect();
        assert_eq!(order, ["ALPHA CENTAURI", "SOL", "SOLA", "SOLATI"]);
    }

    /// A search is the prefix, then a word start, and reads no further
    ///
    /// It used to fall through to a substring scan of *every* name, which
    /// at 200 M was 3.94 GB read on the main thread for most queries — and
    /// the pages it faulted in evicted the cell payloads the map draws
    /// from, so searching stalled the galaxy's reads too. Then it was a
    /// prefix and nothing else, and `SOL` stopped answering `NEW SOL`.
    ///
    /// It is both now, and what changed is the *corpus*: `text.bin` holds
    /// only the names no address spells, 128 MB of a 200 M galaxy against
    /// 3.94 GB, so the scan is thirty times smaller and touches nothing
    /// the drawing wants. What stays refused is a match mid-*word*, which
    /// would answer `OL` with every `SOL`, and a run of bytes straddling
    /// two names, which is what a scan over concatenated text invites.
    #[test]
    fn a_search_is_a_prefix_then_a_word_start() {
        let dir = Scratch::new("matching");
        let entries = vec![
            entry(1, "SOL", 0.0),
            entry(2, "ACRUX", 0.0),
            entry(3, "BOLA", 0.0),
            entry(4, "SOLATI", 0.0),
            entry(5, "NEW SOL", 0.0),
            entry(6, "SAGITTARIUS A*", 0.0),
        ];
        published(&dir.0, &entries);

        let table = Table::open(&dir.0).expect("the table opens");
        let named = |rows: Vec<usize>| -> Vec<String> {
            rows.into_iter().map(|at| table.name_at(at).to_string()).collect()
        };

        // The prefix in name order first, then the names holding it at a
        // word start.
        assert_eq!(
            named(table.matching("SOL", 25)),
            ["SOL", "SOLATI", "NEW SOL"],
        );
        assert_eq!(named(table.matching("BOL", 25)), ["BOLA"]);
        // The one this was for: a word nobody's name begins with.
        assert_eq!(named(table.matching("A*", 25)), ["SAGITTARIUS A*"]);

        // Mid-word is not a match, and neither is a run of bytes that
        // straddles two names — "SOL" and "ACRUX" sit end to end in the
        // text, so the bytes hold "LACR" between them.
        assert_eq!(table.matching("OL", 25), Vec::<usize>::new());
        assert_eq!(table.matching("LACR", 25), Vec::<usize>::new());
        // Nor a run that begins mid-word and runs into the next word.
        assert_eq!(table.matching("EW SOL", 25), Vec::<usize>::new());

        // And the cap is the index's: it stops walking at the limit.
        assert_eq!(table.matching("SOL", 1).len(), 1);
        assert_eq!(table.matching("SOL", 2).len(), 2);
    }

    /// A word of a *derived* name is searchable, and it is not in the text
    ///
    /// The road the dictionary opens: 97.4 % of names store no bytes at
    /// all, so a scan of `text.bin` cannot find them — their words are a
    /// sector and a boxel code. A query matching a sector mid-name is
    /// answered by asking which sectors hold that word and walking each
    /// one's run of the by-name order.
    #[test]
    fn a_word_of_a_derived_name_is_found() {
        let dir = Scratch::new("derived-search");
        // Real systems, and the second word of a two-word sector is the
        // one nothing could scan for.
        let entries = vec![
            entry(1_038_034_644, "PRUE EAEWSY NR-W E1-0", 1.0),
            entry(96_076_086, "SIDGIO AA-A G1", 2.0),
            entry(10, "SOL", 3.0),
        ];
        published(&dir.0, &entries);
        let names = Names::open(&dir.0).expect("the table opens");

        let named = |query: &str| -> Vec<String> {
            names
                .matching(query, 25)
                .into_iter()
                .map(|entry| entry.name.into_string())
                .collect()
        };

        // The prefix road, over a name the table did not store.
        assert_eq!(named("PRUE"), ["PRUE EAEWSY NR-W E1-0"]);
        // The dictionary road: a word no name begins with, and no byte of
        // it is in `text.bin` to be scanned for.
        assert_eq!(named("EAEWSY"), ["PRUE EAEWSY NR-W E1-0"]);
        // The text road still answers for a name that *is* stored.
        assert_eq!(named("SOL"), ["SOL"]);
        // And a word nothing holds answers nothing.
        assert!(named("NOWHERE").is_empty());
    }

    /// A road that answers cannot starve the one that answers differently
    ///
    /// **What `EUQ` did.** Names beginning with the query came out of the
    /// by-name order, filled the limit, and the road that knows which
    /// *sectors* hold that word was never reached — so a query naming a
    /// sector of a hundred thousand systems answered with three systems of
    /// a system named `EUQAIPPY`. Each road takes a share of the limit
    /// now, and the sector road spreads its share over the sectors rather
    /// than spending it all on the first one's first rows.
    #[test]
    fn a_by_name_match_does_not_crowd_out_a_sector() {
        let dir = Scratch::new("shares");
        // Two systems whose *names* begin with the query, and two whose
        // sector merely holds it — one sector each, so the spread shows.
        let entries = vec![
            entry(1_038_034_644, "PRUE EAEWSY NR-W E1-0", 1.0),
            entry(43_113_546_002, "PRUE EAEWSY BQ-P E5-0", 2.0),
            entry(96_076_086, "SIDGIO AA-A G1", 3.0),
            entry(3_107_510, "PRUE AA-A H1", 4.0),
        ];
        published(&dir.0, &entries);
        let names = Names::open(&dir.0).expect("the table opens");
        let named = |query: &str, limit: usize| -> Vec<String> {
            names
                .matching(query, limit)
                .into_iter()
                .map(|entry| entry.name.into_string())
                .collect()
        };

        // `PRUE` begins three of these names and is a sector word of all
        // four. At a limit of two the by-name road takes one and the
        // sector road the other, where it used to take both.
        let two = named("PRUE", 2);
        assert_eq!(two.len(), 2, "{two:?}");
        assert!(
            two.iter().any(|name| name.starts_with("PRUE EAEWSY")),
            "no by-name answer: {two:?}",
        );
        assert!(
            two.iter().any(|name| name == "PRUE AA-A H1"),
            "no sector answer: {two:?}",
        );
    }

    /// The words of a query may come in any order, and one may be wrong
    ///
    /// **Which is how a reader holds a name.** A sector they have been to
    /// and a boxel code off a screenshot, or the two words of a sector the
    /// wrong way round, or a letter of it mistyped — and a search matching
    /// a run of bytes answered every one of those with nothing while
    /// holding two hundred million names spelled that way.
    ///
    /// The slack is a word's own, [`crate::procedural::slack`]: three
    /// letters are matched exactly, because one edit on three reaches a
    /// quarter of the alphabet and says nothing about which was meant.
    #[test]
    fn a_query_is_matched_word_by_word_in_any_order() {
        let dir = Scratch::new("out-of-order");
        let entries = vec![
            entry(1_038_034_644, "PRUE EAEWSY NR-W E1-0", 1.0),
            entry(96_076_086, "SIDGIO AA-A G1", 2.0),
            entry(10, "NEW SOL", 3.0),
        ];
        published(&dir.0, &entries);
        let names = Names::open(&dir.0).expect("the table opens");
        let named = |query: &str| -> Vec<String> {
            names
                .matching(query, 25)
                .into_iter()
                .map(|entry| entry.name.into_string())
                .collect()
        };

        // Both words of a derived name's sector, either way round.
        assert_eq!(named("PRUE EAEWSY"), ["PRUE EAEWSY NR-W E1-0"]);
        assert_eq!(named("EAEWSY PRUE"), ["PRUE EAEWSY NR-W E1-0"]);
        // And a stored name, the same way.
        assert_eq!(named("SOL NEW"), ["NEW SOL"]);
        // A word mistyped, where it is long enough to be worth guessing
        // at: `EAEWSY` is six letters and `SIDGIO` is six.
        assert_eq!(named("EAEWSX PRUE"), ["PRUE EAEWSY NR-W E1-0"]);
        assert_eq!(named("SIDGIP"), ["SIDGIO AA-A G1"]);
        // A prefix is a match, that being what a search is.
        assert_eq!(named("PRU EAEWSY"), ["PRUE EAEWSY NR-W E1-0"]);
        // But three letters are matched exactly, so a wrong one answers
        // nothing rather than a quarter of the alphabet.
        assert!(named("PRX EAEWSY").is_empty());
        // And a word nothing holds still answers nothing, however the rest
        // of the query reads.
        assert!(named("PRUE NOWHERE").is_empty());
    }

    /// A boxel code is built, not matched, and a given name may be
    /// misspelled
    ///
    /// The two roads a derived galaxy needs and a stored one does not.
    /// `EUQ YE-Q` answered nothing before: the sector road walks a
    /// sector's rows in name order and the `YE-Q` boxels are a hundred
    /// thousand rows down them, so the code has to be read as the
    /// coordinates it is. And a misspelled *given* name has no vocabulary
    /// to be offered from — the dictionary holds sectors — so the only
    /// answer is the text, a word at a time.
    #[test]
    fn a_code_is_built_and_a_stored_name_may_be_misspelled() {
        let dir = Scratch::new("coded");
        let entries = vec![
            entry(1_038_034_644, "PRUE EAEWSY NR-W E1-0", 1.0),
            entry(96_076_086, "SIDGIO AA-A G1", 2.0),
            entry(10, "ACHENAR", 3.0),
        ];
        published(&dir.0, &entries);
        let names = Names::open(&dir.0).expect("the table opens");
        let named = |query: &str| -> Vec<String> {
            names
                .matching(query, 25)
                .into_iter()
                .map(|entry| entry.name.into_string())
                .collect()
        };

        // The sector word and the code, which no by-name order reaches.
        assert_eq!(named("EAEWSY NR-W"), ["PRUE EAEWSY NR-W E1-0"]);
        // And the whole tail, which names one address.
        assert_eq!(named("PRUE EAEWSY NR-W E1-0"), ["PRUE EAEWSY NR-W E1-0"]);
        // A code naming a boxel nothing is in answers nothing, the road
        // asking the table rather than trusting the arithmetic.
        assert!(named("EAEWSY AA-B").is_empty());

        // A given name nearly spelled, which only the text can answer.
        assert_eq!(named("ACHENR"), ["ACHENAR"]);
        // And the code is never fuzzed: a wrong letter there is a
        // different boxel rather than a near miss.
        assert!(named("EAEWSY NR-X").is_empty());
    }

    /// One character is not searched, and a name is still resolvable by it
    ///
    /// Both halves have to hold it: the delta is walked with the same
    /// needle, so a guard on the base alone would answer a one-character
    /// query with whatever the feed had lately named.
    #[test]
    fn a_query_shorter_than_the_floor_is_not_searched() {
        let dir = Scratch::new("floor");
        published(&dir.0, &[entry(1, "SOL", 1.0), entry(2, "S", 2.0)]);

        let mut names = Names::open(&dir.0).expect("the table opens");
        assert!(names.name(entry(3, "SIRIUS", 3.0)));

        assert!(names.matching("S", 25).is_empty());
        assert_eq!(
            names
                .matching("SI", 25)
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            ["SIRIUS"]
        );

        // A system really named `S` is still an endpoint a route can name.
        assert_eq!(names.address_of("S"), Some(2));
    }

    /// A name said twice is one row, and the later word wins.
    #[test]
    fn a_system_named_twice_is_one_row() {
        let dir = Scratch::new("twice");
        let entries = vec![
            entry(1, "OLD NAME", 1.0),
            entry(2, "OTHER", 2.0),
            entry(1, "NEW NAME", 9.0),
        ];
        assert_eq!(published(&dir.0, &entries), 2);

        let table = Table::open(&dir.0).expect("the table opens");
        table.audit().expect("a table a build just wrote");
        assert_eq!(table.name_at(0), "NEW NAME");
        assert_eq!(table.row_named("OLD NAME"), None);
    }

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

    /// Folding the log into the base leaves the same table, with no log.
    #[test]
    fn a_fold_keeps_the_table_and_drops_the_log() {
        let dir = Scratch::new("fold");
        published(&dir.0, &[entry(1, "SOL", 1.0), entry(2, "ACRUX", 2.0)]);
        let mut names = Names::open(&dir.0).expect("the table opens");
        names.name(entry(3, "NEW", 3.0));
        names.name(entry(1, "SOL RENAMED", 1.0));
        names.unname(2);
        names.publish(&dir.0).expect("a publish");

        assert_eq!(compact(&dir.0).expect("a fold"), 2);
        assert!(!names_delta_path(&dir.0).exists(), "the log is gone");

        let read = Names::open(&dir.0).expect("the table re-opens");
        read.base().audit().expect("a base a fold just wrote");
        assert!(read.delta().is_empty());
        assert_eq!(read.len(), 2);
        assert_eq!(read.name_of(1).as_deref(), Some("SOL RENAMED"));
        assert_eq!(read.name_of(2), None);
        assert_eq!(read.name_of(3).as_deref(), Some("NEW"));
        assert_eq!(read.address_of("SOL RENAMED"), Some(1));
    }

    /// A log that has grown in bytes without growing in addresses still
    /// folds.
    ///
    /// The feed never ends, so every road to a log that is never folded has
    /// to be closed. Counting addresses does not close this one: a system
    /// the log already mentions that is renamed again appends a row and
    /// leaves the count where it was.
    #[test]
    fn a_log_that_grew_only_in_bytes_still_folds() {
        let one = Delta {
            said: HashMap::from([(1, Said::Named(entry(1, "SOL", 1.0)))]),
            read: FOLD_BYTES,
            ..Delta::default()
        };
        assert_eq!(one.len(), 1, "one address, and a long file");
        assert!(one.worth_folding());

        let short = Delta { read: FOLD_BYTES - 1, ..one.clone() };
        assert!(!short.worth_folding());
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

    /// A generation is swapped by one rename, and the one before it is
    /// swept.
    #[test]
    fn a_republish_sweeps_the_generation_before_it() {
        let dir = Scratch::new("generations");
        published(&dir.0, &[entry(1, "SOL", 1.0)]);
        assert!(generation_dir(&dir.0, 0).exists());

        published(&dir.0, &[entry(1, "SOL", 1.0), entry(2, "ACRUX", 2.0)]);
        assert!(generation_dir(&dir.0, 1).exists());
        assert!(!generation_dir(&dir.0, 0).exists(), "the old generation");

        let table = Table::open(&dir.0).expect("the table opens");
        assert_eq!(table.len(), 2);
    }

    /// A build abandoned part way leaves the table that stood.
    #[test]
    fn an_abandoned_build_leaves_the_table_alone() {
        let dir = Scratch::new("abandon");
        published(&dir.0, &[entry(1, "SOL", 1.0)]);

        let mut writer = Writer::writing(&dir.0).expect("a writer");
        writer.push(entry(2, "SOMETHING ELSE", 2.0)).expect("a push");
        writer.abandon().expect("an abandon");

        let table = Table::open(&dir.0).expect("the table still opens");
        assert_eq!(table.len(), 1);
        assert_eq!(table.name_at(0), "SOL");
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

    /// The by-name radix splits a bucket that will not fit and still writes
    /// one order.
    #[test]
    fn the_by_name_sort_splits_a_bucket_it_cannot_hold() {
        let dir = Scratch::new("radix");
        // Names sharing a long prefix, so the first pass buckets them all
        // together and the split has to go byte by byte.
        let mut entries = Vec::new();
        for n in 0..2_000i64 {
            entries.push(entry(n, &format!("PRAEA EUQ YE-Q D5-{n:05}"), 0.0));
        }
        entries.push(entry(9_999, "SOL", 0.0));
        published(&dir.0, &entries);

        let table = Table::open(&dir.0).expect("the table opens");
        table.audit().expect("a radix-sorted by-name index");
        assert_eq!(table.len(), 2_001);
        assert_eq!(
            table
                .row_named("PRAEA EUQ YE-Q D5-01234")
                .map(|at| table.address_at(at)),
            Some(1_234),
        );
        let found = table.rows_starting("PRAEA EUQ YE-Q D5-0000", 25);
        assert_eq!(found.len(), 10);
    }
}
