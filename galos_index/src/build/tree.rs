//! The live tree: the galaxy tree held open and kept current.
//!
//! [`Snapshot::build`] turns a whole galaxy into a tree at once: the split,
//! the magnitude ordering, the aggregates. [`Tree`] is that same tree held
//! open, so a scan arriving on EDDN moves one system and touches only the
//! cells on its path. The work of one edit is the depth of the tree, not its
//! size.
//!
//! Every edit leaves the tree where a fresh [`Snapshot::build`] over the
//! same systems would: same cells, same ownership, same payloads, which the
//! oracle test checks after every operation. A system sits in exactly one
//! cell, a cell owns the brightest of its subtree its ancestors did not, and
//! a cell splits at the cap and collapses back under it.
//!
//! Two moves do all the work. **Insert** settles the system at the
//! shallowest cell on its path with room; where that cell is full and the
//! newcomer brighter than its faintest, it takes the slot and the evicted
//! system carries on down its own path. **Remove** is the mirror: the hole
//! is filled by promoting the brightest system from the children, which
//! leaves a hole one level down. Splitting and collapsing a cell are local
//! to that cell's own systems.
//!
//! A cell's totals are always re-summed — a leaf from its members, an
//! internal cell from its children — never adjusted by a difference, which
//! drifts and cannot recover `m_min`. Structure, ownership and the per-cell
//! count are integer-exact.

use crate::build::snapshot::{BuildParams, CellDiff, Snapshot};
use crate::core::aggregate::{Aggregate, Cell};
use crate::core::geometry::{CellId, MAX_LEVEL};
use crate::core::record::{Point, StarKind, System};
use crate::read::index::Index;
use std::collections::{BTreeSet, HashMap, HashSet};

/// One system as the live tree holds it: its place and its photometry, the
/// input stripped of its id, the key it is stored under.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Record {
    position: [f64; 3],
    magnitude: f64,
    temperature: f64,
    age_bucket: u32,
    updated_at: u32,
    /// What kind of star a ship arrives at, for the payload to carry.
    kind: StarKind,
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
/// `slice` is what the cell owns, ordered `(magnitude, id)`, brightest
/// first. `physical` is what falls in the cell and is kept only at leaves,
/// where a split reads it; an internal node's physical members live in its
/// descendants. `count` is the subtree's physical total, an integer, so the
/// split and collapse tests never touch a float.
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
/// reports changes. [`publish`](Self::publish) rewrites only the cells the
/// edits touched.
#[derive(Clone, Debug)]
pub struct Tree {
    cells: HashMap<CellId, Node>,
    records: HashMap<u64, Record>,
    /// Which cell owns each system: whose slice, hence payload, holds it.
    owner: HashMap<u64, CellId>,
    /// Which leaf each system physically falls in.
    leaf: HashMap<u64, CellId>,
    /// Cells whose payload has changed since the last publish.
    dirty: HashSet<CellId>,
    /// Cells that existed at the last publish and no longer do.
    gone: HashSet<CellId>,
    /// Every cell's subtree totals, maintained rather than recomputed:
    /// settled along the changed paths only.
    ///
    /// Never *subtracted*: a leaf is re-summed from its own members, at most
    /// [`BuildParams::leaf_cap`] of them, and an internal cell is the merge
    /// of its children — so `m_min` stays exact, which it is not from a
    /// difference, and nothing drifts with the number of edits.
    agg: HashMap<CellId, Aggregate>,
    /// How many of each cell's subtree are owned by it or by something
    /// below it. `rank_lo` is the subtree's count less this; maintained in
    /// the same walk as the totals.
    owned_below: HashMap<CellId, u64>,
    /// Cells whose totals the next settle must work out again.
    restat: HashSet<CellId>,
    params: BuildParams,
}

impl Tree {
    /// Build the tree from scratch, then hold it open.
    ///
    /// The first build is the batch [`Snapshot::build`]; every edit after it
    /// is incremental.
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
                        updated_at: s.updated_at,
                        kind: s.kind,
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
            agg: HashMap::new(),
            owned_below: HashMap::new(),
            restat: HashSet::new(),
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

        // The batch build's totals are what the maintained ones start from.
        // `rank_lo` is the subtree count less what a cell and its
        // descendants own, so the second map is read back out of the first.
        for cell in built.index.cells() {
            tree.agg.insert(cell.id, cell.aggregate);
            tree.owned_below
                .insert(cell.id, cell.aggregate.count() - cell.rank_lo);
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
    /// octant that holds nothing yet, and the leaf is made there. The
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
    /// added. What a feed calls with the systems a run of messages touched.
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

    /// Whether the tree already holds this system.
    ///
    /// Asked per reading by a run counting new systems. [`Self::upsert`]
    /// works the same thing out and discards it, and by the time it runs a
    /// whole pass of readings has accumulated, so there is no telling
    /// which of them brought a system in.
    pub fn holds(&self, address: i64) -> bool {
        self.records.contains_key(&(address as u64))
    }

    /// How many systems the tree holds. No `is_empty`: a size is asked for a
    /// report or a check, and an empty tree answers zero like any other.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// The inputs this tree was built from, reconstructed from its records:
    /// the full-precision [`System`] values a checkpoint persists so the tree
    /// can be rebuilt without the database. Exact, since a record holds every
    /// field a system carries; order is arbitrary, which the
    /// order-independent [`build`](Self::build) does not care about.
    ///
    /// An iterator rather than a collection: a galaxy's worth is gigabytes,
    /// streamed past the checkpoint's writer instead of held beside the tree
    /// it was copied out of.
    pub fn inputs(&self) -> impl Iterator<Item = System> + '_ {
        self.records.iter().map(|(&id64, rec)| System {
            id64,
            position: rec.position,
            absolute_magnitude: rec.magnitude,
            temperature: rec.temperature,
            age_bucket: rec.age_bucket,
            updated_at: rec.updated_at,
            kind: rec.kind,
        })
    }

    // --- insertion --------------------------------------------------------

    fn insert(&mut self, system: System) {
        let rec = Record {
            position: system.position,
            magnitude: system.absolute_magnitude,
            temperature: system.temperature,
            age_bucket: system.age_bucket,
            updated_at: system.updated_at,
            kind: system.kind,
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
    /// along `position`'s path, and mark each of them for a restat.
    ///
    /// The path a count moved along is the path whose totals moved, so this
    /// is where the settle's work is recorded. Insert and remove both come
    /// through here.
    fn bump_count(&mut self, position: [f64; 3], level: u8, delta: i64) {
        for l in 0..=level {
            let cid = CellId::of_point(position, l);
            if let Some(node) = self.cells.get_mut(&cid) {
                node.count = (node.count as i64 + delta) as u64;
                self.restat.insert(cid);
            }
        }
    }

    /// Work out the totals of every cell whose subtree has moved since the
    /// last settle, deepest first.
    ///
    /// A leaf is re-summed from its own physical members and an internal
    /// cell is the merge of its children, so nothing is ever subtracted and
    /// `m_min` stays exact. The set is what [`bump_count`](Self::bump_count)
    /// and the structural moves marked, plus every ancestor of those: a
    /// slice changing without a count changing still moves the owned-below
    /// totals `rank_lo` is read from.
    fn settle(&mut self) {
        for id in &self.gone {
            self.agg.remove(id);
            self.owned_below.remove(id);
        }
        if self.restat.is_empty() && self.dirty.is_empty() {
            return;
        }

        let mut touched: HashSet<CellId> = HashSet::new();
        for &marked in self.restat.iter().chain(self.dirty.iter()) {
            let mut at = Some(marked);
            while let Some(id) = at {
                if self.cells.contains_key(&id) && !touched.insert(id) {
                    // Already walked from a deeper mark, and so is the rest
                    // of the way up.
                    break;
                }
                at = id.parent();
            }
        }

        let mut ordered: Vec<CellId> = touched.into_iter().collect();
        ordered.sort_by_key(|id| std::cmp::Reverse(id.level));
        for id in ordered {
            let (aggregate, owned) = {
                let node = &self.cells[&id];
                let mut aggregate = Aggregate::ZERO;
                let mut owned = node.slice.len() as u64;
                if node.is_leaf() {
                    for pid in &node.physical {
                        let r = &self.records[pid];
                        aggregate = aggregate.merge(Aggregate::of_system(
                            r.position,
                            r.magnitude,
                            r.temperature,
                            r.age_bucket,
                        ));
                    }
                } else {
                    for child in id.children() {
                        if self.cells.contains_key(&child) {
                            aggregate = aggregate.merge(self.agg[&child]);
                            owned += self.owned_below[&child];
                        }
                    }
                }
                (aggregate, owned)
            };
            self.agg.insert(id, aggregate);
            self.owned_below.insert(id, owned);
        }
        self.restat.clear();
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

    /// Take a system out of the tree, and say whether it was there.
    ///
    /// The feed never withdraws one — a report says what is there, never
    /// what is not — so this is the repair path and not the live one: a
    /// directory whose names table and cell tree stand for different sets
    /// is trimmed to what both hold before it is published over. See
    /// `galos::sink::index::Index::open`.
    pub fn forget(&mut self, id: u64) -> bool {
        if !self.records.contains_key(&id) {
            return false;
        }
        self.remove(id);
        true
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

    /// The tree as a [`Snapshot`]: every cell and every payload.
    ///
    /// The shape [`Snapshot::build`] returns, for comparing against a
    /// rebuild or writing whole, and O(galaxy). A publish wants
    /// [`publish`](Self::publish), which assembles the index off the
    /// maintained totals and builds only the payloads it will write.
    pub fn to_snapshot(&mut self) -> Snapshot {
        self.settle();
        let payloads = self
            .cells
            .keys()
            .filter_map(|&id| {
                let points = self.payload_of(id);
                (!points.is_empty()).then_some((id, points))
            })
            .collect();
        Snapshot { index: self.index_of(), payloads }
    }

    /// The index as the maintained totals have it: one pass over the cells,
    /// which is the tree's size and not the galaxy's.
    ///
    /// `rank_lo` is the count of a cell's subtree that something above it
    /// owns, which is the subtree total less what it and its descendants
    /// own — both of them settled, neither of them counted here.
    fn index_of(&self) -> Index {
        Index::from_cells(self.cells.iter().map(|(&id, node)| {
            let aggregate = self.agg[&id];
            let rank_lo = aggregate.count() - self.owned_below[&id];
            Cell {
                id,
                rank_lo,
                rank_hi: rank_lo + node.slice.len() as u64,
                child_mask: node.child_mask,
                aggregate,
            }
        }))
    }

    /// One cell's payload: what it owns, brightest first, as the slice holds
    /// it.
    fn payload_of(&self, id: CellId) -> Vec<Point> {
        let Some(node) = self.cells.get(&id) else { return Vec::new() };
        node.slice
            .iter()
            .map(|&(_, pid)| {
                let r = &self.records[&pid];
                Point::new(
                    pid,
                    r.position,
                    r.magnitude,
                    r.temperature,
                    r.updated_at,
                    r.kind,
                )
            })
            .collect()
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
    /// bulk, and only the cells the edits touched are written or removed.
    ///
    /// Nothing here reads a system the edits did not touch: the index comes
    /// off the settled totals, one pass over the cells, and a payload is
    /// built only for a cell about to be written. See
    /// [`settle`](Self::settle): a hundred edits over eight million systems
    /// publish in 52 ms, against 2.6 s when every payload in the galaxy was
    /// built to write a dozen files.
    pub fn publish(&mut self, dir: &std::path::Path) -> std::io::Result<()> {
        self.settle();
        let mut dirtied = CellDiff::default();
        let mut payloads: HashMap<CellId, Vec<Point>> = HashMap::new();
        let touched: HashSet<CellId> =
            self.dirty.iter().chain(self.gone.iter()).copied().collect();
        for id in touched {
            let points = self.payload_of(id);
            match points.is_empty() {
                false => {
                    dirtied.changed.push(id);
                    payloads.insert(id, points);
                }
                true => dirtied.removed.push(id),
            }
        }
        let built = Snapshot { index: self.index_of(), payloads };
        built.write_diff(dir, &dirtied)?;
        self.dirty.clear();
        self.gone.clear();
        Ok(())
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
            age_bucket: rng.below(8) as u32,
            // Off the id rather than the rng, so the draws below keep the
            // sequence they had, and distinct per system, so a payload that
            // mixed the stamps up fails the equivalence check.
            updated_at: 1_700_000_000 + id as u32,
            kind: crate::core::record::StarKind::G,
        }
    }

    /// The live tree matches a fresh build, cell for cell and system for
    /// system. Aggregates are summed in a different order either side, so
    /// their floats are compared within tolerance and everything discrete
    /// exactly.
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
        let mut tree = Tree::build(&systems, &params);
        assert_equivalent(
            &tree.to_snapshot(),
            &Snapshot::build(&systems, &params),
        );
    }

    /// A tree rebuilt from its own `inputs` equals the original: the resume
    /// path. `inputs` is exact and `build` is order-independent, so a
    /// checkpoint round trip lands on the tree the feed left, cell for
    /// cell.
    #[test]
    fn inputs_rebuild_an_equal_tree() {
        let mut rng = Rng(0xC0FFEE);
        let systems: Vec<_> =
            (1..=9000).map(|id| input(id, &mut rng)).collect();
        let params = BuildParams { internal_slice: 8, leaf_cap: 32 };
        let mut tree = Tree::build(&systems, &params);
        let mut rebuilt =
            Tree::build(&tree.inputs().collect::<Vec<_>>(), &params);
        assert_equivalent(&tree.to_snapshot(), &rebuilt.to_snapshot());
    }

    /// After every edit — an insert or a move — the live tree still equals a
    /// fresh build over the same systems. A small cap makes splits and
    /// collapses common, so the structural moves are exercised as hard as
    /// the cascade.
    #[test]
    fn every_edit_stays_equal_to_a_rebuild() {
        // Several seeds, each a run of edits, checked against a fresh build
        // after *every* one, so a bug that heals in a few steps is still
        // caught at the step it happens.
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
            let what = match rng.below(2) {
                0 => {
                    let s = input(next_id, &mut rng);
                    present.insert(next_id, s);
                    next_id += 1;
                    tree.apply(&[s]);
                    "insert"
                }
                _ if !present.is_empty() => {
                    let ids: Vec<u64> = present.keys().copied().collect();
                    let id = ids[rng.below(ids.len() as u64) as usize];
                    let mut s = input(id, &mut rng);
                    s.id64 = id;
                    present.insert(id, s);
                    tree.apply(&[s]);
                    "move"
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

    /// Assert every structural invariant of the live tree, so a leak is
    /// caught at the edit that caused it rather than as an underflow
    /// later.
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
            // Ownership is consistent: each owned system exists, points
            // back, and physically lies in this cell's subtree.
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

        // Some churn, then an incremental publish: new systems, and one moved
        // to a fresh position.
        let mut edits = Vec::new();
        for id in 501..=560 {
            edits.push(input(id, &mut rng));
        }
        edits.push(input(3, &mut rng));
        tree.apply(&edits);
        tree.publish(&dir).unwrap();

        // The directory now holds exactly the current tree.
        let built = tree.to_snapshot();
        let index = Index::read(&dir).unwrap();
        assert_eq!(index.len(), built.index.len());
        for cell in built.index.cells() {
            assert_eq!(index.get(cell.id), Some(cell));
            let disk = Index::read_payload(&dir, cell.id).unwrap();
            let want = built.payload(cell.id);
            assert_eq!(disk, want, "payload at {:?}", cell.id);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
