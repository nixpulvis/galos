//! Bringing a built directory up to the format this build reads.
//!
//! One place for the migrations an operator runs on purpose, rather than one
//! command per format change: `galos index migrate` is what
//! [`crate::tree::index::Index::read`]'s refusal names, and what it does is
//! whatever the directory turns out to need. At present that is rewriting
//! legacy payloads as columns; anything further lands here beside it rather
//! than as another subcommand named after a layout.
//!
//! Not run at open, unlike [`crate::ops::migrate::migrate`]'s resharding. That
//! is a rename a file and this is a re-encode of every cell plus a sweep of the
//! scan record — hours over a galaxy, which a reader that wants to draw cannot
//! spend without saying so.
//!
//! **A rebuild that is not a reimport.** A legacy payload holds everything
//! a columnar one does but one field: the star kind. That field is derivable
//! from the directory itself — `bodies/` is the scan record the class comes
//! from — so a directory can be brought forward without going back to the
//! dump it was imported from, which is hours of a different order.
//!
//! What it does, per cell: read the legacy block, join the kind on, write
//! the columnar block. Then bring the contributed tables forward, as an open
//! would, and rewrite `index.bin` so its version says what the payloads are.
//! The cells' own records are untouched — the payload is what differs
//! between the versions, and `Cell::LEN` does not — so the tree, the
//! aggregates and the index's own tables stay exactly as they are.

use crate::core::codec::Decode as _;
use crate::core::star::StarKind;
use crate::format::layout::payload_path;
use crate::format::payload::{
    INDEX_VERSION, index_version, legacy_payload_points, payload_bytes,
    payload_head,
};
use crate::store::Directory;
use crate::store::tables::TableSet;
use crate::tree::index::Index;
use std::io;
use std::path::Path;

/// What a rewrite came to.
///
/// Said as it goes as well as at the end: the sweep of the scan record is
/// tens of millions of systems before a single cell is written, and a
/// command that prints nothing for half an hour is one an operator kills.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Rewrote {
    /// Systems the sweep of the scan record has read, which is the phase
    /// before any cell is touched.
    pub swept: u64,
    /// How many cells were written in the columnar layout.
    pub cells: u64,
    /// How many systems those cells held.
    pub systems: u64,
    /// How many of them a star kind was found for.
    pub classed: u64,
    /// How many cells were already columnar and left alone.
    pub kept: u64,
    /// Rows the contributed tables' own upgrades rewrote
    ///
    /// [`crate::store::tables::Table::upgrade`] is a step of an open rather than
    /// of a build, and an open over a stale directory does nothing at all —
    /// [`crate::ops::migrate::migrate`] sets `upgrade` and returns, having
    /// touched nothing. So a directory brought forward by this command alone
    /// would still hold a table in a shape its owner has moved on from, and
    /// anything reading it without opening the galaxy first — the map's perf
    /// guard, for one — fails to decode a row.
    pub upgraded: u64,
}

/// Every system's kind, by address, as one sorted pair of columns
///
/// A `HashMap` over ninety-five million addresses is gigabytes of buckets;
/// two sorted vectors are nine bytes an entry and answer by binary search,
/// which is what a per-cell join asks of it.
struct Kinds {
    addresses: Vec<i64>,
    kinds: Vec<u8>,
}

impl Kinds {
    /// Sweep the scan record for what every scanned system arrives at.
    fn swept(
        dir: &Path,
        stop: &(dyn Fn() -> bool + Sync),
        said: &mut dyn FnMut(&Rewrote),
    ) -> io::Result<Kinds> {
        let mut addresses = Vec::new();
        let mut kinds = Vec::new();
        Directory::at(dir).each_arrival_class(
            stop,
            &mut |address, class| {
                addresses.push(address);
                kinds.push(StarKind::of(class).code());
                // Often enough to see it moving, rarely enough to cost nothing:
                // a line a million systems is some fifty of them over a galaxy.
                if addresses.len() % 1_000_000 == 0 {
                    said(&Rewrote {
                        swept: addresses.len() as u64,
                        ..Rewrote::default()
                    });
                }
            },
        )?;
        said(&Rewrote { swept: addresses.len() as u64, ..Rewrote::default() });

        // Sorted together, the sweep having come in file order.
        let mut order: Vec<usize> = (0..addresses.len()).collect();
        order.sort_unstable_by_key(|&at| addresses[at]);
        let held = Kinds {
            addresses: order.iter().map(|&at| addresses[at]).collect(),
            kinds: order.iter().map(|&at| kinds[at]).collect(),
        };
        Ok(held)
    }

    /// What one system arrives at, or nothing where nothing has scanned it.
    fn of(&self, address: i64) -> StarKind {
        match self.addresses.binary_search(&address) {
            Ok(at) => StarKind::from_code(self.kinds[at]),
            Err(_) => StarKind::Unknown,
        }
    }
}

/// Rewrite every payload in `dir` into the columnar layout
///
/// The one migration there is at present; see the module header for why it
/// is asked for rather than done at open.
///
/// Idempotent: a cell already columnar is counted and left alone, so a run
/// interrupted part way is finished by running it again. The index file is
/// rewritten last, for that reason — a directory whose `index.bin` still
/// names a stale version is one the rewrite has not finished, and nothing
/// reads the columnar payloads until it does.
pub fn rewrite(
    dir: &Path,
    tables: &TableSet,
    stop: &(dyn Fn() -> bool + Sync),
    said: &mut dyn FnMut(&Rewrote),
) -> io::Result<Rewrote> {
    let index = read_any_version(dir)?;

    // Nothing is read off the scan record until a payload is found that
    // wants the join. A galaxy's `bodies/` is 150 GB and the sweep of it
    // is the whole cost of this command — hours — so a directory whose
    // payloads are already columnar, which is every directory this has
    // finished with once, must not pay it to answer "already columnar".
    // The names rewrite below it is the reason that matters: it is
    // reachable no other way, and it should not be behind a sweep that
    // rewrites nothing.
    let mut swept: Option<Kinds> = None;

    let mut wrote = Rewrote::default();
    for cell in index.cells() {
        if stop() {
            return Ok(wrote);
        }
        let path = payload_path(dir, cell.id);
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if payload_head(&bytes).is_some() {
            wrote.kept += 1;
            continue;
        }

        if swept.is_none() {
            // The loose body files first. The sweep reads the packed
            // shards and nothing else, so a directory with scans still
            // loose would have them swept as though nothing had looked at
            // those systems — every one of them coming out `Unknown` and
            // the column quietly wrong. The pack is idempotent and is the
            // same one an open runs.
            Directory::at(dir).pack_bodies(stop)?;
            swept = Some(Kinds::swept(dir, stop, said)?);
        }
        let kinds = swept.as_ref().expect("the sweep has run");

        let mut points = legacy_payload_points(&bytes);
        for point in points.iter_mut() {
            point.kind = kinds.of(point.id64 as i64);
            if point.kind != StarKind::Unknown {
                wrote.classed += 1;
            }
        }
        wrote.systems += points.len() as u64;
        wrote.cells += 1;

        Directory::at(dir)
            .write_payload(cell.id, payload_bytes(cell.id, &points))?;
        if wrote.cells % 4096 == 0 {
            said(&wrote);
        }
    }

    // The contributed tables, which an open would bring forward but an open
    // over a stale directory never reaches: `migrate` names this command
    // and returns without touching anything. A directory this has finished
    // with is one every reader can read, not one the next open has still
    // to finish.
    wrote.upgraded = crate::ops::migrate::upgraded(dir, tables)?
        .iter()
        .map(|&(_, rows)| rows as u64)
        .sum();

    // Last, so an interrupted run is told apart from a finished one by the
    // one file every reader checks first.
    index.write(dir)?;
    said(&wrote);
    Ok(wrote)
}

/// The index file's cells, whatever version it claims
///
/// [`Index::read`] refuses a version it was not built against, which is the
/// rule that makes a stale directory fail loudly rather than decode as
/// nonsense — and exactly what a migration has to get past. Sound here
/// because the index record is the same at every version this accepts:
/// `Cell::LEN` does not vary with it, only the payload beside it does.
fn read_any_version(dir: &Path) -> io::Result<Index> {
    let path = dir.join(crate::format::layout::INDEX_FILE);
    let bytes = std::fs::read(&path)?;
    let refused = |said: String| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {said}", path.display()),
        )
    };
    let Some(version) = index_version(&bytes) else {
        return Err(refused("not an index file".to_owned()));
    };
    if version > INDEX_VERSION {
        return Err(refused(format!(
            "version {version}, which is past the {INDEX_VERSION} this \
             build knows",
        )));
    }

    // Past the header, which the version check above has read.
    let mut cur = &bytes[4 + 2..];
    let count = u32::decode(&mut cur)
        .ok_or_else(|| refused("a header with no count in it".to_owned()))?;
    let mut cells = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let cell = crate::tree::cell::Cell::decode(&mut cur)
            .ok_or_else(|| refused("a cell short of its bytes".to_owned()))?;
        cells.push(cell);
    }
    Ok(Index::from_cells(cells))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::snapshot::BuildParams;
    use crate::build::tree::Tree;
    use crate::core::codec::Encode as _;
    use crate::format::payload::payload_points;
    use crate::records::{Star, SystemBodies};
    use crate::system::System;

    /// A scratch directory unique to this run.
    fn scratch(what: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("galos_columns_{what}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// One system, placed.
    fn system(id: u64, at: [f64; 3]) -> System {
        System {
            id64: id,
            position: at,
            absolute_magnitude: 4.83,
            temperature: 5778.0,
            age_bucket: 0,
            updated_at: 1_700_000_000,
            kind: StarKind::Unknown,
        }
    }

    /// A payload in the legacy record layout.
    fn legacy_bytes(systems: &[System]) -> Vec<u8> {
        let mut out = Vec::new();
        for held in systems {
            held.id64.encode(&mut out);
            held.position.encode(&mut out);
            4.83f32.encode(&mut out);
            3u8.encode(&mut out);
            held.updated_at.encode(&mut out);
        }
        out
    }

    /// A rewrite carries every position through and fills in the kinds
    ///
    /// Which is the whole of what it is for: a legacy payload holds
    /// everything but the star kind, and the kind is derivable from the
    /// scan record beside it — so a directory comes forward without
    /// going back to the dump it was imported from.
    #[test]
    fn a_rewrite_keeps_the_positions_and_finds_the_kinds() {
        let dir = scratch("rewrite");
        let placed = [
            system(1, [0.0, 0.0, 0.0]),
            system(2, [10.03125, -4.5, 7.25]),
            system(3, [-20.5, 3.0, 1.875]),
        ];

        // A tree, written the way a build writes one, then its payloads
        // put back in the legacy layout — which is the directory a
        // migration meets.
        let mut tree = Tree::build(&placed, &BuildParams::default());
        tree.write(&dir).expect("a written tree");
        let built = tree.to_snapshot();
        for cell in built.index.cells() {
            let points = built.payload(cell.id);
            if points.is_empty() {
                continue;
            }
            let legacy: Vec<System> = points
                .iter()
                .map(|point| system(point.id64, point.position))
                .collect();
            Directory::at(&dir)
                .write_payload(cell.id, legacy_bytes(&legacy))
                .expect("an old payload");
        }

        // And a scan record for two of the three: a neutron star and a
        // class G. The third has never been looked at.
        let scanned = |address: i64, class: &str| {
            let star = Star {
                system_address: address,
                id: 0,
                name: String::new(),
                parents: Vec::new(),
                updated_at: chrono::DateTime::UNIX_EPOCH,
                updated_by: String::new(),
                absolute_magnitude: 0.,
                age_my: 0,
                distance_from_arrival_ls: 0.,
                luminosity: String::new(),
                star_class: class.to_owned(),
                stellar_mass: 0.,
                subclass: 0,
                orbit: None,
                spin: elite_journal::body::Spin { period: 0., tilt: 0. },
                radius: 0.,
                temperature: 0.,
                mapped: false,
                discovered_at: None,
            };
            let inside =
                SystemBodies { stars: vec![star], ..SystemBodies::default() };
            (address, inside)
        };
        let mut rows = std::collections::HashMap::new();
        for (address, inside) in [scanned(1, "N"), scanned(2, "G")] {
            rows.insert(address, inside);
        }
        Directory::at(&dir).write_held_bodies(rows);

        let wrote = rewrite(&dir, &TableSet::new(), &|| false, &mut |_| {})
            .expect("the payloads rewrite");
        assert_eq!(wrote.systems, 3, "not every system was rewritten");
        assert_eq!(wrote.classed, 2, "the scan record was not joined on");

        // Every position back exactly, and the kinds where they were known.
        let mut seen = std::collections::HashMap::new();
        let index = Index::read(&dir).expect("the index reads at its version");
        for cell in index.cells() {
            let bytes = match std::fs::read(payload_path(&dir, cell.id)) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            for point in payload_points(cell.id, &bytes).expect("columns") {
                seen.insert(point.id64, (point.position, point.kind));
            }
        }
        assert_eq!(seen.len(), 3);
        for held in placed {
            let (at, kind) = seen[&held.id64];
            assert_eq!(at, held.position, "a position moved in the rewrite");
            let wanted = match held.id64 {
                1 => StarKind::Neutron,
                2 => StarKind::G,
                _ => StarKind::Unknown,
            };
            assert_eq!(
                kind, wanted,
                "system {} took the wrong kind",
                held.id64
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Running it twice is running it once
    ///
    /// A galaxy is hours of this and a run may be stopped in the middle,
    /// so a cell already columnar is counted and left alone.
    #[test]
    fn a_second_rewrite_leaves_the_columns_alone() {
        let dir = scratch("twice");
        let mut tree =
            Tree::build(&[system(7, [1.0, 2.0, 3.0])], &BuildParams::default());
        tree.write(&dir).expect("a written tree");

        let again = rewrite(&dir, &TableSet::new(), &|| false, &mut |_| {})
            .expect("a rewrite");
        assert_eq!(again.cells, 0, "a columnar payload was rewritten");
        assert!(again.kept > 0, "nothing was recognised as already columnar");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rewrite brings the contributed tables forward as well
    ///
    /// Nothing else will. An open over a directory this build cannot read
    /// does *nothing* — [`crate::ops::migrate::migrate`] asks
    /// [`crate::store::Directory::stale_index`] first and returns having named this
    /// command — so a table in a shape its owner has moved on from would
    /// still be in it after the payloads come forward, and a reader that asks
    /// for it without opening the galaxy first, such as the map's perf guard
    /// reading the router's supercharge table, gets a decode error.
    #[test]
    fn a_rewrite_upgrades_the_contributed_tables() {
        /// A table whose every upgrade rewrites three rows.
        struct Stale;
        impl crate::store::tables::Table for Stale {
            const NAME: &'static str = "stale";
            const ABOUT: &'static str = "A table always behind.";
            type Row = i64;
            fn address(row: &i64) -> i64 {
                *row
            }
            fn derive(_: &crate::system::System) -> Option<i64> {
                None
            }
            fn upgrade(_: &Path) -> io::Result<Option<usize>> {
                Ok(Some(3))
            }
        }

        let dir = scratch("tables");
        let tree =
            Tree::build(&[system(7, [1.0, 2.0, 3.0])], &BuildParams::default());
        tree.clone().write(&dir).expect("a written tree");

        let tables = TableSet::new().with::<Stale>();
        let wrote =
            rewrite(&dir, &tables, &|| false, &mut |_| {}).expect("a rewrite");
        assert_eq!(wrote.upgraded, 3, "the contributed table stayed behind");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
