//! Read a dump and say what was in it.
//!
//! ```sh
//! cargo run --release -p spansh --example read -- ~/Downloads/systems.json
//! cargo run --release -p spansh --example read -- ~/Downloads/galaxy_7days.json
//! cargo run --release -p spansh --example read -- ~/Downloads/systems.json 5000000
//! ```
//!
//! Either form, told apart by its first object rather than by a flag: the
//! two files are published together and named alike, and asking a reader
//! to be told which is in hand is asking to be told something the file
//! says. The second argument stops early, for measuring a rate without
//! reading the whole file.
//!
//! What it prints is the rate, the classes it saw and the systems it could
//! not place — and, of a full dump, the bodies hung off them.

use elite_journal::body::StarClass;
use spansh::{Form, Galaxy, Systems};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

/// What a read of either form comes to.
#[derive(Default)]
struct Tally {
    read: u64,
    bodies: u64,
    classes: BTreeMap<String, u64>,
    /// Systems whose middle is something other than a star.
    named_a_body: u64,
    /// Systems nobody has looked into at all.
    classless: u64,
}

impl Tally {
    /// One system, by what is at the middle of it: its class where the
    /// dump's prose names a star, and otherwise whether anything was named
    /// there at all.
    fn system(&mut self, class: Option<StarClass>, named: bool) {
        self.read += 1;
        match class {
            Some(class) => {
                *self.classes.entry(class.token().to_owned()).or_insert(0) += 1;
            }
            None if named => self.named_a_body += 1,
            None => self.classless += 1,
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("a dump to read"));
    let stop: u64 = args
        .next()
        .map(|it| it.parse().expect("a number of systems"))
        .unwrap_or(u64::MAX);

    let form = spansh::form(&path)
        .expect("the dump opens")
        .expect("a dump with an object in it");
    let mut tally = Tally::default();
    let at = Instant::now();

    match form {
        Form::Brief => {
            let mut systems = Systems::open(&path).expect("the dump opens");
            while let Some(system) = systems.next().expect("every line parses")
            {
                tally.system(system.class(), system.main_star.is_some());
                if tally.read >= stop {
                    break;
                }
                said(tally.read, &at);
            }
        }
        Form::Full => {
            let mut systems = Galaxy::open(&path).expect("the dump opens");
            while let Some(system) = systems.next().expect("every line parses")
            {
                tally.bodies += system.bodies.len() as u64;
                tally.system(system.class(), system.main_star().is_some());
                if tally.read >= stop {
                    break;
                }
                said(tally.read, &at);
            }
        }
    }

    let elapsed = at.elapsed();
    let form = match form {
        Form::Brief => "brief",
        Form::Full => "full",
    };
    println!(
        "{} systems of a {form} dump in {elapsed:.1?} ({:.0}/s)\n  \
         {} classes, {} whose main body is not a star, \
         {} with nothing at the middle at all",
        tally.read,
        tally.read as f64 / elapsed.as_secs_f64(),
        tally.classes.len(),
        tally.named_a_body,
        tally.classless,
    );
    if tally.bodies > 0 {
        println!(
            "  {} bodies, {:.1} a system",
            tally.bodies,
            tally.bodies as f64 / tally.read as f64,
        );
    }
    for (class, count) in &tally.classes {
        println!("  {class:<24} {count}");
    }
}

/// The rate so far, every ten million systems, so a read of the whole
/// galaxy says something before it ends.
fn said(read: u64, at: &Instant) {
    if read % 10_000_000 == 0 {
        eprintln!(
            "  {read} systems, {:.0}/s",
            read as f64 / at.elapsed().as_secs_f64()
        );
    }
}
