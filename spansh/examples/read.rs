//! Read a dump and say what was in it.
//!
//! ```sh
//! cargo run --release -p spansh --example read -- ~/Downloads/systems.json
//! cargo run --release -p spansh --example read -- ~/Downloads/systems.json 5000000
//! ```
//!
//! The second argument stops early, for measuring a rate without reading
//! the whole file. What it prints is the rate, the classes it saw and the
//! systems it could not place.

use spansh::Systems;
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

    let mut systems = Systems::open(&path).expect("the dump opens");
    let mut read = 0u64;
    let mut classes: BTreeMap<String, u64> = BTreeMap::new();
    let mut classless = 0u64;
    let mut named_a_body = 0u64;
    let at = Instant::now();

    while let Some(system) = systems.next().expect("every line parses") {
        read += 1;
        match system.class() {
            Some(class) => {
                *classes.entry(class.token().to_owned()).or_insert(0) += 1;
            }
            None => match system.main_star {
                Some(_) => named_a_body += 1,
                None => classless += 1,
            },
        }
        if read >= stop {
            break;
        }
        if read % 10_000_000 == 0 {
            let rate = read as f64 / at.elapsed().as_secs_f64();
            eprintln!("  {read} systems, {:.0}/s", rate);
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
    for (class, count) in &classes {
        println!("  {class:<24} {count}");
    }
}
