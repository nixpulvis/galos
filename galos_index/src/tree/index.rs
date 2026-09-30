//! The cell tree held in memory: every cell's aggregate, and the same cells
//! flattened for the walks.
//!
//! Small enough to hold entire — a few tens of megabytes over the galaxy —
//! so every walk plans on it without a fetch, and the router looks addresses
//! up in it half a million times a route. It is two views of one tree: the
//! map an address is found in, and the same cells flattened breadth-first
//! for the walks to descend ([`crate::read::walk`]).
//!
//! Pure: its bytes are [`crate::codec::cells::format`]'s, and the file it is read
//! from and written to is [`crate::codec::cells`]'s.

use crate::core::aggregate::AGE_BUCKETS;

use crate::core::geometry::{CellId, CellMap};
use crate::tree::cell::Cell;

/// The resident tree of cell aggregates, keyed by address, and the same tree
/// flattened for the walks.
///
/// Small enough to hold whole (a few tens of megabytes over the galaxy), so
/// every walk reads it without a fetch. The payloads it points at are loaded
/// separately and cached elsewhere; this is the index the walks plan on.
///
/// **Two views of one tree, and the second is why a frame is quick.** The map
/// is what an address is looked up in — the router asks it half a million
/// times a route — and `nodes` is what a walk descends: the
/// cells breadth-first with a cell's children next to each other, carrying
/// the figures a walk reads worked out once. Measured over `.index/full`, a
/// 204,466-cell tree at 200,071,629 systems: the walk is **1.1–1.4 ms** a
/// frame off the nodes, for 18 MB beside the map's 44. See
/// `tests/zooming.rs`, which is where the guard lives.
///
/// The nodes are derived, so they are built where the map is and nowhere
/// else: an index is only ever made from a whole set of cells
/// ([`from_cells`](Index::from_cells)), never mutated after, which is what
/// makes one derivation enough.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Index {
    /// Hashed with [`Quick`](crate::core::geometry::Quick): the router and
    /// the field look a cell up here per expansion and per splat.
    pub(crate) cells: CellMap<Cell>,
    pub(crate) nodes: Vec<Node>,
}

/// One node of the walk's own tree: where a cell's contents sit and how far
/// they spread, and where its children are.
///
/// Everything here but the addresses is a figure a walk reads per cell per
/// frame — and all of it is a pure function of the cell, none of it of the
/// view, so it is worked out once when the index is built. What that keeps
/// out of a frame is not the arithmetic (two cube roots and a
/// square root a cell) but the cache: `contents_center` and `count_extent`
/// read the second moments, which sit in a 262-byte [`Cell`], for every
/// cell in the tree.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Node {
    /// Where a cell's contents sit: [`Cell::contents_center`].
    pub(crate) center: [f64; 3],
    /// How far they spread about that centre: [`Cell::contents_extent`], the RMS
    /// radius the field lays a Gaussian at.
    pub(crate) extent: f64,
    /// How wide they are: [`Cell::contents_width`], rolled up from the children.
    ///
    /// A spread and a width are not the same question and the walk asks
    /// both. The field wants a standard deviation, which is what a Gaussian
    /// is laid at; the merge rule wants the *support* — does everything this
    /// cell holds fall inside one mark — and a distribution's support is
    /// several times its RMS radius and is not a scalar fact about it at
    /// all. See [`Cell::contents_width`].
    pub(crate) width: f64,
    /// How many systems the subtree holds.
    pub(crate) count: u64,
    /// How many the cell owns itself.
    pub(crate) slice: u64,
    /// The brightest absolute magnitude in the subtree, for the sky's cut.
    pub(crate) m_min: Option<f32>,
    /// How many systems the subtree holds in each Recency bucket, so a span
    /// can be asked of a merged mark.
    pub(crate) aged: [u32; AGE_BUCKETS],
    /// The cell's address, which is what a walk answers with.
    pub(crate) id: CellId,
    /// Where this node's children begin. They are contiguous, so a walk
    /// reads them as a slice rather than looking eight addresses up.
    pub(crate) first_child: u32,
    /// How many children the tree actually holds for this cell, which is the
    /// set bits of `child_mask` that are present in the map.
    pub(crate) children: u8,
    /// Whether the cell has no children at all, the flag
    /// [`Cell::is_leaf`] answers. Not the same as `children == 0`: a mask can
    /// name a child the map does not hold, and the glow's cut turns on the
    /// mask.
    pub(crate) leaf: bool,
}

impl Node {
    fn of(cell: &Cell) -> Node {
        Node {
            center: cell.contents_center(),
            extent: cell.contents_extent(),
            width: cell.contents_width(),
            count: cell.aggregate.count(),
            slice: cell.slice_len(),
            m_min: cell.aggregate.m_min(),
            aged: *cell.aggregate.aged(),
            id: cell.id,
            first_child: 0,
            children: 0,
            leaf: cell.is_leaf(),
        }
    }
}

/// The tree flattened breadth-first from the root, a cell's children landing
/// together.
///
/// Reachability is the walks' own: a cell the root cannot be descended to is
/// a cell no walk ever visited, so it is in the map and not in the nodes.
/// `u32` for the child link — a galaxy is a few hundred thousand cells, and
/// four billion is a tree no machine holds resident.
fn flatten(cells: &CellMap<Cell>) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::with_capacity(cells.len());
    let Some(root) = cells.get(&CellId::ROOT) else { return nodes };
    nodes.push(Node::of(root));
    let mut at = 0;
    while at < nodes.len() {
        let id = nodes[at].id;
        let cell = &cells[&id];
        let kids = id.children();
        let first = nodes.len() as u32;
        let mut held = 0u8;
        for octant in 0..8u8 {
            if !cell.has_child(octant) {
                continue;
            }
            if let Some(child) = cells.get(&kids[octant as usize]) {
                nodes.push(Node::of(child));
                held += 1;
            }
        }
        nodes[at].first_child = first;
        nodes[at].children = held;
        at += 1;
    }
    widen(&mut nodes);
    nodes
}

/// Roll each cell's contents width up from its children, deepest first.
///
/// **A cell is as wide as the ball that covers its children.** Taken
/// pairwise: the gap between two children's centroids plus each one's own
/// half-width, maximised over every pair — and the pair `(i, i)` is in it,
/// so a cell is never narrower than its widest child. Eight children is at
/// most sixty-four gaps a cell, paid once when the index is built and never
/// in a frame.
///
/// What this buys over the scalar estimate [`Cell::contents_width`] seeds is
/// **shape**. A filament running through a cell has its systems in two or
/// three children with the rest empty, and the gap between those children's
/// centroids is the filament's own length; the RMS radius of the same
/// filament is a third of it, so a scalar test merges three marks' worth of
/// line into one mark and the feature disappears. The children are what the
/// tree holds of a cell's geometry, and this is the whole of what can be
/// read off them without storing more.
///
/// Breadth-first order puts every child after its parent, so one pass in
/// reverse has each cell's children already rolled up.
fn widen(nodes: &mut [Node]) {
    for at in (0..nodes.len()).rev() {
        let first = nodes[at].first_child as usize;
        let kids = first..first + nodes[at].children as usize;
        let mut width = nodes[at].width;
        for near in kids.clone() {
            for far in kids.clone() {
                let gap = distance(nodes[near].center, nodes[far].center)
                    + 0.5 * (nodes[near].width + nodes[far].width);
                width = width.max(gap);
            }
        }
        nodes[at].width = width;
    }
}

impl Index {
    /// Build an index from a set of cells.
    pub fn from_cells(cells: impl IntoIterator<Item = Cell>) -> Index {
        let cells: CellMap<Cell> =
            cells.into_iter().map(|c| (c.id, c)).collect();
        let nodes = flatten(&cells);
        Index { cells, nodes }
    }

    /// The cell at an address, if the tree holds it.
    pub fn get(&self, id: CellId) -> Option<&Cell> {
        self.cells.get(&id)
    }

    /// Hand `each` every cell standing over `point`, the root first and the
    /// deepest cell the tree holds last.
    ///
    /// The descent follows the tree's own children rather than asking
    /// [`CellId::of_point`] a level at a time and looking each answer up, so it
    /// invents no cell the index does not hold and costs no hashing: the
    /// children of a node are contiguous in `nodes`, so each
    /// step is at most eight comparisons. A point outside the cube clamps to
    /// the nearest edge cell, which is `of_point`'s rule and is what makes this
    /// total.
    ///
    /// What a side aggregation is rolled up by: a system contributes to every
    /// cell on its path, which is exactly what makes a cell's record the total
    /// over its whole subtree.
    pub fn descend(&self, point: [f64; 3], mut each: impl FnMut(CellId)) {
        let mut at = 0usize;
        while let Some(node) = self.nodes.get(at) {
            each(node.id);
            if node.children == 0 {
                return;
            }
            let want = CellId::of_point(point, node.id.level + 1);
            let first = node.first_child as usize;
            let kids = first..first + node.children as usize;
            match kids.clone().find(|&kid| self.nodes[kid].id == want) {
                Some(kid) => at = kid,
                None => return,
            }
        }
    }

    /// Where in the walk's nodes the deepest cell standing over `point` sits,
    /// the descent begun at node `from` rather than at the root
    ///
    /// [`descend`](Self::descend)'s rule, answering a position in `nodes`
    /// rather than an address, so a caller keeping a figure per cell can hold
    /// it in a `Vec` beside them and roll it up without hashing. Begun below
    /// the root for a system known to sit inside a cell — a payload's own.
    pub(crate) fn deepest_below(&self, from: usize, point: [f64; 3]) -> usize {
        let mut at = from;
        loop {
            let node = &self.nodes[at];
            if node.children == 0 {
                return at;
            }
            let want = CellId::of_point(point, node.id.level + 1);
            let first = node.first_child as usize;
            let kids = first..first + node.children as usize;
            match kids.clone().find(|&kid| self.nodes[kid].id == want) {
                Some(kid) => at = kid,
                None => return at,
            }
        }
    }

    /// How many cells the index holds.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Every cell in the index, in no particular order: what the builder's
    /// checks and the serialization walk read.
    pub fn cells(&self) -> impl Iterator<Item = &Cell> {
        self.cells.values()
    }

    /// The root cell, if present.
    pub fn root(&self) -> Option<&Cell> {
        self.get(CellId::ROOT)
    }

    /// The children of a cell that exist in the tree, in octant order.
    pub fn children<'a>(
        &'a self,
        cell: &'a Cell,
    ) -> impl Iterator<Item = &'a Cell> {
        let ids = cell.id.children();
        (0..8u8).filter_map(move |octant| {
            cell.has_child(octant)
                .then(|| self.get(ids[octant as usize]))
                .flatten()
        })
    }

    /// Every cell whose box comes within `radius` light years of `center`,
    /// found by descending the tree rather than scanning it.
    ///
    /// A linear scan over every cell there is would answer the same set, and
    /// is hopeless for a router, which asks per expansion: 204,466 cells at
    /// 200 M systems, half a million times a route.
    ///
    /// This descends from the root instead, dropping a subtree whose box is
    /// already further than `radius` from `center`, so the work is the cells
    /// the sphere actually touches. Every level is visited and not only the
    /// leaves: a cell owns a *slice* of its subtree's magnitude order (see
    /// [`Cell::rank_lo`]), so the brightest systems in reach sit in the
    /// ancestors and a walk that stopped at leaves would route past them.
    ///
    /// A cell straddling the sphere is handed over rather than measured: the
    /// caller weighs its systems' true distances. Additive slices put a
    /// system in exactly one cell, so the union of these cells' payloads is
    /// every system in reach with no duplicate.
    pub fn each_near(
        &self,
        center: [f64; 3],
        radius: f64,
        mut found: impl FnMut(CellId),
    ) {
        let Some(root) = self.root() else { return };
        // Depth-first over an explicit stack: the tree is 21 levels at most
        // but this runs per expansion, and a recursive call per cell costs
        // more than pushing an id.
        let mut stack = vec![root.id];
        while let Some(id) = stack.pop() {
            let Some(cell) = self.get(id) else { continue };
            if id.bounds().distance_to(center) > radius {
                continue;
            }
            found(id);
            for octant in 0..8 {
                if cell.has_child(octant) {
                    stack.push(id.child(octant));
                }
            }
        }
    }
}

/// Straight-line distance between two points, light years.
pub(crate) fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}
