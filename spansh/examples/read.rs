//! Read a dump and say what was in it.
//!
//! ```sh
//! cargo run --release -p spansh --example read -- ~/Downloads/systems.json
//! cargo run --release -p spansh --example read -- ~/Downloads/galaxy_7days.json
//! cargo run --release -p spansh --example read -- ~/Downloads/systems.json 5000000
//! ```
//!
//! Either of Spansh's files, and nothing here asks which: one is the
//! other with more filled in. The second argument stops early, for
//! measuring a rate without reading the whole file.
//!
//! What it prints is the rate, the classes it saw, the systems it could
//! not place, and the bodies where the file carries any.

use spansh::Dump;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("a dump to read"));
    let stop: u64 = args
        .next()
        .map(|it| it.parse().expect("a number of systems"))
        .unwrap_or(u64::MAX);

    let mut read = 0u64;
    let mut bodies = 0u64;
    let mut classes: BTreeMap<String, u64> = BTreeMap::new();
    let mut named_a_body = 0u64;
    let mut classless = 0u64;
    let at = Instant::now();

    for system in Dump::open(&path).expect("the dump opens") {
        let system = system.expect("every line parses");
        read += 1;
        bodies += system.bodies.len() as u64;
        match system.class() {
            Some(class) => {
                *classes.entry(class.token().to_owned()).or_insert(0) += 1;
            }
            // Nothing at the middle, which is a planet a ship arrives at
            // or a system nobody has looked into — and the two are told
            // apart by whether the file named anything there at all.
            None if system.main_star.is_some()
                || system.arrival().is_some() =>
            {
                named_a_body += 1
            }
            None => classless += 1,
        }
        if read >= stop {
            break;
        }
        if read.is_multiple_of(10_000_000) {
            let rate = read as f64 / at.elapsed().as_secs_f64();
            eprintln!("  {read} systems, {rate:.0}/s");
        }
    }

    let elapsed = at.elapsed();
    println!(
        "{read} systems in {elapsed:.1?} ({:.0}/s)\n  \
         {} classes, {named_a_body} whose main body is not a star, \
         {classless} with nothing at the middle at all",
        read as f64 / elapsed.as_secs_f64(),
        classes.len(),
    );
    if bodies > 0 {
        println!(
            "  {bodies} bodies, {:.1} a system",
            bodies as f64 / read as f64,
        );
    }
    for (class, count) in &classes {
        println!("  {class:<24} {count}");
    }
}
