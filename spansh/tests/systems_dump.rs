//! Reading a `systems.json`, over lines cut out of the real file.
//!
//! Four lines: the array's `[`, three systems, the array's `]`. This
//! proves the framing — the punctuation is skipped, a line is one whole
//! object, and the end of the array ends the read.

use elite_journal::body::StarClass;
use spansh::Systems;
use std::path::Path;

fn fixture() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/systems.json"))
}

#[test]
fn the_dump_reads_as_systems() {
    let mut systems = Systems::open(fixture()).expect("the fixture opens");
    let mut read = Vec::new();
    while let Some(system) = systems.next().expect("every line parses") {
        read.push(system);
    }

    assert_eq!(read.len(), 3, "the array's own punctuation was read too");

    let first = &read[0];
    assert_eq!(first.id64, 688319);
    assert_eq!(first.name, "Cygni X-3");
    assert_eq!(first.class(), Some(StarClass::H));
    assert_eq!(first.coords.x, -36417.71875);

    // Ordered as the file has them, since a bulk build reads a dump once
    // and in one direction.
    assert!(read[0].id64 < read[1].id64 && read[1].id64 < read[2].id64);
}

#[test]
fn a_missing_dump_is_an_error_and_not_a_panic() {
    let missing = fixture().with_file_name("not-a-dump.json");
    assert!(Systems::open(&missing).is_err());
}
