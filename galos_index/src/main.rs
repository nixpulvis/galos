//! Command-line tools for the galaxy index files.
//!
//! ```sh
//! cargo run -p galos_index -- info .galos_index
//! cargo run -p galos_index -- diff .index/from_dump .index/from_db
//! ```
//!
//! Read-only and database-free: everything here reads the cell records the
//! builder wrote, never Postgres. `info` says what one directory holds;
//! `diff` says whether two of them are the same derivation, which is the
//! question a dump-built index and a database-built index of the same
//! galaxy are there to answer.

use clap::{Parser, Subcommand};
use galos_index::geometry::MAX_LEVEL;
use galos_index::{
    source, store, Bodies, Cell, Index, NameEntry, PopulatedSystem, Published,
    SystemBoost, SystemReach,
};
use serde::de::DeserializeOwned;
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

/// Read and inspect galaxy index files.
#[derive(Parser)]
#[command(name = "galos-index", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarise a built index directory: its shape and the galaxy's summed light.
    Info {
        /// The index directory to read.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
    },
    /// Compare two built index directories: are they the same derivation?
    Diff {
        /// The two index directories to compare.
        a: PathBuf,
        b: PathBuf,
        /// Compare the body files as well, which is a file a scanned
        /// system and hours of them over a galaxy.
        #[arg(long)]
        bodies: bool,
        /// Name the rows that differ, which holds both names tables in
        /// memory: a galaxy's worth is tens of gigabytes.
        #[arg(long)]
        detail: bool,
        /// How many differing rows to name before counting the rest.
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
}

fn main() {
    match Cli::parse().command {
        Command::Info { dir } => info(&dir),
        Command::Diff { a, b, bodies, detail, limit } => {
            diff(&a, &b, Compare { bodies, detail, limit })
        }
    }
}

/// Print a summary of a built index directory.
fn info(dir: &Path) {
    let index = match Index::read(dir) {
        Ok(index) => index,
        Err(e) => {
            eprintln!("cannot read index at {}: {e}", dir.display());
            std::process::exit(1);
        }
    };
    if index.is_empty() {
        println!("{}: empty index", dir.display());
        return;
    }

    // Walk the cells once for the shape.
    let mut leaves = 0usize;
    let mut deepest = 0u8;
    let mut per_level = [0usize; MAX_LEVEL as usize + 1];
    let mut largest_leaf = 0u64;
    let mut owned = 0u64;
    for cell in index.cells() {
        if cell.is_leaf() {
            leaves += 1;
            largest_leaf = largest_leaf.max(cell.slice_len());
        }
        deepest = deepest.max(cell.id.level);
        per_level[cell.id.level as usize] += 1;
        owned += cell.slice_len();
    }
    let cells = index.len();
    let root = index.root().expect("a non-empty index has a root");
    let systems = root.aggregate.count();

    println!("{}", dir.display());
    println!(
        "  cells         {cells}  ({leaves} leaves, {} internal)",
        cells - leaves
    );
    println!("  levels        0..{deepest}");
    println!("  systems       {systems}");
    if owned == systems {
        println!("  owned         {owned}  (sum of cell slices, matches)");
    } else {
        println!("  owned         {owned}  (MISMATCH: expected {systems})");
    }
    println!("  largest leaf  {largest_leaf} systems");
    match root.aggregate.m_min() {
        Some(m) => println!("  brightest     M_abs {m:.2}"),
        None => println!("  brightest     none"),
    }
    println!("  total flux    {:.3e}  (relative)", root.aggregate.total_flux());

    // On-disk footprint, straight off the filesystem.
    if let Ok(meta) = std::fs::metadata(dir.join(store::INDEX_FILE)) {
        print!("  on disk       index.bin ({:.2} MB)", mib(meta.len()));
        let (count, bytes) = payload_footprint(&dir.join(store::PAYLOAD_DIR));
        print!(", {count} payload files ({:.2} MB)", mib(bytes));
        println!();
    }

    println!("  cells per level:");
    for (level, count) in per_level.iter().enumerate() {
        if *count > 0 {
            println!("    L{level:<2}  {count}");
        }
    }
}

/// How many payload files a directory holds and how many bytes they take.
///
/// Payloads are sharded one directory deep, and a directory published
/// before the sharding still has them loose, so both are walked.
fn payload_footprint(dir: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return (0, 0) };
    let mut count = 0;
    let mut bytes = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            let (n, b) = payload_footprint(&entry.path());
            count += n;
            bytes += b;
        } else {
            count += 1;
            bytes += meta.len();
        }
    }
    (count, bytes)
}

/// Bytes as mebibytes.
fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// What a comparison was asked to look at.
struct Compare {
    /// Compare the body files too.
    bodies: bool,
    /// Name differing rows rather than counting them.
    detail: bool,
    /// How many of those to name.
    limit: usize,
}

/// What a part of the comparison found.
///
/// Three answers rather than two, because a float that moved in its last
/// bit is not a directory that disagrees: see [`aggregates`].
#[derive(Clone, Copy, PartialEq)]
enum Verdict {
    Same,
    Differ,
}

impl Verdict {
    /// Whichever of the two is the worse news.
    fn and(self, other: Verdict) -> Verdict {
        match self == Verdict::Same && other == Verdict::Same {
            true => Verdict::Same,
            false => Verdict::Differ,
        }
    }
}

/// How far apart two summed `f64` aggregates may be and still be the same
/// derivation.
///
/// A cell's flux is summed in the order its systems were merged, which is
/// a `HashMap` iteration order, so two honest builds of one galaxy differ
/// in the last bits — the crate's own cross-build test compares `index.bin`
/// by its integers alone for exactly this reason. Anything past this is a
/// difference in what was summed rather than in what order.
const DRIFT: f64 = 1e-9;

/// Compare two built index directories.
///
/// Exit code is the answer: 0 they are the same derivation, 1 they differ,
/// 2 one of them could not be read. That is what makes it scriptable, and
/// a 610 GB import checked against a database-built index of the same file
/// is what it is for.
///
/// What is compared, and how:
///
/// - the cell tree, by its integer columns — the cells present, `rank_lo`,
///   `rank_hi`, `child_mask` and each aggregate's count;
/// - the aggregates' summed light, to [`DRIFT`];
/// - every cell's payload, byte for byte, which is the systems it owns and
///   the order it owns them in;
/// - the names table, by count and an order-independent digest, read a
///   chunk at a time so a galaxy's worth is never held;
/// - `populated.bin`, `reaches.bin` and `boosts.bin`, row by row, absent
///   and empty told apart;
/// - the body files, on `--bodies`, which is a file a scanned system.
fn diff(a: &Path, b: &Path, how: Compare) {
    let left = open(a);
    let right = open(b);
    println!("{} vs {}", a.display(), b.display());

    let (verdict, shared) = cells(&left, &right, how.limit);
    let verdict = verdict
        .and(aggregates(&shared))
        .and(payloads(a, b, &shared, how.limit))
        .and(names(a, b, &how))
        .and(tables(a, b, &how));
    let verdict = match how.bodies {
        true => verdict.and(bodies(a, b, how.limit)),
        false => {
            println!("  bodies        not compared (pass --bodies)");
            verdict
        }
    };

    match verdict {
        Verdict::Same => println!("the same derivation"),
        Verdict::Differ => {
            println!("the two directories differ");
            std::process::exit(1);
        }
    }
}

/// Read a directory's index file, or say which one could not be read.
fn open(dir: &Path) -> Index {
    match Index::read(dir) {
        Ok(index) => index,
        Err(e) => {
            eprintln!("cannot read index at {}: {e}", dir.display());
            std::process::exit(2);
        }
    }
}

/// The cells, by the columns that do not drift.
///
/// Answers the cells both sides hold, for everything downstream to compare
/// over: a cell only one side has is already a difference and has no pair
/// to be read against.
fn cells(a: &Index, b: &Index, limit: usize) -> (Verdict, Vec<(Cell, Cell)>) {
    let keyed = |index: &Index| -> BTreeMap<(u8, u64), Cell> {
        index.cells().map(|it| ((it.id.level, it.id.morton()), *it)).collect()
    };
    let left = keyed(a);
    let right = keyed(b);

    let only_left: Vec<_> =
        left.keys().filter(|it| !right.contains_key(*it)).collect();
    let only_right: Vec<_> =
        right.keys().filter(|it| !left.contains_key(*it)).collect();
    let shared: Vec<(Cell, Cell)> = left
        .iter()
        .filter_map(|(key, cell)| right.get(key).map(|it| (*cell, *it)))
        .collect();

    match only_left.is_empty() && only_right.is_empty() {
        true => println!("  cells         {} in both", shared.len()),
        false => {
            println!(
                "  cells         {} in both, {} only in A, {} only in B",
                shared.len(),
                only_left.len(),
                only_right.len(),
            );
            for (level, morton) in
                only_left.iter().chain(&only_right).take(limit)
            {
                println!("                  L{level} {morton:016x}");
            }
        }
    }

    // The integer columns: what a cell holds, how much of it, and where in
    // the ranking its slice sits.
    let mut differing = Vec::new();
    for (left, right) in &shared {
        let same = left.rank_lo == right.rank_lo
            && left.rank_hi == right.rank_hi
            && left.child_mask == right.child_mask
            && left.aggregate.count() == right.aggregate.count();
        if !same {
            differing.push(*left);
        }
    }
    match differing.is_empty() {
        true => println!("  columns       identical"),
        false => {
            println!("  columns       {} cells differ", differing.len());
            for cell in differing.iter().take(limit) {
                println!(
                    "                  L{} {:016x}",
                    cell.id.level,
                    cell.id.morton(),
                );
            }
        }
    }

    let verdict = match only_left.is_empty()
        && only_right.is_empty()
        && differing.is_empty()
    {
        true => Verdict::Same,
        false => Verdict::Differ,
    };
    (verdict, shared)
}

/// The summed light, which is allowed to drift and not to move.
fn aggregates(shared: &[(Cell, Cell)]) -> Verdict {
    let mut worst = 0.0f64;
    let mut faintest = 0.0f32;
    for (left, right) in shared {
        let (x, y) =
            (left.aggregate.total_flux(), right.aggregate.total_flux());
        let scale = x.abs().max(y.abs());
        if scale > 0.0 {
            worst = worst.max((x - y).abs() / scale);
        }
        match (left.aggregate.m_min(), right.aggregate.m_min()) {
            (Some(x), Some(y)) => faintest = faintest.max((x - y).abs()),
            (None, None) => {}
            // One cell owns something bright and the other owns nothing:
            // the columns above have already called that a difference.
            _ => faintest = f32::INFINITY,
        }
    }
    match worst <= DRIFT && faintest == 0.0 {
        true => {
            println!(
                "  aggregates    agree (flux within {worst:.1e}, same M_abs)"
            );
            Verdict::Same
        }
        false => {
            println!(
                "  aggregates    flux differs by {worst:.1e}, M_abs by \
                 {faintest:.3}",
            );
            Verdict::Differ
        }
    }
}

/// Every shared cell's payload, byte for byte.
///
/// Read through [`Index::read_payload`], so a directory part way through
/// the shard migration answers off whichever path it has.
fn payloads(
    a: &Path,
    b: &Path,
    shared: &[(Cell, Cell)],
    limit: usize,
) -> Verdict {
    let mut differing = Vec::new();
    for (cell, _) in shared {
        let left = Index::read_payload(a, cell.id);
        let right = Index::read_payload(b, cell.id);
        match (left, right) {
            (Ok(left), Ok(right)) if left == right => {}
            (Ok(_), Ok(_)) => differing.push(cell.id),
            (left, right) => {
                if let Err(e) = left {
                    eprintln!("cannot read a payload of {}: {e}", a.display());
                    std::process::exit(2);
                }
                if let Err(e) = right {
                    eprintln!("cannot read a payload of {}: {e}", b.display());
                    std::process::exit(2);
                }
            }
        }
    }
    match differing.is_empty() {
        true => {
            println!("  payloads      {} identical", shared.len());
            Verdict::Same
        }
        false => {
            println!(
                "  payloads      {} of {} differ",
                differing.len(),
                shared.len(),
            );
            for id in differing.iter().take(limit) {
                println!(
                    "                  L{} {:016x}",
                    id.level,
                    id.morton()
                );
            }
            Verdict::Differ
        }
    }
}

/// The names table, a chunk at a time.
///
/// By count and digest rather than by holding the table: 200 M entries is
/// tens of gigabytes on each side, and which chunk a system landed in is
/// append order rather than an invariant, so the digest is over the entries
/// and not over the files. `--detail` is the road that names the rows, and
/// it is the one that holds both tables.
fn names(a: &Path, b: &Path, how: &Compare) -> Verdict {
    let left = digest(a).unwrap_or_else(|e| fatal(a, e));
    let right = digest(b).unwrap_or_else(|e| fatal(b, e));
    if left == right {
        println!("  names         {} entries, identical", left.0);
        return Verdict::Same;
    }

    println!("  names         {} entries in A, {} in B", left.0, right.0);
    match how.detail {
        true => {
            let left = source::read_names(a).unwrap_or_else(|e| fatal(a, e));
            let right = source::read_names(b).unwrap_or_else(|e| fatal(b, e));
            rows(
                "names rows",
                Some(left),
                Some(right),
                |it: &NameEntry| it.address,
                how,
            );
        }
        false => println!("                  pass --detail to name the rows"),
    }
    Verdict::Differ
}

/// A names table's count and an order-independent digest of its entries.
///
/// Chunks are numbered from zero with no manifest, so the first one missing
/// is the end of the table.
fn digest(dir: &Path) -> io::Result<(usize, u64, u64)> {
    let (mut count, mut sum, mut xor) = (0usize, 0u64, 0u64);
    for chunk in 0.. {
        let path = source::names_chunk_path(dir, chunk);
        let entries: Vec<NameEntry> = match source::read_meta(&path) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => return Err(e),
        };
        for entry in &entries {
            let mut hasher = DefaultHasher::new();
            entry.address.hash(&mut hasher);
            entry.name.hash(&mut hasher);
            for axis in entry.position {
                axis.to_bits().hash(&mut hasher);
            }
            let hash = hasher.finish();
            count += 1;
            sum = sum.wrapping_add(hash);
            xor ^= hash;
        }
    }
    Ok((count, sum, xor))
}

/// The three tables a record can fill.
///
/// Each is written sorted by address, so equal content is equal bytes and
/// a row comparison says the same thing as a byte one — but a row
/// comparison can say which system.
fn tables(a: &Path, b: &Path, how: &Compare) -> Verdict {
    let populated = rows(
        "populated",
        table::<PopulatedSystem>(a, &source::populated_path(a)),
        table::<PopulatedSystem>(b, &source::populated_path(b)),
        |it: &PopulatedSystem| it.address,
        how,
    );
    let reaches = rows(
        "reaches",
        table::<SystemReach>(a, &source::reaches_path(a)),
        table::<SystemReach>(b, &source::reaches_path(b)),
        |it: &SystemReach| it.address,
        how,
    );
    let boosts = rows(
        "boosts",
        table::<SystemBoost>(a, &source::boosts_path(a)),
        table::<SystemBoost>(b, &source::boosts_path(b)),
        |it: &SystemBoost| it.address,
        how,
    );
    populated.and(reaches).and(boosts)
}

/// One table, or [`None`] where the directory does not hold it.
///
/// An absent table is not an empty one: it says this index cannot tell,
/// where an empty one says the galaxy has none.
fn table<T: DeserializeOwned>(dir: &Path, path: &Path) -> Option<Vec<T>> {
    match source::read_meta(path) {
        Ok(rows) => Some(rows),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => fatal(dir, e),
    }
}

/// Compare two tables of rows keyed by address.
fn rows<T: PartialEq>(
    label: &str,
    a: Option<Vec<T>>,
    b: Option<Vec<T>>,
    key: impl Fn(&T) -> i64,
    how: &Compare,
) -> Verdict {
    let (mut left, mut right) = match (a, b) {
        (None, None) => {
            println!("  {label:<13} absent from both");
            return Verdict::Same;
        }
        (Some(_), None) => {
            println!("  {label:<13} only A holds one");
            return Verdict::Differ;
        }
        (None, Some(_)) => {
            println!("  {label:<13} only B holds one");
            return Verdict::Differ;
        }
        (Some(left), Some(right)) => (left, right),
    };
    left.sort_by_key(&key);
    right.sort_by_key(&key);

    // Two sorted runs walked together: a row on one side and not the other
    // is a missing system, and one on both that is not the same row is a
    // system the two derivations say different things about.
    let (mut i, mut j) = (0usize, 0usize);
    let (mut only_left, mut only_right) = (0usize, 0usize);
    let mut differing = Vec::new();
    while i < left.len() && j < right.len() {
        let (x, y) = (key(&left[i]), key(&right[j]));
        match x.cmp(&y) {
            std::cmp::Ordering::Less => {
                only_left += 1;
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                only_right += 1;
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                if left[i] != right[j] {
                    differing.push(x);
                }
                i += 1;
                j += 1;
            }
        }
    }
    only_left += left.len() - i;
    only_right += right.len() - j;

    if only_left == 0 && only_right == 0 && differing.is_empty() {
        println!("  {label:<13} {} rows, identical", left.len());
        return Verdict::Same;
    }
    println!(
        "  {label:<13} {} rows in A, {} in B: {} only in A, {} only in B, \
         {} differ",
        left.len(),
        right.len(),
        only_left,
        only_right,
        differing.len(),
    );
    if how.detail {
        for address in differing.iter().take(how.limit) {
            println!("                  {address}");
        }
    }
    Verdict::Differ
}

/// The body files, which is a file a scanned system.
fn bodies(a: &Path, b: &Path, limit: usize) -> Verdict {
    let left = Published::new(a).scanned();
    let right = Published::new(b).scanned();

    let mut differing = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut only_left, mut only_right) = (0usize, 0usize);
    while i < left.len() && j < right.len() {
        match left[i].cmp(&right[j]) {
            std::cmp::Ordering::Less => {
                only_left += 1;
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                only_right += 1;
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                let address = left[i];
                let x = source::read_bodies(a, address)
                    .unwrap_or_else(|e| fatal(a, e));
                let y = source::read_bodies(b, address)
                    .unwrap_or_else(|e| fatal(b, e));
                if x != y {
                    differing.push(address);
                }
                i += 1;
                j += 1;
            }
        }
    }
    only_left += left.len() - i;
    only_right += right.len() - j;

    if only_left == 0 && only_right == 0 && differing.is_empty() {
        println!("  bodies        {} files, identical", left.len());
        return Verdict::Same;
    }
    println!(
        "  bodies        {} files in A, {} in B: {} only in A, {} only in \
         B, {} differ",
        left.len(),
        right.len(),
        only_left,
        only_right,
        differing.len(),
    );
    for address in differing.iter().take(limit) {
        println!("                  {address}");
    }
    Verdict::Differ
}

/// A directory that stopped being readable part way through a comparison.
fn fatal(dir: &Path, e: io::Error) -> ! {
    eprintln!("cannot read {}: {e}", dir.display());
    std::process::exit(2);
}
