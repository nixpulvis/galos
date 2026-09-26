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
//!   the rest, a field colored by the political mix.
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

use crate::core::aggregate::AGE_BUCKETS;
use crate::core::geometry::CellId;
use crate::read::index::Index;
use crate::read::index::Node;
use crate::read::index::distance;
use galos_photometry::{Distance, Magnitude};

/// The field's resolution limit, in pixels: the widest a cell's own contents
/// may be drawn as one splat.
///
/// **Half a pixel of RMS radius, which is about a pixel of cell.** A cell
/// stands for its systems by a centroid and a radius, so whatever structure
/// lies inside it is laid down as one blob — and the blob is as wide as the
/// cell's contents are. At two pixels the frontier cell was four to eight
/// pixels across once [`SPLIT_FULL_PX`] and the field's own Gaussian reach
/// were spent on it, and a colonisation filament or an arm's edge came out
/// as a row of overlapping blobs: blurred across the feature and lumpy
/// along it, on a lattice whose pitch was the cell. Under half a pixel the
/// cell it cannot resolve past is the pixel, which is the finest thing the
/// display can carry, and [`galos_map`]'s field floors its kernel at half a
/// pixel so neighbours still sum flat.
///
/// What it costs is the descent, and the descent is nearly free: the walk
/// already visits every cell at every zoom inside 25 kly, and the tree runs
/// out before the criterion does — measured over `.index/full` from 120 kly
/// out on a 1000-line frame, 131,893 splats at two pixels against 179,388
/// here, which is the whole of the tree the field can ever splat.
pub const SPLIT_PX: f64 = 0.5;

/// The top of the split's cross-fade band, an octave above [`SPLIT_PX`]. Across
/// `SPLIT_PX..SPLIT_FULL_PX` a cell and its children both draw, their weights
/// summing to one, so the level handoff crosses over rather than popping; above
/// it the children carry the region alone.
pub(crate) const SPLIT_FULL_PX: f64 = 1.0;

/// Two marks merge into one when their centres fall within this many pixels
///
/// **The mark's own size plus a margin.** A map mark is drawn at
/// [`galos_map`]'s `field::SMALLEST` radius, a pixel and a half across, and
/// two of them closer than about that read as one smudge rather than as two
/// places. Four pixels is the mark plus a couple of pixels of air, which is
/// where a pair still reads as a pair.
///
/// It is the whole of the marks' level of detail. A cell whose contents all
/// fall inside one mark is drawn as one aggregate mark
/// ([`BlobRef`]) instead of being read; a cell wider than that is
/// descended into, and its own slice is drawn at one mark to every
/// `MERGE_PX` squared of the footprint it covers. Both halves are the same
/// statement — *marks that would overlap are drawn as one* — so the drawn
/// count is set by the screen and never by how the tree happened to fall.
///
/// What it replaced was a floor: a cell was read only once it was worth
/// eight marks at 8.5 px of separation, which is a cell whose contents
/// subtend 13.6 px, and every finer cell drew nothing at all. The tree is
/// finest where the sky is densest, so that floor deleted the densest sky —
/// measured over `.index/full`, at 4 kly 73–89 % of the galactic plane's
/// systems sat in cells the walk marked nothing for, and the hole it left
/// widened with every zoom out because the cut is fixed in pixels and so is
/// a growing physical size.
pub const MERGE_PX: f64 = 4.0;

/// How far above the merge distance a cell is fully split, as a multiple of
/// it
///
/// An octave, which is the glow's band ([`SPLIT_PX`]..[`SPLIT_FULL_PX`]) read
/// at mark scale. Across it a cell's blob and the draw beneath it — its own
/// slice's marks and its children's blobs — share the weight, the blob taking
/// `1 - alpha` and everything below it `alpha`, so a level handoff crosses
/// over rather than popping. A cell's levels are an octave apart in size, so
/// only one level of the tree is ever mid-fade under any point of the sky.
pub(crate) const MERGE_BAND: f64 = 2.0;

/// The merge distance the realistic view resolves stars at, in pixels
///
/// A star is a point the size of the instrument's spread (a pixel or two), so
/// two of them read apart far closer than two map marks do — [`MERGE_PX`] is
/// the mark's, this is the point spread's. The realistic view resolves a
/// cell's systems down to this, which is why a cluster stays a field of stars
/// where the map would collapse it to one mark: the merge distance is the
/// point spread in [`Mode::Real`] and the smallest stable mark in
/// [`Mode::Shell`].
pub(crate) const STAR_MERGE_PX: f64 = 2.0;

/// A cell wider than this on screen is refined for the glow; narrower, it
/// splats. Half a degree, the "fraction of a degree" the opening-angle test
/// turns on.
pub(crate) const GLOW_OPENING_ANGLE: f64 = 0.5 * std::f64::consts::PI / 180.0;

/// Which presentation the tree is read for.
///
/// Not the same as the walks: `Real` runs two of them at once, since discrete
/// stars and the glow are one photometric quantity split at the visibility
/// floor rather than a choice between them.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Mode {
    /// Translucent balls over a political field.
    Shell,
    /// The sky: discrete stars over the glow, on the photometric limit and the
    /// opening angle together.
    ///
    /// `limit` is the faintest apparent magnitude the eye behind this view
    /// draws — the exposure's own zero point, not a constant. It is what the
    /// photometric cut measures against, so opening the exposure deepens the
    /// sky the walk answers with rather than only enlarging the stars already
    /// in it. See [`crate::Magnitude::EYE_LIMIT`] for where it rests.
    Real { limit: f64 },
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

/// The bubble a walk is clamped to: where the spyglass is centred and how
/// far it reaches, in light years.
///
/// **The clamp belongs in the walk and nowhere else.** It was applied
/// three times over after the fact — once in the fetch, once in the draw
/// and once in the evictor — and each of those first had to be handed
/// every cell the walk had marked. Which at a close zoom is the whole
/// tree: the merge distance in light years shrinks with the camera, so
/// nothing anywhere merges and every cell in the galaxy is marked.
/// Measured over `.index/full` from a hundred light years out, the walk
/// answered **195,524 marks** for a view holding twenty-seven cells, and
/// the frame spent milliseconds a pass throwing the rest away again.
///
/// Cut here, a subtree the bubble does not touch is never descended into
/// and never answered, so the sets the client works over are the sets it
/// draws from. Measured to the nearest point of a cell's box, so a cell
/// straddling the edge is kept and its own points are cut by their own
/// distance.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Reach {
    /// What the bubble is centred on: the camera's target.
    pub center: [f64; 3],
    /// How far it holds, light years.
    pub radius: f64,
}

impl Reach {
    /// Whether the bubble comes within a cell's box.
    fn holds(&self, id: CellId) -> bool {
        id.bounds().distance_to(self.center) <= self.radius
    }
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
    pub fn sees(&self, bounds: &crate::core::geometry::Aabb) -> bool {
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

/// One cell drawn as a single aggregate mark: everything it holds falls
/// inside one mark, so the marks it would draw are drawn as one.
///
/// Needs no payload — a blob says how many systems it stands for, how bright
/// the brightest of them is, and what their temperature and political mixes
/// are, all off the resident aggregate. `blend` is the cross-level fade in
/// `0.0..=1.0`, the blob taking `1 - alpha` of the weight while what is
/// beneath it takes the rest.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BlobRef {
    /// The cell whose aggregate is drawn as one mark.
    pub id: CellId,
    /// How many systems it stands for, which is its whole subtree. Carried
    /// for the same reason [`MarkRef`] carries its slice: the draw spreads
    /// its share over this and the alternative is a lookup a blob a frame.
    pub count: u64,
    /// The share of the draw this blob carries, `0.0..=1.0`.
    pub blend: f64,
    /// Where the mark goes: the count centroid of everything it stands for,
    /// in light years.
    pub at: [f64; 3],
    /// How many systems under it fall in each Recency bucket, which is what
    /// a span is answered against: [`crate::core::aggregate::Aggregate::aged`].
    ///
    /// The histogram and not merely its lowest bucket, because what a
    /// filter is owed is not "is anything here recent" but *how much of
    /// this mark is* — a merged mark stands for thousands of systems and
    /// is drawn at what their own marks would come to. Eight `u32`s a
    /// blob, which at the twenty thousand a wide view holds is 640 kB of a
    /// plan that is rebuilt only when the eye moves.
    pub aged: [u32; AGE_BUCKETS],
    /// The brightest absolute magnitude under it, which is what the sky's
    /// cut is taken against.
    ///
    /// Carried rather than looked up, as `count` and `at` are. **This is
    /// what a blob costs.** The draw reads all three of them once a blob a
    /// frame, and the index they would otherwise be read out of is the
    /// whole tree — millions of cells, so every read is a cache miss on a
    /// random address. Measured over `.index/full` at sixty thousand light
    /// years out, 10,114 drawn blobs cost 7.2 ms a frame that way, which
    /// was two thirds of the whole reconciliation pass.
    pub m_min: Option<f32>,
}

/// What a walk asks for: the cells whose systems draw as discrete marks, the
/// cells that draw as one merged mark, and the cells that draw as a splat.
///
/// `marks` is also the fetch set, since a mark is a system from a cell's
/// payload; `blobs` and `splats` draw from the aggregates alone and need
/// nothing loaded, each with the weight it lays down so a split conserves
/// what it draws.
///
/// **How much of a marked cell is drawn is not said here, and cannot be.**
/// The drawn density has to follow the sky's own — a region with ten times
/// the systems wants ten times the marks — so what each cell draws is a
/// share of its *population*, and a share is only meaningful against the
/// whole frame's. `galos_map`'s `bounded::reconcile` strikes it. A per-cell
/// answer worked out here was tried twice: as a share of the cell's
/// footprint area it drew one mark to every patch of screen whatever was in
/// it, which is a galaxy of uniform density, and the same figure with a
/// floor under it drew nothing at all in the finest cells, which is a hole
/// where the sky is densest.
/// One cell whose own slice the draw reads, and how many systems that is
///
/// The count rides along because every reader of the marks needs it — the
/// fetch to know how much of the cell to ask for, the draw to know how
/// much of it to take — and the walk has it in hand while the alternative
/// is a hash lookup per cell per frame in each of them. Measured over
/// `.index/full` at a wide zoom, that is sixty thousand lookups a pass
/// and three passes a frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MarkRef {
    /// The cell whose payload is read.
    pub id: CellId,
    /// How many systems it owns in its own slice.
    pub slice: u32,
    /// Where its contents sit, light years: the count centroid, as
    /// [`BlobRef::at`] is.
    ///
    /// What the draw bins by to find the patches of sky it is leaving
    /// dark ([`crate::read::screen::Empty`]). A cell above the frontier spreads
    /// its marks over the patch it covers rather than standing at one
    /// point, so this is where the cell *is* and not where each of its
    /// marks lands — which is the grain that question is asked at.
    pub at: [f64; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Needed {
    pub mode: Mode,
    pub marks: Vec<MarkRef>,
    pub blobs: Vec<BlobRef>,
    pub splats: Vec<SplatRef>,
}

impl Index {
    /// The cells the view needs for a presentation: the marks to draw, the
    /// cells to draw as one merged mark, and the cells to splat as a field.
    pub fn needed(
        &self,
        view: &View,
        mode: Mode,
        within: Option<Reach>,
    ) -> Needed {
        match mode {
            Mode::Shell => self.walk_screen(view, within),
            Mode::Real { limit } => {
                let (marks, blobs) =
                    self.frontier(view, STAR_MERGE_PX, Some(limit), within);
                Needed {
                    mode: Mode::Real { limit },
                    marks,
                    blobs,
                    splats: self.glow_field(view, within),
                }
            }
        }
    }

    /// The merge frontier: the anti-chain of cells whose whole contents fall
    /// inside one mark, and the cells above it whose own slices draw.
    ///
    /// **One rule, read twice.** Two marks that would overlap on screen are
    /// drawn as one, so:
    ///
    /// - a cell whose contents are no wider than `merge_px` is one mark —
    ///   [`BlobRef`], drawn off its aggregate, nothing read;
    /// - a cell wider than that is descended into, and its own slice draws at
    ///   one mark to every `merge_px` squared of the footprint it covers —
    ///   [`MarkRef`], the prefix of its magnitude order.
    ///
    /// The two meet: at the frontier a cell's footprint is one mark's worth
    /// of screen and carries one mark, which is the blob. So the count is
    /// self-bounding — one mark to every `merge_px` squared per layer of the
    /// anti-chain — with no budget, no share and no clamp anywhere in it.
    /// The share machinery this replaced was a global factor a frame's demand
    /// was divided by, and it could not be continuous across a cell face:
    /// two neighbours with different demands drew at different densities and
    /// the index's own boxes showed through.
    ///
    /// **Geometry survives by construction.** A cell merges only when
    /// everything it holds fits inside one mark, so a ring passes through
    /// many cells each still too wide to merge and comes out a ring; a line
    /// comes out a line. Hollow and filled stop being distinguishable exactly
    /// where the whole shape is smaller than a mark, where nothing could have
    /// told them apart anyway.
    ///
    /// **Merging never costs a mark.** A cell holding no more than its
    /// footprint can show separately is read rather than merged, however
    /// narrow it is: a lone star in the halo has a contents width of zero and
    /// would otherwise collapse into an unnamed blob at any distance, when it
    /// is exactly one mark and the map should draw it as itself.
    ///
    /// `photometric` is the sky's cut, and the magnitude in it is the eye's
    /// own: a subtree whose brightest star cannot clear that limit from here
    /// is dropped whole, which is what keeps [`Mode::Real`] from drawing the
    /// dwarfs the glow already carries. [`None`] asks for no cut at all,
    /// which is [`Mode::Shell`].
    fn frontier(
        &self,
        view: &View,
        merge_px: f64,
        photometric: Option<f64>,
        within: Option<Reach>,
    ) -> (Vec<MarkRef>, Vec<BlobRef>) {
        let mut marks = Vec::new();
        let mut blobs = Vec::new();
        if self.nodes.is_empty() {
            return (marks, blobs);
        }
        let mut stack = vec![(0u32, 1.0f64)];
        while let Some((at, shown)) = stack.pop() {
            let node = &self.nodes[at as usize];
            if within.is_some_and(|reach| !reach.holds(node.id)) {
                continue;
            }
            if photometric.is_some_and(|limit| !node_visible(view, node, limit))
            {
                continue;
            }
            let alpha = splitting(view, node, merge_px);
            if alpha < 1.0 {
                blobs.push(BlobRef {
                    id: node.id,
                    count: node.count,
                    blend: shown * (1.0 - alpha),
                    at: node.center,
                    aged: node.aged,
                    m_min: node.m_min,
                });
            }
            if alpha <= 0.0 {
                continue;
            }
            if node.slice > 0 {
                marks.push(MarkRef {
                    id: node.id,
                    slice: node.slice.min(u64::from(u32::MAX)) as u32,
                    at: node.center,
                });
            }
            for child in
                node.first_child..node.first_child + node.children as u32
            {
                stack.push((child, shown * alpha));
            }
        }
        (marks, blobs)
    }

    /// The Shell walk: the merge frontier over the political field.
    ///
    /// One traversal, two cuts, and both are pure functions of where the eye is
    /// — no budget, no frustum, nothing history-dependent — so the same eye
    /// position always returns the same view, whatever path reached it.
    ///
    /// - **Marks.** [`Index::frontier`] at [`MERGE_PX`]: the cells whose
    ///   contents fall inside one mark draw as one, and every cell above them
    ///   lays down as much of its own slice as its footprint can hold apart.
    /// - **Glow.** A cell splats as one aggregate until its contents' spread
    ///   subtends more than [`SPLIT_PX`]; then it splits into its children,
    ///   cross-faded across the band up to [`SPLIT_FULL_PX`] so neither level
    ///   pops. Weight is conserved: the splat blends under any point sum to one.
    ///
    /// **Two frontiers, one descent, and the field's is the finer of them.**
    /// The glow cuts at half a pixel of RMS radius and the marks at four
    /// pixels of contents width, so the field always refines past where the
    /// marks have merged — which is why the two are worked out in the same
    /// traversal and why the merged half of it carries on descending.
    ///
    /// **What it costs is the tree, not the marks.** The descent reaches all
    /// 204,466 cells at every zoom inside 25 kly: four fifths of the tree is
    /// 128–512 Ly cells whose contents subtend far more than the half pixel
    /// the split turns on, so nothing stops short of a leaf and the walk is
    /// linear in the tree with the marked count riding along. Which is why it
    /// descends [`Index::nodes`] rather than the map, and reads each cell's
    /// figures rather than working them out: **23 ms to 1.5 ms**, the same
    /// marks and the same field.
    pub fn walk_screen(&self, view: &View, within: Option<Reach>) -> Needed {
        let (marks, blobs) = self.frontier(view, MERGE_PX, None, within);
        Needed {
            mode: Mode::Shell,
            marks,
            blobs,
            splats: self.glow(view, within),
        }
    }

    /// The political field: descend while a cell's contents subtend more than
    /// [`SPLIT_PX`], splat it once they do not, and cross-fade the handoff.
    ///
    /// The parent keeps `1 - alpha` of its weight and hands `alpha` to the
    /// children by their count, so the two sum to the cell's own weight
    /// throughout the transition and the blends under any point of the sky
    /// sum to one.
    fn glow(&self, view: &View, within: Option<Reach>) -> Vec<SplatRef> {
        let mut splats = Vec::new();
        if self.nodes.is_empty() {
            return splats;
        }
        // Each entry is a node and the weight its ancestors' cross-fades have
        // handed down, one at the root. Order does not matter — every cell is
        // judged on its own — so a plain stack stands in for a heap.
        let mut stack = vec![(0u32, 1.0f64)];
        while let Some((at, weight)) = stack.pop() {
            let node = &self.nodes[at as usize];
            if within.is_some_and(|reach| !reach.holds(node.id)) {
                continue;
            }

            // Only children carrying systems can take the handoff.
            let kids = node.first_child as usize
                ..node.first_child as usize + node.children as usize;
            let total: u64 = self.nodes[kids.clone()]
                .iter()
                .filter(|child| child.count > 0)
                .map(|child| child.count)
                .sum();

            // A leaf, or a cell whose children are all empty, is the glow
            // frontier: one splat carrying its subtree's whole density.
            if total == 0 {
                splats.push(SplatRef { id: node.id, blend: weight });
                continue;
            }

            let size =
                view.projected_px(node.extent, distance(view.eye, node.center));
            let alpha = ((size - SPLIT_PX) / (SPLIT_FULL_PX - SPLIT_PX))
                .clamp(0.0, 1.0);
            if alpha < 1.0 {
                splats.push(SplatRef {
                    id: node.id,
                    blend: weight * (1.0 - alpha),
                });
            }
            if alpha > 0.0 {
                for child in kids {
                    let count = self.nodes[child].count;
                    if count == 0 {
                        continue;
                    }
                    let share = count as f64 / total as f64;
                    stack.push((child as u32, weight * alpha * share));
                }
            }
        }
        splats
    }

    /// The glow under the Real sky: descend while a cell subtends more than the
    /// opening angle, and splat it once it subtends less: the summed light of
    /// everything below the visibility floor. Full weight each — the Real glow
    /// does not cross-fade levels yet — so a splat carries its whole cell.
    fn glow_field(&self, view: &View, within: Option<Reach>) -> Vec<SplatRef> {
        let mut splats = Vec::new();
        if self.nodes.is_empty() {
            return splats;
        }
        let mut stack = vec![0u32];
        while let Some(at) = stack.pop() {
            let node = &self.nodes[at as usize];
            if within.is_some_and(|reach| !reach.holds(node.id)) {
                continue;
            }
            let d = distance(view.eye, node.id.bounds().center());
            let angle =
                if d <= 0.0 { f64::INFINITY } else { node.id.edge_ly() / d };
            if node.leaf || angle <= GLOW_OPENING_ANGLE {
                splats.push(SplatRef { id: node.id, blend: 1.0 });
            } else {
                for child in
                    node.first_child..node.first_child + node.children as u32
                {
                    stack.push(child);
                }
            }
        }
        splats
    }
}

/// Whether any star a cell holds could clear the visibility limit, measured
/// to the nearest point of the cell so the test never drops a visible star.
///
/// `limit` is the faintest apparent magnitude the eye draws, which is the
/// exposure's zero point rather than a constant: a cut frozen at
/// [`Magnitude::EYE_LIMIT`] while the client's floor moved with the exposure
/// meant opening the exposure could not deepen the sky, only fatten the stars
/// already in it.
///
/// A bare threshold, with no hysteresis band behind it. One was tried, on
/// the ground that an orbit drag translates the eye and a cell on the
/// threshold would cross it repeatedly: measured over `.index/full` turning
/// at two thousand light years back, six hundred frames, half a magnitude of
/// band changed the drawn set by one line of churn and left the count of
/// stars that left and came back at **zero either way**. What made the sky
/// blink was never this cut; it was the mark ration on top of it, which is
/// no longer there.
fn node_visible(view: &View, node: &Node, limit: f64) -> bool {
    let Some(m_min) = node.m_min else {
        return false;
    };
    let d_min = node.id.bounds().distance_to(view.eye);
    if d_min <= 0.0 {
        return true;
    }
    Magnitude(m_min as f64).apparent(Distance::light_years(d_min))
        <= Magnitude(limit)
}

/// How far a cell has split out of its own blob: nothing at the merge
/// distance, all of it an octave above ([`MERGE_BAND`]), a cross-fade
/// between.
///
/// The whole of the merge rule, off one figure: `width`, how wide the cell's
/// contents are on screen. Under the merge distance everything the cell
/// holds falls inside one mark, so one mark is what it is worth drawing;
/// over it the cell is descended into and its own slice draws.
///
/// **How much of that slice draws is not settled here.** It cannot be: the
/// drawn density has to follow the sky's, and that is a statement about the
/// whole frame rather than about one cell. See [`Needed`].
///
/// A cell holding one system is never merged, whatever its width — its
/// contents width is zero, so it would merge at any distance, and merging
/// one mark into one mark buys nothing while costing the star its name and
/// its place.
fn splitting(view: &View, node: &Node, merge_px: f64) -> f64 {
    if node.count <= 1 {
        return 1.0;
    }
    let width = view.projected_px(node.width, distance(view.eye, node.center));
    let full = merge_px * MERGE_BAND;
    ((width - merge_px) / (full - merge_px)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::aggregate::Aggregate;
    use crate::core::aggregate::Cell;
    use crate::read::index::{contents_extent, contents_width};

    /// The sky read at the eye's own limit, which is where the exposure
    /// rests.
    fn real() -> Mode {
        Mode::Real { limit: Magnitude::EYE_LIMIT.0 }
    }

    /// The cell ids of a plan's splats, for the tests that only care which
    /// cells the field drew and not what weight each carried.
    fn splat_ids(needed: &Needed) -> Vec<CellId> {
        needed.splats.iter().map(|s| s.id).collect()
    }

    /// The cell ids of a plan's marks, the slice each carries being beside
    /// the point for the tests that only ask which cells were read.
    fn mark_ids(needed: &Needed) -> Vec<CellId> {
        needed.marks.iter().map(|mark| mark.id).collect()
    }

    /// The cell ids of a plan's merged marks.
    fn blob_ids(needed: &Needed) -> Vec<CellId> {
        needed.blobs.iter().map(|b| b.id).collect()
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
        let ahead = crate::core::geometry::Aabb {
            min: [-1.0, -1.0, -11.0],
            max: [1.0, 1.0, -9.0],
        };
        let behind = crate::core::geometry::Aabb {
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
        let around = crate::core::geometry::Aabb {
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

    /// What a test cell holds: where its systems sit, how far they spread,
    /// how many it owns against how many its subtree holds, and what is
    /// under it.
    ///
    /// All six matter to the merge rule, which reads a cell's count, the
    /// distance to its contents and how wide they are. A cell holding one
    /// system is one mark and is never merged; a cell whose hundred systems
    /// are stacked at a point is one mark too, and rightly. A real
    /// ancestor's centroid is also where its systems are rather than where
    /// its box is — the level-one cell over the galactic centre has a box
    /// middle thirty thousand light years off.
    #[derive(Copy, Clone)]
    struct Held {
        /// Where its systems sit.
        at: [f64; 3],
        /// How far they spread along x, light years.
        across: f64,
        /// How many it owns in its own slice.
        slice: u64,
        /// How many its whole subtree holds.
        count: u64,
        /// Which octants have children.
        child_mask: u8,
        /// The brightest absolute magnitude under it.
        m_min: f64,
    }

    /// A cell with the aggregate `held` describes.
    fn holding(id: CellId, held: Held) -> Cell {
        let last = held.count.max(1) - 1;
        let step = if last == 0 { 0.0 } else { held.across / last as f64 };
        let agg = (0..held.count.max(1))
            .map(|n| {
                let off = n as f64 * step - held.across / 2.0;
                Aggregate::of_system(
                    [held.at[0] + off, held.at[1], held.at[2]],
                    held.m_min,
                    5000.0,
                    0,
                )
            })
            .fold(Aggregate::ZERO, Aggregate::merge);
        Cell {
            id,
            rank_lo: 0,
            rank_hi: held.slice,
            child_mask: held.child_mask,
            aggregate: agg,
        }
    }

    /// A one-system cell at its own box's middle: what the field's tests
    /// want, where a count and a spread are beside the point.
    fn cell(id: CellId, slice: u64, child_mask: u8, m_min: f64) -> Cell {
        holding(
            id,
            Held {
                at: id.bounds().center(),
                across: 0.0,
                slice,
                count: 1,
                child_mask,
                m_min,
            },
        )
    }

    /// The connected chain of ancestors from ROOT down to `target`, each
    /// linking to the next with a one-system slice and each standing for the
    /// `count` systems below it, so a walk starting at ROOT can descend to
    /// the small cells a test works on.
    ///
    /// Their contents are stacked at [`HERE`], which is where everything the
    /// chain stands for is; how wide they come out is the roll-up's to say,
    /// off the children.
    fn chain_to(target: CellId, m_min: f64, count: u64) -> Vec<Cell> {
        (0..target.level)
            .map(|level| {
                let here = CellId::of_point(HERE, level);
                let next = CellId::of_point(HERE, level + 1);
                holding(
                    here,
                    Held {
                        at: HERE,
                        across: 0.0,
                        slice: 1,
                        count,
                        child_mask: 1 << octant_of(next),
                        m_min,
                    },
                )
            })
            .collect()
    }

    /// A connected tree: ROOT down to a 16 ly parent at level 13 with its two
    /// low children at level 14.
    ///
    /// The slice length asked for here is the cell's own, and the ancestors
    /// stand for every system beneath them, so these cells have
    /// `slice_len != count` — which is what a real cell looks like. Each
    /// cell's systems are spread across its own box, as a real cell's are.
    fn small_tree(
        parent_slice: u64,
        child_slice: u64,
        m_min: f64,
    ) -> (Index, CellId, [CellId; 2]) {
        let parent = CellId::of_point(HERE, 13);
        let kids = parent.children();
        let whole = parent_slice + 2 * child_slice;
        let mut cells = chain_to(parent, m_min, whole);
        cells.push(holding(
            parent,
            Held {
                at: parent.bounds().center(),
                across: parent.edge_ly(),
                slice: parent_slice,
                count: whole,
                child_mask: 0b0000_0011,
                m_min,
            },
        ));
        for kid in [kids[0], kids[1]] {
            cells.push(holding(
                kid,
                Held {
                    at: kid.bounds().center(),
                    across: kid.edge_ly(),
                    slice: child_slice,
                    count: child_slice,
                    child_mask: 0,
                    m_min,
                },
            ));
        }
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

    /// Close leaves draw as marks: their contents are far wider than one mark
    /// on screen, so nothing about them merges. The glow still carries them —
    /// a mark is a weightless symbol over the field, not a replacement for it
    /// — so the leaves splat as well.
    #[test]
    fn close_leaves_draw_as_marks() {
        let (index, _parent, kids) = small_tree(100, 100, 4.0);
        let view = eye_out(CellId::of_point(HERE, 13), 4.0);
        let needed = index.walk_screen(&view, None);
        assert!(mark_ids(&needed).contains(&kids[0]));
        assert!(mark_ids(&needed).contains(&kids[1]));
        assert!(blob_ids(&needed).is_empty(), "a close leaf merged");
    }

    /// Far off, the whole tree is one circle at the root and nothing spawns;
    /// the root carries the full weight since nothing finer draws, and the
    /// marks it stands for are the one blob it merged into.
    ///
    /// How far "far off" is comes off [`SPLIT_PX`] rather than being picked,
    /// so the test moves with the band: at two pixels a hundred million light
    /// years was far enough, and at half a pixel it is not.
    #[test]
    fn far_is_one_circle_no_marks() {
        let (index, _parent, _kids) = small_tree(10, 10, 4.0);
        let root = index.root().expect("a root");
        let lens = eye_out(CellId::ROOT, 1.0);
        let out =
            contents_extent(root) * lens.pixels_per_radian() / (SPLIT_PX * 0.5);
        let far = eye_out(CellId::ROOT, out);
        let out = index.walk_screen(&far, None);
        assert!(out.marks.is_empty(), "a point-sized galaxy read a payload");
        assert_eq!(blob_ids(&out), vec![CellId::ROOT], "not one merged mark");
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
        let mut cells = chain_to(id, 4.0, 64);
        cells.push(leaf);
        let index = Index::from_cells(cells);

        let far = eye_out(id, 60_000.0);
        let out = index.walk_screen(&far, None);
        assert!(out.marks.is_empty(), "a far dense leaf read its payload");
        // The frontier is the *topmost* cell that merges, and every cell on
        // the chain above the leaf holds exactly what it holds, so the whole
        // tree comes out as the root's one mark.
        assert_eq!(blob_ids(&out), vec![CellId::ROOT], "not one merged mark");
        assert!(
            splat_ids(&out).contains(&id),
            "a far dense leaf was not splatted"
        );

        let near = eye_out(id, 3.0);
        assert!(mark_ids(&index.walk_screen(&near, None)).contains(&id));
    }

    /// A cell is read while its contents are wider than one mark and merged
    /// once they are not, and where the two meet is its contents' own width
    /// and nothing else.
    ///
    /// The frontier is the whole of the marks' level of detail. How much of
    /// a read cell is drawn is the frame's to say ([`Needed`]), so what is
    /// checked here is the cut itself: the distance the cell stops being
    /// read at is the distance its contents stop covering [`MERGE_PX`].
    #[test]
    fn a_cell_is_read_until_it_fits_inside_one_mark() {
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
        let leaf = Cell {
            id,
            rank_lo: 0,
            rank_hi: 512,
            child_mask: 0,
            aggregate: agg,
        };
        let mut cells = chain_to(id, 4.0, 512);
        cells.push(leaf);
        let index = Index::from_cells(cells);

        // A hundred light years of systems, so up close the cell is a long
        // way past one mark and is read.
        let reads = |away: f64| {
            mark_ids(&index.walk_screen(&eye_out(id, away), None)).contains(&id)
        };
        assert!(reads(5.0));
        assert!(reads(3_000.0));

        // The distance at which its contents stop covering the merge
        // distance, from the far side of the band. Past it nothing is read
        // and the tree comes out as one merged mark.
        let lens = eye_out(id, 1.0);
        let span = contents_width(&leaf);
        let out = span * lens.pixels_per_radian() / MERGE_PX;
        let far = index.walk_screen(&eye_out(id, out * 2.0), None);
        assert!(!mark_ids(&far).contains(&id), "still reading at one dot");
        assert_eq!(blob_ids(&far), vec![CellId::ROOT], "not one merged mark");
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
        let toward = index.walk_screen(&looking([0.0, 0.0, 1.0]), None);
        let away = index.walk_screen(&looking([0.0, 0.0, -1.0]), None);
        let side = index.walk_screen(&looking([1.0, 0.0, 0.0]), None);
        assert_eq!(toward.marks, away.marks, "facing away changed the marks");
        assert_eq!(toward.marks, side.marks, "facing side changed the marks");
    }

    /// Closing in, the root's contents clear the limit and it splits: the root
    /// is no longer the circle drawn, its children carry the region.
    #[test]
    fn closing_in_splits_the_root() {
        let (index, _parent, _kids) = small_tree(10, 10, 4.0);
        let near = eye_out(CellId::ROOT, 1.0e6);
        let out = index.walk_screen(&near, None);
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
        let needed = index.walk_screen(&view, None);
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
        let seen = bright.needed(&near, real(), None);
        assert!(mark_ids(&seen).contains(&kids[0]));
        // Real also carries the glow beneath the stars.
        assert!(!seen.splats.is_empty());

        // The same tree but dim, seen from thirty thousand light years: the deep
        // cells cannot clear the limit, so the walk never reaches the leaves.
        let (dim, _p, kids) = small_tree(10, 10, 15.0);
        let far = eye_out(parent, 30_000.0);
        assert!(!mark_ids(&dim.needed(&far, real(), None)).contains(&kids[0]));
    }

    /// The glow walk refines a cell that fills the view down to its leaves, and
    /// splats a cell that subtends less than the opening angle.
    #[test]
    fn the_glow_refines_near_and_splats_far() {
        let (index, _parent, kids) = small_tree(10, 10, 4.0);

        // Close in, the 16 ly parent subtends far more than half a degree and
        // refines to its leaves, which splat.
        let near = eye_out(CellId::of_point(HERE, 13), 2.0);
        let close = splat_ids(&index.needed(&near, real(), None));
        assert!(close.contains(&kids[0]));
        assert!(close.contains(&kids[1]));
        assert!(!close.contains(&CellId::ROOT));

        // From far enough that even the whole cube subtends under the angle, the
        // root itself splats.
        let far = eye_out(CellId::ROOT, 20_000_000.0);
        assert!(
            splat_ids(&index.needed(&far, real(), None))
                .contains(&CellId::ROOT)
        );
    }

    /// An empty index asks for nothing, in every mode.
    #[test]
    fn an_empty_index_needs_nothing() {
        let index = Index::default();
        let view = eye_out(CellId::ROOT, 1.0);
        for mode in [Mode::Shell, real()] {
            let needed = index.needed(&view, mode, None);
            assert!(needed.marks.is_empty());
            assert!(needed.blobs.is_empty());
            assert!(needed.splats.is_empty());
        }
    }

    /// Descending finds exactly the cells scanning finds.
    ///
    /// `each_near` is what a router asks per expansion, and a linear scan
    /// over every cell is the answer it must not differ from: a cell the
    /// descent prunes is a cell whose systems a route would never see, and a
    /// system missed is a jump the plan does not know it can make.
    #[test]
    fn descending_finds_what_scanning_finds() {
        let (index, parent, _) = small_tree(10, 10, 4.0);
        let center = parent.bounds().center();

        for radius in [0.0, 1.0, parent.edge_ly(), 1.0e5] {
            let mut walked = Vec::new();
            index.each_near(center, radius, |id| walked.push(id));
            walked.sort_unstable_by_key(|id| (id.level, id.morton()));
            let mut scanned: Vec<CellId> = index
                .cells()
                .filter(|cell| cell.id.bounds().distance_to(center) <= radius)
                .map(|cell| cell.id)
                .collect();
            scanned.sort_unstable_by_key(|id| (id.level, id.morton()));
            assert_eq!(walked, scanned, "at radius {radius}");
        }

        // And a centre outside the galaxy descends into nothing at all: the
        // root's own box fails the test, so no child is ever looked up.
        let mut none = 0;
        index.each_near([1.0e9, 1.0e9, 1.0e9], 1.0, |_| none += 1);
        assert_eq!(none, 0);
    }
}

/// The merge rule over hand-placed skies.
///
/// The failure this replaced passed a full battery of per-axis profile
/// measurements, because a profile averages a bright box and a dark box into
/// a reasonable number. So these are not profiles: each stands a shape up —
/// a pair, a line, a ring, a disc, a clump in a void — and checks the drawn
/// set against the systems themselves, both ways round.
///
/// - **Coverage**: every system is within one merge distance of something
///   drawn. Nothing is dropped from the picture.
/// - **Fidelity**: everything drawn is within one merge distance of some
///   system. The geometry is not changed — a ring does not fill in and a
///   line does not thicken.
///
/// Both together are the design's own statement: *reduce marks, do not
/// change the geometry*.
#[cfg(test)]
mod merging {
    use super::*;
    use crate::build::snapshot::{BuildParams, Snapshot};
    use crate::core::record::{StarKind, System};
    use crate::read::index::contents_center;

    /// Where the test skies are hung: the galactic centre, so the cells a
    /// build makes are the ones a real sky would land in.
    const HERE: [f64; 3] = [0.0, 900.0, 24400.0];

    /// A sky of hand-placed systems, built as the galaxy is.
    ///
    /// One system a cell and one a slice, so the tree is as deep as the
    /// positions allow and a cell's own slice is a small share of its
    /// subtree — which is the shape of the real tree, where a slice is 512
    /// against a subtree of millions. At the defaults a handful of systems
    /// is one leaf and there is no frontier to test.
    fn sky(at: &[[f64; 3]]) -> Snapshot {
        let systems: Vec<System> = at
            .iter()
            .enumerate()
            .map(|(n, position)| System {
                id64: n as u64 + 1,
                position: *position,
                absolute_magnitude: 4.0,
                temperature: 5000.0,
                age_bucket: 0,
                updated_at: 0,
                kind: StarKind::Unknown,
            })
            .collect();
        Snapshot::build(
            &systems,
            &BuildParams { internal_slice: 1, leaf_cap: 1 },
        )
    }

    /// An eye `out_ly` from `HERE`, looking at it down a 1280x720 frame.
    fn eye(out_ly: f64) -> View {
        View {
            eye: [HERE[0], HERE[1], HERE[2] - out_ly],
            forward: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
            fov_y: std::f32::consts::FRAC_PI_4,
            viewport_height: 720.0,
            aspect: 16.0 / 9.0,
        }
    }

    /// How far apart two points fall on screen, in pixels: the angle between
    /// them from the eye, times pixels per radian.
    fn gap(view: &View, a: [f64; 3], b: [f64; 3]) -> f64 {
        let to = |p: [f64; 3]| {
            let v =
                [p[0] - view.eye[0], p[1] - view.eye[1], p[2] - view.eye[2]];
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / len, v[1] / len, v[2] / len]
        };
        let (u, v) = (to(a), to(b));
        let dot = (u[0] * v[0] + u[1] * v[1] + u[2] * v[2]).clamp(-1.0, 1.0);
        dot.acos() * view.pixels_per_radian()
    }

    /// Where every mark the plan offers lands: a blob at its cell's own
    /// centroid, and a read cell's whole slice at the systems themselves.
    ///
    /// The whole slice, because how much of it a frame draws is the frame's
    /// to say and is a share of every *other* marked cell's — see
    /// [`Needed`]. What the frontier settles, and what these tests are
    /// about, is which cells are read at all and which stand in for
    /// themselves.
    fn drawn(sky: &Snapshot, needed: &Needed) -> Vec<[f64; 3]> {
        let mut at = Vec::new();
        for blob in &needed.blobs {
            let cell = sky.index.get(blob.id).expect("a blob names a cell");
            at.push(contents_center(cell));
        }
        for mark in &needed.marks {
            at.extend(sky.payload(mark.id).iter().map(|point| point.pos));
        }
        at
    }

    /// How many marks the plan offers: one a merged cell, and one a system
    /// of every read cell's slice.
    fn offered(sky: &Snapshot, needed: &Needed) -> usize {
        needed.blobs.len()
            + needed
                .marks
                .iter()
                .map(|mark| sky.payload(mark.id).len())
                .sum::<usize>()
    }

    /// The nearest drawn mark to a point, in pixels.
    fn nearest(view: &View, at: &[[f64; 3]], to: [f64; 3]) -> f64 {
        at.iter().map(|&p| gap(view, p, to)).fold(f64::MAX, f64::min)
    }

    /// Every system is drawn or stood for, and nothing is drawn where no
    /// system is: the whole of "reduce marks, do not change the geometry".
    ///
    /// The slack is the merge distance itself. A merged mark stands where
    /// its systems' centroid is, and they are all inside one mark of it, so
    /// neither direction can be out by more than that.
    fn holds_the_shape(view: &View, systems: &[[f64; 3]], at: &[[f64; 3]]) {
        for &system in systems {
            let off = nearest(view, at, system);
            assert!(
                off <= MERGE_PX,
                "a system {off:.1} px from anything drawn: nothing stands \
                 for it"
            );
        }
        for &mark in at {
            let off = nearest(view, systems, mark);
            assert!(
                off <= MERGE_PX,
                "a mark {off:.1} px from any system: the draw invented \
                 geometry"
            );
        }
    }

    /// Two systems merge into one mark when they close to within the merge
    /// distance and part into two when they open past it — crossed in both
    /// directions, off one sky and two distances.
    #[test]
    fn a_pair_merges_and_parts() {
        let apart = 2.0;
        let at = [
            [HERE[0] - apart / 2.0, HERE[1], HERE[2]],
            [HERE[0] + apart / 2.0, HERE[1], HERE[2]],
        ];
        let sky = sky(&at);

        // Close enough that the pair is well past the top of the band.
        let near = eye(apart * 869.0 / (MERGE_PX * MERGE_BAND * 4.0));
        let parted = sky.index.walk_screen(&near, None);
        assert_eq!(gap(&near, at[0], at[1]).round() as u64, 32);
        assert!(parted.blobs.is_empty(), "a resolved pair merged");
        assert_eq!(
            offered(&sky, &parted),
            2,
            "a resolved pair did not draw two marks"
        );
        holds_the_shape(&near, &at, &drawn(&sky, &parted));

        // And far enough that it is inside one mark.
        let far = eye(apart * 869.0 / (MERGE_PX * 0.25));
        let merged = sky.index.walk_screen(&far, None);
        assert!(gap(&far, at[0], at[1]) < MERGE_PX);
        assert!(merged.marks.is_empty(), "a merged pair read a payload");
        assert_eq!(merged.blobs.len(), 1, "a merged pair is not one mark");
        holds_the_shape(&far, &at, &drawn(&sky, &merged));
    }

    /// A line is drawn as a line: as many marks as it is merge distances
    /// long, laid along it, and never collapsed into one.
    ///
    /// **The scalar-radius bug, caught.** A cell's RMS radius is a third of
    /// the length of a filament it holds, so judging the merge on the radius
    /// merges three marks' worth of line into one and the feature leaves the
    /// map. The measure is a width rolled up from the children
    /// ([`widen`]), and this is what says so.
    #[test]
    fn a_line_stays_a_line() {
        let span = 40.0;
        let at: Vec<[f64; 3]> = (0..41)
            .map(|n| {
                let t = n as f64 / 40.0 - 0.5;
                [HERE[0] + t * span, HERE[1], HERE[2]]
            })
            .collect();
        let sky = sky(&at);

        // Far enough out that the line is 40 px long: ten marks' worth.
        let view = eye(span * 869.0 / 40.0);
        let length = gap(&view, at[0], at[at.len() - 1]);
        assert!((length - 40.0).abs() < 1.0, "the line is {length:.1} px");

        let needed = sky.index.walk_screen(&view, None);
        let marks = drawn(&sky, &needed);
        assert!(
            marks.len() >= (length / MERGE_PX) as usize,
            "a {length:.0} px line came out as {} marks, under the {} a \
             merge distance apart would give",
            marks.len(),
            (length / MERGE_PX) as usize
        );
        holds_the_shape(&view, &at, &marks);
    }

    /// A ring keeps its hole: nothing is drawn in the middle of it.
    #[test]
    fn a_ring_keeps_its_hole() {
        let radius = 30.0;
        let at: Vec<[f64; 3]> = (0..64)
            .map(|n| {
                let turn = n as f64 / 64.0 * std::f64::consts::TAU;
                [
                    HERE[0] + radius * turn.cos(),
                    HERE[1] + radius * turn.sin(),
                    HERE[2],
                ]
            })
            .collect();
        let sky = sky(&at);

        // The ring 60 px across, so the hole is 30 px of empty sky: seven
        // merge distances of it, far more than any slack in the rule.
        let view = eye(radius * 2.0 * 869.0 / 60.0);
        let needed = sky.index.walk_screen(&view, None);
        let marks = drawn(&sky, &needed);
        holds_the_shape(&view, &at, &marks);

        let middle = nearest(&view, &marks, HERE);
        assert!(
            middle > MERGE_PX * 2.0,
            "a mark {middle:.1} px from the centre of a ring 30 px across: \
             the hole filled in"
        );
    }

    /// A filled disc stays filled: its middle is drawn, not just its rim.
    #[test]
    fn a_disc_stays_filled() {
        let radius = 30.0;
        let mut at = Vec::new();
        for ring in 0..6 {
            let r = radius * ring as f64 / 5.0;
            let count = if ring == 0 { 1 } else { ring * 12 };
            for n in 0..count {
                let turn = n as f64 / count as f64 * std::f64::consts::TAU;
                at.push([
                    HERE[0] + r * turn.cos(),
                    HERE[1] + r * turn.sin(),
                    HERE[2],
                ]);
            }
        }
        let sky = sky(&at);

        let view = eye(radius * 2.0 * 869.0 / 60.0);
        let needed = sky.index.walk_screen(&view, None);
        let marks = drawn(&sky, &needed);
        holds_the_shape(&view, &at, &marks);

        let middle = nearest(&view, &marks, HERE);
        assert!(
            middle <= MERGE_PX,
            "nothing within {middle:.1} px of the middle of a filled disc"
        );
    }

    /// A clump inside a void stays a clump: the cell that holds it merges,
    /// nothing is ever drawn in the emptiness around it, and what is drawn
    /// over the clump is fewer marks than it holds systems.
    ///
    /// **Not one mark, and the gap is the tree's own.** The clump's cell
    /// merges into one blob, but every cell *above* it owns a slice of its
    /// subtree's brightest — and a subtree that is a clump in a void has all
    /// its brightest in the clump, wherever the owning cell's box reaches.
    /// Those are real systems at real places, drawn individually and
    /// selectable, which is what "every area shows its largest marks" asks
    /// for; what the tree cannot say is that they land inside a blob one
    /// mark wide, because a cell knows where its *subtree* sits and not
    /// where its own slice does. A per-cell occupancy grid is what would
    /// close it.
    #[test]
    fn a_clump_in_a_void_does_not_spill() {
        let mut at = Vec::new();
        for n in 0..64 {
            let turn = n as f64 / 64.0 * std::f64::consts::TAU;
            at.push([
                HERE[0] + 2.0 * turn.cos(),
                HERE[1] + 2.0 * turn.sin(),
                HERE[2] + (n % 5) as f64 * 0.5,
            ]);
        }
        // Four witnesses out in the void, 400 ly off, so the tree's cells
        // above the clump are enormous beside it.
        for corner in [[1.0, 1.0], [-1.0, 1.0], [1.0, -1.0], [-1.0, -1.0]] {
            at.push([
                HERE[0] + corner[0] * 400.0,
                HERE[1] + corner[1] * 400.0,
                HERE[2],
            ]);
        }
        let sky = sky(&at);

        // Far enough that the clump is inside one mark and the witnesses are
        // hundreds of pixels out from it.
        let view = eye(4.0 * 869.0 / (MERGE_PX * 0.5));
        let needed = sky.index.walk_screen(&view, None);
        let marks = drawn(&sky, &needed);
        // Nothing in the void: every mark is on a system.
        holds_the_shape(&view, &at, &marks);

        let over = |at: &[[f64; 3]]| {
            at.iter().filter(|&&p| gap(&view, p, HERE) <= MERGE_PX).count()
        };
        assert!(
            over(&marks) < over(&at),
            "a clump of {} systems inside one mark drew {} of them",
            over(&at),
            over(&marks)
        );
        let merged = needed.blobs.iter().any(|blob| {
            let cell = sky.index.get(blob.id).expect("a blob names a cell");
            gap(&view, contents_center(cell), HERE) <= MERGE_PX
        });
        assert!(merged, "nothing merged over the clump");
    }

    /// A lone system is drawn as itself however far off it is: its contents
    /// width is zero, so it merges at any distance, and merging one mark
    /// into one mark buys nothing while costing its name and its place.
    #[test]
    fn a_lone_system_is_never_merged() {
        let sky = sky(&[HERE]);
        for out in [1.0, 1.0e3, 1.0e6] {
            let needed = sky.index.walk_screen(&eye(out), None);
            assert_eq!(
                offered(&sky, &needed),
                1,
                "a lone system was not drawn from {out} ly out"
            );
            assert!(
                needed.blobs.is_empty(),
                "a lone system merged into a blob from {out} ly out"
            );
        }
    }
}
