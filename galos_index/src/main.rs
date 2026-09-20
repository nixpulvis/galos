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
    Bodies, Cell, Index, NameEntry, PopulatedSystem, Published, SystemBoost,
    SystemReach, source, store,
};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

/// Read and inspect galaxy index files.
#[derive(Parser)]
#[command(name = "galos-index", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Clear a lock left behind by a builder that was killed, and take it.
    ///
    /// The refusal names the pid holding the directory. Check it first: a
    /// lock cleared while its builder is merely slow to answer is two
    /// writers over one directory, which is what the lock is for.
    #[arg(long, global = true)]
    force_lock: bool,
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
    /// Walk a directory's loose body files into the packed shard files.
    Pack {
        /// The index directory to pack.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
    },
    /// Remove the payloads of cells the index no longer names.
    Sweep {
        /// The index directory to sweep.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
        /// Delete them. Without this the orphans are only counted.
        #[arg(long)]
        apply: bool,
    },
    /// Reclaim the body shards' dead records, which a re-import leaves one
    /// of for every system it rewrote.
    SweepBodies {
        /// The index directory to compact.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
        /// Rewrite them. Without this the dead bytes are only weighed.
        #[arg(long)]
        apply: bool,
    },
    /// Fold a directory's MessagePack names chunks into the mapped table.
    FoldNames {
        /// The index directory to fold.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
    },
    /// Bring a directory up to the format this build reads, in place.
    Upgrade {
        /// The index directory to upgrade.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
    },
    /// Write the sector dictionary `galos_index::procedural` derives names
    /// through, learned from a built directory.
    Sectors {
        /// The index directory to learn from.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
        /// Where to write it. `galos_index/data/sectors.csv` is the one
        /// the crate compiles in.
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Write it even where a sector the dictionary already names comes
        /// out differently, which re-spells every name dropped under it.
        #[arg(long)]
        force: bool,
    },
}

/// Leave with `code`, having dropped whatever was holding the directory
///
/// **`std::process::exit` runs no destructors**, so a command that exits
/// out of its error arm while holding [`galos_index::Lock`] leaves the lock
/// file behind and the next run refuses the directory as "already being
/// written" by a process that is gone. Reported twice in one sitting, once
/// off `upgrade` and once off `pack`.
///
/// So the lock is handed over and dropped here, on the way out. A command
/// that holds nothing passes nothing.
fn leave(lock: Option<galos_index::Lock>, code: i32) -> ! {
    drop(lock);
    std::process::exit(code)
}

fn main() {
    let cli = Cli::parse();
    let forced = cli.force_lock;
    match cli.command {
        Command::Info { dir } => info(&dir),
        Command::Diff { a, b, bodies, detail, limit } => {
            diff(&a, &b, Compare { bodies, detail, limit })
        }
        Command::Pack { dir } => pack(&dir, forced),
        Command::Sweep { dir, apply } => sweep(&dir, apply, forced),
        Command::SweepBodies { dir, apply } => {
            sweep_bodies(&dir, apply, forced)
        }
        Command::FoldNames { dir } => fold_names(&dir, forced),
        Command::Upgrade { dir } => upgrade(&dir, forced),
        Command::Sectors { dir, out, force } => {
            sectors(&dir, out.as_deref(), force)
        }
    }
}

/// Count, and on request remove, the payloads of cells the published tree
/// does not name.
///
/// What a whole-directory rebuild leaves behind: it writes its own cells
/// and knows nothing of the tree that stood before it, so the old tree's
/// are still there, referred to by nothing. Measured at 200,248 files and
/// 4.9 GB on a directory rebuilt from the database over one built from a
/// dump.
///
/// A build sweeps for itself now — `galos_index::store::sweep_payloads`,
/// run once the new index file stands — so this is for the directories
/// rebuilt before it did, and for looking before acting. Reporting is the
/// default because deleting from a served directory on a typo is not.
fn sweep(dir: &Path, apply: bool, forced: bool) {
    let lock = held(dir, forced);
    let index = match galos_index::Index::read(dir) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("{}: {err}", dir.display());
            leave(Some(lock), 1);
        }
    };
    let at = std::time::Instant::now();
    match galos_index::sweep_payloads(dir, &index, apply) {
        Ok(swept) if swept.orphans == 0 => {
            println!(
                "{}: every payload belongs to a cell of the {} the index \
                 names",
                dir.display(),
                index.len(),
            );
        }
        Ok(swept) => {
            let one = swept.orphans == 1;
            println!(
                "{}: {} payload{} {}, {}, in {:.1?}",
                dir.display(),
                swept.orphans,
                if one { "" } else { "s" },
                if one { "names no cell" } else { "name no cell" },
                match apply {
                    true => format!("removed ({})", size(swept.bytes)),
                    false => format!("holding {}", size(swept.bytes)),
                },
                at.elapsed(),
            );
            if !apply {
                println!("pass --apply to remove them");
            }
        }
        Err(err) => {
            eprintln!("{}: {err}", dir.display());
            leave(Some(lock), 1);
        }
    }
}

/// Weigh, and on request reclaim, the dead records in the body shards.
///
/// What a re-import leaves behind: the shards are append-only and a dump
/// names each system once, so a second import over the same directory
/// writes a fresh record for every system and the one behind it is dead.
/// Measured on one: `bodies/` at 301 GB, 49.8 % of it live.
///
/// A build sweeps for itself now — `galos_index::pack::sweep_bodies`, run
/// once the new index file stands — so this is for the directories
/// imported before it did, and for looking before acting. Reporting is the
/// default for the reason the cell sweep's is.
///
/// Safe to run against a directory a map is *reading*: a shard is
/// compacted whole and a reader whose data file goes out from under it
/// reads the index again. Not safe against one something is *writing*,
/// which is what [`held`] is for.
fn sweep_bodies(dir: &Path, apply: bool, forced: bool) {
    let lock = held(dir, forced);
    let at = std::time::Instant::now();
    match galos_index::pack::sweep_bodies(dir, &|| false, apply) {
        Ok(swept) if swept.shards == 0 => println!(
            "{}: every body shard's data file is live records",
            dir.display(),
        ),
        Ok(swept) => {
            let one = swept.shards == 1;
            let bytes = size(swept.bytes);
            println!(
                "{}: {} shard{} {}, in {:.1?}{}",
                dir.display(),
                swept.shards,
                if one { "" } else { "s" },
                match apply {
                    true => format!("rewritten, {bytes} reclaimed"),
                    false => format!("holding {bytes} of dead record"),
                },
                at.elapsed(),
                match swept.finished {
                    true => "",
                    false => ", and the rest were not reached",
                },
            );
            if !apply {
                println!("pass --apply to rewrite them");
            }
        }
        Err(err) => {
            eprintln!("{}: {err}", dir.display());
            leave(Some(lock), 1);
        }
    }
}

/// Bytes in the unit a person would have said them in.
fn size(bytes: u64) -> String {
    const KB: f64 = 1e3;
    let bytes = bytes as f64;
    match bytes {
        b if b >= 1e9 => format!("{:.1} GB", b / 1e9),
        b if b >= 1e6 => format!("{:.1} MB", b / 1e6),
        b if b >= KB => format!("{:.1} kB", b / KB),
        b => format!("{b:.0} bytes"),
    }
}

/// Bring a directory up to the format this build reads
///
/// What [`galos_index::store`]'s version refusal names, so an operator met
/// by "rebuild the directory" has one thing to run. It rewrites the
/// payloads without reimporting the galaxy:
///
/// The payloads written before the columns hold every field the new ones do
/// but the star kind, and that is derivable from `bodies/` — the scan
/// record the class comes from. So this joins the two and rewrites each
/// cell, where the alternative is running the importer over the dump again.
///
/// The names table comes forward too, where its version is older than the
/// one this build writes. That rewrite is what drops every name the
/// address spells — 97.4 % of a galaxy, and 3.94 GB of `text.bin` down to
/// 133 MB — and it is a whole rewrite of the base, so it is done here on
/// purpose rather than at the front of somebody's import.
///
/// Idempotent and interruptible: a cell already columnar is left alone,
/// `index.bin` is rewritten last, and a names table already at this
/// version is not touched. The bodies, the sidecars and the tree itself
/// are unchanged.
fn upgrade(dir: &Path, forced: bool) {
    let lock = held(dir, forced);
    let at = std::time::Instant::now();
    // No stop flag of its own: a run cut short by a Ctrl-C leaves the
    // directory in a state the next run takes up, `index.bin` being
    // rewritten last.
    let stop = || false;
    let mut said = |wrote: &galos_index::upgrade::Rewrote| {
        // The sweep first and the rewrite after it, which is the order they
        // happen in: a line about cells while the scan record is still
        // being read would be a line of zeroes.
        match wrote.cells == 0 && wrote.kept == 0 {
            true => {
                eprint!("\r{} systems swept, {:.0?}", wrote.swept, at.elapsed())
            }
            false => eprint!(
                "\r{} cells, {} systems, {} classed, {} already columnar, \
                 {:.0?}",
                wrote.cells,
                wrote.systems,
                wrote.classed,
                wrote.kept,
                at.elapsed(),
            ),
        }
    };

    match galos_index::upgrade::rewrite(dir, &stop, &mut said) {
        Ok(wrote) => {
            eprintln!();
            println!(
                "{} cells rewritten, {} systems, {} of them classed, \
                 {} already columnar, in {:.1?}",
                wrote.cells,
                wrote.systems,
                wrote.classed,
                wrote.kept,
                at.elapsed(),
            );
            // Only where there was a table to bring forward, which is a
            // directory built before the place rode in the published row.
            if wrote.placed > 0 {
                println!("{} supercharge rows given their place", wrote.placed,);
            }
            names_forward(dir, &lock);
        }
        Err(err) => {
            eprintln!();
            eprintln!("{}: {err}", dir.display());
            leave(Some(lock), 1);
        }
    }
}

/// Bring a directory's names table to the version this build writes.
///
/// The rewrite is the whole base — an external sort over every row — so it
/// runs only where the version says it is owed, which makes `upgrade`
/// idempotent over a table already forward. What it buys is the text of
/// every name the address spells: measured over a 200,071,629-name table,
/// `text.bin` 3.94 GB to 133 MB.
///
/// A table that cannot be read is not a failure of the payload rewrite
/// that has already landed, so this reports and leaves the exit code
/// alone.
fn names_forward(dir: &Path, lock: &galos_index::Lock) {
    let _ = lock;
    match galos_index::names::version(dir) {
        Ok(None) => {}
        Ok(Some(version)) if version >= galos_index::names::writes() => {
            println!("the names table is already version {version}");
        }
        Ok(Some(version)) => {
            let at = std::time::Instant::now();
            println!(
                "rewriting the names table, version {version} to {}",
                galos_index::names::writes(),
            );
            match galos_index::names::compact(dir) {
                Ok(count) => {
                    println!("{count} names rewritten in {:.1?}", at.elapsed())
                }
                Err(err) => eprintln!("the names table: {err}"),
            }
        }
        Err(err) => eprintln!("the names table: {err}"),
    }
}

/// Pack a directory's loose body files, saying what it moved.
///
/// The same migration a sync runs at every open, for a directory nothing is
/// about to sync: a galaxy of loose files is hours of packing, and an
/// operator would rather spend them on purpose. Interruptible, idempotent,
/// and safe to run against a directory a map is *reading* — a loose file is
/// dropped only once the pack holds its record, and a read falls back to
/// whatever is still loose.
///
/// Not safe to run against a directory something is *writing*, which is
/// what [`held`] is for.
fn pack(dir: &Path, forced: bool) {
    let lock = held(dir, forced);
    let start = std::time::Instant::now();
    match galos_index::pack::pack(dir, &|| false) {
        Ok(done) => println!(
            "{}: {} files packed in {:.1?}{}",
            dir.display(),
            done.moved,
            start.elapsed(),
            match done.finished {
                true => "",
                false => ", and some are still loose",
            },
        ),
        Err(e) => {
            eprintln!("cannot pack {}: {e}", dir.display());
            leave(Some(lock), 2);
        }
    }
}

/// Fold a directory's names chunks into the mapped table, saying what it
/// came to.
///
/// The same migration a sync runs at its open, for a directory nothing is
/// about to sync — a galaxy's worth of chunks is an external sort of
/// gigabytes, and an operator would rather spend it on purpose than
/// discover it at the front of a build.
///
/// It also brings the table's *version* forward, which is the other reason
/// to run it on purpose: a version 1 base stored every name, and rewriting
/// it drops the 97.4 % of them the address spells — see [`names_forward`].
///
/// Idempotent: a directory with no chunks and a table already at this
/// version has nothing to do. Safe to run against a directory a map is
/// *reading*, the table being swapped in by one rename and the chunks
/// removed only after.
fn fold_names(dir: &Path, forced: bool) {
    let lock = held(dir, forced);
    let start = std::time::Instant::now();
    match galos_index::names::fold_chunks(dir) {
        Ok(Some(named)) => println!(
            "{}: {named} systems folded into the mapped table in {:.1?}",
            dir.display(),
            start.elapsed(),
        ),
        Ok(None) => println!("{}: no names chunks to fold", dir.display()),
        // The lock goes with it: `fatal` exits, and an exit runs no
        // destructors. See [`leave`].
        Err(e) => {
            eprintln!("cannot read {}: {e}", dir.display());
            leave(Some(lock), 2);
        }
    }
    names_forward(dir, &lock);
}

/// Learn the sector dictionary from a directory's names table.
///
/// What `galos_index::procedural` compiles in, and the only way to refresh
/// it: a sector enters the dictionary when the first system in it is
/// reported, so the file is as complete as the galaxy anybody has imported.
///
/// **A name that claims more than one sector coordinate is left out.**
/// Those are Frontier's hand-authored regions — `COL 285 SECTOR`, `IC 2944
/// SECTOR` — laid over the procedural grid as spheres, and their boxels are
/// numbered from the region's own origin. Deriving one would spell the
/// wrong name for every procedural system in the same cell, so the module
/// answers nothing there and every system under a region is stored.
///
/// **An entry that already exists may be added to but never changed.**
/// That is the one safety property the whole scheme rests on: a name is
/// dropped from the table because the dictionary spelled it, so a
/// regeneration that renamed a sector would silently re-spell every name
/// already dropped under it. A key whose name disagrees with the one
/// compiled in is therefore refused rather than written, and `--force`
/// is the only way past — which is what somebody rebuilding the dictionary
/// on purpose, against a table they are about to rewrite anyway, passes.
///
/// Read-only on the directory, and takes no lock: it reads the published
/// table and writes somewhere else entirely.
fn sectors(dir: &Path, out: Option<&Path>, force: bool) {
    let table = match galos_index::names::Table::open(dir) {
        Ok(table) => table,
        Err(e) => {
            eprintln!("cannot read the names table at {}: {e}", dir.display());
            std::process::exit(2);
        }
    };

    // Which names each sector coordinate is claimed by, and which
    // coordinates each name claims.
    let mut votes: BTreeMap<u32, BTreeMap<String, u64>> = BTreeMap::new();
    let mut claims: BTreeMap<String, std::collections::BTreeSet<u32>> =
        BTreeMap::new();
    for row in 0..table.len() {
        let name = table.name_at(row);
        let Some(sector) = sector_words(&name) else { continue };
        let key = galos_index::procedural::sector_key(
            elite_journal::Boxel::of(table.address_at(row)).sector,
        );
        let sector = sector.to_owned();
        *votes.entry(key).or_default().entry(sector.clone()).or_insert(0) += 1;
        claims.entry(sector).or_default().insert(key);
    }

    let mut written = 0usize;
    let mut regions = 0usize;
    let mut added = 0usize;
    let mut changed: Vec<(u32, &'static str, String)> = Vec::new();
    let mut text = String::new();
    for (key, names) in &votes {
        let settled = names
            .iter()
            .filter(|(name, _)| claims[name.as_str()].len() == 1)
            .max_by_key(|(_, rows)| **rows);
        match settled {
            Some((name, _)) => {
                written += 1;
                match galos_index::procedural::sector_at(*key) {
                    Some(held) if held != name => {
                        changed.push((*key, held, name.clone()));
                    }
                    Some(_) => {}
                    None => added += 1,
                }
                text.push_str(&format!("{key},{name}\n"));
            }
            None => regions += 1,
        }
    }

    // What the dictionary holds and this table does not: a directory
    // smaller than the galaxy the file was learned from, which is the
    // ordinary case for anything but a full import. Dropping those entries
    // would put every name under them back into the text at the next fold,
    // so it is refused alongside the renames.
    let lost = galos_index::procedural::sectors()
        .filter(|(key, _)| !votes.contains_key(key))
        .count();

    if (!changed.is_empty() || lost > 0) && !force {
        if !changed.is_empty() {
            eprintln!(
                "{} sector(s) would be renamed, and a rename re-spells \
                 every name already dropped under them:",
                changed.len(),
            );
            for (key, held, found) in changed.iter().take(10) {
                eprintln!("  {key}: {held} would become {found}");
            }
        }
        if lost > 0 {
            eprintln!(
                "{lost} sector(s) the dictionary names are not in this \
                 table, and dropping them puts every name under them back \
                 into the text"
            );
        }
        eprintln!("pass --force to write it anyway");
        std::process::exit(1);
    }

    let out = out.unwrap_or(Path::new("galos_index/data/sectors.csv"));
    if let Err(e) = std::fs::write(out, &text) {
        eprintln!("cannot write {}: {e}", out.display());
        std::process::exit(2);
    }
    eprintln!(
        "{written} sectors, {} bytes to {}; {added} new, {} renamed, \
         {lost} dropped, {regions} coordinates only a hand-authored region \
         claims",
        text.len(),
        out.display(),
        changed.len(),
    );
}

/// The words a procedural name begins with, or [`None`] where it is not
/// one.
///
/// The shape and nothing else: whether the address agrees is
/// `galos_index::procedural`'s business, and this runs before there is a
/// dictionary for it to agree through.
fn sector_words(name: &str) -> Option<&str> {
    let (head, last) = name.rsplit_once(' ')?;
    let digit = last.find(|c: char| c.is_ascii_digit())?;
    let (class, numbers) = last.split_at(digit);
    if class.len() != 1 || !class.as_bytes()[0].is_ascii_uppercase() {
        return None;
    }
    if !numbers.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return None;
    }
    let (sector, code) = head.rsplit_once(' ')?;
    let (pair, one) = code.split_once('-')?;
    (pair.len() == 2 && one.len() == 1).then_some(sector)
}

/// Take the directory for as long as this command holds it, or refuse.
///
/// Every command here that *writes* needs it, and for the reason the lock
/// exists: a builder and one of these share scratch paths — the names
/// writer's is `names/.building`, and whoever opens it second removes what
/// the first is streaming into. Measured the hard way: a fold run beside a
/// live import unlinked the import's row file and the build ended in a bare
/// "No such file or directory" three minutes later.
///
/// `galos-sync` takes the same lock, so either order of the two refuses
/// rather than interleaves.
///
/// `forced` is `--force-lock`, and is for the one thing a refusal cannot
/// tell apart from a live builder: a lock whose process was killed. The
/// refusal names the pid, and clearing one that is still running is two
/// writers over a directory published whole — see [`galos_index::Lock`].
fn held(dir: &Path, forced: bool) -> galos_index::Lock {
    let taken = match forced {
        true => galos_index::Lock::force(dir),
        false => galos_index::Lock::take(dir),
    };
    match taken {
        Ok(lock) => lock,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
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
            rows(
                "names rows",
                Some(entries(a).unwrap_or_else(|e| fatal(a, e))),
                Some(entries(b).unwrap_or_else(|e| fatal(b, e))),
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
/// Read off the mapping a row at a time, base and log together, so the
/// digest of a galaxy costs a row and not a table.
fn digest(dir: &Path) -> io::Result<(usize, u64, u64)> {
    let held = galos_index::Names::open(dir)?;
    let (mut count, mut sum, mut xor) = (0usize, 0u64, 0u64);
    for address in held.addresses() {
        let Some(entry) = held.entry_of(address) else {
            continue;
        };
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
    Ok((count, sum, xor))
}

/// Every row of a names table, for the road that names them.
///
/// The one place a whole table is held: `--detail` is asked for a pair of
/// directories a human is going to read the difference between, not for a
/// galaxy.
fn entries(dir: &Path) -> io::Result<Vec<NameEntry>> {
    let held = galos_index::Names::open(dir)?;
    Ok(held.addresses().filter_map(|at| held.entry_of(at)).collect())
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
