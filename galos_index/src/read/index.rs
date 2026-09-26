//! The resident index: every cell's aggregate, held whole, and the file it
//! is read from.
//!
//! Small enough to hold entire — a few tens of megabytes over the galaxy —
//! so every walk plans on it without a fetch, and the router looks addresses
//! up in it half a million times a route. It is two views of one tree: the
//! map an address is found in, and the same cells flattened breadth-first
//! for the walks to descend ([`crate::read::walk`]).
//!
//! The file is `index.bin`: a header naming its magic and
//! [`INDEX_VERSION`], a count, and that many fixed-width [`Cell`] records.
//! It is rewritten whole on every publish, and a file of another width or
//! version is refused rather than misread. The payloads beside it are read
//! through here too, one cell at a time.

use crate::core::aggregate::AGE_BUCKETS;
use crate::core::aggregate::Cell;
use crate::core::codec::{Decode, Encode, FixedCodec};
use crate::core::geometry::CellId;
use crate::core::record::Point;
use crate::format::layout::{INDEX_FILE, legacy_payload_path, payload_path};
use crate::format::payload::{INDEX_MAGIC, INDEX_VERSION, index_version};
use crate::store::cells::Payload;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

/// The resident tree of cell aggregates, keyed by address, and the same tree
/// flattened for the walks.
///
/// Small enough to hold whole (a few tens of megabytes over the galaxy), so
/// every walk reads it without a fetch. The payloads it points at are loaded
/// separately and cached elsewhere; this is the index the walks plan on.
///
/// **Two views of one tree, and the second is why a frame is quick.** The map
/// is what an address is looked up in — the router asks it half a million
/// times a route — and [`nodes`](Index::nodes) is what a walk descends: the
/// cells breadth-first with a cell's children next to each other, carrying
/// the figures a walk reads worked out once. Measured over `.index/full`, a
/// 204,466-cell tree at 200,071,629 systems: the walk was **23 ms** a frame
/// off the map and is **1.1–1.4 ms** off the nodes, for 18 MB beside the
/// map's 44. See `tests/zooming.rs`, which is where the guard lives.
///
/// The nodes are derived, so they are built where the map is and nowhere
/// else: an index is only ever made from a whole set of cells
/// ([`from_cells`](Index::from_cells)), never mutated after, which is what
/// makes one derivation enough.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Index {
    pub(super) cells: HashMap<CellId, Cell>,
    pub(super) nodes: Vec<Node>,
}

/// One node of the walk's own tree: where a cell's contents sit and how far
/// they spread, and where its children are.
///
/// Everything here but the addresses is a figure the walks used to work out
/// per cell per frame — and all of it is a pure function of the cell, none of
/// it of the view, so it is worked out once when the index is built. What
/// that took out of a frame is not the arithmetic (two cube roots and a
/// square root a cell) but the cache: `contents_center` and `count_extent`
/// read the second moments, which is most of a 216-byte [`Cell`], for every
/// cell in the tree.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct Node {
    /// Where a cell's contents sit: [`contents_center`].
    pub(super) center: [f64; 3],
    /// How far they spread about that centre: [`contents_extent`], the RMS
    /// radius the field lays a Gaussian at.
    pub(super) extent: f64,
    /// How wide they are: [`contents_width`], rolled up from the children.
    ///
    /// A spread and a width are not the same question and the walk asks
    /// both. The field wants a standard deviation, which is what a Gaussian
    /// is laid at; the merge rule wants the *support* — does everything this
    /// cell holds fall inside one mark — and a distribution's support is
    /// several times its RMS radius and is not a scalar fact about it at
    /// all. See [`contents_width`].
    pub(super) width: f64,
    /// How many systems the subtree holds.
    pub(super) count: u64,
    /// How many the cell owns itself.
    pub(super) slice: u64,
    /// The brightest absolute magnitude in the subtree, for the sky's cut.
    pub(super) m_min: Option<f32>,
    /// How many systems the subtree holds in each Recency bucket, so a span
    /// can be asked of a merged mark.
    pub(super) aged: [u32; AGE_BUCKETS],
    /// The cell's address, which is what a walk answers with.
    pub(super) id: CellId,
    /// Where this node's children begin. They are contiguous, so a walk
    /// reads them as a slice rather than looking eight addresses up.
    pub(super) first_child: u32,
    /// How many children the tree actually holds for this cell, which is the
    /// set bits of `child_mask` that are present in the map.
    pub(super) children: u8,
    /// Whether the cell has no children at all, the flag
    /// [`Cell::is_leaf`] answers. Not the same as `children == 0`: a mask can
    /// name a child the map does not hold, and the glow's cut turns on the
    /// mask.
    pub(super) leaf: bool,
}

impl Node {
    fn of(cell: &Cell) -> Node {
        Node {
            center: contents_center(cell),
            extent: contents_extent(cell),
            width: contents_width(cell),
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
fn flatten(cells: &HashMap<CellId, Cell>) -> Vec<Node> {
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
/// What this buys over the scalar estimate [`contents_width`] seeds is
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
        let cells: HashMap<CellId, Cell> =
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
    /// children of a node are contiguous in [`nodes`](Index::nodes), so each
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
pub(super) fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// A cell's contents' spread in light years: the count-weighted RMS radius,
/// floored at the mean spacing so a cell of one or a few systems still resolves
/// as the camera closes rather than staying a zero-extent point forever.
pub(super) fn contents_extent(cell: &Cell) -> f64 {
    let count = cell.aggregate.count().max(1) as f64;
    let spacing = cell.id.edge_ly() / count.cbrt();
    cell.aggregate.count_extent().max(spacing)
}

/// Where a cell's contents sit: the count-weighted centroid, or the box centre
/// where the aggregate carries no weight of its own.
pub(super) fn contents_center(cell: &Cell) -> [f64; 3] {
    cell.aggregate.count_centroid().unwrap_or_else(|| cell.id.bounds().center())
}

/// How wide a cell's contents are in light years, before its children are
/// rolled into the answer: the RMS radius read as the span of an even
/// spread.
///
/// **A spread is not a width, and the merge rule needs the width.** A cell
/// carries one scalar about how far its systems sit from their centroid, the
/// RMS radius `r`. For a set spread evenly along anything — a box, a line, a
/// sheet — the distance between its two furthest members is `2·sqrt(3)·r`:
/// a uniform segment of length `L` has `r = L/sqrt(12)`, and a uniform cube
/// of edge `e` has `r = e/2` against a diagonal of `e·sqrt(3)`. So the span
/// is the radius times [`UNIFORM_SPAN`], and reading the radius itself as a
/// width understates a cell by three and a half — which is a filament of
/// three marks merged into one and gone from the picture.
///
/// Unfloored, where [`contents_extent`] floors at the mean spacing. The
/// floor is the field's: a lone system must still splat as something. Here
/// the honest answer for one system is zero width, and what keeps it drawn
/// as itself rather than merged into a blob is [`split_to_marks`]'s rule
/// that a cell holding no more than its footprint can show is never merged.
///
/// This is the seed [`widen`] rolls up. Where a cell has children their
/// centroids say far more about its shape than this does, and the larger of
/// the two answers stands.
pub(super) fn contents_width(cell: &Cell) -> f64 {
    UNIFORM_SPAN * cell.aggregate.count_extent()
}

/// How many RMS radii across an evenly spread set is: `2·sqrt(3)`.
pub const UNIFORM_SPAN: f64 = 3.464_101_615_137_754_6;

impl Encode for Index {
    fn encode(&self, out: &mut Vec<u8>) {
        out.reserve(
            INDEX_MAGIC.len() + u16::LEN + u32::LEN + self.len() * Cell::LEN,
        );
        INDEX_MAGIC.encode(out);
        INDEX_VERSION.encode(out);
        (self.len() as u32).encode(out);
        for cell in self.cells() {
            cell.encode(out);
        }
    }
}

impl Decode for Index {
    /// [`None`] for a header this does not read, and for a body that disagrees
    /// with it.
    ///
    /// The header says how many cells follow and a cell is a fixed width, so
    /// the length is a thing the file can be held to: anything but exactly
    /// `count * Cell::LEN` bytes of body was written by a different build of
    /// this code. That is the check that makes the index record's width safe
    /// to change without moving [`INDEX_VERSION`] — without it a stale index
    /// passes the header, decodes one record's bytes as another's, and hands
    /// back a plausible-looking tree of nonsense. Refused here, it is a
    /// rebuild instead of a wrong sky.
    ///
    /// The payload has no such check — it carries no count to be held against,
    /// and drops a short tail rather than failing — so a change to *its* width
    /// is caught here instead, by the version this refuses on. See
    /// [`INDEX_VERSION`].
    fn decode(cur: &mut &[u8]) -> Option<Index> {
        if <[u8; 4]>::decode(cur)? != INDEX_MAGIC {
            return None;
        }
        if u16::decode(cur)? != INDEX_VERSION {
            return None;
        }
        let count = u32::decode(cur)? as usize;
        if cur.len() != count * Cell::LEN {
            return None;
        }
        let mut cells = Vec::with_capacity(count);
        for _ in 0..count {
            cells.push(Cell::decode(cur)?);
        }
        Some(Index::from_cells(cells))
    }
}

impl Index {
    /// Read an index from a build directory.
    ///
    /// A directory at another format version is refused here and nowhere
    /// else: its payloads carry no header, so reading it would decode every
    /// field of every system out of the wrong bytes. The error names the
    /// version met, and a rebuild is the fix — `bring_level` falls back to a
    /// full build when a resume cannot read what is there.
    pub fn read(dir: &Path) -> io::Result<Index> {
        let bytes = fs::read(dir.join(INDEX_FILE))?;
        Index::from_bytes(&bytes).ok_or_else(|| {
            let what = match index_version(&bytes) {
                Some(found) if found != INDEX_VERSION => format!(
                    "index format version {found}, this build reads \
                     {INDEX_VERSION}: the payloads changed layout, so run \
                     `galos index migrate` over the directory"
                ),
                _ => "not an index file".to_string(),
            };
            io::Error::new(io::ErrorKind::InvalidData, what)
        })
    }

    /// Write the index file, and nothing else.
    ///
    /// Rewritten whole every time: the aggregates and the rank ranges, a few
    /// megabytes over today's galaxy, and some 73 MB at two hundred million
    /// systems — written entire on every publish, which is what it costs.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        fs::write(dir.join(INDEX_FILE), self.to_bytes())
    }

    /// Read one cell's payload from a build directory, empty when the cell owns
    /// nothing and so has no file. Positions are in light years.
    ///
    /// The sharded path first and the flat one after it, so a directory
    /// published before the sharding, or one being resharded as this reads,
    /// answers with what it has.
    pub fn read_payload(dir: &Path, id: CellId) -> io::Result<Vec<Point>> {
        let bytes = match fs::read(payload_path(dir, id)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                match fs::read(legacy_payload_path(dir, id)) {
                    Ok(bytes) => bytes,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        return Ok(Vec::new());
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };
        Ok(crate::format::payload::payload_points(id, &bytes)
            .unwrap_or_default())
    }

    /// The first `limit` systems of a cell's payload, brightest first.
    ///
    /// **What a draw asks for is a share of a cell and not the cell.** The
    /// payload is magnitude-ordered, the map draws the brightest few of it
    /// (`galos_map`'s `bounded::wanted`), and reading the rest is bytes
    /// faulted, decoded, held and never looked at: measured over
    /// `.index/full`, a flight that drew eight thousand marks read 121 M
    /// points and 5.8 GB to do it.
    ///
    /// Mapped rather than read, so the pages behind the rows nobody asked
    /// for are never touched. A directory written before the columnar
    /// layout has no head to map and falls back to the whole file, which is
    /// what it could always do.
    pub fn read_payload_prefix(
        dir: &Path,
        id: CellId,
        limit: usize,
    ) -> io::Result<Vec<Point>> {
        let Some(payload) = Payload::open(dir, id)? else {
            let mut points = Index::read_payload(dir, id)?;
            points.truncate(limit);
            return Ok(points);
        };
        let take = limit.min(payload.len());
        Ok((0..take).map(|at| payload.point_at(at)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::aggregate::Aggregate;

    /// An index round-trips its cells, and a file that is not one is refused
    /// rather than misread.
    #[test]
    fn an_index_round_trips_and_rejects_a_bad_header() {
        let index = Index::from_cells([
            Cell {
                id: CellId::ROOT,
                rank_lo: 0,
                rank_hi: 512,
                child_mask: 0xFF,
                aggregate: Aggregate::of_system([0.0; 3], 1.0, 5000.0, 0),
            },
            Cell {
                id: CellId { level: 1, x: 0, y: 1, z: 1 },
                rank_lo: 512,
                rank_hi: 520,
                child_mask: 0,
                aggregate: Aggregate::ZERO,
            },
        ]);
        let bytes = index.to_bytes();
        assert_eq!(Index::from_bytes(&bytes), Some(index));
        assert_eq!(
            Index::from_bytes(b"nope and then some padding bytes"),
            None
        );
    }

    /// An index written when a *cell* record was a different width is refused
    ///
    /// [`INDEX_VERSION`] does not move for a change to the index record, so
    /// a stale file carries the same magic and the same version and the
    /// header cannot tell it apart. Only its length can. Without this the
    /// decoder reads one record's bytes as another's and hands back a tree of
    /// plausible nonsense — the wrong sky, drawn with no complaint.
    ///
    /// Both directions, since a record may grow as easily as shrink.
    #[test]
    fn an_index_of_another_width_is_refused() {
        let cell = Cell {
            id: CellId::ROOT,
            rank_lo: 0,
            rank_hi: 512,
            child_mask: 0xFF,
            aggregate: Aggregate::of_system([0.0; 3], 1.0, 5000.0, 0),
        };
        let bytes = Index::from_cells([cell]).to_bytes();
        assert!(Index::from_bytes(&bytes).is_some(), "a good file reads");

        let mut wider = bytes.clone();
        wider.extend_from_slice(&[0u8; 32]);
        assert_eq!(Index::from_bytes(&wider), None, "a wider record read");

        let narrower = &bytes[..bytes.len() - 32];
        assert_eq!(Index::from_bytes(narrower), None, "a narrower record read");
    }

    /// An index at another format version is refused, and says which.
    ///
    /// The payload's record width rides on this version and on nothing
    /// else, a block of points carrying no header of its own. A directory
    /// from before the magnitude went to `f32` is well-formed at every
    /// other check, so this is the only thing standing between it and a
    /// galaxy decoded out of the wrong bytes.
    #[test]
    fn an_index_at_another_version_is_refused() {
        let cell = Cell {
            id: CellId::ROOT,
            rank_lo: 0,
            rank_hi: 512,
            child_mask: 0xFF,
            aggregate: Aggregate::of_system([0.0; 3], 1.0, 5000.0, 0),
        };
        let bytes = Index::from_cells([cell]).to_bytes();
        assert_eq!(index_version(&bytes), Some(INDEX_VERSION));

        let mut stale = bytes.clone();
        stale[4..6].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(index_version(&stale), Some(1));
        assert_eq!(Index::from_bytes(&stale), None);

        assert_eq!(index_version(b"nope, not an index at all"), None);
    }
}
