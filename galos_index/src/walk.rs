//! The walks: one traversal of the tree, read for each presentation.
//!
//! Drawing and fetching are one question, what the view needs, and
//! [`Index::needed`] answers it, turning a viewpoint and a presentation into
//! the cells to draw as marks and the cells to splat as a field. Drawing takes
//! what is needed and resident, loading fetches what is needed and absent, and
//! eviction drops what is resident and no longer needed. One
//! predicate, three consumers.
//!
//! Both presentations are marks over a field; they differ in the cut and in
//! what the field carries.
//!
//! - **Shell** is the overview: a cell's slice draws as marks once its systems
//!   separate on screen, and a cell whose contents do not yet separate splats
//!   the rest, a field coloured by the political mix.
//! - **Real** is the sky, and it is one quantity split at the visibility floor
//!   rather than two modes. Stars that clear the limit draw as discrete marks
//!   (the photometric walk keeps a giant far out and prunes a cell of dim
//!   dwarfs), and everything below the floor sums into the glow, the field the
//!   opening-angle walk splats beneath them. The residual rule keeps a star
//!   drawn discretely out of the glow behind it, so the two never double count
//!   and no star falls between them.
//!
//! The index is small and always resident, so a walk touches no payload and no
//! server: it plans on the aggregates alone, and what a slow fetch costs is
//! detail, never presence.

use crate::aggregate::Cell;
use crate::geometry::CellId;
use galos_photometry::{Distance, Magnitude};
use std::collections::HashMap;

/// The mark limit, in pixels: the smallest a splat draws as more than a point.
/// A cell whose contents' own spread projects to less than this is one circle;
/// past it the circle splits, the "One circle, splitting" law of galaxy.md.
/// Two pixels on a 1080-line window, the figure that section's ladder turns on.
pub const SPLIT_PX: f64 = 2.0;

/// The top of the split's cross-fade band, an octave above [`SPLIT_PX`]. Across
/// `SPLIT_PX..SPLIT_FULL_PX` a cell and its children both draw, their weights
/// summing to one, so the level handoff crosses over rather than popping; above
/// it the children carry the region alone.
pub const SPLIT_FULL_PX: f64 = 4.0;

/// Two marks read as two only when their centres are more than this many pixels
/// apart — a 2 px mark at about 0.3 coverage. A leaf spawns its systems as
/// individual marks once their mean spacing subtends this, and stays one splat
/// until then, so a far cell never spawns a square of overlapping points.
pub const MARK_SEPARATION_PX: f64 = 6.7;

/// The separation the realistic view resolves stars at, in pixels
///
/// A star is a point the size of the instrument's spread (a pixel or two), so
/// two of them read apart far closer than two map marks do — [`MARK_SEPARATION_PX`]
/// is the mark's, this is the point spread's. The realistic view resolves a
/// cell's systems down to this, which is why a cluster stays a field of stars
/// where the map would collapse it to one mark. See galaxy.md: the limit is the
/// PSF in Real mode, the smallest stable mark in map mode.
pub const STAR_SEPARATION_PX: f64 = 2.0;

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
    /// Translucent balls over a political field.
    Shell,
    /// The sky: discrete stars over the glow, on the photometric limit and the
    /// opening angle together.
    Real,
}

/// A viewpoint in light years: where the eye is, which way it looks, and the
/// lens it looks through.
///
/// Orientation is the two unit vectors `forward` and `up`, not a rotation in
/// any particular form, so the walk is plain vector arithmetic and the caller
/// converts from whatever it keeps its own orientation in.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct View {
    /// The eye position, light years.
    pub eye: [f64; 3],
    /// The unit direction looked along.
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
    /// `distance_ly` away. Infinite at zero distance, where the eye is inside
    /// it.
    pub fn projected_px(&self, size_ly: f64, distance_ly: f64) -> f64 {
        if distance_ly <= 0.0 {
            f64::INFINITY
        } else {
            size_ly / distance_ly * self.pixels_per_radian()
        }
    }

    /// Whether a box falls within the view, for culling the walk to the frame.
    ///
    /// A cone about `forward`, wide enough to circumscribe the rectangular
    /// frustum (its diagonal half-angle), grown by the box's own angular radius
    /// so a box straddling the edge is kept. A box the eye sits inside is
    /// always kept. Conservative by design: it may keep a box just off a
    /// corner, but never drops one the frame would show.
    pub fn sees(&self, bounds: &crate::geometry::Aabb) -> bool {
        if bounds.distance_to(self.eye) <= 0.0 {
            return true;
        }
        let center = bounds.center();
        let to = [
            center[0] - self.eye[0],
            center[1] - self.eye[1],
            center[2] - self.eye[2],
        ];
        let dist = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
        let along = (to[0] * self.forward[0]
            + to[1] * self.forward[1]
            + to[2] * self.forward[2])
            / dist;
        let angle = along.clamp(-1.0, 1.0).acos();
        let ext = [
            bounds.max[0] - bounds.min[0],
            bounds.max[1] - bounds.min[1],
            bounds.max[2] - bounds.min[2],
        ];
        let radius =
            0.5 * (ext[0] * ext[0] + ext[1] * ext[1] + ext[2] * ext[2]).sqrt();
        let box_angle = (radius / dist).min(1.0).asin();
        angle - box_angle <= self.diagonal_half_fov()
    }

    /// Half the angle across the frustum's diagonal, radians: the cone that
    /// circumscribes the rectangular field, so a cull about it never cuts a
    /// visible corner.
    fn diagonal_half_fov(&self) -> f64 {
        let half_y = (self.fov_y as f64 / 2.0).tan();
        let half_x = half_y * self.aspect as f64;
        (half_x * half_x + half_y * half_y).sqrt().atan()
    }
}

/// One cell to draw as a splat, and the weight it lays into the field.
///
/// `blend` is a cross-level fade in `0.0..=1.0`. A frontier cell that is safely
/// one circle carries the full weight; a cell partway into its split shares its
/// weight with its children, the parent taking `1 - alpha` and the children the
/// rest by their count, so the handoff crosses over rather than popping. The
/// blends under any point of the sky sum to one, so the field they accumulate
/// into is conserved through every split.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SplatRef {
    /// The cell whose aggregate is drawn.
    pub id: CellId,
    /// The share of its weight this draw carries, `0.0..=1.0`.
    pub blend: f64,
}

/// What a walk asks for: the cells whose systems draw as discrete marks and the
/// cells that draw as a splat.
///
/// `marks` is also the fetch set, since a mark is a system from a cell's
/// payload; `splats` draw from the aggregate alone and need nothing loaded,
/// each with the weight it lays down so a split conserves the field.
#[derive(Clone, Debug, PartialEq)]
pub struct Needed {
    pub mode: Mode,
    pub marks: Vec<CellId>,
    pub splats: Vec<SplatRef>,
}

/// The resident tree of cell aggregates, keyed by address.
///
/// Small enough to hold whole (a few megabytes over the galaxy), so every walk
/// reads it without a fetch. The payloads it points at are loaded separately and
/// cached elsewhere; this is the index the walks plan on.
#[derive(Clone, Debug, Default, PartialEq)]
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

    /// Every cell whose box comes within `radius` light years of `center`: the
    /// cells a spyglass region must load to hold every system inside it.
    ///
    /// A cell straddling the sphere is kept, so the caller filters the points it
    /// loads by their true distance; a cell wholly outside cannot own a system
    /// inside and is left out. Additive slices put a system in exactly one cell,
    /// so the union of these cells' payloads is every system in reach with no
    /// duplicate. Linear over the resident index, which is small and asked only
    /// when the region moves, not per frame.
    pub fn region(&self, center: [f64; 3], radius: f64) -> Vec<CellId> {
        self.cells
            .values()
            .filter(|cell| cell.id.bounds().distance_to(center) <= radius)
            .map(|cell| cell.id)
            .collect()
    }

    /// The projected size, in pixels, of a cell's *contents* — their own spread
    /// from the count-weighted second moments, not the box that holds them.
    ///
    /// This is the quantity the split test turns on. A coarse cell near the
    /// galactic plane holds a thin slab: its box is a cube spanning the whole
    /// thickness, but its contents are shallow, and it is the contents that
    /// decide when the cell's systems separate on screen.
    fn projected_extent(&self, view: &View, cell: &Cell) -> f64 {
        view.projected_px(
            contents_extent(cell),
            distance(view.eye, contents_center(cell)),
        )
    }

    /// The cells the view needs for a presentation: the marks to draw and the
    /// cells to splat as a field.
    pub fn needed(&self, view: &View, mode: Mode) -> Needed {
        match mode {
            Mode::Shell => self.walk_screen(view),
            Mode::Real => Needed {
                mode: Mode::Real,
                marks: self.discrete_stars(view),
                splats: self.glow_field(view),
            },
        }
    }

    /// The Shell walk: the "One circle, splitting" descent.
    ///
    /// One traversal, two cuts, and both are pure functions of where the eye is
    /// — no budget, no frustum, nothing history-dependent — so the same eye
    /// position always returns the same view, whatever path reached it.
    ///
    /// - **Marks.** A cell's own magnitude slice draws as discrete symbols
    ///   exactly when its systems separate on screen: their mean spacing
    ///   subtends more than [`MARK_SEPARATION_PX`]. Additive — every cell from
    ///   the root down to where separation fails lays its slice down — so the
    ///   bright systems that coarse slices carry are never lost.
    /// - **Glow.** A cell splats as one aggregate until its contents' spread
    ///   subtends more than [`SPLIT_PX`]; then it splits into its children,
    ///   cross-faded across the band up to [`SPLIT_FULL_PX`] so neither level
    ///   pops. Weight is conserved: the splat blends under any point sum to one.
    ///
    /// Nothing here bounds how many marks come back. The separation test is the
    /// only limit, and for a framed view it holds the count near the screen's
    /// own capacity; a frame-cost ceiling is a drawing concern that belongs at
    /// draw time, not in a set that must stay a function of position.
    pub fn walk_screen(&self, view: &View) -> Needed {
        let mut marks = Vec::new();
        let mut splats = Vec::new();
        let Some(root) = self.root() else {
            return Needed { mode: Mode::Shell, marks, splats };
        };

        // Each entry is a cell and the glow weight its ancestors' cross-fades
        // have handed down, one at the root. Order does not matter — every cell
        // is judged on its own — so a plain stack stands in for a heap.
        let mut stack = vec![(root.id, 1.0f64)];
        while let Some((id, weight)) = stack.pop() {
            let cell = self.get(id).expect("frontier cell is in the tree");

            // Marks: the cell's payload is wanted once even one of its systems
            // separates on screen. How many actually draw is the resolvable
            // prefix (see resolvable_count), grown per system at draw time; the
            // walk only says which cells the draw will want.
            if resolvable_count(cell, view, MARK_SEPARATION_PX) >= 1 {
                marks.push(id);
            }

            // Glow: only children carrying systems can take the handoff.
            let children: Vec<&Cell> = self
                .children(cell)
                .filter(|c| c.aggregate.count() > 0)
                .collect();
            let total: u64 = children.iter().map(|c| c.aggregate.count()).sum();

            // A leaf, or a cell whose children are all empty, is the glow
            // frontier: one splat carrying its subtree's whole density.
            if children.is_empty() || total == 0 {
                splats.push(SplatRef { id, blend: weight });
                continue;
            }

            // How far the cell has split into its children for the glow: none
            // below SPLIT_PX (one circle), all above SPLIT_FULL_PX (children
            // alone), a cross-fade between. The parent keeps `1 - alpha` of its
            // weight and hands `alpha` to the children by their count, so the
            // two sum to the cell's own weight throughout the transition.
            let size = self.projected_extent(view, cell);
            let alpha = ((size - SPLIT_PX) / (SPLIT_FULL_PX - SPLIT_PX))
                .clamp(0.0, 1.0);
            if alpha < 1.0 {
                splats.push(SplatRef { id, blend: weight * (1.0 - alpha) });
            }
            if alpha > 0.0 {
                for child in children {
                    let share = child.aggregate.count() as f64 / total as f64;
                    stack.push((child.id, weight * alpha * share));
                }
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
        Magnitude(m_min as f64).apparent(Distance::light_years(d_min))
            <= Magnitude::EYE_LIMIT
    }

    /// The glow under the Real sky: descend while a cell subtends more than the
    /// opening angle, and splat it once it subtends less: the summed light of
    /// everything below the visibility floor. Full weight each — the Real glow
    /// does not cross-fade levels yet — so a splat carries its whole cell.
    fn glow_field(&self, view: &View) -> Vec<SplatRef> {
        let mut splats = Vec::new();
        let mut stack = vec![CellId::ROOT];
        while let Some(id) = stack.pop() {
            let Some(cell) = self.get(id) else { continue };
            let d = distance(view.eye, cell.id.bounds().center());
            let angle =
                if d <= 0.0 { f64::INFINITY } else { cell.id.edge_ly() / d };
            if cell.is_leaf() || angle <= GLOW_OPENING_ANGLE {
                splats.push(SplatRef { id, blend: 1.0 });
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

/// A cell's contents' spread in light years: the count-weighted RMS radius,
/// floored at the mean spacing so a cell of one or a few systems still resolves
/// as the camera closes rather than staying a zero-extent point forever.
fn contents_extent(cell: &Cell) -> f64 {
    let count = cell.aggregate.count().max(1) as f64;
    let spacing = cell.id.edge_ly() / count.cbrt();
    cell.aggregate.count_extent().max(spacing)
}

/// Where a cell's contents sit: the count-weighted centroid, or the box centre
/// where the aggregate carries no weight of its own.
fn contents_center(cell: &Cell) -> [f64; 3] {
    cell.aggregate.count_centroid().unwrap_or_else(|| cell.id.bounds().center())
}

/// The mean spacing of a cell's own magnitude slice, in light years.
///
/// A slice's systems are a spatially uniform sample of the cell's subtree, so
/// they are spread across its whole [`contents_extent`] however few they are.
/// Their spacing is that extent shared among the slice's count, and it decides
/// whether the slice draws as separate marks: a coarse slice holds a handful of
/// bright systems spread galaxy-wide and separates from far off, while a leaf's
/// dense slice only separates up close.
fn slice_spacing(cell: &Cell) -> f64 {
    let slice = cell.slice_len().max(1) as f64;
    contents_extent(cell) / slice.cbrt()
}

/// How many of a cell's own systems separate on screen: the prefix of its
/// magnitude-ordered payload worth drawing as discrete marks.
///
/// The slice's systems sit [`slice_spacing`] apart across the subtree. Where
/// that already subtends the mark separation every one draws; where it is
/// finer the prefix is decimated to the separation — `slice · (projected /
/// MARK_SEPARATION_PX)^3` — so the drawn systems land one mark apart whatever
/// the distance. Continuous in distance, so a cell fills in and empties one
/// system at a time rather than switching on whole and exposing its box.
pub fn resolvable_count(cell: &Cell, view: &View, separation_px: f64) -> u64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::Aggregate;

    /// The cell ids of a plan's splats, for the tests that only care which
    /// cells the field drew and not what weight each carried.
    fn splat_ids(needed: &Needed) -> Vec<CellId> {
        needed.splats.iter().map(|s| s.id).collect()
    }

    /// The view keeps a box ahead of the eye and drops one behind it.
    #[test]
    fn sees_ahead_not_behind() {
        let view = View {
            eye: [0.0, 0.0, 0.0],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            fov_y: std::f32::consts::FRAC_PI_2,
            viewport_height: 1000.0,
            aspect: 1.0,
        };
        let ahead = crate::geometry::Aabb {
            min: [-1.0, -1.0, -11.0],
            max: [1.0, 1.0, -9.0],
        };
        let behind = crate::geometry::Aabb {
            min: [-1.0, -1.0, 9.0],
            max: [1.0, 1.0, 11.0],
        };
        assert!(view.sees(&ahead), "a box ahead is in view");
        assert!(!view.sees(&behind), "a box behind is culled");
    }

    /// A box the eye sits inside is always in view.
    #[test]
    fn sees_the_box_it_is_inside() {
        let view = View {
            eye: [0.0, 0.0, 0.0],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            fov_y: std::f32::consts::FRAC_PI_2,
            viewport_height: 1000.0,
            aspect: 1.0,
        };
        let around = crate::geometry::Aabb {
            min: [-5.0, -5.0, -5.0],
            max: [5.0, 5.0, 5.0],
        };
        assert!(view.sees(&around));
    }

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

    /// Close leaves draw as marks: their slices' systems separate on screen. The
    /// glow still carries them — a mark is a weightless symbol over the field,
    /// not a replacement for it — so the leaves splat as well.
    #[test]
    fn close_leaves_draw_as_marks() {
        let (index, _parent, kids) = small_tree(100, 100, 4.0);
        let view = eye_out(CellId::of_point(HERE, 13), 4.0);
        let needed = index.walk_screen(&view);
        assert!(needed.marks.contains(&kids[0]));
        assert!(needed.marks.contains(&kids[1]));
    }

    /// Far off, the whole tree is one circle at the root and nothing spawns; the
    /// root carries the full weight since nothing finer draws.
    #[test]
    fn far_is_one_circle_no_marks() {
        let (index, _parent, _kids) = small_tree(10, 10, 4.0);
        let far = eye_out(CellId::ROOT, 1.0e8);
        let out = index.walk_screen(&far);
        assert!(out.marks.is_empty(), "a point-sized galaxy spawned entities");
        assert_eq!(out.splats.len(), 1, "more than one circle for the galaxy");
        assert_eq!(out.splats[0].id, CellId::ROOT);
        assert!((out.splats[0].blend - 1.0).abs() < 1e-9);
    }

    /// A far dense leaf stays one splat and spawns nothing — the fix for far
    /// cells loading as squares of overlapping points — and only resolves to
    /// marks once its systems separate on screen up close.
    #[test]
    fn a_dense_leaf_splats_far_and_marks_near() {
        let id = CellId::of_point(HERE, 11);
        let c = id.bounds().center();
        let mut agg = Aggregate::ZERO;
        for i in 0..64u64 {
            let off = i as f64 * 0.5;
            agg = agg.merge(Aggregate::of_system(
                [c[0] + off, c[1], c[2]],
                4.0,
                5000.0,
                0,
            ));
        }
        let leaf =
            Cell { id, rank_lo: 0, rank_hi: 64, child_mask: 0, aggregate: agg };
        let mut cells = chain_to(id, 4.0);
        cells.push(leaf);
        let index = Index::from_cells(cells);

        let far = eye_out(id, 60_000.0);
        let out = index.walk_screen(&far);
        assert!(!out.marks.contains(&id), "a far dense leaf spawned its slice");
        assert!(
            splat_ids(&out).contains(&id),
            "a far dense leaf was not splatted"
        );

        let near = eye_out(id, 3.0);
        assert!(index.walk_screen(&near).marks.contains(&id));
    }

    /// A cell's resolvable prefix is full up close, a decimated fraction at
    /// range, and nothing once its systems fall below one mark apart — the
    /// continuous fill that keeps a cell from switching on whole.
    #[test]
    fn resolvable_count_shrinks_with_distance() {
        let id = CellId::of_point(HERE, 11);
        let c = id.bounds().center();
        let mut agg = Aggregate::ZERO;
        for i in 0..512u64 {
            let off = i as f64 * 0.2;
            agg = agg.merge(Aggregate::of_system(
                [c[0] + off, c[1], c[2]],
                4.0,
                5000.0,
                0,
            ));
        }
        let cell = Cell {
            id,
            rank_lo: 0,
            rank_hi: 512,
            child_mask: 0,
            aggregate: agg,
        };

        assert_eq!(
            resolvable_count(&cell, &eye_out(id, 5.0), MARK_SEPARATION_PX),
            512,
            "not full up close"
        );
        let mid =
            resolvable_count(&cell, &eye_out(id, 900.0), MARK_SEPARATION_PX);
        assert!(mid > 0 && mid < 512, "expected a decimated prefix, got {mid}");
        assert_eq!(
            resolvable_count(
                &cell,
                &eye_out(id, 5_000_000.0),
                MARK_SEPARATION_PX
            ),
            0,
            "still drawing when one dot"
        );
    }

    /// Residency reads position, never direction: the walk returns the same
    /// marks however the camera turns about one eye, so a turn changes nothing
    /// to fetch or evict and cannot churn the resident set — and a view reached
    /// by turning is the view reached any other way.
    #[test]
    fn marks_ignore_rotation() {
        let (index, parent, _kids) = small_tree(100, 100, 4.0);
        let c = parent.bounds().center();
        let looking = |forward: [f64; 3]| View {
            eye: [c[0], c[1], c[2] - 200.0],
            forward,
            up: [0.0, 1.0, 0.0],
            fov_y: std::f32::consts::FRAC_PI_4,
            viewport_height: 1080.0,
            aspect: 16.0 / 9.0,
        };
        let toward = index.walk_screen(&looking([0.0, 0.0, 1.0]));
        let away = index.walk_screen(&looking([0.0, 0.0, -1.0]));
        let side = index.walk_screen(&looking([1.0, 0.0, 0.0]));
        assert_eq!(toward.marks, away.marks, "facing away changed the marks");
        assert_eq!(toward.marks, side.marks, "facing side changed the marks");
    }

    /// Closing in, the root's contents clear the limit and it splits: the root
    /// is no longer the circle drawn, its children carry the region.
    #[test]
    fn closing_in_splits_the_root() {
        let (index, _parent, _kids) = small_tree(10, 10, 4.0);
        let near = eye_out(CellId::ROOT, 1.0e6);
        let out = index.walk_screen(&near);
        assert!(
            !splat_ids(&out).contains(&CellId::ROOT),
            "the root refused to split"
        );
    }

    /// A split conserves the weight: a cell partway into its cross-fade draws
    /// alongside its children, and the blends laid down sum to one, so the field
    /// neither brightens nor dims across the transition.
    #[test]
    fn a_split_conserves_the_weight() {
        let root = CellId::ROOT;
        let kids = root.children();
        let index = Index::from_cells(vec![
            cell(root, 1, 0b0000_0011, 4.0),
            cell(kids[0], 1, 0, 4.0),
            cell(kids[1], 1, 0, 4.0),
        ]);
        // A distance that puts the root partway into its band, so root and
        // children both draw rather than one replacing the other outright.
        let view = eye_out(root, 6.0e7);
        let needed = index.walk_screen(&view);
        assert!(needed.splats.len() > 1, "the root did not begin to split");
        let total: f64 = needed.splats.iter().map(|s| s.blend).sum();
        assert!((total - 1.0).abs() < 1e-9, "weight not conserved: {total}");
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
        let close = splat_ids(&index.needed(&near, Mode::Real));
        assert!(close.contains(&kids[0]));
        assert!(close.contains(&kids[1]));
        assert!(!close.contains(&CellId::ROOT));

        // From far enough that even the whole cube subtends under the angle, the
        // root itself splats.
        let far = eye_out(CellId::ROOT, 20_000_000.0);
        assert!(
            splat_ids(&index.needed(&far, Mode::Real)).contains(&CellId::ROOT)
        );
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

    /// The region query keeps a cell whose box reaches the sphere and drops one
    /// beyond it, so a spyglass loads every cell that could hold a system in
    /// reach and no cell that cannot.
    #[test]
    fn region_keeps_cells_within_reach() {
        let (index, parent, kids) = small_tree(10, 10, 4.0);
        let center = parent.bounds().center();

        // A radius spanning the parent's own box takes it and its children.
        let near = index.region(center, parent.edge_ly());
        assert!(near.contains(&parent));
        assert!(near.contains(&kids[0]));

        // A vanishing radius about the centre still takes the cells that contain
        // the point, but a centre far outside the galaxy takes nothing.
        let far = index.region([1.0e9, 1.0e9, 1.0e9], 1.0);
        assert!(far.is_empty());
    }
}
