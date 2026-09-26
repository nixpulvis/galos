//! Bringing a built directory up to the format this build reads.
//!
//! One place for the migrations an operator runs on purpose, rather than one
//! command per format change: `galos index migrate` is what
//! [`crate::read::index::Index::read`]'s refusal names, and what it does is
//! whatever the directory turns out to need. Today that is the payloads,
//! which became columns; the next thing lands here beside it rather than as
//! another subcommand named after a layout.
//!
//! Not run at open, unlike [`crate::ops::migrate::migrate`]'s resharding. That
//! is a rename a file and this is a re-encode of every cell plus a sweep of the
//! scan record — hours over a galaxy, which a client that wants to draw cannot
//! spend without saying so.
//!
//! **A rebuild that is not a reimport.** The payloads written before the
//! columns hold everything the new ones do but one field: the star kind,
//! which was never in them. That field is derivable from the directory
//! itself — `bodies/` is the scan record the class comes from — so a
//! directory can be brought forward without going back to the dump it was
//! imported from, which is hours of a different order.
//!
//! What it does, per cell: read the old block, join the kind on, write the
//! new block. Then rewrite `index.bin` so its version says what the
//! payloads now are. The cells' own records are untouched — only
//! [`crate::core::record::Point`]'s width changed, and `Cell::LEN` did
//! not — so the tree, the aggregates and every other table stay exactly as
//! they were.

use crate::core::codec::Decode as _;
use crate::core::record::StarKind;
use crate::format::layout::payload_path;
use crate::format::payload::{
    INDEX_VERSION, index_version, legacy_payload_points, payload_bytes,
    payload_head,
};
use crate::read::index::Index;
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
    /// How many cells were written in the new layout.
    pub cells: u64,
    /// How many systems those cells held.
    pub systems: u64,
    /// How many of them a star kind was found for.
    pub classed: u64,
    /// How many cells were already columnar and left alone.
    pub kept: u64,
    /// Supercharge rows given the place they had always implied, where the
    /// table still wanted one
    ///
    /// [`crate::ops::migrate::place_boosts`] is a step of an open rather than
    /// of a build, and an open over a stale directory does nothing at all —
    /// [`crate::ops::migrate::migrate`] sets `upgrade` and returns, having
    /// touched nothing. So a directory brought forward by this command alone
    /// would still hold a two-field table, and anything reading it without
    /// opening the galaxy first — the map's own perf guard did — fails to
    /// decode a row rather than finding a jet cone.
    pub placed: u64,
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
        crate::store::bodies::each_arrival_class(
            dir,
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
/// Idempotent: a cell already in the new layout is counted and left alone,
/// so a run interrupted part way is finished by running it again. The index
/// file is rewritten last, for that reason — a directory whose `index.bin`
/// still says the old version is one the rewrite has not finished, and
/// nothing reads the new payloads until it does.
pub fn rewrite(
    dir: &Path,
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
            crate::store::bodies::pack(dir, stop)?;
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

        crate::store::cells::write_payload(
            dir,
            cell.id,
            payload_bytes(cell.id, &points),
        )?;
        if wrote.cells % 4096 == 0 {
            said(&wrote);
        }
    }

    // The supercharge table, which an open would place but an open over a
    // stale directory never reaches: `migrate` names this command and
    // returns without touching anything. A directory this has finished
    // with is one every reader can read, not one the next open has still
    // to finish.
    let placed = crate::ops::migrate::place_boosts(dir)?;
    wrote.placed = placed.unwrap_or(0) as u64;

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
/// because the record it reads is unchanged: `Cell::LEN` is what it was,
/// and only the payload beside it moved.
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
        let cell = crate::core::aggregate::Cell::decode(&mut cur)
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
    use crate::core::record::System;
    use crate::format::payload::payload_points;
    use crate::records::{Star, SystemBodies};

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

    /// A payload in the layout written before the columns.
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
    /// Which is the whole of what it is for: the old payloads hold
    /// everything but the star kind, and the kind is derivable from the
    /// scan record beside them — so a directory comes forward without
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
        // put back in the old layout — which is the directory a migration
        // meets.
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
                .map(|point| system(point.id64, point.pos))
                .collect();
            crate::store::cells::write_payload(
                &dir,
                cell.id,
                legacy_bytes(&legacy),
            )
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
        crate::store::bodies::write(&dir, rows);

        let wrote = rewrite(&dir, &|| false, &mut |_| {})
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
                seen.insert(point.id64, (point.pos, point.kind));
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
    /// A galaxy is minutes of this and a run may be stopped in the middle,
    /// so a cell already in the new layout is counted and left alone.
    #[test]
    fn a_second_rewrite_leaves_the_columns_alone() {
        let dir = scratch("twice");
        let mut tree =
            Tree::build(&[system(7, [1.0, 2.0, 3.0])], &BuildParams::default());
        tree.write(&dir).expect("a written tree");

        let again = rewrite(&dir, &|| false, &mut |_| {}).expect("a rewrite");
        assert_eq!(again.cells, 0, "a columnar payload was rewritten");
        assert!(again.kept > 0, "nothing was recognised as already columnar");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rewrite brings the supercharge table forward as well
    ///
    /// Nothing else will. An open over a directory this build cannot read does
    /// *nothing* — [`crate::ops::migrate::migrate`] asks
    /// [`crate::store::cells::stale`] first and returns having named this
    /// command — so a table published before a row carried a place would still
    /// be two fields wide after the payloads came forward, and a reader that
    /// asks for the boosts without opening the galaxy first gets a decode error
    /// rather than a jet cone. Which is how it was found: the map's perf guard
    /// reported `invalid length 2, expected struct SystemBoost with 3 elements`
    /// over a directory `galos index migrate` had just said it had finished
    /// with.
    #[test]
    fn a_rewrite_places_the_supercharge_table() {
        /// The row as it was published before it carried a place.
        #[derive(serde::Serialize)]
        struct Unplaced {
            address: i64,
            boost: crate::core::record::Boost,
        }

        let dir = scratch("boosts");
        let at = [1.0, 2.0, 3.0];
        let mut tree = Tree::build(&[system(7, at)], &BuildParams::default());
        tree.write(&dir).expect("a written tree");

        // The names table, which is where a place comes from.
        let mut names =
            crate::store::names::Names::open(&dir).expect("a table");
        names.name(crate::records::NameEntry {
            address: 7,
            name: crate::core::name::SystemName::new("SOL"),
            position: [at[0] as f32, at[1] as f32, at[2] as f32],
        });
        names.publish(&dir).expect("a published name");

        crate::format::msgpack::write_meta(
            &crate::format::layout::boosts_path(&dir),
            &vec![Unplaced {
                address: 7,
                boost: crate::core::record::Boost::Neutron,
            }],
        )
        .expect("a table of the old shape");

        let wrote = rewrite(&dir, &|| false, &mut |_| {}).expect("a rewrite");
        assert_eq!(wrote.placed, 1, "the supercharge table stayed behind");

        // And it reads as the row the router asks for, at the place the
        // names table gave it.
        let table: Vec<crate::records::SystemBoost> =
            crate::format::msgpack::read_meta(
                &crate::format::layout::boosts_path(&dir),
            )
            .expect("the table reads as placed rows");
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].address, 7);
        assert_eq!(table[0].position, [1.0, 2.0, 3.0]);

        // Run again, there is nothing left to place.
        let again = rewrite(&dir, &|| false, &mut |_| {}).expect("a rewrite");
        assert_eq!(again.placed, 0, "a placed table was rewritten");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
