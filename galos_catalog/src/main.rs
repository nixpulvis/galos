//! Read and inspect star catalogs.
//!
//! ```sh
//! # What is in a catalog, and what it could not place.
//! cargo run -p galos_catalog -- info hygdata.csv
//!
//! # How far the catalog is from agreeing with itself.
//! cargo run -p galos_catalog -- consistency hygdata.csv
//! ```
//!
//! Catalog-only and file-only: nothing here reaches a database. Comparing a
//! catalog against the Elite dataset needs one, so that lives where the
//! database does, as `galos-db catalog` — the same split `galos-db index` sits
//! on, where the tree belongs to `galos_index` and the build to whoever can
//! read the rows.

use clap::{Parser, Subcommand};
use galos_catalog::{check, hyg};
use std::fs::File;
use std::path::PathBuf;
use std::process::exit;

/// Read and inspect star catalogs.
#[derive(Parser)]
#[command(name = "galos-catalog", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarise a catalog: what it holds, and what it could not place.
    Info {
        /// The HYG catalog CSV to read.
        file: PathBuf,
    },
    /// Report how far a catalog's coordinates are from its own distances.
    ///
    /// A survey records a distance and also the coordinates it computed from
    /// that distance, so the length of the second should be the first. What is
    /// left over is the noise floor under every comparison against another
    /// dataset: a disagreement smaller than this is the catalog's own
    /// arithmetic coming back, not a finding.
    Consistency {
        /// The HYG catalog CSV to read.
        file: PathBuf,
        /// How many of the worst rows to name.
        #[arg(long, default_value_t = 10)]
        worst: usize,
    },
}

fn main() {
    match Cli::parse().command {
        Command::Info { file } => info(&file),
        Command::Consistency { file, worst } => consistency(&file, worst),
    }
}

fn read(path: &PathBuf) -> galos_catalog::Catalog {
    let handle = File::open(path).unwrap_or_else(|e| {
        eprintln!("cannot open {}: {e}", path.display());
        exit(1);
    });
    hyg::read(handle).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", path.display());
        exit(1);
    })
}

fn info(path: &PathBuf) {
    let catalog = read(path);
    let named = catalog.stars.iter().filter(|s| s.name.is_some()).count();
    println!("{} catalog", catalog.source.name);
    println!("  {} stars placed, {named} of them named", catalog.stars.len());
    println!(
        "  {} located on the sky but not in space",
        catalog.unplaced.len()
    );

    let skipped = catalog.skipped;
    println!(
        "  {} rows dropped ({} no parallax, {} at the origin, {} unreadable)",
        skipped.total(),
        skipped.no_parallax,
        skipped.at_the_origin,
        skipped.unreadable,
    );
    // The part that changes a picture, said on its own line: the stars a survey
    // cannot place are the distant luminous ones, so the hole sits at the
    // bright end rather than the faint one.
    if skipped.naked_eye > 0 {
        println!(
            "  of which {} are naked-eye stars, brightest magnitude {:.2}",
            skipped.naked_eye,
            skipped.brightest.unwrap_or(f64::NAN),
        );
    }
}

fn consistency(path: &PathBuf, worst: usize) {
    let catalog = read(path);
    let report = check::consistency(&catalog.stars, worst);
    println!("{} catalog, {} stars", catalog.source.name, report.stars);
    if report.stars == 0 {
        return;
    }
    println!("  median disagreement  {:.3e}", report.median);
    println!("  99th percentile      {:.3e}", report.p99);
    println!("  worst                {:.3e}", report.max());
    println!(
        "\nAnything a cross-check reports below the 99th percentile is this \
         catalog's own\narithmetic rather than a disagreement with anybody."
    );

    if report.worst.is_empty() {
        return;
    }
    println!("\nworst rows:");
    println!(
        "  {:<24} {:>12} {:>12} {:>11} {:>11}",
        "star", "distance", "|position|", "diff (ly)", "fraction"
    );
    for d in &report.worst {
        let name = d
            .name
            .clone()
            .unwrap_or_else(|| format!("{} {}", catalog.source.name, d.id));
        println!(
            "  {name:<24} {:>12.4} {:>12.4} {:>11.5} {:>11.3e}",
            d.recorded,
            d.derived,
            d.light_years(),
            d.fraction(),
        );
    }
}
