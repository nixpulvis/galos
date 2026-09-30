//! Bringing a built directory up to the format this build reads.
//!
//! One place for the migrations an operator runs on purpose, rather than one
//! command per format change: `galos index migrate` is what
//! [`crate::tree::index::Index::read`]'s refusal names, and what it does is
//! whatever the directory turns out to need. At present that is two steps,
//! taken in the one pass over the cells: rewriting legacy payloads as
//! columns, and counting the star kinds into index records written before
//! they had them. Anything further lands here beside them rather than as
//! another subcommand named after a layout.
//!
//! Not run at open, unlike [`crate::ops::migrate::migrate`]'s resharding. That
//! is a rename a file and this is a read of every payload — and for the
//! oldest, a re-encode of every cell plus a sweep of the scan record — which
//! a reader that wants to draw cannot spend without saying so.
//!
//! **A rebuild that is not a reimport.** Neither step goes back to the dump
//! the directory was imported from, which is hours of a different order:
//!
//! - A legacy payload (before version 3) holds everything a columnar one
//!   does but one field: the star kind. That field is derivable from the
//!   directory itself — `bodies/` is the scan record the class comes from.
//!   Per cell: read the legacy block, join the kind on, write the columnar
//!   block.
//! - An index record before version 4 holds everything but the aggregate's
//!   star-kind histogram, and that is derivable from the payloads: every
//!   system sits in exactly one cell's payload, with the kind the build
//!   gave it, and a cell's aggregate is the total over every system whose
//!   position lies inside it. So each payload system is walked down the tree
//!   from the cell that holds it to the deepest cell over its position,
//!   counted there, and the counts are rolled up to the root. Every cell's
//!   histogram must then sum to the `count` its record already states, and a
//!   directory where one does not is refused whole, `index.bin` untouched.
//!
//! Then `index.bin` is replaced — beside it and renamed over it, once every
//! cell is forward — so its version says what the directory now is. Until
//! that rename the old index stands, and it is the only copy of the
//! aggregates' moments and photometry there is, so it is never written in
//! place. Last, the contributed tables come forward as an open would bring
//! them, over a tree `Index::read` now accepts; a run cut short there
//! leaves a directory the next open finishes.

use crate::codec::Directory;
use crate::codec::bytes::{Decode as _, Encode as _, FixedCodec as _};
use crate::codec::cells::Payload;
use crate::codec::cells::format::{
    CELL_LEN_BEFORE_KINDS, FIRST_WITH_KINDS, INDEX_VERSION, POSITION_STEP,
    cell_before_kinds, index_version, legacy_payload_points, payload_bytes,
    payload_head,
};
use crate::codec::layout::{INDEX_FILE, legacy_payload_path, payload_path};
use crate::codec::tables::TableSet;
use crate::core::geometry::CellId;
use crate::core::star::StarKind;
use crate::tree::cell::Cell;
use crate::tree::index::Index;
use std::collections::HashMap;
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
    /// How many cells' payloads were counted into the star kinds of an index
    /// written before it had them; none for an index that has them already.
    pub counted: u64,
    /// Rows the contributed tables' own upgrades rewrote
    ///
    /// [`crate::codec::tables::Table::upgrade`] is a step of an open rather than
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

/// Bring every cell in `dir` to the format this build reads
///
/// Both steps of the module header, in one pass over the cells: a legacy
/// payload is rewritten as columns, and where the index was written before
/// it had star kinds, every payload is counted into them.
///
/// Idempotent: a cell already columnar is counted and left alone, so a run
/// interrupted part way is finished by running it again, and the counting
/// is redone whole, off the payloads as they then stand. The index file is
/// replaced once every cell is forward, for that reason — a directory whose
/// `index.bin` still names a stale version is one the rewrite has not
/// finished, and nothing reads it until it does.
pub fn rewrite(
    dir: &Path,
    tables: &TableSet,
    stop: &(dyn Fn() -> bool + Sync),
    said: &mut dyn FnMut(&Rewrote),
) -> io::Result<Rewrote> {
    let (index, version) = read_any_version(dir)?;
    let mut tally = (version < FIRST_WITH_KINDS).then(|| Tally::new(&index));

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
    for (seen, cell) in index.cells().enumerate() {
        if stop() {
            return Ok(wrote);
        }
        if seen % 4096 == 4095 {
            said(&wrote);
        }

        // Mapped, so counting a columnar payload's kinds faults its
        // position and kind columns and nothing else: seven bytes a
        // system of a payload's twenty-four.
        if let Some(payload) = Payload::open(dir, cell.id)? {
            wrote.kept += 1;
            if let Some(tally) = tally.as_mut() {
                tally.payload(
                    cell.id,
                    (0..payload.len()).map(|at| {
                        (payload.position_at(at), payload.kind_at(at))
                    }),
                )?;
                wrote.counted += 1;
            }
            continue;
        }
        let Some(bytes) = payload_file(dir, cell.id)? else { continue };
        if payload_head(&bytes).is_some() {
            // Columnar and shorter than its header says, which a reader
            // already refuses as empty and this has nothing to rewrite
            // from. Left as it stands: a count of kinds comes up short of
            // the record's own count for it, and says so.
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
        if let Some(tally) = tally.as_mut() {
            tally.payload(
                cell.id,
                points.iter().map(|point| (point.position, point.kind)),
            )?;
            wrote.counted += 1;
        }

        Directory::at(dir)
            .write_payload(cell.id, payload_bytes(cell.id, &points))?;
    }

    // Checked before the index is written: a directory whose kinds do not
    // come out exact keeps its old index, and says which cell.
    let index = match tally {
        Some(tally) => tally.index(dir)?,
        None => index,
    };

    // Once every cell is forward, so an interrupted run is told apart from
    // a finished one by the one file every reader checks first. Beside it
    // and renamed over it: the old file is the only record of the
    // aggregates there is, and a write in place cut short would leave
    // neither version.
    let path = dir.join(INDEX_FILE);
    let beside = path.with_extension("tmp");
    std::fs::write(&beside, index.to_bytes())?;
    std::fs::rename(&beside, &path)?;

    // The contributed tables, which an open would bring forward but an open
    // over a stale directory never reaches: `migrate` names this command
    // and returns without touching anything. A directory this has finished
    // with is one every reader can read, not one the next open has still
    // to finish. After the index rather than before it, because an upgrade
    // may read the tree — the boost table places its rows off the payloads —
    // and a tree at a stale version is one `Index::read` refuses. Cut short
    // here, the directory is one an open finishes: it runs these same
    // upgrades over any directory it can read.
    wrote.upgraded = crate::ops::migrate::upgraded(dir, tables)?
        .iter()
        .map(|&(_, rows)| rows as u64)
        .sum();
    said(&wrote);
    Ok(wrote)
}

/// A cell's payload bytes, sharded or flat, or [`None`] where it has none
///
/// The sharded path first and the flat one after it, as
/// [`Index::read_payload`] reads them: a directory brought forward is not
/// necessarily one an open has resharded.
fn payload_file(dir: &Path, id: CellId) -> io::Result<Option<Vec<u8>>> {
    for path in [payload_path(dir, id), legacy_payload_path(dir, id)] {
        match std::fs::read(&path) {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// Star kinds counted per cell, off the payloads, for an index written
/// before its records carried them
///
/// Held beside the walk's nodes rather than in a map by address: a galaxy
/// is two hundred million systems each walked a dozen levels down, and a
/// hash a level is most of the cost of that.
struct Tally<'a> {
    index: &'a Index,
    /// Where each reachable cell sits in `index.nodes`.
    node: HashMap<CellId, usize>,
    /// Kinds per node: first the systems whose deepest cell it is, then,
    /// rolled up, every system inside it.
    kinds: Vec<[u32; StarKind::COUNT]>,
}

impl<'a> Tally<'a> {
    fn new(index: &'a Index) -> Tally<'a> {
        Tally {
            index,
            node: index
                .nodes
                .iter()
                .enumerate()
                .map(|(at, node)| (node.id, at))
                .collect(),
            kinds: vec![[0; StarKind::COUNT]; index.nodes.len()],
        }
    }

    /// Count one cell's payload: each system at the deepest cell over it
    ///
    /// Walked down from the cell that holds it, which its position lies
    /// inside. A columnar position is rounded to the payload's grid, and a
    /// system just short of the cell's far face can round onto it — which
    /// the tree reads as the neighbour's — so the far face is drawn in by
    /// half a step, back to the side the system was on.
    fn payload(
        &mut self,
        cell: CellId,
        points: impl Iterator<Item = ([f64; 3], StarKind)>,
    ) -> io::Result<()> {
        let Some(&from) = self.node.get(&cell) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "cell {cell:?} holds systems and cannot be reached from \
                     the root, so its star kinds cannot be counted"
                ),
            ));
        };
        let far = cell.bounds().max.map(|face| face - POSITION_STEP / 2.0);
        for (at, kind) in points {
            let inside = [0, 1, 2].map(|n| at[n].min(far[n]));
            let deepest = self.index.deepest_below(from, inside);
            self.kinds[deepest][usize::from(kind.code())] += 1;
        }
        Ok(())
    }

    /// Roll the counts up to the root and write them into the records,
    /// refusing the lot unless every cell's come to the count it states
    fn index(mut self, dir: &Path) -> io::Result<Index> {
        // Breadth-first puts every child after its parent, so one pass in
        // reverse has each cell's children whole before it takes them.
        let nodes = &self.index.nodes;
        for at in (0..nodes.len()).rev() {
            let first = nodes[at].first_child as usize;
            for kid in first..first + nodes[at].children as usize {
                let below = self.kinds[kid];
                for (held, more) in self.kinds[at].iter_mut().zip(below) {
                    *held += more;
                }
            }
        }

        let mut cells: Vec<Cell> = Vec::with_capacity(self.index.len());
        let mut wrong: Vec<(CellId, u64, u64)> = Vec::new();
        for cell in self.index.cells() {
            let kinds = match self.node.get(&cell.id) {
                Some(&at) => self.kinds[at],
                None => [0; StarKind::COUNT],
            };
            let placed: u64 = kinds.iter().map(|&n| u64::from(n)).sum();
            if placed != cell.aggregate.count() {
                wrong.push((cell.id, placed, cell.aggregate.count()));
            }
            let mut cell = *cell;
            cell.aggregate.kinds = kinds;
            cells.push(cell);
        }
        if let Some(&(id, placed, count)) = wrong.first() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{}: the payloads place {placed} systems inside cell \
                     {id:?} and its record counts {count} ({} cells of {} \
                     disagree like it), so its star kinds cannot be filled \
                     in exactly; `index.bin` is left as it was",
                    dir.display(),
                    wrong.len(),
                    self.index.len(),
                ),
            ));
        }
        Ok(Index::from_cells(cells))
    }
}

/// The index file's cells, whatever version it claims, and that version
///
/// [`Index::read`] refuses a version it was not built against, which is the
/// rule that makes a stale directory fail loudly rather than decode as
/// nonsense — and exactly what a migration has to get past. Every version
/// this accepts wrote one of two records: before [`FIRST_WITH_KINDS`], the
/// current one short of its star kinds, read back with them zero; from it
/// on, the current one. The body is held to that width exactly, as
/// [`Index`]'s own decode holds it.
fn read_any_version(dir: &Path) -> io::Result<(Index, u16)> {
    let path = dir.join(INDEX_FILE);
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
    let before_kinds = version < FIRST_WITH_KINDS;
    let record = if before_kinds { CELL_LEN_BEFORE_KINDS } else { Cell::LEN };

    // Past the header, which the version check above has read.
    let mut cur = &bytes[4 + 2..];
    let count = u32::decode(&mut cur)
        .ok_or_else(|| refused("a header with no count in it".to_owned()))?
        as usize;
    if cur.len() != count * record {
        return Err(refused(format!(
            "{count} cells of {record} bytes at version {version}, and {} \
             bytes of them",
            cur.len(),
        )));
    }
    let cells = cur
        .chunks_exact(record)
        .map(|bytes| match before_kinds {
            true => cell_before_kinds(bytes),
            false => Cell::decode(&mut &bytes[..]),
        })
        .collect::<Option<Vec<Cell>>>()
        .ok_or_else(|| refused("a cell that does not decode".to_owned()))?;
    Ok((Index::from_cells(cells), version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::snapshot::BuildParams;
    use crate::build::tree::Tree;
    use crate::codec::cells::format::payload_points;
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

    /// Put `dir`'s index back as `version` wrote it, records cut short of
    /// the star kinds — which is the file a migration meets.
    fn written_before_kinds(dir: &Path, version: u16) {
        let path = dir.join(INDEX_FILE);
        let bytes = std::fs::read(&path).expect("an index");
        let (head, body) = bytes.split_at(4 + 2 + 4);
        let mut old = head.to_vec();
        old[4..6].copy_from_slice(&version.to_le_bytes());
        for record in body.chunks_exact(Cell::LEN) {
            old.extend_from_slice(&record[..CELL_LEN_BEFORE_KINDS]);
        }
        std::fs::write(&path, old).expect("an old index");
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
        // Beside an index of the same age, whose records had no kinds.
        written_before_kinds(&dir, 2);

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

        // And the index counts them: the tree's one cell holds all three.
        let root = index.root().expect("a root");
        let mut wanted = [0u32; StarKind::COUNT];
        wanted[usize::from(StarKind::Neutron.code())] = 1;
        wanted[usize::from(StarKind::G.code())] = 1;
        wanted[usize::from(StarKind::Unknown.code())] = 1;
        assert_eq!(root.aggregate.kinds(), &wanted, "the index missed a kind");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tree of systems scattered deep enough to promote, each with a kind.
    ///
    /// Positions on the game's grid, so the payload holds them exactly, and
    /// some on a cell's face, where which child owns one is the tree's floor
    /// to decide and a walk down from a payload has to agree with it.
    fn scattered() -> (Vec<System>, BuildParams) {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let systems = (0..600u64)
            .map(|n| {
                let mut at = [0.0; 3];
                for axis in &mut at {
                    *axis = (next() % (4096 * 32)) as f64 / 32.0 - 2048.0;
                }
                if n % 25 == 0 {
                    at[0] = 64.0 * (n / 25) as f64 - 512.0;
                }
                System {
                    kind: StarKind::from_code((next() % 16) as u8),
                    absolute_magnitude: (next() % 2000) as f64 / 100.0 - 5.0,
                    ..system(n + 1, at)
                }
            })
            .collect();
        (systems, BuildParams { internal_slice: 2, leaf_cap: 8 })
    }

    /// An index from before the star kinds comes forward with the kinds a
    /// build of today would have given it
    ///
    /// Every cell, not just the root: the kinds are counted off the payloads
    /// and a payload holds a cell's own slice, not its subtree — the systems
    /// promoted out of a leaf sit in an ancestor's payload and still count
    /// towards the leaf. The whole index is compared, so nothing else in a
    /// record moved either.
    #[test]
    fn an_index_from_before_the_kinds_is_counted_into_them() {
        let dir = scratch("kinds");
        let (systems, params) = scattered();
        let mut tree = Tree::build(&systems, &params);
        tree.write(&dir).expect("a written tree");
        let fresh = Index::read(&dir).expect("a fresh index");
        assert!(
            fresh.cells().any(|cell| !cell.is_leaf() && cell.slice_len() > 0),
            "no cell holds a promoted slice, so nothing tests the roll-up",
        );

        written_before_kinds(&dir, 3);
        assert!(Index::read(&dir).is_err(), "a version 3 index read as 4");

        let wrote = rewrite(&dir, &TableSet::new(), &|| false, &mut |_| {})
            .expect("the kinds are counted");
        assert_eq!(wrote.cells, 0, "a columnar payload was rewritten");
        let migrated = Index::read(&dir).expect("the index reads at 4");
        for cell in migrated.cells() {
            let placed: u64 =
                cell.aggregate.kinds().iter().map(|&n| u64::from(n)).sum();
            assert_eq!(
                placed,
                cell.aggregate.count(),
                "{:?}'s kinds do not sum to its count",
                cell.id,
            );
        }
        assert_eq!(migrated, fresh, "the migrated index is not the built one");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Kinds that cannot be counted exactly are refused, and the old index
    /// stays
    ///
    /// A payload gone missing leaves its systems uncounted, and an index
    /// with a histogram short of its count would draw a sky whose colors
    /// do not add up. The old file is the only record of the aggregates, so
    /// it is left exactly as it was for a later run to try again.
    #[test]
    fn kinds_that_do_not_add_up_leave_the_old_index() {
        let dir = scratch("short");
        let (systems, params) = scattered();
        let mut tree = Tree::build(&systems, &params);
        tree.write(&dir).expect("a written tree");
        written_before_kinds(&dir, 3);
        let before = std::fs::read(dir.join(INDEX_FILE)).expect("an index");

        let (held, _) = read_any_version(&dir).expect("a version 3 index");
        let lost = held
            .cells()
            .find(|cell| cell.slice_len() > 0)
            .expect("a cell with a payload")
            .id;
        std::fs::remove_file(payload_path(&dir, lost)).expect("a payload");

        let err = rewrite(&dir, &TableSet::new(), &|| false, &mut |_| {})
            .expect_err("kinds short of the count were written");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read(dir.join(INDEX_FILE)).expect("an index"),
            before,
            "the old index was touched",
        );

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
    /// [`crate::codec::Directory::stale_index`] first and returns having named this
    /// command — so a table in a shape its owner has moved on from would
    /// still be in it after the payloads come forward, and a reader that asks
    /// for it without opening the galaxy first, such as the map's perf guard
    /// reading the router's supercharge table, gets a decode error.
    #[test]
    fn a_rewrite_upgrades_the_contributed_tables() {
        /// A table whose every upgrade rewrites three rows.
        struct Stale;
        impl crate::codec::tables::Table for Stale {
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
