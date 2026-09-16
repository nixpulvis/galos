//! Reading a `systems.json`, over lines cut out of the real file.
//!
//! Four lines: the array's `[`, three systems, the array's `]`. This
//! proves the framing — the punctuation is skipped, a line is one whole
//! object, and the end of the array ends the read.

use elite_journal::body::StarClass;
use spansh::{Form, Systems};
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

/// The two dumps are told apart by what is in them, not by what the
/// caller was told to expect.
#[test]
fn a_dump_says_which_form_it_is() {
    let full = fixture().with_file_name("galaxy.json");
    assert_eq!(spansh::form(fixture()).unwrap(), Some(Form::Brief));
    assert_eq!(spansh::form(&full).unwrap(), Some(Form::Full));
}

/// Pointing the brief reader at a full dump says so, rather than naming
/// the first field the full form spells differently.
///
/// `galaxy_7days.json` read this way answered `missing field `updateTime`
/// at line 1 column 3900`, which is true and no help: the two files are
/// published together, named alike, and tens of gigabytes each.
#[test]
fn a_full_dump_read_as_a_brief_one_says_which_it_is() {
    let full = fixture().with_file_name("galaxy.json");
    let mut systems = Systems::open(&full).expect("the fixture opens");
    let err = systems.next().expect_err("a full line is not a brief one");
    let said = err.to_string();
    assert!(said.contains("line 1"), "{said}");
    assert!(said.contains("full dump"), "{said}");
    assert!(said.contains("`Galaxy`"), "{said}");
}
