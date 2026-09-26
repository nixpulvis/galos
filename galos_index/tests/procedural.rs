// TODO: reorganize. This measures a local `GALOS_PERF_DIR` and passes
// silently without one, so it is not an integration test of anything a user
// runs. Revisit alongside `examples/names_bench.rs` when setting up proper
// criterion benchmarks.

//! What a whole galaxy's names say about deriving them.
//!
//! `galos_index::core::procedural` spells a system's name from its address, and
//! the names table stores only the ones that disagree. The unit tests pin
//! the arithmetic against known systems; this pins it against **every name
//! a real directory holds**, because the thing that matters is a rate: a
//! derivation wrong for one system in a thousand is two hundred thousand
//! wrong names, and no sample of a few would show it.
//!
//! Stands down without `GALOS_PERF_DIR` naming a built index directory:
//!
//! ```sh
//! GALOS_PERF_DIR=.index/full cargo test -p galos_index --test procedural -- --nocapture
//! ```
//!
//! ## Measured 2026-09-17 over `.index/full`
//!
//! 200,071,629 names, the dictionary `galos index sectors` learned from the
//! same directory:
//!
//! | | rows | |
//! |---|---|---|
//! | derived exactly | 194,667,563 | **97.2989 %** |
//! | names people gave | 151,463 | stored |
//! | hand-authored regions | 5,252,594 | stored |
//! | **the tail wrong where the sector was right** | **0** | the bug this is for |
//!
//! The last row is the assertion. The first is a floor, because a
//! dictionary regenerated against a larger galaxy can only cover more.

use elite_journal::Boxel;
use galos_index::core::procedural;
use galos_index::store::names::Table;
use std::path::PathBuf;
use std::time::Instant;

/// The directory to measure against, or [`None`] to stand down.
fn measured() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("GALOS_PERF_DIR").ok()?);
    if !dir.join("index.bin").exists() {
        eprintln!("{}: not a built index; standing down", dir.display());
        return None;
    }
    Some(dir)
}

/// The words a name begins with, where it has the shape of a procedural
/// one.
fn shaped(name: &str) -> Option<(&str, &str)> {
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
    (pair.len() == 2 && one.len() == 1)
        .then(|| (sector, &name[sector.len() + 1..]))
}

/// Every name in a real directory, derived and compared.
///
/// Three things are checked and each fails differently.
///
/// **The tail, where the sector agrees.** This is the arithmetic, and it is
/// the only one of the three that can be a bug in this crate: the boxel
/// ordinal and the system's index come from the address's own bits, so a
/// disagreement means a constant is wrong. Measured at zero, and asserted
/// at zero.
///
/// **The rate.** A floor rather than a figure, since the dictionary grows
/// with the galaxy it was learned from.
///
/// **The round trip.** Every name that derives must resolve back to the
/// address that spelled it, which is what lets a search answer a procedural
/// name with no table at all.
#[test]
fn a_galaxy_of_names_derives_from_its_addresses() {
    let Some(dir) = measured() else { return };
    let table = Table::open(&dir).expect("the names table opens");
    let rows = table.len();
    assert!(rows > 0, "{}: the names table is empty", dir.display());

    let at = Instant::now();
    let (mut derived, mut given, mut region, mut tail_off, mut no_sector) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut round_trips = 0u64;
    let mut said = 0;
    for row in 0..rows {
        let name = table.name_at(row);
        let address = table.address_at(row);
        if procedural::spells(address, &name) {
            derived += 1;
            // Only a sample round-trips: it re-spells the name and searches
            // the dictionary, and a galaxy of that is minutes for a thing
            // one in a thousand names proves just as well.
            if row % 1_000 == 0 {
                assert_eq!(
                    procedural::address_of(&name),
                    Some(address),
                    "{name} derives but does not resolve back",
                );
                round_trips += 1;
            }
            continue;
        }
        let Some((sector, tail)) = shaped(&name) else {
            given += 1;
            continue;
        };
        let boxel = Boxel::of(address);
        match procedural::sector_at(procedural::sector_key(boxel.sector)) {
            // The sector the address keys is not the one the name says, so
            // the name belongs to a hand-authored region laid over this
            // cell. Stored, and not this crate's arithmetic.
            Some(held) if held != sector => region += 1,
            None => no_sector += 1,
            // The sector agrees and the tail does not: the arithmetic.
            Some(_) => {
                tail_off += 1;
                if said < 5 {
                    said += 1;
                    eprintln!(
                        "  tail off: {name} spells {tail}, derives {}",
                        boxel.tail(),
                    );
                }
            }
        }
    }
    let took = at.elapsed();

    let rate = 100.0 * derived as f64 / rows as f64;
    eprintln!(
        "{rows} names in {took:.1?}: {derived} derived ({rate:.4} %), \
         {given} given, {region} in a hand-authored region, {no_sector} in \
         no known sector, {tail_off} with the tail wrong; {round_trips} \
         round trips"
    );

    assert_eq!(
        tail_off, 0,
        "the boxel arithmetic disagrees with {tail_off} names whose sector \
         it agrees about — a constant in `procedural` is wrong",
    );
    assert!(
        rate > 95.0,
        "only {rate:.4} % of names derive, against the 97.2989 % measured: \
         the dictionary or the arithmetic has regressed",
    );
}
