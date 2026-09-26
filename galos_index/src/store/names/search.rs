//! Finding a name: by prefix, by word, near a place, and by the sectors a
//! derived name is spelled from.
//!
//! What the search box and a route endpoint ask. Most of it is a binary
//! search over the by-name permutation; a derived name, which nothing
//! stored, is found through the sector dictionary instead.

use super::Names;
use super::format::{SWEEP, placed};
use super::table::Table;
use crate::core::name::SystemName;
use crate::records::NameEntry;

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

/// How many candidates a road is asked for per row wanted, where the query
/// has more than one word
///
/// The roads match one word and the sieve checks the rest, so a road asked
/// for exactly what the caller wants would answer a two-word query with
/// almost nothing. Sixty-four is enough for a query whose words sit
/// together — which is what a name is — and the cap below is what keeps a
/// query whose words do not from reading a sector's whole run.
pub(super) const SIFT: usize = 64;

/// And at most this many candidates however large the limit.
pub(super) const SIFTED: usize = 4_096;

/// The words of a query, upper case and in the order typed.
pub(super) fn words_of(needle: &str) -> Vec<&str> {
    needle.split(' ').filter(|word| !word.is_empty()).collect()
}

/// Whether `name` holds every one of `words`, in any order
///
/// The order they were typed in is not the order they have to appear in,
/// which is the whole point; a word start each, for
/// [`Table::rows_holding`]'s reason — `SOL` answering every `RESOLUTE` is
/// noise dressed as an answer.
///
/// A word matches at the start of one of the name's words, or near enough to it
/// — [`crate::core::procedural::matches_word`], which is the same rule the
/// sector vocabulary is searched by, so the sieve cannot throw away a row the
/// road in front of it just found.
pub(super) fn holds_words(name: &str, words: &[&str]) -> bool {
    words.iter().all(|word| {
        name.split(' ')
            .any(|held| crate::core::procedural::matches_word(word, held))
    })
}

/// Where `needle` first sits in `hay`, or [`None`].
///
/// Hand-rolled rather than a dependency: the sweep it serves reads 128 MB
/// of a real table, so what matters is that the inner loop is a byte
/// compare over a mapping and that a miss on the first byte costs one.
pub(super) fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
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

impl Names {
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
    /// 3. **The sector dictionary**, for the 97.4 % of names nothing stored. A
    ///    derived name's words are a sector and a boxel code, so a query
    ///    matching a sector *mid-name* — `EUQ` for `PRAEA EUQ YE-Q D5-0` — is
    ///    answered by asking
    ///    [`sectors_holding_all`](crate::core::procedural::sectors_holding_all)
    ///    which sectors hold that word and walking each one's run of the
    ///    by-name order. 11,662 sectors and 192 KB, compiled in, so finding the
    ///    sector costs microseconds and the rows come back through the same
    ///    prefix search as ever.
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
        // Room for the sieve to throw candidates away. A road asked for `limit`
        // rows and filtered would answer a two-word query with almost nothing;
        // asked for this many it has something to filter. Bounded, because the
        // roads are a scan and a sector's run is millions of rows: a query
        // whose words are spread thinly through one sector is the case
        // [`crate::core::procedural`]'s coordinate search is for, and is not
        // this.
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
        // [`crate::core::procedural::addresses_in`].
        if let Some(coded) = crate::core::procedural::coded(&words) {
            // The words that are not coordinates name the sector; where
            // there are none, the query is a boxel of *every* sector and
            // only where the reader is looking can say which to try.
            let named: Vec<&str> = words
                .iter()
                .copied()
                .filter(|word| !crate::core::procedural::is_coordinate(word))
                .collect();
            let sectors: Vec<u32> = match (named.is_empty(), near) {
                (true, Some(near)) => crate::core::procedural::sectors_near(
                    near,
                    crate::core::procedural::TRIED_SECTORS,
                ),
                (true, None) => Vec::new(),
                _ => crate::core::procedural::sectors_holding_all(
                    &named, near, limit,
                )
                .into_iter()
                .filter_map(|(_, sector)| {
                    crate::core::procedural::sector_named(sector)
                        .map(crate::core::procedural::sector_key)
                })
                .collect(),
            };
            let each = (limit - found.len()).div_ceil(sectors.len().max(1));
            for key in sectors {
                let mut kept = 0;
                for address in crate::core::procedural::addresses_in(
                    key,
                    &coded,
                    crate::core::procedural::TRIED,
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
            crate::core::procedural::sectors_holding_all(&words, near, limit);
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
}

impl Table {
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
    /// asks [`crate::core::procedural`] and expands through
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
    /// The bound is the word's own ([`crate::core::procedural::matches_word`]),
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
                                crate::core::procedural::edits_to(word, part)
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
    pub(super) fn by_name(&self) -> &[u32] {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::names::fixtures::{Scratch, entry, published};

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
    /// The slack is a word's own, [`crate::core::procedural::slack`]: three
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
}
