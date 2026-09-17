//! What the names table costs to open, to look an address up in, and to
//! search.
//!
//! ```sh
//! cargo build --release --example names_bench -p galos_index
//! /usr/bin/time -l ./target/release/examples/names_bench .index/full
//! ```
//!
//! Run under `/usr/bin/time -l` (macOS) or `-v` (GNU) for the peak resident
//! set, which is the number this format exists to hold down: the table it
//! replaced measured 7.9 GB resident and 33 s to read at 200,071,629
//! systems, and 47 GB before it was packed.
//!
//! The lookups are sampled across the whole table on purpose. A warm run
//! measures the arithmetic; the first run after a build measures the page
//! faults, which is what a user opening the map actually waits for.

use galos_index::Names;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| {
        eprintln!(
            "usage: names_bench <index directory> [--no-search] [QUERY…]"
        );
        std::process::exit(2);
    });
    let rest: Vec<String> = args.collect();
    // Left out, what a run's peak resident set measures is opening the
    // table and looking addresses up in it, which is what a map that draws
    // the sky and never searches pays.
    let searching = !rest.iter().any(|it| it == "--no-search");
    let asked: Vec<String> =
        rest.into_iter().filter(|it| !it.starts_with("--")).collect();
    let dir = std::path::PathBuf::from(dir);

    let at = Instant::now();
    let names = Names::open(&dir).expect("the names table should open");
    let open = at.elapsed();
    let count = names.len();
    println!(
        "open      {count} systems in {open:.1?} ({} in the log)",
        names.delta().len(),
    );
    if count == 0 {
        return;
    }

    // Spread over the whole table rather than clustered, so the lookups
    // fault the pages a search would rather than the ones just written.
    let base = names.base();
    let step = (count / 1000).max(1);
    let sample: Vec<i64> =
        (0..count).step_by(step).map(|at| base.address_at(at)).collect();

    let at = Instant::now();
    let mut found = 0usize;
    for &address in &sample {
        found += names.name_of(address).is_some() as usize;
    }
    let each = at.elapsed() / sample.len() as u32;
    println!("address   {found} of {} found, {each:.1?} each", sample.len());

    let names_of: Vec<String> = sample
        .iter()
        .filter_map(|&address| names.name_of(address).map(|it| it.to_string()))
        .collect();

    let at = Instant::now();
    let mut resolved = 0usize;
    for name in &names_of {
        resolved += names.address_of(name).is_some() as usize;
    }
    let each = at.elapsed() / names_of.len().max(1) as u32;
    println!(
        "by name   {resolved} of {} resolved, {each:.1?} each",
        names_of.len()
    );

    if !searching {
        return;
    }

    // A search is a prefix walk of `byname.bin`, so what it costs is the
    // answer's size and not the galaxy's. `QUERY` arguments are measured
    // as given — upper case, as every name in the table is.
    let mut queries: Vec<String> = asked;
    if queries.is_empty() {
        queries.extend(
            names_of.first().map(|it| it[..it.len().min(6)].to_owned()),
        );
    }
    for query in &queries {
        let at = Instant::now();
        let hits = names.matching(query, 25).len();
        println!("search    {hits} hits for {query:?} in {:.1?}", at.elapsed());
    }
}
