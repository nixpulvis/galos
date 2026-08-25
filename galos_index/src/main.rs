//! Command-line tools for the galaxy index files.
//!
//! ```sh
//! cargo run -p galos_index -- info .galos_index
//! ```
//!
//! Read-only and database-free: everything here reads the cell records the
//! builder wrote, never Postgres. `info` is the first subcommand.

use clap::{Parser, Subcommand};
use galos_index::geometry::MAX_LEVEL;
use galos_index::{Index, store};
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
}

fn main() {
    match Cli::parse().command {
        Command::Info { dir } => info(&dir),
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
        if let Ok(entries) = std::fs::read_dir(dir.join(store::PAYLOAD_DIR)) {
            let (count, bytes) = entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .fold((0u64, 0u64), |(n, b), m| (n + 1, b + m.len()));
            print!(", {count} payload files ({:.2} MB)", mib(bytes));
        }
        println!();
    }

    println!("  cells per level:");
    for (level, count) in per_level.iter().enumerate() {
        if *count > 0 {
            println!("    L{level:<2}  {count}");
        }
    }
}

/// Bytes as mebibytes.
fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}
