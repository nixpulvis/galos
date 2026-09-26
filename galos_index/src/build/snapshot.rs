//! The batch build: a whole galaxy raised into a tree at once, and the tree
//! written to a directory.
//!
//! [`Snapshot::build`] turns a list of systems into a tree in one pass: the
//! split, the magnitude ordering, the aggregates. The same systems build the
//! same tree however they arrive, which is what lets a regional build
//! ([`crate::build::region`]) and the live tree ([`crate::build::tree`]) be
//! checked against it. A built tree is written whole, or as a [`CellDiff`]
//! against the one before it, which touches only the cells whose systems
//! moved.

use crate::core::aggregate::{Aggregate, Cell};
use crate::core::codec::Encode;
use crate::core::geometry::{CellId, MAX_LEVEL};
use crate::core::record::{Point, System};
use crate::format::layout::{
    INDEX_FILE, PAYLOAD_DIR, legacy_payload_path, payload_path,
};
use crate::read::index::Index;
use crate::store::cells::write_payload;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

/// How many systems an internal node owns in its own slice.
///
/// Small: budget granularity matters most at coarse levels, where one
/// expansion moves many points.
pub const INTERNAL_SLICE: usize = 512;

/// The most systems a cell holds before it splits, and the most a leaf owns.
///
/// Large, bulk transfer mattering more than granularity at the leaves. A
/// cell over this divides; one that cannot stays a leaf regardless.
pub const LEAF_CAP: usize = 4096;

/// The two cuts the build turns on: how big a slice each kind of node owns.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BuildParams {
    /// The slice an internal node owns, [`INTERNAL_SLICE`] by default.
    pub internal_slice: usize,
    /// The split threshold and the slice a leaf owns, [`LEAF_CAP`] by default.
    pub leaf_cap: usize,
}

impl Default for BuildParams {
    fn default() -> BuildParams {
        BuildParams { internal_slice: INTERNAL_SLICE, leaf_cap: LEAF_CAP }
    }
}

/// A built tree: the index the walks plan on and the per-cell payloads.
///
/// The index is the aggregates and rank ranges, a few megabytes over a galaxy
/// and always resident. The payloads are the systems themselves, keyed by the
/// cell that owns them, and are what a client fetches a cell at a time. Every
/// system sits in exactly one cell's payload.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub index: Index,
    pub payloads: HashMap<CellId, Vec<Point>>,
}

/// Which cells changed between one build and the next, the whole of what a
/// publisher must touch.
///
/// The index file is small and always rewritten whole; only the payload files
/// are worth diffing, and those are the bulk of a galaxy. `changed` is written
/// afresh and `removed` deleted, so the store on disk ends identical to a full
/// write of the new tree while touching only the cells whose systems actually
/// moved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CellDiff {
    /// Cells whose payload differs and must be rewritten.
    pub changed: Vec<CellId>,
    /// Cells that owned systems before and own none now, whose file must go.
    pub removed: Vec<CellId>,
}

impl Snapshot {
    /// The payload of a cell, empty if the cell owns no systems.
    pub fn payload(&self, id: CellId) -> &[Point] {
        self.payloads.get(&id).map_or(&[], Vec::as_slice)
    }

    /// How many systems the tree holds, across every cell's payload.
    pub fn point_count(&self) -> usize {
        self.payloads.values().map(Vec::len).sum()
    }

    /// Build the light snapshot from a list of systems.
    ///
    /// The order of the input does not matter: the split is by position and the
    /// slicing is by magnitude, so the same systems build the same tree however
    /// they arrive. Within a cell's payload the systems come out brightest
    /// first. For the live, editable form raise a [`Tree`](crate::Tree) with
    /// [`Tree::build`](crate::Tree::build), which builds this and holds it
    /// open.
    pub fn build(systems: &[System], params: &BuildParams) -> Snapshot {
        Snapshot::of_region(CellId::ROOT, systems, &HashSet::new(), params)
    }

    /// Build the subtree of one cell, out of the systems that fall in it.
    ///
    /// [`build`](Self::build) is this with the root cell and nothing claimed;
    /// rooted lower down, a galaxy is raised a region at a time and never held
    /// whole. See [`crate::build::region`], which decides the regions, works
    /// out what the cells above them own, and joins the pieces.
    ///
    /// `claimed` is the systems of this region that a cell *above* it took:
    /// they are still in `systems`, every cell here holding them, but
    /// nothing here owns them. It is the whole of the coupling between a
    /// region and the rest of the galaxy.
    pub fn of_region(
        region: CellId,
        systems: &[System],
        claimed: &HashSet<u64>,
        params: &BuildParams,
    ) -> Snapshot {
        let leaves = split_into_leaves(region, systems, params.leaf_cap);
        let (cells, child_mask) = tree_of(region, &leaves);
        let aggregates = roll_up(systems, &leaves, &cells);
        let Slices { payloads, rank_lo, owned } =
            assign_slices(region, systems, &leaves, claimed, params);

        let built_cells = cells.iter().map(|&id| {
            let lo = rank_lo.get(&id).copied().unwrap_or(0);
            let slice = owned.get(&id).copied().unwrap_or(0) as u64;
            Cell {
                id,
                rank_lo: lo,
                rank_hi: lo + slice,
                child_mask: child_mask.get(&id).copied().unwrap_or(0),
                aggregate: aggregates
                    .get(&id)
                    .copied()
                    .unwrap_or(Aggregate::ZERO),
            }
        });

        Snapshot { index: Index::from_cells(built_cells), payloads }
    }

    /// Which cells' payloads differ from a previous build `since`.
    ///
    /// `self` is the new build. A cell is `changed` when its systems are not
    /// byte-for-byte what they were and `removed` when it owned systems before
    /// and owns none now; an untouched cell is in neither, so its file is left
    /// exactly as it lies.
    pub fn diff(&self, since: &Snapshot) -> CellDiff {
        let mut dirtied = CellDiff::default();
        let ids: HashSet<CellId> = since
            .payloads
            .keys()
            .chain(self.payloads.keys())
            .copied()
            .collect();
        for id in ids {
            let (before, after) = (since.payload(id), self.payload(id));
            if after.is_empty() && !before.is_empty() {
                dirtied.removed.push(id);
            } else if before != after {
                dirtied.changed.push(id);
            }
        }
        dirtied
    }

    /// Rebuild over the updated systems, reporting which cells changed from
    /// this one.
    ///
    /// A full rebuild in CPU, but the write cost is only the [`CellDiff`]
    /// cells, positions being immutable and churn clustered. The live
    /// [`Tree`](crate::Tree) cuts the rebuild itself to an O(depth) edit and
    /// lands the same directory.
    pub fn rebuild(
        &self,
        systems: &[System],
        params: &BuildParams,
    ) -> (Snapshot, CellDiff) {
        let next = Snapshot::build(systems, params);
        let dirtied = next.diff(self);
        (next, dirtied)
    }
}

/// Drop every system into `root` and split any cell past the cap, returning
/// each leaf and the systems that fell in it. The one system-to-leaf map the
/// rest of the build reads back through the leaf a system landed in.
///
/// `root` is [`CellId::ROOT`] for a build of the whole galaxy and the region
/// cell for a build of one region; see [`Snapshot::of_region`]. Every system
/// handed over must fall inside it.
fn split_into_leaves(
    root: CellId,
    systems: &[System],
    leaf_cap: usize,
) -> HashMap<CellId, Vec<usize>> {
    let mut leaves: HashMap<CellId, Vec<usize>> = HashMap::new();
    let mut stack: Vec<(CellId, Vec<usize>)> =
        vec![(root, (0..systems.len()).collect())];

    while let Some((id, members)) = stack.pop() {
        if members.len() <= leaf_cap || id.level >= MAX_LEVEL {
            leaves.insert(id, members);
            continue;
        }
        let mut groups: HashMap<CellId, Vec<usize>> = HashMap::new();
        for i in members {
            let child = CellId::of_point(systems[i].position, id.level + 1);
            groups.entry(child).or_default().push(i);
        }
        stack.extend(groups);
    }

    // An empty galaxy is still a tree: the root leaf, holding nothing.
    if leaves.is_empty() {
        leaves.insert(root, Vec::new());
    }
    leaves
}

/// The set of every cell from `root` down (leaves and the cells that hold
/// them) and each cell's mask of which octants have a child.
///
/// The walk up stops at `root`: for a whole build that is [`CellId::ROOT`]
/// and stops where `parent` runs out, and for a region it stops at the
/// region, whose own ancestors belong to whoever is building them.
fn tree_of(
    root: CellId,
    leaves: &HashMap<CellId, Vec<usize>>,
) -> (HashSet<CellId>, HashMap<CellId, u8>) {
    let mut cells: HashSet<CellId> = HashSet::new();
    let mut child_mask: HashMap<CellId, u8> = HashMap::new();

    for &leaf in leaves.keys() {
        cells.insert(leaf);
        child_mask.entry(leaf).or_insert(0);
        let mut c = leaf;
        while c != root
            && let Some(p) = c.parent()
        {
            *child_mask.entry(p).or_insert(0) |= 1 << c.octant();
            cells.insert(p);
            c = p;
        }
    }
    (cells, child_mask)
}

/// The subtree totals of every cell, leaves summed from their systems and
/// internal nodes rolled up from their children.
///
/// Deepest first, so a node has all its children before it merges into its
/// parent. The result at the root is the whole galaxy, and every cell between
/// is the exact total of the systems beneath it.
fn roll_up(
    systems: &[System],
    leaves: &HashMap<CellId, Vec<usize>>,
    cells: &HashSet<CellId>,
) -> HashMap<CellId, Aggregate> {
    let mut agg: HashMap<CellId, Aggregate> =
        cells.iter().map(|&c| (c, Aggregate::ZERO)).collect();

    for (&leaf, members) in leaves {
        let a = members.iter().fold(Aggregate::ZERO, |a, &i| {
            let s = &systems[i];
            a.merge(Aggregate::of_system(
                s.position,
                s.absolute_magnitude,
                s.temperature,
                s.age_bucket,
            ))
        });
        agg.insert(leaf, a);
    }

    let mut ordered: Vec<CellId> = cells.iter().copied().collect();
    ordered.sort_by_key(|a| std::cmp::Reverse(a.level));
    for c in ordered {
        if let Some(p) = c.parent() {
            let child = agg[&c];
            if let Some(parent) = agg.get_mut(&p) {
                *parent = parent.merge(child);
            }
        }
    }
    agg
}

/// What [`assign_slices`] hands back: the per-cell payloads and the two counts
/// the index needs beside them.
struct Slices {
    /// Each cell's owned systems, packed into its payload.
    payloads: HashMap<CellId, Vec<Point>>,
    /// Each cell's `rank_lo`: how many of its subtree its ancestors claimed.
    rank_lo: HashMap<CellId, u64>,
    /// How many systems each cell owns in its own slice.
    owned: HashMap<CellId, usize>,
}

/// Place each system at the shallowest cell on its path with room, brightest
/// first, and pack it into that cell's payload.
///
/// Returns the payloads, each cell's `rank_lo` (how many of its subtree the
/// ancestors claimed), and how many systems each cell owns. A system claimed
/// shallow raises the rank floor of every deeper cell on its path, those
/// cells' subtrees holding it but not owning it.
///
/// A region's build as well as a galaxy's: the walk starts at `root` rather
/// than at level zero, and a system in `claimed` was already taken by a cell
/// *above* `root`, so it competes for nothing here and only raises the rank
/// floor of every cell on its path. A whole build passes [`CellId::ROOT`]
/// and an empty claim.
fn assign_slices(
    root: CellId,
    systems: &[System],
    leaves: &HashMap<CellId, Vec<usize>>,
    claimed: &HashSet<u64>,
    params: &BuildParams,
) -> Slices {
    let mut leaf_of: HashMap<usize, CellId> = HashMap::new();
    for (&leaf, members) in leaves {
        for &i in members {
            leaf_of.insert(i, leaf);
        }
    }

    let mut order: Vec<usize> = (0..systems.len()).collect();
    order.sort_by(|&a, &b| {
        systems[a]
            .absolute_magnitude
            .total_cmp(&systems[b].absolute_magnitude)
            .then(systems[a].id64.cmp(&systems[b].id64))
    });

    let mut payloads: HashMap<CellId, Vec<Point>> = HashMap::new();
    let mut slice_count: HashMap<CellId, usize> = HashMap::new();
    let mut rank_lo: HashMap<CellId, u64> = HashMap::new();

    for i in order {
        let s = &systems[i];
        let leaf = leaf_of[&i];
        let claimed_above = claimed.contains(&s.id64);

        // Which level owns it. A leaf never overflows — it holds at most
        // `leaf_cap` members and its own slice is that wide — so a system
        // nothing above claimed always finds room somewhere on its path.
        let mut owner = leaf.level;
        if !claimed_above {
            for level in root.level..=leaf.level {
                let cid = CellId::of_point(s.position, level);
                let cap = if cid == leaf {
                    params.leaf_cap
                } else {
                    params.internal_slice
                };
                let count = slice_count.entry(cid).or_insert(0);
                if *count < cap {
                    *count += 1;
                    owner = level;
                    payloads.entry(cid).or_default().push(Point::new(
                        s.id64,
                        s.position,
                        s.absolute_magnitude,
                        s.temperature,
                        s.updated_at,
                        s.kind,
                    ));
                    break;
                }
            }
        }

        // The cells that hold it without owning it: everything below the
        // owner, or everything in this build where the owner is above it.
        let held_by = match claimed_above {
            true => root.level,
            false => owner + 1,
        };
        for level in held_by..=leaf.level {
            let cid = CellId::of_point(s.position, level);
            *rank_lo.entry(cid).or_insert(0) += 1;
        }
    }

    Slices { payloads, rank_lo, owned: slice_count }
}

impl Snapshot {
    /// Write the whole tree to a directory: the index file and one payload
    /// file per cell that owns any systems. Existing files are overwritten,
    /// and a cell the previous tree had and this one does not is left
    /// standing — this writes what it holds and reads nothing.
    ///
    /// That "a full write goes to a fresh directory" was the assumption, and
    /// directories are not fresh: a rebuild over a published one left 200,248
    /// payloads and 4.9 GB of a tree nothing refers to. Sweeping them is
    /// [`sweep_payloads`](crate::store::cells::sweep_payloads), which a build
    /// calls once its index file stands; [`write_diff`](Self::write_diff) is
    /// the incremental publish, which removes what it is told went.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        self.write_payloads(dir)?;
        self.index.write(dir)
    }

    /// Write the payloads and no index file.
    ///
    /// What a regional build writes: its cells are only part of the
    /// galaxy's, so the index file belongs to whoever joins them — see
    /// [`crate::build::region`]. A whole build is this and then the index.
    pub fn write_payloads(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir.join(PAYLOAD_DIR))?;
        for (&id, points) in &self.payloads {
            if !points.is_empty() {
                write_payload(
                    dir,
                    id,
                    crate::format::payload::payload_bytes(id, points),
                )?;
            }
        }
        Ok(())
    }

    /// Apply a diff to a directory already holding the previous tree: rewrite
    /// the index whole, write the changed cells, and delete the removed ones.
    /// The directory ends identical to a full [`write`](Self::write) of this
    /// tree, having touched only the cells whose systems moved.
    pub fn write_diff(&self, dir: &Path, dirtied: &CellDiff) -> io::Result<()> {
        fs::create_dir_all(dir.join(PAYLOAD_DIR))?;
        fs::write(dir.join(INDEX_FILE), self.index.to_bytes())?;
        for &id in &dirtied.changed {
            write_payload(
                dir,
                id,
                crate::format::payload::payload_bytes(id, self.payload(id)),
            )?;
        }
        for &id in &dirtied.removed {
            for path in [payload_path(dir, id), legacy_payload_path(dir, id)] {
                match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A grid of systems spaced `step` ly apart, `n` on a side, each a touch
    /// brighter than the last so the ordering is unambiguous. Positions are
    /// pulled toward the cube centre so they sit well inside it whatever `n`
    /// and `step` are.
    fn grid(n: usize, step: f64) -> Vec<System> {
        let mut out = Vec::new();
        let span = (n as f64 - 1.0) * step;
        let base = [-span / 2.0, 900.0 - span / 2.0, 24400.0 - span / 2.0];
        let mut id = 1u64;
        for x in 0..n {
            for y in 0..n {
                for z in 0..n {
                    out.push(System {
                        id64: id,
                        position: [
                            base[0] + x as f64 * step,
                            base[1] + y as f64 * step,
                            base[2] + z as f64 * step,
                        ],
                        absolute_magnitude: id as f64 * 0.001,
                        temperature: 5000.0,
                        age_bucket: 0,
                        updated_at: 0,
                        kind: crate::core::record::StarKind::G,
                    });
                    id += 1;
                }
            }
        }
        out
    }

    /// Every system lands in exactly one cell's payload, and none is lost or
    /// duplicated. This is the invariant the whole additive scheme rests on.
    #[test]
    fn every_system_is_owned_exactly_once() {
        let systems = grid(20, 100.0); // 8,000 systems, forces a split
        let built = Snapshot::build(&systems, &BuildParams::default());

        assert_eq!(built.point_count(), systems.len());
        let mut seen: HashSet<u64> = HashSet::new();
        for points in built.payloads.values() {
            for p in points {
                assert!(seen.insert(p.id64), "id {} owned twice", p.id64);
            }
        }
        let want: HashSet<u64> = systems.iter().map(|s| s.id64).collect();
        assert_eq!(seen, want);
    }

    /// The root owns the brightest systems and nothing fainter than what it
    /// left to its children — the boundary the magnitude ordering is easiest
    /// to break at.
    #[test]
    fn the_root_owns_the_brightest() {
        let systems = grid(20, 100.0);
        let params = BuildParams::default();
        let built = Snapshot::build(&systems, &params);

        let root = built.payload(CellId::ROOT);
        assert_eq!(root.len(), params.internal_slice.min(systems.len()));

        let brightest_in_root =
            root.iter().map(|p| p.magnitude).fold(f32::MIN, f32::max);
        let owned: HashSet<u64> = root.iter().map(|p| p.id64).collect();
        for s in &systems {
            if !owned.contains(&s.id64) {
                assert!(
                    s.absolute_magnitude as f32 >= brightest_in_root,
                    "a system brighter than the root's faintest was left out",
                );
            }
        }
    }

    /// Within a cell the payload is brightest first, which the client leans
    /// on to draw a prefix without re-sorting.
    #[test]
    fn a_payload_is_ordered_brightest_first() {
        let built = Snapshot::build(&grid(20, 100.0), &BuildParams::default());
        for points in built.payloads.values() {
            for pair in points.windows(2) {
                assert!(pair[0].magnitude <= pair[1].magnitude);
            }
        }
    }

    /// The root aggregate is the whole galaxy: every system counted once and
    /// every flux summed, whatever cell drew it — what a splat over an
    /// unloaded region stands on.
    #[test]
    fn the_root_aggregate_is_the_whole_galaxy() {
        let systems = grid(16, 80.0);
        let built = Snapshot::build(&systems, &BuildParams::default());
        let root = built.index.root().expect("root exists");

        assert_eq!(root.aggregate.count(), systems.len() as u64);

        let want_flux: f64 = systems
            .iter()
            .map(|s| galos_photometry::Magnitude(s.absolute_magnitude).flux().0)
            .sum();
        assert!(
            (root.aggregate.total_flux() - want_flux).abs() < want_flux * 1e-9
        );
    }

    /// An internal node's aggregate is exactly its children's, merged, so
    /// refining cannot pump brightness or lose a star.
    #[test]
    fn a_parents_aggregate_composes_from_its_children() {
        let systems = grid(20, 100.0);
        let built = Snapshot::build(&systems, &BuildParams::default());

        for cell in built.index.cells() {
            if cell.is_leaf() {
                continue;
            }
            let from_children = built
                .index
                .children(cell)
                .fold(Aggregate::ZERO, |a, c| a.merge(c.aggregate));
            assert_eq!(cell.aggregate.count(), from_children.count());
            assert!(
                (cell.aggregate.total_flux() - from_children.total_flux())
                    .abs()
                    < cell.aggregate.total_flux() * 1e-9
            );
        }
    }

    /// A cell's rank range is `[claimed by ancestors, that plus its own
    /// slice)`, and a leaf's top rank is its whole subtree: the leaf owns
    /// everything its ancestors did not.
    #[test]
    fn ranks_are_contiguous_down_each_path() {
        let systems = grid(20, 100.0);
        let built = Snapshot::build(&systems, &BuildParams::default());

        let root = built.index.root().unwrap();
        assert_eq!(root.rank_lo, 0);
        assert_eq!(root.rank_hi, built.payload(CellId::ROOT).len() as u64);

        for cell in built.index.cells() {
            assert_eq!(
                cell.slice_len(),
                built.payload(cell.id).len() as u64,
                "slice length must match the payload it stands for",
            );
            if cell.is_leaf() {
                assert_eq!(
                    cell.rank_hi,
                    cell.aggregate.count(),
                    "a leaf owns the whole tail of its subtree",
                );
            }
        }
    }

    /// A tight cluster splits and a sparse field does not, and no leaf holds
    /// more than the cap unless it cannot divide any further.
    #[test]
    fn dense_regions_split_and_sparse_ones_do_not() {
        let sparse = Snapshot::build(&grid(10, 500.0), &BuildParams::default());
        assert_eq!(sparse.index.len(), 1, "1,000 systems fit in the root leaf");
        assert!(sparse.index.root().unwrap().is_leaf());

        let dense = Snapshot::build(&grid(20, 100.0), &BuildParams::default());
        assert!(dense.index.len() > 1, "8,000 systems force a split");
        for cell in dense.index.cells() {
            if cell.is_leaf() {
                assert!(built_leaf_within_cap(&dense, cell.id));
            }
        }
    }

    fn built_leaf_within_cap(built: &Snapshot, id: CellId) -> bool {
        built.payload(id).len() <= LEAF_CAP
    }

    /// Positions survive the payload bytes exactly, whatever cell owns
    /// them.
    #[test]
    fn positions_round_trip_through_the_payload() {
        let systems = grid(16, 80.0);
        let built = Snapshot::build(&systems, &BuildParams::default());
        let by_id: HashMap<u64, [f64; 3]> =
            systems.iter().map(|s| (s.id64, s.position)).collect();

        for cell in built.index.cells() {
            let bytes = crate::format::payload::payload_bytes(
                cell.id,
                built.payload(cell.id),
            );
            let back = crate::format::payload::payload_points(cell.id, &bytes)
                .unwrap();
            assert_eq!(back.len(), built.payload(cell.id).len());
            for p in &back {
                assert_eq!(
                    p.pos, by_id[&p.id64],
                    "position not carried exactly"
                );
            }
        }
    }

    /// An empty galaxy still builds a well-formed tree: one empty root,
    /// nothing owned, no panic.
    #[test]
    fn an_empty_build_is_a_bare_root() {
        let built = Snapshot::build(&[], &BuildParams::default());
        assert_eq!(built.index.len(), 1);
        assert_eq!(built.point_count(), 0);
        let root = built.index.root().unwrap();
        assert!(root.is_leaf());
        assert_eq!(root.aggregate.count(), 0);
    }
}
