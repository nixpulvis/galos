//! The galaxy tree: the batch build that raises it and the live tree that keeps
//! it current.
//!
//! [`Snapshot::build`] turns a whole galaxy into a tree at once: the split, the
//! magnitude ordering, the aggregates. [`Tree`] is that same tree held open, so a scan
//! arriving on EDDN moves one system and touches only the handful of cells on
//! its path rather than rebuilding anything. That is what lets the index ride a
//! live feed: the work of one edit is the depth of the tree, a dozen cells, not
//! its size.
//!
//! Every edit keeps the tree in exactly the state a fresh [`Snapshot::build`] over
//! the same systems would produce: same cells, same ownership, same payloads. That
//! is not a hope but the contract the oracle test holds it to, comparing the
//! live tree to a rebuild after every single operation. The invariants are the
//! doc's: a system sits in exactly one cell, a cell owns the brightest of its
//! subtree its ancestors did not, and a cell splits at the cap and collapses
//! back under it.
//!
//! Two moves do all the work. **Insert** walks the system down its path,
//! settling it at the shallowest cell with room; where a cell is full and the
//! newcomer is brighter than its faintest, it takes that slot and the evicted
//! system carries on down its own path, one system bumped down one level per
//! step, a chain no longer than the tree is deep. **Remove** is the mirror: the
//! hole a departing system leaves is filled by promoting the brightest system
//! from the children, which leaves a hole one level down, and so on. Splitting a
//! cell that has outgrown the cap and collapsing one that has shrunk under it
//! are local to that cell's own systems.
//!
//! Aggregates are not maintained here. They compose exactly but drift under
//! repeated floating-point addition and subtraction, and `m_min` cannot be
//! recovered from a summed flux at all, so they are computed fresh from the
//! records when the tree is [published](Tree::publish), cheap beside the
//! writes, and exact. What the tree maintains is structure, ownership, and a
//! per-cell count, all of them integer-exact.

use crate::aggregate::{Aggregate, Cell};
use crate::cache::Point;
use crate::geometry::{CellId, MAX_LEVEL};
use crate::walk::Index;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

/// One system as the live tree holds it: its place and its photometry, the
/// input stripped of its id, the key it is stored under.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Record {
    position: [f64; 3],
    magnitude: f64,
    temperature: f64,
    age_bucket: usize,
}

/// A monotonic `u64` image of a magnitude, so a `BTreeSet` orders systems
/// brightest first without a float key. Standard order-preserving transform:
/// negatives flip every bit, non-negatives flip the sign bit, and the result
/// sorts as the `f64` did. Magnitudes are finite, so no NaN reaches this.
fn mag_key(magnitude: f64) -> u64 {
    let bits = magnitude.to_bits();
    if bits & 0x8000_0000_0000_0000 != 0 {
        !bits
    } else {
        bits | 0x8000_0000_0000_0000
    }
}

/// One node of the live tree.
///
/// `slice` is what the cell owns, ordered `(magnitude, id)` so its brightest is
/// first and its faintest last. `physical` is what physically falls in the cell
/// and is kept only at leaves, where a split reads it; an internal node's
/// physical members live in its descendants. `count` is the subtree's physical
/// total, held as an integer so the split and collapse tests never touch a
/// drifting float.
#[derive(Clone, Debug, Default)]
struct Node {
    child_mask: u8,
    count: u64,
    slice: BTreeSet<(u64, u64)>,
    physical: Vec<u64>,
}

impl Node {
    fn is_leaf(&self) -> bool {
        self.child_mask == 0
    }
}

/// The galaxy index, held open for editing.
///
/// Raised once from every system, then moved a system at a time as a feed
/// reports changes. [`publish`](Self::publish) writes what has changed since the
/// last one, so a directory is kept current by rewriting only the cells the
/// edits touched.
#[derive(Clone, Debug)]
pub struct Tree {
    cells: HashMap<CellId, Node>,
    records: HashMap<u64, Record>,
    /// Which cell owns each system: the cell whose slice, hence payload, holds it.
    owner: HashMap<u64, CellId>,
    /// Which leaf each system physically falls in.
    leaf: HashMap<u64, CellId>,
    /// Cells whose payload has changed since the last publish.
    dirty: HashSet<CellId>,
    /// Cells that existed at the last publish and no longer do.
    gone: HashSet<CellId>,
    params: BuildParams,
}

impl Tree {
    /// Build the tree from scratch, then hold it open.
    ///
    /// The first build is the batch [`Snapshot::build`], so the live tree starts in
    /// exactly the state the pass produces; editing keeps it there. Everything
    /// after is incremental.
    pub fn build(systems: &[System], params: &BuildParams) -> Tree {
        let built = Snapshot::build(systems, params);
        let records = systems
            .iter()
            .map(|s| {
                (
                    s.id64,
                    Record {
                        position: s.position,
                        magnitude: s.absolute_magnitude,
                        temperature: s.temperature,
                        age_bucket: s.age_bucket,
                    },
                )
            })
            .collect();

        let mut tree = Tree {
            cells: HashMap::new(),
            records,
            owner: HashMap::new(),
            leaf: HashMap::new(),
            dirty: HashSet::new(),
            gone: HashSet::new(),
            params: *params,
        };

        // Structure and ownership straight from the build.
        for cell in built.index.cells() {
            let mut node = Node {
                child_mask: cell.child_mask,
                count: cell.aggregate.count(),
                slice: BTreeSet::new(),
                physical: Vec::new(),
            };
            for point in built.payload(cell.id) {
                node.slice.insert((
                    mag_key(tree.records[&point.id64].magnitude),
                    point.id64,
                ));
                tree.owner.insert(point.id64, cell.id);
            }
            tree.cells.insert(cell.id, node);
        }

        // Physical membership: the deepest existing cell each system falls in.
        for (&id, rec) in &tree.records {
            let leaf = tree.physical_leaf(rec.position);
            tree.leaf.insert(id, leaf);
            tree.cells.get_mut(&leaf).unwrap().physical.push(id);
        }

        tree
    }

    /// The deepest existing cell that a point falls in.
    fn physical_leaf(&self, position: [f64; 3]) -> CellId {
        let mut cur = CellId::ROOT;
        loop {
            let child = CellId::of_point(position, cur.level + 1);
            if self.cells[&cur].child_mask & (1 << child.octant()) != 0 {
                cur = child;
            } else {
                return cur;
            }
        }
    }

    /// The true leaf a point belongs in, creating it where the octant is empty.
    ///
    /// A descent that stops at an internal cell means the point falls in an
    /// octant that holds nothing yet; the leaf is made there, which is the same
    /// cell a rebuild would create for the first system in that octant. The
    /// returned cell is always a leaf, so physical members never land on an
    /// internal node.
    fn find_or_create_leaf(&mut self, position: [f64; 3]) -> CellId {
        let mut cur = CellId::ROOT;
        loop {
            if self.cells[&cur].child_mask == 0 {
                return cur;
            }
            let child = CellId::of_point(position, cur.level + 1);
            let octant = child.octant();
            if self.cells[&cur].child_mask & (1 << octant) == 0 {
                self.cells.insert(child, Node::default());
                self.cells.get_mut(&cur).unwrap().child_mask |= 1 << octant;
                self.dirty.insert(child);
                return child;
            }
            cur = child;
        }
    }

    /// Apply a batch of changed or new systems.
    ///
    /// A system already known is moved to its new record; one never seen is
    /// added. This is what a feed calls with the systems a run of messages
    /// touched.
    pub fn apply(&mut self, changed: &[System]) {
        for system in changed {
            self.upsert(*system);
        }
    }

    /// Add a system, or move one already present to its new record.
    pub fn upsert(&mut self, system: System) {
        if self.records.contains_key(&system.id64) {
            self.remove(system.id64);
        }
        self.insert(system);
    }

    /// Drop a system the feed reports gone.
    pub fn remove_system(&mut self, id: u64) {
        if self.records.contains_key(&id) {
            self.remove(id);
        }
    }

    /// How many systems the tree holds.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the tree holds no systems.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The inputs this tree was built from, reconstructed from its records: the
    /// full-precision [`System`] values a checkpoint persists so the tree can be
    /// rebuilt without the database. Exact, since a record holds every field a
    /// system carries; order is arbitrary, which the order-independent
    /// [`build`](Self::build) does not care about.
    pub fn to_inputs(&self) -> Vec<System> {
        self.records
            .iter()
            .map(|(&id64, rec)| System {
                id64,
                position: rec.position,
                absolute_magnitude: rec.magnitude,
                temperature: rec.temperature,
                age_bucket: rec.age_bucket,
            })
            .collect()
    }

    // --- insertion --------------------------------------------------------

    fn insert(&mut self, system: System) {
        let rec = Record {
            position: system.position,
            magnitude: system.absolute_magnitude,
            temperature: system.temperature,
            age_bucket: system.age_bucket,
        };
        let id = system.id64;
        self.records.insert(id, rec);

        // Physical placement: drop it in its leaf and count it down the path.
        let leaf = self.find_or_create_leaf(rec.position);
        self.leaf.insert(id, leaf);
        self.cells.get_mut(&leaf).unwrap().physical.push(id);
        self.bump_count(rec.position, leaf.level, 1);

        // A leaf grown past the cap divides before ownership is settled, so the
        // cascade below always has a child to descend into.
        if self.cells[&leaf].physical.len() > self.params.leaf_cap
            && leaf.level < MAX_LEVEL
        {
            self.split(leaf);
        }

        self.own_insert(id);
    }

    /// Add `delta` to the count of every cell from the root down to `level`
    /// along `position`'s path.
    fn bump_count(&mut self, position: [f64; 3], level: u8, delta: i64) {
        for l in 0..=level {
            let cid = CellId::of_point(position, l);
            if let Some(node) = self.cells.get_mut(&cid) {
                node.count = (node.count as i64 + delta) as u64;
            }
        }
    }

    /// Settle a system into the shallowest cell on its path with room,
    /// displacing a fainter owner down its own path where it must.
    fn own_insert(&mut self, start: u64) {
        let mut id = start;
        let mut cur = CellId::ROOT;
        loop {
            let (mk, position) = {
                let r = &self.records[&id];
                (mag_key(r.magnitude), r.position)
            };
            let key = (mk, id);

            let (full, faintest) = {
                let node = &self.cells[&cur];
                let cap = if node.is_leaf() {
                    self.params.leaf_cap
                } else {
                    self.params.internal_slice
                };
                (
                    node.slice.len() >= cap,
                    node.slice.iter().next_back().copied(),
                )
            };

            if !full {
                self.cells.get_mut(&cur).unwrap().slice.insert(key);
                self.owner.insert(id, cur);
                self.dirty.insert(cur);
                return;
            }

            let faintest = faintest.expect("a full cell has a faintest owner");
            if key < faintest {
                let node = self.cells.get_mut(&cur).unwrap();
                node.slice.remove(&faintest);
                node.slice.insert(key);
                self.owner.insert(id, cur);
                self.dirty.insert(cur);
                // The evicted system carries on down its own path.
                let evicted = faintest.1;
                let evicted_pos = self.records[&evicted].position;
                id = evicted;
                cur = CellId::of_point(evicted_pos, cur.level + 1);
            } else {
                cur = CellId::of_point(position, cur.level + 1);
            }
        }
    }

    /// Divide a leaf that has outgrown the cap: hand its systems to eight
    /// children, keep the brightest [`BuildParams::internal_slice`] of what it
    /// owned, and push the rest down.
    fn split(&mut self, id: CellId) {
        let physical =
            std::mem::take(&mut self.cells.get_mut(&id).unwrap().physical);

        // Group physical members by child and create the child leaves.
        let mut by_child: HashMap<CellId, Vec<u64>> = HashMap::new();
        for pid in physical {
            let child =
                CellId::of_point(self.records[&pid].position, id.level + 1);
            by_child.entry(child).or_default().push(pid);
        }
        let mut child_mask = 0u8;
        for (child, ids) in &by_child {
            child_mask |= 1 << child.octant();
            for &pid in ids {
                self.leaf.insert(pid, *child);
            }
            self.cells.insert(
                *child,
                Node {
                    child_mask: 0,
                    count: ids.len() as u64,
                    slice: BTreeSet::new(),
                    physical: ids.clone(),
                },
            );
            self.dirty.insert(*child);
        }
        self.cells.get_mut(&id).unwrap().child_mask = child_mask;
        self.dirty.insert(id);

        // The node keeps the brightest of what it owned; the rest belong to the
        // children now, by the same path rule the cascade uses.
        let slice = std::mem::take(&mut self.cells.get_mut(&id).unwrap().slice);
        let mut kept = BTreeSet::new();
        let mut pushed = Vec::new();
        for (i, key) in slice.into_iter().enumerate() {
            if i < self.params.internal_slice {
                kept.insert(key);
            } else {
                pushed.push(key);
            }
        }
        self.cells.get_mut(&id).unwrap().slice = kept;
        for key in pushed {
            let child =
                CellId::of_point(self.records[&key.1].position, id.level + 1);
            self.cells.get_mut(&child).unwrap().slice.insert(key);
            self.owner.insert(key.1, child);
            self.dirty.insert(child);
        }

        // A child that itself overflowed divides in turn.
        let children: Vec<CellId> = by_child.keys().copied().collect();
        for child in children {
            if self.cells[&child].physical.len() > self.params.leaf_cap
                && child.level < MAX_LEVEL
            {
                self.split(child);
            }
        }
    }

    // --- removal ----------------------------------------------------------

    fn remove(&mut self, id: u64) {
        let rec = self.records[&id];
        let owner = self.owner.remove(&id).unwrap();
        let leaf = self.leaf.remove(&id).unwrap();

        self.cells
            .get_mut(&owner)
            .unwrap()
            .slice
            .remove(&(mag_key(rec.magnitude), id));
        self.dirty.insert(owner);
        self.cells.get_mut(&leaf).unwrap().physical.retain(|&x| x != id);
        self.bump_count(rec.position, leaf.level, -1);
        self.records.remove(&id);

        // Fill the hole the departed owner left by promoting the brightest
        // system from below, which leaves a hole one level down.
        self.pull_up(owner);

        self.repair(leaf, rec.position);
    }

    /// Promote the brightest owned system from the children of `node` up into
    /// it, then repeat one level down, until a leaf or an empty subtree.
    fn pull_up(&mut self, start: CellId) {
        let mut node = start;
        loop {
            if self.cells[&node].is_leaf() {
                return;
            }
            // The brightest system owned anywhere below is the brightest owned
            // by a direct child, each of which owns the brightest of its own.
            let mut best: Option<(u64, u64)> = None;
            let mut from = None;
            for octant in 0..8u8 {
                if self.cells[&node].child_mask & (1 << octant) == 0 {
                    continue;
                }
                let child = node.child(octant);
                if let Some(&key) = self.cells[&child].slice.iter().next()
                    && best.is_none_or(|b| key < b)
                {
                    best = Some(key);
                    from = Some(child);
                }
            }
            let (Some(key), Some(child)) = (best, from) else {
                return;
            };
            self.cells.get_mut(&child).unwrap().slice.remove(&key);
            self.cells.get_mut(&node).unwrap().slice.insert(key);
            self.owner.insert(key.1, node);
            self.dirty.insert(node);
            self.dirty.insert(child);
            node = child;
        }
    }

    /// Restore the structure after a removal: prune cells emptied of systems,
    /// then collapse the shallowest subtree that has shrunk to the cap.
    fn repair(&mut self, leaf: CellId, position: [f64; 3]) {
        // Prune from the leaf up while a cell holds nothing at all.
        let mut cur = leaf;
        while cur != CellId::ROOT && self.cells[&cur].count == 0 {
            let parent = cur.parent().unwrap();
            if self.cells.remove(&cur).is_some() {
                self.dirty.remove(&cur);
                self.gone.insert(cur);
            }
            self.cells.get_mut(&parent).unwrap().child_mask &=
                !(1 << cur.octant());
            cur = parent;
        }

        // Collapse the shallowest internal cell on the path whose subtree now
        // fits in one leaf.
        let mut cur = CellId::ROOT;
        loop {
            let node = &self.cells[&cur];
            let count = node.count;
            if !node.is_leaf()
                && count > 0
                && count <= self.params.leaf_cap as u64
            {
                self.collapse(cur);
                return;
            }
            if node.is_leaf() {
                return;
            }
            let child = CellId::of_point(position, cur.level + 1);
            if self.cells.contains_key(&child) {
                cur = child;
            } else {
                return;
            }
        }
    }

    /// Fold a whole subtree back into one leaf: it owns everything its subtree
    /// owned, holds every physical member, and its descendants are gone.
    fn collapse(&mut self, root: CellId) {
        // Every cell strictly below `root`.
        let mut descendants = Vec::new();
        let mut stack: Vec<CellId> = Vec::new();
        for octant in 0..8u8 {
            if self.cells[&root].child_mask & (1 << octant) != 0 {
                stack.push(root.child(octant));
            }
        }
        while let Some(id) = stack.pop() {
            descendants.push(id);
            for octant in 0..8u8 {
                if self.cells[&id].child_mask & (1 << octant) != 0 {
                    stack.push(id.child(octant));
                }
            }
        }

        let mut physical =
            std::mem::take(&mut self.cells.get_mut(&root).unwrap().physical);
        let mut gained: Vec<(u64, u64)> = Vec::new();
        for id in descendants {
            let node = self.cells.remove(&id).unwrap();
            physical.extend(node.physical);
            gained.extend(node.slice.iter().copied());
            if !node.slice.is_empty() {
                self.gone.insert(id);
            }
            self.dirty.remove(&id);
        }

        let node = self.cells.get_mut(&root).unwrap();
        node.child_mask = 0;
        for key in &gained {
            node.slice.insert(*key);
        }
        node.physical = physical;
        let owned: Vec<u64> = node.slice.iter().map(|&(_, id)| id).collect();
        let members: Vec<u64> = node.physical.clone();
        for id in owned {
            self.owner.insert(id, root);
        }
        for id in members {
            self.leaf.insert(id, root);
        }
        self.dirty.insert(root);
    }

    // --- publishing -------------------------------------------------------

    /// The tree as a [`Snapshot`], aggregates computed fresh from the records.
    ///
    /// This is the whole state, the shape [`Snapshot::build`] returns, so it can be
    /// compared to a rebuild or written whole. Aggregates and `m_min` are
    /// summed here rather than carried, so they are exact; `rank_lo` is the
    /// count of a subtree owned above it, read off the owned-below totals.
    pub fn to_snapshot(&self) -> Snapshot {
        // Fresh aggregates: leaves from their members, internal rolled up.
        let mut agg: HashMap<CellId, Aggregate> =
            self.cells.keys().map(|&id| (id, Aggregate::ZERO)).collect();
        for (&id, node) in &self.cells {
            if node.is_leaf() {
                let a = node.physical.iter().fold(Aggregate::ZERO, |a, pid| {
                    let r = &self.records[pid];
                    a.merge(Aggregate::of_system(
                        r.position,
                        r.magnitude,
                        r.temperature,
                        r.age_bucket,
                    ))
                });
                agg.insert(id, a);
            }
        }
        // Owned-below totals, both rolled up deepest first.
        let mut owned_below: HashMap<CellId, u64> = self
            .cells
            .iter()
            .map(|(&id, node)| (id, node.slice.len() as u64))
            .collect();
        let mut ordered: Vec<CellId> = self.cells.keys().copied().collect();
        ordered.sort_by_key(|a| std::cmp::Reverse(a.level));
        for id in ordered {
            if let Some(parent) = id.parent()
                && self.cells.contains_key(&parent)
            {
                let child_agg = agg[&id];
                let child_owned = owned_below[&id];
                *agg.get_mut(&parent).unwrap() = agg[&parent].merge(child_agg);
                *owned_below.get_mut(&parent).unwrap() += child_owned;
            }
        }

        let mut payloads: HashMap<CellId, Vec<Point>> = HashMap::new();
        let cells = self.cells.iter().map(|(&id, node)| {
            let count = agg[&id].count();
            let rank_lo = count - owned_below[&id];
            let slice_len = node.slice.len() as u64;
            if slice_len > 0 {
                let points = node
                    .slice
                    .iter()
                    .map(|&(_, pid)| {
                        let r = &self.records[&pid];
                        Point::new(pid, r.position, r.magnitude, r.temperature)
                    })
                    .collect();
                payloads.insert(id, points);
            }
            Cell {
                id,
                rank_lo,
                rank_hi: rank_lo + slice_len,
                child_mask: node.child_mask,
                aggregate: agg[&id],
            }
        });

        Snapshot { index: Index::from_cells(cells), payloads }
    }

    /// Write the tree whole to a directory, as a first publish or a reset.
    pub fn write(&mut self, dir: &std::path::Path) -> std::io::Result<()> {
        let built = self.to_snapshot();
        built.write(dir)?;
        self.dirty.clear();
        self.gone.clear();
        Ok(())
    }

    /// Write only what has changed since the last publish, and forget it.
    ///
    /// The index file is small and rewritten whole; the payload files are the
    /// bulk, and only the cells the edits touched are written or removed, which
    /// is what keeps a live directory current for the cost of the churn rather
    /// than the galaxy.
    pub fn publish(&mut self, dir: &std::path::Path) -> std::io::Result<()> {
        let built = self.to_snapshot();
        let mut dirtied = Dirtied::default();
        let touched: HashSet<CellId> =
            self.dirty.iter().chain(self.gone.iter()).copied().collect();
        for id in touched {
            match built.payloads.get(&id) {
                Some(points) if !points.is_empty() => dirtied.changed.push(id),
                _ => dirtied.removed.push(id),
            }
        }
        built.write_diff(dir, &dirtied)?;
        self.dirty.clear();
        self.gone.clear();
        Ok(())
    }
}

// The batch build below raises the whole tree at once; the live tree above
// keeps it current. Both hold the same invariants, so a fresh build and a
// sequence of edits land on the same tree, which is what the oracle checks.

/// How many systems an internal node owns in its own slice.
///
/// Small, because budget granularity matters most at coarse levels where one
/// expansion moves many points. Tuned once a real build has run.
pub const INTERNAL_SLICE: usize = 512;

/// The most systems a cell holds before it splits, and the most a leaf owns.
///
/// Bulk-transfer efficiency wins over granularity at the leaves, so they are
/// large. A cell over this divides; one that cannot stays a leaf regardless.
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

/// One system as the build reads it: where it is and the photometry the
/// ordering and the glow need.
///
/// Absolute magnitude and temperature are the finished figures from the
/// photometry fallback chain (scanned stars summed, else the primary's class,
/// else a default), not anything the build works out. `age_bucket` is the
/// Recency axis the caller has already binned.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct System {
    pub id64: u64,
    pub position: [f64; 3],
    pub absolute_magnitude: f64,
    pub temperature: f64,
    pub age_bucket: usize,
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
pub struct Dirtied {
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
    /// first, the order they were claimed in. For the live, editable form raise
    /// a [`Tree`] with [`Tree::build`] instead, which builds this and holds it
    /// open.
    pub fn build(systems: &[System], params: &BuildParams) -> Snapshot {
        let leaves = split_into_leaves(systems, params.leaf_cap);
        let (cells, child_mask) = tree_of(&leaves);
        let aggregates = roll_up(systems, &leaves, &cells);
        let Slices { payloads, rank_lo, owned } =
            assign_slices(systems, &leaves, params);

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
    pub fn diff(&self, since: &Snapshot) -> Dirtied {
        let mut dirtied = Dirtied::default();
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

    /// Rebuild over the updated systems, reporting which cells changed from this
    /// one.
    ///
    /// A full rebuild in CPU, but the write cost is only the [`Dirtied`] cells,
    /// what a nightly or per-minute publish pays, since positions are immutable
    /// and churn is clustered. The live [`Tree`] cuts the rebuild itself to an
    /// O(depth) edit; the on-disk result is the same, which is what
    /// [`diff`](Self::diff) guarantees and the tests check.
    pub fn rebuild(
        &self,
        systems: &[System],
        params: &BuildParams,
    ) -> (Snapshot, Dirtied) {
        let next = Snapshot::build(systems, params);
        let dirtied = next.diff(self);
        (next, dirtied)
    }
}

/// Drop every system into the cube and split any cell past the cap, returning
/// each leaf and the systems that fell in it. The one system-to-leaf map the
/// rest of the build reads back through the leaf a system landed in.
fn split_into_leaves(
    systems: &[System],
    leaf_cap: usize,
) -> HashMap<CellId, Vec<usize>> {
    let mut leaves: HashMap<CellId, Vec<usize>> = HashMap::new();
    let mut stack: Vec<(CellId, Vec<usize>)> =
        vec![(CellId::ROOT, (0..systems.len()).collect())];

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
        leaves.insert(CellId::ROOT, Vec::new());
    }
    leaves
}

/// The set of every cell (leaves and the ancestors that hold them) and each
/// cell's mask of which octants have a child.
fn tree_of(
    leaves: &HashMap<CellId, Vec<usize>>,
) -> (HashSet<CellId>, HashMap<CellId, u8>) {
    let mut cells: HashSet<CellId> = HashSet::new();
    let mut child_mask: HashMap<CellId, u8> = HashMap::new();

    for &leaf in leaves.keys() {
        cells.insert(leaf);
        child_mask.entry(leaf).or_insert(0);
        let mut c = leaf;
        while let Some(p) = c.parent() {
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
/// ancestors claimed), and how many systems each cell owns. `rank_lo` is
/// counted the same way it is meant: a system claimed shallow raises the rank
/// floor of every deeper cell on its path, since those cells' subtrees hold it
/// but do not own it.
fn assign_slices(
    systems: &[System],
    leaves: &HashMap<CellId, Vec<usize>>,
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
        let mut placed_level = leaf.level;
        for level in 0..=leaf.level {
            let cid = CellId::of_point(s.position, level);
            let cap = if cid == leaf {
                params.leaf_cap
            } else {
                params.internal_slice
            };
            let count = slice_count.entry(cid).or_insert(0);
            if *count < cap {
                *count += 1;
                placed_level = level;
                payloads.entry(cid).or_default().push(Point::new(
                    s.id64,
                    s.position,
                    s.absolute_magnitude,
                    s.temperature,
                ));
                break;
            }
        }

        for level in (placed_level + 1)..=leaf.level {
            let cid = CellId::of_point(s.position, level);
            *rank_lo.entry(cid).or_insert(0) += 1;
        }
    }

    Slices { payloads, rank_lo, owned: slice_count }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    use crate::serialization::{Decode, Encode};
    use std::collections::HashSet;

    /// A grid of systems spaced `step` ly apart, `n` on a side, each a touch
    /// brighter than the last so magnitudes are all distinct and the ordering
    /// is unambiguous. Positions are pulled toward the cube centre so they sit
    /// well inside it whatever `n` and `step` are.
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
    /// left to its children. Magnitude ordering is the invariant the sky's
    /// completeness depends on, so it is checked at the one boundary it is
    /// easiest to break: the split between the root's slice and the rest.
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

    /// Within a cell the payload is brightest first, the order it was claimed
    /// in, which the client leans on to draw a prefix without re-sorting.
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
    /// every flux summed, whatever cell drew it. This is what a splat over an
    /// unloaded region stands on.
    #[test]
    fn the_root_aggregate_is_the_whole_galaxy() {
        let systems = grid(16, 80.0);
        let built = Snapshot::build(&systems, &BuildParams::default());
        let root = built.index.root().expect("root exists");

        assert_eq!(root.aggregate.count(), systems.len() as u64);

        let want_flux: f64 = systems
            .iter()
            .map(|s| galos_photometry::flux(s.absolute_magnitude))
            .sum();
        assert!(
            (root.aggregate.total_flux() - want_flux).abs() < want_flux * 1e-9
        );
    }

    /// An internal node's aggregate is exactly its children's, merged. This is
    /// the composition the LOD cross-fade needs: parent and children integrate
    /// to the same totals, so refining cannot pump brightness or lose a star.
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

    /// A cell's rank range is `[claimed by ancestors, that plus its own slice)`,
    /// and a leaf's top rank is its whole subtree: the leaf owns everything its
    /// ancestors did not. The ranges are what tell how much a cell
    /// adds when it refines.
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

    /// Positions survive the payload bytes exactly, whatever cell owns them, so
    /// a drawn star sits precisely where it belongs.
    #[test]
    fn positions_round_trip_through_the_payload() {
        let systems = grid(16, 80.0);
        let built = Snapshot::build(&systems, &BuildParams::default());
        let by_id: HashMap<u64, [f64; 3]> =
            systems.iter().map(|s| (s.id64, s.position)).collect();

        for cell in built.index.cells() {
            let bytes = built.payload(cell.id).to_bytes();
            let back = Vec::<Point>::from_bytes(&bytes).unwrap();
            assert_eq!(back.len(), built.payload(cell.id).len());
            for p in &back {
                assert_eq!(
                    p.pos, by_id[&p.id64],
                    "position not carried exactly"
                );
            }
        }
    }

    /// An empty galaxy still builds a well-formed tree: one empty root, nothing
    /// owned, no panic.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic PRNG, so a randomized oracle run is reproducible
    /// and needs no dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn magnitude(&mut self) -> f64 {
            // -8 .. +16, the range the sky spans.
            (self.below(24000) as f64) / 1000.0 - 8.0
        }
        fn position(&mut self) -> [f64; 3] {
            // Inside the root cube, clustered near the centre so cells fill and
            // split rather than scatter one-per-leaf.
            let axis =
                |r: &mut Rng, c: f64| c + (r.below(4000) as f64) - 2000.0;
            [axis(self, 0.0), axis(self, 900.0), axis(self, 24400.0)]
        }
    }

    fn input(id: u64, rng: &mut Rng) -> System {
        System {
            id64: id,
            position: rng.position(),
            absolute_magnitude: rng.magnitude(),
            temperature: 3000.0 + (rng.below(20000) as f64),
            age_bucket: rng.below(8) as usize,
        }
    }

    /// The live tree matches a fresh build, cell for cell and system for system.
    /// Aggregates are summed in a different order either side, so their floats
    /// are compared within tolerance; everything discrete is compared exactly.
    fn assert_equivalent(live: &Snapshot, fresh: &Snapshot) {
        assert_eq!(live.index.len(), fresh.index.len(), "cell count differs");
        for cell in fresh.index.cells() {
            let got = live.index.get(cell.id).unwrap_or_else(|| {
                panic!("live tree is missing cell {:?}", cell.id)
            });
            assert_eq!(
                got.child_mask, cell.child_mask,
                "child mask at {:?}",
                cell.id
            );
            assert_eq!(got.rank_lo, cell.rank_lo, "rank_lo at {:?}", cell.id);
            assert_eq!(got.rank_hi, cell.rank_hi, "rank_hi at {:?}", cell.id);
            assert_eq!(
                got.aggregate.count(),
                cell.aggregate.count(),
                "count at {:?}",
                cell.id
            );
            assert_eq!(
                got.aggregate.m_min(),
                cell.aggregate.m_min(),
                "m_min at {:?}",
                cell.id
            );
            let (a, b) =
                (got.aggregate.total_flux(), cell.aggregate.total_flux());
            assert!(
                (a - b).abs() <= b.abs() * 1e-6 + 1e-12,
                "flux at {:?}",
                cell.id
            );
            // Ownership is exact: the same systems in the same cell.
            assert_eq!(
                live.payload(cell.id),
                fresh.payload(cell.id),
                "payload at {:?}",
                cell.id
            );
        }
    }

    /// From scratch, the live tree is the build it was made from.
    #[test]
    fn a_fresh_tree_is_its_build() {
        let mut rng = Rng(0x1234_5678);
        let systems: Vec<_> =
            (1..=9000).map(|id| input(id, &mut rng)).collect();
        let params = BuildParams::default();
        let tree = Tree::build(&systems, &params);
        assert_equivalent(
            &tree.to_snapshot(),
            &Snapshot::build(&systems, &params),
        );
    }

    /// A tree rebuilt from its own `to_inputs` equals the original: the resume
    /// path. `to_inputs` is exact and `build` is order-independent, so a
    /// checkpoint round trip lands on the same tree the feed left, cell for
    /// cell, which is what lets a restart follow from a cursor rather than
    /// rebuild from the database.
    #[test]
    fn to_inputs_rebuilds_an_equal_tree() {
        let mut rng = Rng(0xC0FFEE);
        let systems: Vec<_> =
            (1..=9000).map(|id| input(id, &mut rng)).collect();
        let params = BuildParams { internal_slice: 8, leaf_cap: 32 };
        let tree = Tree::build(&systems, &params);
        let rebuilt = Tree::build(&tree.to_inputs(), &params);
        assert_equivalent(&tree.to_snapshot(), &rebuilt.to_snapshot());
    }

    /// After every edit (insert, move, or remove) the live tree still equals a
    /// fresh build over the same systems. This is the whole contract: correct in
    /// place, not merely correct once. A small cap makes splits and collapses
    /// common so the structural moves are exercised, not just the cascade.
    #[test]
    fn every_edit_stays_equal_to_a_rebuild() {
        // Several seeds, each a run of edits, and the tree is checked against a
        // fresh build after *every* one; a bug that heals within a few steps
        // still gets caught the step it happens.
        for seed in [0xDEAD_BEEF, 0x0BADC0DE, 0xF00D_CAFE, 0x5EED_1234u64] {
            run_oracle(seed);
        }
    }

    fn run_oracle(seed: u64) {
        // A small cap makes splits and collapses common, so the structural
        // moves are exercised as hard as the cascade.
        let params = BuildParams { internal_slice: 8, leaf_cap: 32 };
        let mut rng = Rng(seed);
        let mut present: std::collections::BTreeMap<u64, System> =
            std::collections::BTreeMap::new();
        let mut next_id = 1u64;

        for _ in 0..400 {
            let s = input(next_id, &mut rng);
            present.insert(next_id, s);
            next_id += 1;
        }
        let mut tree = Tree::build(
            &present.values().copied().collect::<Vec<_>>(),
            &params,
        );

        for step in 0..2500u64 {
            let what = match rng.below(3) {
                0 => {
                    let s = input(next_id, &mut rng);
                    present.insert(next_id, s);
                    next_id += 1;
                    tree.apply(&[s]);
                    "insert"
                }
                1 if !present.is_empty() => {
                    let ids: Vec<u64> = present.keys().copied().collect();
                    let id = ids[rng.below(ids.len() as u64) as usize];
                    let mut s = input(id, &mut rng);
                    s.id64 = id;
                    present.insert(id, s);
                    tree.apply(&[s]);
                    "move"
                }
                _ if !present.is_empty() => {
                    let ids: Vec<u64> = present.keys().copied().collect();
                    let id = ids[rng.below(ids.len() as u64) as usize];
                    present.remove(&id);
                    tree.remove_system(id);
                    "remove"
                }
                _ => continue,
            };

            check(&tree, seed, step, what);

            let systems: Vec<_> = present.values().copied().collect();
            assert_eq!(
                tree.len(),
                present.len(),
                "seed {seed:#x} step {step} {what}"
            );
            assert_equivalent(
                &tree.to_snapshot(),
                &Snapshot::build(&systems, &params),
            );
        }
    }

    /// Whether `a` is an ancestor of, or equal to, `b`.
    fn is_ancestor(a: CellId, b: CellId) -> bool {
        if b.level < a.level {
            return false;
        }
        let mut c = b;
        for _ in 0..(b.level - a.level) {
            c = c.parent().unwrap();
        }
        c == a
    }

    /// The physical count of a subtree, recomputed from the leaves.
    fn subtree_count(tree: &Tree, id: CellId) -> u64 {
        let node = &tree.cells[&id];
        if node.is_leaf() {
            node.physical.len() as u64
        } else {
            (0..8u8)
                .filter(|o| node.child_mask & (1 << o) != 0)
                .map(|o| subtree_count(tree, id.child(o)))
                .sum()
        }
    }

    /// Assert every structural invariant of the live tree, so a leak is caught
    /// at the edit that caused it rather than as an underflow later.
    fn check(tree: &Tree, seed: u64, step: u64, what: &str) {
        let ctx = || format!("seed {seed:#x} step {step} {what}");
        for (&id, node) in &tree.cells {
            if !node.is_leaf() {
                assert!(
                    node.physical.is_empty(),
                    "internal {id:?} holds physical; {}",
                    ctx()
                );
            }
            // Count is exact.
            let recomputed = subtree_count(tree, id);
            if node.count != recomputed {
                let by_leaf = tree
                    .records
                    .keys()
                    .filter(|k| is_ancestor(id, tree.leaf[k]))
                    .count();
                let phys: u64 = tree
                    .cells
                    .iter()
                    .filter(|(cid, _)| is_ancestor(id, **cid))
                    .map(|(_, n)| n.physical.len() as u64)
                    .sum();
                panic!(
                    "count at {id:?}: maintained {}, subtree {}, by-leaf-map {}, by-physical {}; {}",
                    node.count,
                    recomputed,
                    by_leaf,
                    phys,
                    ctx()
                );
            }
            // Ownership is consistent: each owned system exists, points back, and
            // physically lies in this cell's subtree.
            for &(_, sid) in &node.slice {
                assert_eq!(
                    tree.owner.get(&sid),
                    Some(&id),
                    "owner of {sid} not {id:?}; {}",
                    ctx()
                );
                assert!(
                    tree.records.contains_key(&sid),
                    "cell {id:?} owns ghost {sid}; {}",
                    ctx()
                );
                assert!(
                    is_ancestor(id, tree.leaf[&sid]),
                    "cell {id:?} owns {sid} outside its subtree; {}",
                    ctx()
                );
            }
        }
        // Every record is owned once and physically placed at the deepest cell.
        for (&sid, rec) in &tree.records {
            let owner = *tree
                .owner
                .get(&sid)
                .unwrap_or_else(|| panic!("{sid} unowned; {}", ctx()));
            assert!(
                tree.cells.contains_key(&owner),
                "owner cell of {sid} gone; {}",
                ctx()
            );
            let leaf = tree.leaf[&sid];
            assert!(
                tree.cells[&leaf].is_leaf(),
                "leaf of {sid} is internal; {}",
                ctx()
            );
            assert!(
                tree.cells[&leaf].physical.contains(&sid),
                "{sid} not in its leaf; {}",
                ctx()
            );
            assert_eq!(
                tree.physical_leaf(rec.position),
                leaf,
                "leaf of {sid} wrong; {}",
                ctx()
            );
        }
    }

    /// A publish after edits lands the directory exactly where a full write of
    /// the current tree would, touching only changed cells.
    #[test]
    fn publish_writes_only_what_changed() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "galos_index_tree_{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));

        let params = BuildParams { internal_slice: 8, leaf_cap: 32 };
        let mut rng = Rng(0x0BAD_F00D);
        let seed: Vec<_> = (1..=500).map(|id| input(id, &mut rng)).collect();
        let mut tree = Tree::build(&seed, &params);
        tree.write(&dir).unwrap();

        // Some churn, then an incremental publish.
        let mut edits = Vec::new();
        for id in 501..=560 {
            edits.push(input(id, &mut rng));
        }
        tree.apply(&edits);
        tree.remove_system(3);
        tree.publish(&dir).unwrap();

        // The directory now holds exactly the current tree.
        let built = tree.to_snapshot();
        let index = Index::read(&dir).unwrap();
        assert_eq!(index.len(), built.index.len());
        for cell in built.index.cells() {
            assert_eq!(index.get(cell.id), Some(cell));
            let disk = Index::read_payload(&dir, cell.id).unwrap();
            let bytes =
                crate::serialization::Encode::to_bytes(built.payload(cell.id));
            let want =
                <Vec<Point> as crate::serialization::Decode>::from_bytes(
                    &bytes,
                )
                .unwrap();
            assert_eq!(disk, want, "payload at {:?}", cell.id);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
