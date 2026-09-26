//! Writing a generation: the rows sorted, the sections written, and the
//! head renamed over.
//!
//! A build pushes names in whatever order it reads them; the rows go to a
//! file and are sorted externally, so the galaxy is never held. A fold of
//! the delta into the base is the same road, taken from the table that
//! stands.

use super::Names;
use super::format::{BUCKET_BYTES, HEAD, MAGIC, SPAN, Text, VERSION, refused};
use crate::format::layout::{
    ADDR_FILE, BYNAME_FILE, EXCEPTION_FILE, SPAN_FILE, TEXT_FILE,
    generation_dir, names_delta_path, names_dir, names_head_path, scratch_dir,
};
use crate::format::rows;
use crate::format::rows::Sheet;
use crate::records::NameEntry;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// The names table written straight to disk, sorted, without ever being in
/// memory.
///
/// What a build uses in place of [`Names`]: it names each system once, in
/// whatever order it reads them, and never looks one up. The rows go to a file
/// as they arrive, the file is sorted by address externally
/// ([`crate::format::rows`]), and the sections are written from the sorted rows
/// in one pass.
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
    pub(super) dir: PathBuf,
    pub(super) scratch: PathBuf,
    pub(super) rows: Sheet,
    pub(super) named: usize,
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
    /// [`crate::format::rows`]'s one rule: the last row an address has wins.
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
    /// [`crate::ops::migrate::migrate`] first.
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
    pub(super) fn take_chunks(&mut self, dir: &Path) -> io::Result<usize> {
        let mut taken = 0;
        for chunk in legacy_chunks(dir)? {
            let entries: Vec<NameEntry> =
                crate::format::msgpack::read_meta(&chunk)?;
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
/// the same refusal [`Table::open`](super::Table::open) makes.
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
pub(super) fn write_base(
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
pub(super) fn write_sections(
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
            if !crate::core::procedural::spells(entry.address, &entry.name) {
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
pub(super) fn span_bytes(at: usize) -> [u8; SPAN] {
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
pub(super) fn write_by_name(
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
pub(super) fn write_pair(
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
pub(super) fn emit_by_name(
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
pub(super) struct Pair {
    pub(super) from: u32,
    pub(super) to: u32,
    pub(super) row: u32,
}

impl Pair {
    pub(super) fn span(&self) -> std::ops::Range<usize> {
        self.from as usize..self.to as usize
    }
}

/// Read a bucket: its name bytes in one block, and a pair per row.
pub(super) fn read_pairs(path: &Path) -> io::Result<(Vec<u8>, Vec<Pair>)> {
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
pub(super) fn write_head(
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
pub(super) fn live_generation(dir: &Path) -> io::Result<Option<u64>> {
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
pub(super) fn sweep_generations(dir: &Path, live: u64) -> io::Result<()> {
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
        let entries: Vec<NameEntry> = crate::format::msgpack::read_meta(chunk)?;
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
pub(super) fn legacy_chunks(dir: &Path) -> io::Result<Vec<PathBuf>> {
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
    use crate::store::names::delta::{Delta, DeltaRow, FOLD_BYTES};
    use crate::store::names::fixtures::{Scratch, entry, published};
    use crate::store::names::table::Table;
    use std::collections::HashMap;

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
            said: HashMap::from([(1, DeltaRow::Named(entry(1, "SOL", 1.0)))]),
            read: FOLD_BYTES,
            ..Delta::default()
        };
        assert_eq!(one.len(), 1, "one address, and a long file");
        assert!(one.worth_folding());

        let short = Delta { read: FOLD_BYTES - 1, ..one.clone() };
        assert!(!short.worth_folding());
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
