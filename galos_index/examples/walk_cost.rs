//! Where `Index::needed`'s milliseconds go.
//!
//! ```sh
//! cargo build --release --example walk_cost -p galos_index
//! ./target/release/examples/walk_cost .index/full
//! ```
//!
//! The screen walk is 22–29 ms a frame at 200 M systems and the marked cell
//! count (151,619) is the number everyone assumes is the cost. This attributes
//! the wall clock instead: one replica of `walk_screen` that counts what it
//! touches, then the same walk with one thing taken away at a time, then the
//! same walk again over a flat array of the fields it actually reads.
//!
//! Every variant visits the same cells and answers the same marks and splats
//! — checked, not assumed — so a difference between two rows is the cost of
//! the one thing between them.

use galos_index::walk::{
    GLOW_OPENING_ANGLE, MARK_SEPARATION_PX, SPLIT_FULL_PX, SPLIT_PX,
};
use galos_index::{Cell, CellId, Index, Mode, View};
use galos_photometry::{Distance, Magnitude};
use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::hint::black_box;
use std::time::{Duration, Instant};

/// A view of the galaxy from `distance` light years out, looking in, as
/// `tests/zooming.rs`'s `looking_in` frames it.
fn looking_in(distance: f64) -> View {
    View {
        eye: [distance, 0.0, 25_000.0],
        forward: [-1.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        fov_y: 0.8,
        viewport_height: 1080.0,
        aspect: 16.0 / 9.0,
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

// The three view-independent figures the walk reads per cell, replicated from
// `walk.rs` (they are private there). Each is a pure function of the cell.

fn contents_extent(cell: &Cell) -> f64 {
    let count = cell.aggregate.count().max(1) as f64;
    let spacing = cell.id.edge_ly() / count.cbrt();
    cell.aggregate.count_extent().max(spacing)
}

fn contents_center(cell: &Cell) -> [f64; 3] {
    cell.aggregate.count_centroid().unwrap_or_else(|| cell.id.bounds().center())
}

fn slice_spacing(cell: &Cell) -> f64 {
    let slice = cell.slice_len().max(1) as f64;
    contents_extent(cell) / slice.cbrt()
}

/// What one walk answered and what it touched on the way.
#[derive(Default)]
struct Tally {
    marks: usize,
    splats: usize,
    visited: usize,
    gets: usize,
    blend: f64,
}

impl Tally {
    fn line(&self, what: &str, took: Duration) {
        println!(
            "{what:<34} {took:>9.2?}  marks {:>7} splats {:>7} \
             visited {:>7} gets {:>8}",
            self.marks, self.splats, self.visited, self.gets,
        );
    }
}

/// The walk as it stands, replicated so the lookups can be counted: every
/// `get` the map's own walk makes, and nothing else.
fn replica(index: &Index, view: &View, record: &mut Vec<CellId>) -> Tally {
    let mut tally = Tally::default();
    let mut marks: Vec<CellId> = Vec::new();
    let mut splats: Vec<(CellId, f64)> = Vec::new();
    let Some(root) = index.root() else { return tally };
    tally.gets += 1;

    let mut stack = vec![(root.id, 1.0f64)];
    while let Some((id, weight)) = stack.pop() {
        record.push(id);
        tally.gets += 1;
        let cell = index.get(id).expect("frontier cell is in the tree");
        tally.visited += 1;

        if resolvable(cell, view, MARK_SEPARATION_PX) >= 1 {
            marks.push(id);
        }

        // `Index::children` is a `get` per set child bit.
        let mut children: Vec<&Cell> = Vec::new();
        let ids = cell.id.children();
        for octant in 0..8u8 {
            if !cell.has_child(octant) {
                continue;
            }
            record.push(ids[octant as usize]);
            tally.gets += 1;
            if let Some(child) = index.get(ids[octant as usize]) {
                if child.aggregate.count() > 0 {
                    children.push(child);
                }
            }
        }
        let total: u64 = children.iter().map(|c| c.aggregate.count()).sum();

        if children.is_empty() || total == 0 {
            splats.push((id, weight));
            continue;
        }

        let size = projected_extent(view, cell);
        let alpha =
            ((size - SPLIT_PX) / (SPLIT_FULL_PX - SPLIT_PX)).clamp(0.0, 1.0);
        if alpha < 1.0 {
            splats.push((id, weight * (1.0 - alpha)));
        }
        if alpha > 0.0 {
            for child in children {
                let share = child.aggregate.count() as f64 / total as f64;
                stack.push((child.id, weight * alpha * share));
            }
        }
    }

    tally.marks = marks.len();
    tally.splats = splats.len();
    tally.blend = splats.iter().map(|s| s.1).sum();
    black_box(&marks);
    black_box(&splats);
    tally
}

fn projected_extent(view: &View, cell: &Cell) -> f64 {
    view.projected_px(
        contents_extent(cell),
        distance(view.eye, contents_center(cell)),
    )
}

fn resolvable(cell: &Cell, view: &View, separation_px: f64) -> u64 {
    let slice = cell.slice_len();
    if slice == 0 {
        return 0;
    }
    let projected = view.projected_px(
        slice_spacing(cell),
        distance(view.eye, contents_center(cell)),
    );
    let fraction = (projected / separation_px).powi(3).min(1.0);
    ((slice as f64) * fraction).round() as u64
}

/// The replica with the mark test taken out: the descent and its lookups
/// alone, so the difference is what `resolvable_count` and the mark push cost.
fn without_marks(index: &Index, view: &View) -> Tally {
    let mut tally = Tally::default();
    let mut splats: Vec<(CellId, f64)> = Vec::new();
    let Some(root) = index.root() else { return tally };

    let mut stack = vec![(root.id, 1.0f64)];
    while let Some((id, weight)) = stack.pop() {
        tally.gets += 1;
        let cell = index.get(id).expect("frontier cell is in the tree");
        tally.visited += 1;

        let mut children: Vec<&Cell> = Vec::new();
        let ids = cell.id.children();
        for octant in 0..8u8 {
            if cell.has_child(octant) {
                tally.gets += 1;
                if let Some(child) = index.get(ids[octant as usize]) {
                    if child.aggregate.count() > 0 {
                        children.push(child);
                    }
                }
            }
        }
        let total: u64 = children.iter().map(|c| c.aggregate.count()).sum();
        if children.is_empty() || total == 0 {
            splats.push((id, weight));
            continue;
        }
        let size = projected_extent(view, cell);
        let alpha =
            ((size - SPLIT_PX) / (SPLIT_FULL_PX - SPLIT_PX)).clamp(0.0, 1.0);
        if alpha < 1.0 {
            splats.push((id, weight * (1.0 - alpha)));
        }
        if alpha > 0.0 {
            for child in children {
                let share = child.aggregate.count() as f64 / total as f64;
                stack.push((child.id, weight * alpha * share));
            }
        }
    }
    tally.splats = splats.len();
    black_box(&splats);
    tally
}

/// The replica with the per-cell `Vec` of children taken out: the same
/// lookups into a fixed array on the stack, so the difference is the
/// allocation.
fn without_child_vec(index: &Index, view: &View) -> Tally {
    let mut tally = Tally::default();
    let mut marks: Vec<CellId> = Vec::new();
    let mut splats: Vec<(CellId, f64)> = Vec::new();
    let Some(root) = index.root() else { return tally };

    let mut stack = vec![(root.id, 1.0f64)];
    while let Some((id, weight)) = stack.pop() {
        tally.gets += 1;
        let cell = index.get(id).expect("frontier cell is in the tree");
        tally.visited += 1;

        if resolvable(cell, view, MARK_SEPARATION_PX) >= 1 {
            marks.push(id);
        }

        let mut kept: [Option<(CellId, u64)>; 8] = [None; 8];
        let mut n = 0usize;
        let mut total = 0u64;
        let ids = cell.id.children();
        for octant in 0..8u8 {
            if cell.has_child(octant) {
                tally.gets += 1;
                if let Some(child) = index.get(ids[octant as usize]) {
                    let count = child.aggregate.count();
                    if count > 0 {
                        kept[n] = Some((child.id, count));
                        n += 1;
                        total += count;
                    }
                }
            }
        }

        if n == 0 || total == 0 {
            splats.push((id, weight));
            continue;
        }
        let size = projected_extent(view, cell);
        let alpha =
            ((size - SPLIT_PX) / (SPLIT_FULL_PX - SPLIT_PX)).clamp(0.0, 1.0);
        if alpha < 1.0 {
            splats.push((id, weight * (1.0 - alpha)));
        }
        if alpha > 0.0 {
            for slot in kept.iter().take(n) {
                let (child, count) = slot.expect("kept child");
                let share = count as f64 / total as f64;
                stack.push((child, weight * alpha * share));
            }
        }
    }
    tally.marks = marks.len();
    tally.splats = splats.len();
    tally.blend = splats.iter().map(|s| s.1).sum();
    black_box(&marks);
    black_box(&splats);
    tally
}

/// One node of a tree flattened with the cells as they stand: the same
/// aggregate the map's walk reads, reached by an index rather than a hash.
#[derive(Copy, Clone)]
struct CellNode {
    cell: Cell,
    first_child: u32,
    children: u8,
}

/// The tree flattened breadth-first, carrying whole cells: what the walk
/// costs with the map taken out and nothing else changed.
fn flatten_cells(index: &Index) -> Vec<CellNode> {
    let mut nodes: Vec<CellNode> = Vec::with_capacity(index.len());
    let Some(root) = index.root() else { return nodes };
    nodes.push(CellNode { cell: *root, first_child: 0, children: 0 });
    let mut at = 0usize;
    while at < nodes.len() {
        let cell = nodes[at].cell;
        let held = index.get(cell.id).expect("a flattened cell is in the tree");
        let first = nodes.len() as u32;
        let mut n = 0u8;
        for child in index.children(held) {
            if child.aggregate.count() > 0 {
                nodes.push(CellNode {
                    cell: *child,
                    first_child: 0,
                    children: 0,
                });
                n += 1;
            }
        }
        nodes[at].first_child = first;
        nodes[at].children = n;
        at += 1;
    }
    nodes
}

/// The walk over whole cells in a flat array: every figure worked out per
/// visit exactly as the map's walk does it, with no lookup to pay for. The
/// difference from the replica is the map; the difference from
/// [`flat_walk`] is the recomputation.
fn cells_walk(nodes: &[CellNode], view: &View) -> Tally {
    let mut tally = Tally::default();
    let mut marks: Vec<CellId> = Vec::new();
    let mut splats: Vec<(CellId, f64)> = Vec::new();
    if nodes.is_empty() {
        return tally;
    }
    let mut stack = vec![(0u32, 1.0f64)];
    while let Some((at, weight)) = stack.pop() {
        let node = &nodes[at as usize];
        let cell = &node.cell;
        tally.visited += 1;

        if resolvable(cell, view, MARK_SEPARATION_PX) >= 1 {
            marks.push(cell.id);
        }

        let first = node.first_child as usize;
        let last = first + node.children as usize;
        let total: u64 =
            nodes[first..last].iter().map(|n| n.cell.aggregate.count()).sum();
        if node.children == 0 || total == 0 {
            splats.push((cell.id, weight));
            continue;
        }

        let size = projected_extent(view, cell);
        let alpha =
            ((size - SPLIT_PX) / (SPLIT_FULL_PX - SPLIT_PX)).clamp(0.0, 1.0);
        if alpha < 1.0 {
            splats.push((cell.id, weight * (1.0 - alpha)));
        }
        if alpha > 0.0 {
            for (offset, child) in nodes[first..last].iter().enumerate() {
                let share = child.cell.aggregate.count() as f64 / total as f64;
                stack.push(((first + offset) as u32, weight * alpha * share));
            }
        }
    }
    tally.marks = marks.len();
    tally.splats = splats.len();
    tally.blend = splats.iter().map(|s| s.1).sum();
    black_box(&marks);
    black_box(&splats);
    tally
}

/// One node of the flat tree: the fields the walk reads, precomputed, with
/// the children contiguous.
#[derive(Copy, Clone)]
struct Node {
    id: CellId,
    center: [f64; 3],
    extent: f64,
    spacing: f64,
    slice: u64,
    count: u64,
    first_child: u32,
    children: u8,
}

/// The tree flattened breadth-first: a cell's children land next to each
/// other, and the three view-independent figures are worked out once.
fn flatten(index: &Index) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::with_capacity(index.len());
    let Some(root) = index.root() else { return nodes };
    let node_of = |cell: &Cell| Node {
        id: cell.id,
        center: contents_center(cell),
        extent: contents_extent(cell),
        spacing: slice_spacing(cell),
        slice: cell.slice_len(),
        count: cell.aggregate.count(),
        first_child: 0,
        children: 0,
    };
    nodes.push(node_of(root));
    let mut at = 0usize;
    while at < nodes.len() {
        let id = nodes[at].id;
        let cell = index.get(id).expect("a flattened cell is in the tree");
        let first = nodes.len() as u32;
        let mut n = 0u8;
        for child in index.children(cell) {
            if child.aggregate.count() > 0 {
                nodes.push(node_of(child));
                n += 1;
            }
        }
        nodes[at].first_child = first;
        nodes[at].children = n;
        at += 1;
    }
    nodes
}

/// The same walk over the flat tree: no map, no hashing, one distance a cell,
/// and the per-cell figures read rather than recomputed.
fn flat_walk(nodes: &[Node], view: &View) -> Tally {
    let mut tally = Tally::default();
    let mut marks: Vec<CellId> = Vec::new();
    let mut splats: Vec<(CellId, f64)> = Vec::new();
    if nodes.is_empty() {
        return tally;
    }
    let px_per_rad = view.pixels_per_radian();
    let mut stack = vec![(0u32, 1.0f64)];
    while let Some((at, weight)) = stack.pop() {
        let node = &nodes[at as usize];
        tally.visited += 1;

        // One distance for both tests, where the map's walk takes it twice.
        let d = distance(view.eye, node.center);
        let scale = if d <= 0.0 { f64::INFINITY } else { px_per_rad / d };

        if node.slice > 0 {
            let projected = node.spacing * scale;
            let fraction = (projected / MARK_SEPARATION_PX).powi(3).min(1.0);
            if ((node.slice as f64) * fraction).round() as u64 >= 1 {
                marks.push(node.id);
            }
        }

        if node.children == 0 {
            splats.push((node.id, weight));
            continue;
        }
        let first = node.first_child as usize;
        let last = first + node.children as usize;
        let total: u64 = nodes[first..last].iter().map(|n| n.count).sum();
        if total == 0 {
            splats.push((node.id, weight));
            continue;
        }

        let size = node.extent * scale;
        let alpha =
            ((size - SPLIT_PX) / (SPLIT_FULL_PX - SPLIT_PX)).clamp(0.0, 1.0);
        if alpha < 1.0 {
            splats.push((node.id, weight * (1.0 - alpha)));
        }
        if alpha > 0.0 {
            for (offset, child) in nodes[first..last].iter().enumerate() {
                let share = child.count as f64 / total as f64;
                stack.push(((first + offset) as u32, weight * alpha * share));
            }
        }
    }
    tally.marks = marks.len();
    tally.splats = splats.len();
    tally.blend = splats.iter().map(|s| s.1).sum();
    black_box(&marks);
    black_box(&splats);
    tally
}

/// Every `get` the walk made, replayed in the order it made them: the lookup
/// half of the walk with none of the arithmetic.
fn replay(index: &Index, record: &[CellId]) -> Duration {
    let at = Instant::now();
    let mut found = 0usize;
    for &id in record {
        found += index.get(id).is_some() as usize;
    }
    let took = at.elapsed();
    black_box(found);
    took
}

/// The sky's two walks as they were written against the map: what
/// `Mode::Real` must still answer, cell for cell.
fn real_off_the_map(index: &Index, view: &View) -> (Vec<CellId>, Vec<CellId>) {
    let mut marks = Vec::new();
    let mut stack = vec![CellId::ROOT];
    while let Some(id) = stack.pop() {
        let Some(cell) = index.get(id) else { continue };
        let visible = match cell.aggregate.m_min() {
            None => false,
            Some(m_min) => {
                let d_min = cell.id.bounds().distance_to(view.eye);
                d_min <= 0.0
                    || Magnitude(m_min as f64)
                        .apparent(Distance::light_years(d_min))
                        <= Magnitude::EYE_LIMIT
            }
        };
        if !visible {
            continue;
        }
        marks.push(id);
        for child in index.children(cell) {
            stack.push(child.id);
        }
    }

    let mut splats = Vec::new();
    let mut stack = vec![CellId::ROOT];
    while let Some(id) = stack.pop() {
        let Some(cell) = index.get(id) else { continue };
        let d = distance(view.eye, cell.id.bounds().center());
        let angle =
            if d <= 0.0 { f64::INFINITY } else { cell.id.edge_ly() / d };
        if cell.is_leaf() || angle <= GLOW_OPENING_ANGLE {
            splats.push(id);
        } else {
            for child in index.children(cell) {
                stack.push(child.id);
            }
        }
    }

    marks.sort();
    splats.sort();
    (marks, splats)
}

/// A multiply-shift hasher over the four words of a [`CellId`]: what a
/// lookup costs when the hash itself is arithmetic rather than SipHash.
#[derive(Default)]
struct Quick(u64);

impl std::hash::Hasher for Quick {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }

    fn write_u32(&mut self, v: u32) {
        self.0 = (self.0 ^ v as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_u8(&mut self, v: u8) {
        self.write_u32(v as u32);
    }
}

type QuickMap = HashMap<CellId, u32, BuildHasherDefault<Quick>>;

/// The same lookups against a map of addresses to indices rather than to
/// whole cells: one with the default hasher, one with [`Quick`]. Against the
/// row above, the first is what the 216-byte value costs and the second is
/// what SipHash costs.
fn replay_indices(
    index: &Index,
    record: &[CellId],
) -> (Duration, Duration, usize, usize) {
    let sip: HashMap<CellId, u32> = index
        .cells()
        .enumerate()
        .map(|(at, cell)| (cell.id, at as u32))
        .collect();
    let quick: QuickMap = index
        .cells()
        .enumerate()
        .map(|(at, cell)| (cell.id, at as u32))
        .collect();

    let at = Instant::now();
    let mut found = 0usize;
    for &id in record {
        found += sip.get(&id).is_some() as usize;
    }
    let sipped = at.elapsed();

    let at = Instant::now();
    let mut quickly = 0usize;
    for &id in record {
        quickly += quick.get(&id).is_some() as usize;
    }
    let quicked = at.elapsed();

    (sipped, quicked, found, quickly)
}

/// How deep the tree goes, and how wide each level is: what the walk's
/// visit count is a fact about.
fn levels(index: &Index) -> Vec<(u8, usize, usize, f64)> {
    let mut per: HashMap<u8, (usize, usize)> = HashMap::new();
    for cell in index.cells() {
        let row = per.entry(cell.id.level).or_default();
        row.0 += 1;
        row.1 += cell.is_leaf() as usize;
    }
    let mut rows: Vec<(u8, usize, usize, f64)> = per
        .into_iter()
        .map(|(level, (cells, leaves))| {
            (level, cells, leaves, CellId { level, x: 0, y: 0, z: 0 }.edge_ly())
        })
        .collect();
    rows.sort_by_key(|row| row.0);
    rows
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: walk_cost <index dir> [distance ly ...]");
        std::process::exit(2);
    });
    let dir = std::path::PathBuf::from(dir);
    let distances: Vec<f64> =
        std::env::args().skip(2).filter_map(|it| it.parse().ok()).collect();
    let distances = if distances.is_empty() {
        vec![10.0, 1_000.0, 25_000.0, 100_000.0]
    } else {
        distances
    };

    let at = Instant::now();
    let index = Index::read(&dir).expect("the index should read");
    println!(
        "index    {} cells, {} B a cell, {:.1} MB, read in {:.2?}",
        index.len(),
        std::mem::size_of::<Cell>(),
        (index.len() * std::mem::size_of::<Cell>()) as f64 / 1e6,
        at.elapsed(),
    );

    let at = Instant::now();
    let nodes = flatten(&index);
    println!(
        "flat     {} nodes, {} B a node, {:.1} MB, built in {:.2?}\n",
        nodes.len(),
        std::mem::size_of::<Node>(),
        (nodes.len() * std::mem::size_of::<Node>()) as f64 / 1e6,
        at.elapsed(),
    );

    let at = Instant::now();
    let cells = flatten_cells(&index);
    println!(
        "cells    {} nodes, {} B a node, {:.1} MB, built in {:.2?}",
        cells.len(),
        std::mem::size_of::<CellNode>(),
        (cells.len() * std::mem::size_of::<CellNode>()) as f64 / 1e6,
        at.elapsed(),
    );

    println!("\nlevel   cells   leaves    edge ly");
    for (level, count, leaves, edge) in levels(&index) {
        println!("{level:>5} {count:>7} {leaves:>8} {edge:>10.1}");
    }
    println!();

    for distance in distances {
        let view = looking_in(distance);
        println!("== {distance} ly out, looking in");

        // The walk as the map calls it, three times: the first pays the page
        // faults, the rest are what a frame costs.
        for round in 0..3 {
            let at = Instant::now();
            let needed = index.needed(&view, Mode::Shell);
            let took = at.elapsed();
            println!(
                "{:<34} {took:>9.2?}  marks {:>7} splats {:>7}",
                format!("Index::needed, round {round}"),
                needed.marks.len(),
                needed.splats.len(),
            );
            black_box(needed);
        }

        let mut record: Vec<CellId> = Vec::new();
        let at = Instant::now();
        let tally = replica(&index, &view, &mut record);
        tally.line("the replica, counted", at.elapsed());

        // The replica again with the recording left out, which is what the
        // rows below are compared against.
        let mut sink = Vec::new();
        let at = Instant::now();
        let tally = replica(&index, &view, &mut sink);
        let base = at.elapsed();
        sink.clear();
        sink.shrink_to_fit();
        tally.line("the replica", base);
        let blend = tally.blend;

        // The walk as it is now against the walk as it was written: the same
        // cells, in the same order, carrying the same field.
        let now = index.needed(&view, Mode::Shell);
        let mut was = Vec::new();
        let then = replica(&index, &view, &mut was);
        assert_eq!(now.marks.len(), then.marks, "the marks moved");
        assert_eq!(now.splats.len(), then.splats, "the splats moved");
        let carried: f64 = now.splats.iter().map(|s| s.blend).sum();
        assert!(
            (carried - blend).abs() < 1e-9,
            "the field moved: {carried} against {blend}",
        );
        println!(
            "{:<34} {:>9}  the same {} marks, {} splats, {:.6} of field",
            "Shell, as it was",
            "",
            now.marks.len(),
            now.splats.len(),
            carried,
        );

        // And the sky's two walks, cell for cell against the map-walked set.
        let at = Instant::now();
        let real = index.needed(&view, Mode::Real);
        let took = at.elapsed();
        let off_map = Instant::now();
        let (marks, splats) = real_off_the_map(&index, &view);
        let walked_the_map = off_map.elapsed();
        let mut now_marks = real.marks.clone();
        now_marks.sort();
        let mut now_splats: Vec<CellId> =
            real.splats.iter().map(|s| s.id).collect();
        now_splats.sort();
        assert_eq!(now_marks, marks, "the sky's discrete stars moved");
        assert_eq!(now_splats, splats, "the sky's glow moved");
        println!(
            "{:<34} {took:>9.2?}  the same {} stars, {} glow cells, \
             {walked_the_map:.2?} off the map",
            "Real, as it was",
            marks.len(),
            splats.len(),
        );

        let at = Instant::now();
        let tally = without_marks(&index, &view);
        tally.line("without the mark test", at.elapsed());

        let at = Instant::now();
        let tally = without_child_vec(&index, &view);
        tally.line("without the child Vec", at.elapsed());
        assert!(
            (tally.blend - blend).abs() < 1e-6,
            "the array walk answered a different field",
        );

        for round in 0..3 {
            let at = Instant::now();
            let tally = flat_walk(&nodes, &view);
            tally.line(&format!("the flat tree, round {round}"), at.elapsed());
            assert!(
                (tally.blend - blend).abs() < 1e-6,
                "the flat walk answered a different field",
            );
        }

        for round in 0..2 {
            let at = Instant::now();
            let tally = cells_walk(&cells, &view);
            tally.line(&format!("the cells flat, round {round}"), at.elapsed());
            assert!(
                (tally.blend - blend).abs() < 1e-6,
                "the flat cells answered a different field",
            );
        }

        let took = replay(&index, &record);
        println!(
            "{:<34} {took:>9.2?}  {} lookups, {:.1?} each",
            "the lookups alone, replayed",
            record.len(),
            took / record.len().max(1) as u32,
        );
        let (sipped, quicked, found, quickly) = replay_indices(&index, &record);
        println!(
            "{:<34} {sipped:>9.2?}  {found} of {} found, {:.1?} each",
            "indices, the same hasher",
            record.len(),
            sipped / record.len().max(1) as u32,
        );
        println!(
            "{:<34} {quicked:>9.2?}  {quickly} of {} found, {:.1?} each",
            "indices, a multiply-shift hash",
            record.len(),
            quicked / record.len().max(1) as u32,
        );
        println!();
    }
}
