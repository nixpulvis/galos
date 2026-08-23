//! The walks: one traversal of the tree, read for each presentation.
//!
//! Drawing and fetching are one question — what the camera needs — and
//! [`Index::needed`] answers it, turning a camera pose and a presentation into
//! the cells to draw as marks and the cells to splat as a field. The renderer
//! draws what is needed and resident, the loader fetches what is needed and
//! absent, and the evictor drops what is resident and no longer needed. One
//! predicate, three consumers.
//!
//! Both presentations are marks over a field; they differ in the cut and in
//! what the field carries.
//!
//! - **Shell** is the map: a screen-space budget refines the cell that covers
//!   the most screen first and stops when the budget is spent. Its own systems
//!   draw as marks, and a cell it does not fully refine splats the rest, a field
//!   coloured by the political mix.
//! - **Real** is the sky, and it is one quantity split at the visibility floor
//!   rather than two modes. Stars that clear the limit draw as discrete marks —
//!   the photometric walk keeps a giant far out and prunes a cell of dim
//!   dwarfs — and everything below the floor sums into the glow, the field the
//!   opening-angle walk splats beneath them. The residual rule keeps a star
//!   drawn discretely out of the glow behind it, so the two never double count
//!   and no star falls between them.
//!
//! The index is small and always resident, so a walk touches no payload and no
//! server: it plans on the aggregates alone, and what a slow fetch costs is
//! detail, never presence.

use crate::aggregate::Cell;
use crate::geometry::CellId;
use galos_photometry::{EYE_LIMIT, apparent_magnitude_ly};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// The point budget the map traversal spends, in systems drawn as marks.
///
/// Sized so it cannot bind while marks are still separable: a 2 px mark needs
/// about 6.7 px of separation, which bounds a 1080p screen to some 46,000
/// distinguishable points. Set to that, the walk runs out only once marks
/// already overlap, so completeness is a consequence rather than a rule. Tuned
/// once the build runs.
pub const DEFAULT_POINT_BUDGET: u64 = 46_000;

/// Below this projected size a cell's contents land on about one pixel and can
/// add nothing, so the map does not refine past it.
pub const FLOOR_PX: f64 = 1.0;

/// A cell wider than this on screen is refined for the glow; narrower, it
/// splats. Half a degree, the "fraction of a degree" the opening-angle test
/// turns on.
pub const GLOW_OPENING_ANGLE: f64 = 0.5 * std::f64::consts::PI / 180.0;

/// Which presentation the tree is read for.
///
/// Not the same as the walks: `Real` runs two of them at once, since discrete
/// stars and the glow are one photometric quantity split at the visibility
/// floor rather than a choice between them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The map's translucent balls over a political field, on the point budget.
    Shell,
    /// The sky: discrete stars over the glow, on the photometric limit and the
    /// opening angle together.
    Real,
}

/// A camera pose and lens, in light years, with no renderer in it.
///
/// The map fills this from its orbit camera each frame — `forward` and `up`
/// resolved from the rotation rather than left as a quaternion, so the walk
/// stays renderer-agnostic — and everything downstream reads these plain
/// numbers.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct View {
    /// The eye position, light years.
    pub eye: [f64; 3],
    /// The unit direction the camera looks along.
    pub forward: [f64; 3],
    /// The unit up direction.
    pub up: [f64; 3],
    /// Vertical field of view, radians.
    pub fov_y: f32,
    /// Viewport height, pixels.
    pub viewport_height: f32,
    /// Viewport width over height.
    pub aspect: f32,
}

impl View {
    /// How many pixels one radian of arc covers vertically, the factor that
    /// turns an angular size into a projected one.
    pub fn pixels_per_radian(&self) -> f64 {
        self.viewport_height as f64 / (2.0 * (self.fov_y as f64 / 2.0).tan())
    }

    /// The projected size, in pixels, of something `size_ly` across seen from
    /// `distance_ly` away. Infinite at zero distance, where the camera is inside
    /// it.
    pub fn projected_px(&self, size_ly: f64, distance_ly: f64) -> f64 {
        if distance_ly <= 0.0 {
            f64::INFINITY
        } else {
            size_ly / distance_ly * self.pixels_per_radian()
        }
    }
}

/// What a walk asks for: the cells whose systems draw as discrete marks and the
/// cells that draw as a splat.
///
/// `marks` is also the fetch set, since a mark is a system from a cell's
/// payload; `splats` draw from the aggregate alone and need nothing loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct Needed {
    pub mode: Mode,
    pub marks: Vec<CellId>,
    pub splats: Vec<CellId>,
}

/// The resident tree of cell aggregates, keyed by address.
///
/// Small enough to hold whole — a few megabytes over the galaxy — so every walk
/// reads it without a fetch. The payloads it points at are loaded separately and
/// cached elsewhere; this is the index the walks plan on.
#[derive(Clone, Debug, Default)]
pub struct Index {
    cells: HashMap<CellId, Cell>,
}

impl Index {
    /// Build an index from a set of cells.
    pub fn from_cells(cells: impl IntoIterator<Item = Cell>) -> Index {
        Index { cells: cells.into_iter().map(|c| (c.id, c)).collect() }
    }

    /// The cell at an address, if the tree holds it.
    pub fn get(&self, id: CellId) -> Option<&Cell> {
        self.cells.get(&id)
    }

    /// How many cells the index holds.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// The root cell, if present.
    pub fn root(&self) -> Option<&Cell> {
        self.get(CellId::ROOT)
    }

    /// The children of a cell that exist in the tree, in octant order.
    pub fn children<'a>(&'a self, cell: &'a Cell) -> impl Iterator<Item = &'a Cell> {
        let ids = cell.id.children();
        (0..8u8).filter_map(move |octant| {
            cell.has_child(octant).then(|| self.get(ids[octant as usize])).flatten()
        })
    }

    /// The projected size of a cell's edge, in pixels, from this view.
    fn projected_edge(&self, view: &View, cell: &Cell) -> f64 {
        let center = cell.id.bounds().center();
        view.projected_px(cell.id.edge_ly(), distance(view.eye, center))
    }

    /// The cells the camera needs for a presentation: the marks to draw and the
    /// cells to splat as a field.
    pub fn needed(&self, view: &View, mode: Mode) -> Needed {
        match mode {
            Mode::Shell => self.walk_screen(view, DEFAULT_POINT_BUDGET),
            Mode::Real => Needed {
                mode: Mode::Real,
                marks: self.discrete_stars(view),
                splats: self.glow_field(view),
            },
        }
    }

    /// The Shell walk: refine the largest cell first until the point budget is
    /// spent, then splat whatever was left unrefined as the political field.
    pub fn walk_screen(&self, view: &View, budget: u64) -> Needed {
        let mut marks = Vec::new();
        let mut splats = Vec::new();
        let Some(root) = self.root() else {
            return Needed { mode: Mode::Shell, marks, splats };
        };

        // The root's own slice is always drawn; the heap holds the frontier,
        // largest on screen first.
        let mut drawn = root.slice_len();
        marks.push(root.id);
        let mut heap = BinaryHeap::new();
        heap.push(Priority { size: self.projected_edge(view, root), id: root.id });

        while let Some(Priority { size, id }) = heap.pop() {
            let cell = self.get(id).expect("frontier cell is in the tree");
            let children: Vec<&Cell> = self.children(cell).collect();
            let adds: u64 = children.iter().map(|c| c.slice_len()).sum();
            let can_refine = !children.is_empty()
                && size >= FLOOR_PX
                && drawn + adds <= budget;

            if can_refine {
                drawn += adds;
                for child in children {
                    marks.push(child.id);
                    heap.push(Priority {
                        size: self.projected_edge(view, child),
                        id: child.id,
                    });
                }
            } else if !cell.is_leaf() {
                // Left unrefined with a subtree still under it: splat the rest.
                splats.push(id);
            }
        }

        Needed { mode: Mode::Shell, marks, splats }
    }

    /// The discrete stars of the Real sky: descend where a cell's brightest star
    /// clears the limit, prune where it cannot, and mark every cell reached.
    fn discrete_stars(&self, view: &View) -> Vec<CellId> {
        let mut marks = Vec::new();
        let mut stack = vec![CellId::ROOT];
        while let Some(id) = stack.pop() {
            let Some(cell) = self.get(id) else { continue };
            if !self.cell_visible(view, cell) {
                continue;
            }
            marks.push(id);
            for child in self.children(cell) {
                stack.push(child.id);
            }
        }
        marks
    }

    /// Whether any star a cell holds could clear the visibility limit, measured
    /// to the nearest point of the cell so the test never drops a visible star.
    fn cell_visible(&self, view: &View, cell: &Cell) -> bool {
        let Some(m_min) = cell.aggregate.m_min() else {
            return false;
        };
        let d_min = cell.id.bounds().distance_to(view.eye);
        if d_min <= 0.0 {
            return true;
        }
        apparent_magnitude_ly(m_min as f64, d_min) <= EYE_LIMIT
    }

    /// The glow under the Real sky: descend while a cell subtends more than the
    /// opening angle, and splat it once it subtends less — the summed light of
    /// everything below the visibility floor.
    fn glow_field(&self, view: &View) -> Vec<CellId> {
        let mut splats = Vec::new();
        let mut stack = vec![CellId::ROOT];
        while let Some(id) = stack.pop() {
            let Some(cell) = self.get(id) else { continue };
            let d = distance(view.eye, cell.id.bounds().center());
            let angle = if d <= 0.0 { f64::INFINITY } else { cell.id.edge_ly() / d };
            if cell.is_leaf() || angle <= GLOW_OPENING_ANGLE {
                splats.push(id);
            } else {
                for child in self.children(cell) {
                    stack.push(child.id);
                }
            }
        }
        splats
    }
}

/// Straight-line distance between two points, light years.
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// A frontier entry ordered by projected size, so the heap yields the cell that
/// covers the most screen first.
struct Priority {
    size: f64,
    id: CellId,
}

impl PartialEq for Priority {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size
    }
}
impl Eq for Priority {}
impl Ord for Priority {
    fn cmp(&self, other: &Self) -> Ordering {
        self.size.total_cmp(&other.size)
    }
}
impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::Aggregate;

    /// The galactic centre, where the test cells are hung so a nearby eye has
    /// small light-year distances to work with.
    const HERE: [f64; 3] = [0.0, 900.0, 24400.0];

    /// Which octant of its parent a child sits in, from the low bit of each
    /// coordinate.
    fn octant_of(child: CellId) -> u8 {
        (child.x & 1) as u8
            | (((child.y & 1) as u8) << 1)
            | (((child.z & 1) as u8) << 2)
    }

    /// A leaf-or-branch cell with a chosen slice, children, and brightest
    /// magnitude, its aggregate placed at the cell's own centre.
    fn cell(id: CellId, slice: u64, child_mask: u8, m_min: f64) -> Cell {
        let agg = Aggregate::of_system(id.bounds().center(), m_min, 5000.0, 0);
        Cell { id, rank_lo: 0, rank_hi: slice, child_mask, aggregate: agg }
    }

    /// The connected chain of ancestors from ROOT down to `target`, each linking
    /// to the next with a one-system slice, so a walk starting at ROOT can
    /// descend to the small cells a test works on.
    fn chain_to(target: CellId, m_min: f64) -> Vec<Cell> {
        (0..target.level)
            .map(|level| {
                let here = CellId::of_point(HERE, level);
                let next = CellId::of_point(HERE, level + 1);
                cell(here, 1, 1 << octant_of(next), m_min)
            })
            .collect()
    }

    /// A connected tree: ROOT down to a 16 ly parent at level 13 with its two
    /// low children at level 14.
    fn small_tree(
        parent_slice: u64,
        child_slice: u64,
        m_min: f64,
    ) -> (Index, CellId, [CellId; 2]) {
        let parent = CellId::of_point(HERE, 13);
        let kids = parent.children();
        let mut cells = chain_to(parent, m_min);
        cells.push(cell(parent, parent_slice, 0b0000_0011, m_min));
        cells.push(cell(kids[0], child_slice, 0, m_min));
        cells.push(cell(kids[1], child_slice, 0, m_min));
        (Index::from_cells(cells), parent, [kids[0], kids[1]])
    }

    fn eye_out(cell: CellId, out_ly: f64) -> View {
        let c = cell.bounds().center();
        View {
            eye: [c[0], c[1], c[2] - out_ly],
            forward: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
            fov_y: std::f32::consts::FRAC_PI_4,
            viewport_height: 1080.0,
            aspect: 16.0 / 9.0,
        }
    }

    /// The chain really is connected: ROOT's descent reaches the deep parent.
    #[test]
    fn the_test_chain_is_connected() {
        let (index, parent, _kids) = small_tree(10, 10, 4.0);
        let mut here = index.root().expect("a root");
        while here.id != parent {
            here = index.children(here).next().expect("a child on the chain");
        }
        assert_eq!(here.id, parent);
    }

    /// Projected size is the angular size times pixels per radian: something as
    /// wide as it is far off subtends one radian.
    #[test]
    fn projection_is_angle_times_pixels_per_radian() {
        let view = eye_out(CellId::ROOT, 1.0);
        let ppr = view.pixels_per_radian();
        assert!((view.projected_px(10.0, 10.0) - ppr).abs() < 1e-6);
        assert!((view.projected_px(5.0, 10.0) - ppr / 2.0).abs() < 1e-6);
        assert_eq!(view.projected_px(1.0, 0.0), f64::INFINITY);
    }

    /// A generous budget refines to the leaves: every cell's slice is a mark and
    /// nothing is left to splat.
    #[test]
    fn a_generous_budget_reaches_the_leaves() {
        let (index, _parent, kids) = small_tree(100, 100, 4.0);
        let view = eye_out(CellId::of_point(HERE, 13), 4.0);
        let needed = index.walk_screen(&view, DEFAULT_POINT_BUDGET);
        assert!(needed.marks.contains(&kids[0]));
        assert!(needed.marks.contains(&kids[1]));
        assert!(needed.splats.is_empty());
    }

    /// A budget too small to admit the children leaves the parent unrefined, so
    /// it draws its own slice as marks and splats the subtree under it.
    #[test]
    fn a_tight_budget_splats_rather_than_refines() {
        let (index, parent, kids) = small_tree(100, 5000, 4.0);
        let view = eye_out(CellId::of_point(HERE, 13), 4.0);
        let needed = index.walk_screen(&view, 1000);
        assert!(needed.marks.contains(&parent));
        assert!(!needed.marks.contains(&kids[0]));
        assert!(needed.splats.contains(&parent));
    }

    /// The photometric walk keeps a cell whose brightest star clears the limit
    /// from close by, and prunes a dim cell seen from far off.
    #[test]
    fn photometry_keeps_the_bright_and_prunes_the_dim() {
        let parent = CellId::of_point(HERE, 13);

        let (bright, _p, kids) = small_tree(10, 10, -1.0);
        let near = eye_out(parent, 4.0);
        let seen = bright.needed(&near, Mode::Real);
        assert!(seen.marks.contains(&kids[0]));
        // Real also carries the glow beneath the stars.
        assert!(!seen.splats.is_empty());

        // The same tree but dim, seen from thirty thousand light years: the deep
        // cells cannot clear the limit, so the walk never reaches the leaves.
        let (dim, _p, kids) = small_tree(10, 10, 15.0);
        let far = eye_out(parent, 30_000.0);
        assert!(!dim.needed(&far, Mode::Real).marks.contains(&kids[0]));
    }

    /// The glow walk refines a cell that fills the view down to its leaves, and
    /// splats a cell that subtends less than the opening angle.
    #[test]
    fn the_glow_refines_near_and_splats_far() {
        let (index, _parent, kids) = small_tree(10, 10, 4.0);

        // Close in, the 16 ly parent subtends far more than half a degree and
        // refines to its leaves, which splat.
        let near = eye_out(CellId::of_point(HERE, 13), 2.0);
        let close = index.needed(&near, Mode::Real);
        assert!(close.splats.contains(&kids[0]));
        assert!(close.splats.contains(&kids[1]));
        assert!(!close.splats.contains(&CellId::ROOT));

        // From far enough that even the whole cube subtends under the angle, the
        // root itself splats.
        let far = eye_out(CellId::ROOT, 20_000_000.0);
        assert!(index.needed(&far, Mode::Real).splats.contains(&CellId::ROOT));
    }

    /// An empty index asks for nothing, in every mode.
    #[test]
    fn an_empty_index_needs_nothing() {
        let index = Index::default();
        let view = eye_out(CellId::ROOT, 1.0);
        for mode in [Mode::Shell, Mode::Real] {
            let needed = index.needed(&view, mode);
            assert!(needed.marks.is_empty());
            assert!(needed.splats.is_empty());
        }
    }
}
