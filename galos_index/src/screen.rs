//! How much of what the frontier answers actually gets drawn, and where on
//! screen it lands.
//!
//! The walk ([`crate::walk`]) says *which* cells the view needs; this says
//! *how many marks* each of them is worth. One copy, because there are two
//! readers that must agree to the mark: the client's own draw and the
//! offline renderer (`examples/frontier.rs`) that is the check on it. A
//! picture drawn with a second copy of this arithmetic checks the second
//! copy.
//!
//! **The frame's budget is the screen.** Marks on a grid of [`MERGE_PX`]
//! fill the viewport, so the frame carries area over pitch squared of them
//! — 57,600 on a 1280x720 frame — and [`share`] spreads that total over
//! the population in reach. Nothing in here is a per-cell cap: where the
//! sky is dense the marks land closer than the pitch and read as a brighter
//! patch, which is what a dense sky looks like.

use crate::cache::Quick;
use crate::geometry::CellId;
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasherDefault;
use crate::walk::{MERGE_PX, View};

/// How many marks the frame itself carries
///
/// A *total*, not a density: how those marks are spread over the frame is
/// [`share`]'s to say.
pub fn frame_marks(view: &View) -> f64 {
    let [width, height] = frame(view);
    width * height / (MERGE_PX * MERGE_PX)
}

/// The frame in whole pixels
///
/// Rounded, because the aspect is a ratio of two pixel counts held as a
/// `f32` and 1280 over 720 comes back as 1280.0000095: a frame a
/// ten-thousandth of a pixel wider than it is has one more column of tiles
/// than it has room for.
fn frame(view: &View) -> [f64; 2] {
    let height = f64::from(view.viewport_height);
    [(height * f64::from(view.aspect)).round(), height]
}

/// What share of the systems in reach the frame draws
///
/// **One figure over the whole frame, and it is a share of *population*.**
/// That is the whole of the density response: every cell draws the same
/// fraction of what it holds, so a region with ten times the systems draws
/// ten times the marks and the sky's own structure survives the thinning.
/// The drawn set is a uniform sample of the galaxy, biased within each cell
/// toward the brightest.
///
/// **Two other rules were tried and both are wrong.** A share of a cell's
/// *footprint area* — one mark to every patch of screen the cell covers —
/// answers the same number wherever it is pointed, so the galaxy comes out
/// at one uniform density and the arms, the core and the voids all read
/// alike; a floor under that same figure drew nothing at all in the finest
/// cells, and since the tree is finest where the sky is densest, that is a
/// hole exactly where the most systems are. Population is the only one of
/// the three that is monotone in density.
///
/// No clamp anywhere in it. A cell draws `share` of its own payload and can
/// never be asked for more than it holds, which is what the attempt that
/// drew hard-edged cubes got wrong: it clamped a coarse cell's ask up to
/// its whole payload while its neighbour served a few per cent.
pub fn share(population: u64, capacity: f64) -> f64 {
    if population == 0 {
        return 1.;
    }
    (capacity / population as f64).min(1.)
}

/// How many marks a cell of `held` systems draws at `share`, without a
/// rounding cliff
///
/// `share * held` is rarely a whole number, and rounding it down loses
/// every cell that wants less than one mark — which at a wide zoom is most
/// of them, and a whole sparse sky with them. Rounding up instead hands
/// every cell a mark it has not earned, and a wide view holds hundreds of
/// thousands of cells.
///
/// So the fraction is dithered against the cell's own address: a cell that
/// wants a third of a mark draws one in a third of the places rather than
/// nowhere or everywhere. Off the address and not off a clock, so the
/// answer is the same for the same cell at the same zoom and the set moves
/// by marks arriving and leaving rather than by flickering.
pub fn wanted(share: f64, held: usize, id: CellId) -> usize {
    ((share * held as f64 + dither(id)) as usize).min(held)
}

/// A cell's own place in `0..1`, for [`wanted`]'s rounding
///
/// SplitMix64's finalizer over the address, which is anything but uniform —
/// a cell id is a level and three grid coordinates, so its low bits are
/// position — and the top twenty-four bits of the mix, which is all that is
/// wanted of it.
pub fn dither(id: CellId) -> f64 {
    let mut z = id.morton().wrapping_add(u64::from(id.level));
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f64 / 16_777_216.
}

/// How wide a patch of screen is promised a mark, in pixels
///
/// **Not a mark's own width, and the difference is the whole of the
/// setting.** [`Empty`] lights a tile that would otherwise draw nothing, so
/// the tile is the grain at which "nothing" and "something" are told apart.
/// At a mark's own pitch the promise is a mark every sixteen square pixels,
/// which is dense enough to stand beside the plane's own marks and flatten
/// the difference between them. At thirty-two it is one mark to a thousand
/// square pixels: measured over `.index/full` from sixty thousand light
/// years out, the galactic plane draws 5.8 marks to the pixel, so a lit
/// tile is some six thousand times fainter than the sky it sits beside and
/// cannot be mistaken for it.
///
/// It also bounds the cost. One mark a tile over a 1280x720 frame is 900
/// marks, an sixtieth of [`frame_marks`], whatever the tree holds.
pub const TILE_PX: f64 = 32.0;

impl View {
    /// Where a position lands on screen, in pixels from the top left, or
    /// [`None`] where it is behind the eye.
    ///
    /// The map's own projection, read off the same [`View`] the walk is
    /// given, so what this says a mark's place is and what the renderer
    /// draws cannot come apart.
    pub fn project(&self, at: [f64; 3]) -> Option<[f64; 2]> {
        let forward = unit(self.forward);
        let right = unit(cross(forward, self.up));
        let up = cross(right, forward);
        let from = [
            at[0] - self.eye[0],
            at[1] - self.eye[1],
            at[2] - self.eye[2],
        ];
        let ahead = dot(from, forward);
        if ahead <= 0.0 {
            return None;
        }
        let focal = self.pixels_per_radian();
        let [width, height] = frame(self);
        Some([
            width / 2.0 + dot(from, right) / ahead * focal,
            height / 2.0 - dot(from, up) / ahead * focal,
        ])
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 { v } else { [v[0] / len, v[1] / len, v[2] / len] }
}

/// One mark to a mark's worth of screen, for a draw whose marks are not
/// thinned by the merge frontier
///
/// **The frontier merges on the whole sky, and one draw does not draw
/// the whole sky.** A cell merges when everything it holds falls inside
/// one mark, which is the right question for the marks taken out of a
/// payload — and the wrong one for the populated draw, where what is
/// drawn is the one system in forty-four anybody lives in. A cell wide
/// enough to stay split can hold ten thousand systems and fifty
/// colonies, and those fifty are drawn at whatever separation *they*
/// have, which around the bubble is none: measured over `.index/full`,
/// the inhabited sky within the reach runs to tens of thousands of
/// systems overlapping into a white sheet.
///
/// So the same rule is applied where the frontier cannot reach it: a
/// mark claims a tile [`MERGE_PX`] across, and the next one that would
/// land in it is not drawn. What stands alone is drawn whole — which is
/// the half of this the merge frontier was always for — and what would
/// pile up is one mark instead of forty.
///
/// **Whoever claims first keeps it**, so the order the caller offers in
/// is the order that survives: the populated draw offers busiest first,
/// and shallower cells before deeper, so the mark that holds a crowded
/// tile is the largest thing in it.
#[derive(Default)]
pub struct Crowded {
    taken: HashSet<u64, BuildHasherDefault<Quick>>,
    /// How many marks stand along each mark-wide line of sight.
    stacked: HashMap<u64, u8, BuildHasherDefault<Quick>>,
    /// The angle one mark subtends, in radians: what a mark's width is
    /// worth out at whatever distance a system stands.
    pitch: f64,
    eye: [f64; 3],
}

impl Crowded {
    /// A lattice as fine as a mark is wide, seen from where `view` stands.
    pub fn over(view: &View) -> Crowded {
        Crowded {
            taken: HashSet::default(),
            stacked: HashMap::default(),
            pitch: MERGE_PX / view.pixels_per_radian(),
            eye: view.eye,
        }
    }

    /// Whether a mark at `at` is the first to want that patch of *sky*.
    ///
    /// **In the galaxy, not on the frame.** Two things were wrong with
    /// ruling the screen. It turns with the camera, so its boundaries
    /// sweep the sky as the eye rotates and the drawn set churns for as
    /// long as you are turning. And it has no depth: two systems in line
    /// with one another merge however far apart they stand, so a volume
    /// of sky reads as a mosaic laid on a sphere.
    ///
    /// So the lattice is in world coordinates, and only its *spacing*
    /// comes from the view: a mark is `pitch` radians across, which out
    /// at a distance `d` is `d · pitch` of galaxy, so that is the size of
    /// a cell there. Two systems merge when they are within a mark of
    /// each other **as the galaxy has them** — which is the same
    /// question the merge frontier asks of a cell's contents, asked of
    /// two systems.
    ///
    /// Squared to the world's axes and rounded to a power of two, so
    /// neighbours at a similar distance rule the same lattice rather
    /// than each carrying one of its own. A shell's worth of sky shares
    /// a spacing and the spacing doubles every octave of distance.
    ///
    /// Turning the eye moves none of this. Travelling moves it slowly,
    /// through the distance alone, and must: a mark is only so wide, and
    /// what it covers of the galaxy depends on how far off that galaxy
    /// is.
    pub fn claim(&mut self, at: [f64; 3]) -> bool {
        let from = [
            at[0] - self.eye[0],
            at[1] - self.eye[1],
            at[2] - self.eye[2],
        ];
        let away =
            (from[0] * from[0] + from[1] * from[1] + from[2] * from[2]).sqrt();
        if !away.is_finite() || away <= 0.0 {
            // The eye is standing on it, and nothing merges with it.
            return true;
        }
        // One mark's worth of galaxy out there, to the octave.
        let spacing = (away * self.pitch).max(f64::MIN_POSITIVE);
        let octave = spacing.log2().round();
        let spacing = octave.exp2();
        let cell = |it: f64| (it / spacing).floor() as i64;
        let mixed = [
            octave as i64,
            cell(at[0]),
            cell(at[1]),
            cell(at[2]),
        ]
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |key, &part| {
            (key ^ part as u64).wrapping_mul(0x100_0000_01b3)
        });
        if !self.taken.insert(mixed) {
            return false;
        }

        // And the other half of it: how many marks are already stacked
        // along this line of sight.
        //
        // **A lattice in the galaxy keeps depth, and depth is what
        // stacks.** Two systems a thousand light years apart are two
        // marks however close together they land, which is right —
        // and through a bubble a thousand light years deep it is dozens
        // of them to a line of sight, all landing in the same few
        // pixels. Measured over `.index/full` with the reach at five
        // hundred light years, 66,114 marks of the 116,511 systems in
        // it, which is a sheet.
        //
        // So a line of sight carries [`STACKED`] marks and no more. The
        // direction is binned on a cube about the eye, its faces ruled
        // at the angle one mark subtends and squared to the *world's*
        // axes — so this turns with nothing either, and what it costs is
        // the same cube-map third toward a face's corners.
        let (face, major) = [0usize, 1, 2].iter().fold(
            (0usize, 0.0f64),
            |(face, major), &axis| match from[axis].abs() > major {
                true => (axis, from[axis].abs()),
                false => (face, major),
            },
        );
        let (u, v) = match face {
            0 => (from[1], from[2]),
            1 => (from[2], from[0]),
            _ => (from[0], from[1]),
        };
        let ruled = |it: f64| (it / major / self.pitch).floor() as i64 as u64;
        let ray = (face as u64) << 62
            | u64::from(from[face] > 0.0) << 61
            | (ruled(u) & 0x3fff_ffff) << 30
            | (ruled(v) & 0x3fff_ffff);
        let along = self.stacked.entry(ray).or_default();
        if *along >= STACKED {
            return false;
        }
        *along += 1;
        true
    }
}

/// How many marks one mark-wide line of sight carries
///
/// **One is a mosaic and none is a sheet.** At one, a volume of sky
/// collapses onto a sphere: whatever stands behind a mark is merged into
/// it however far behind it stands, and turning the eye re-tiles the
/// picture. With no cap at all the depth of a bubble stacks dozens of
/// marks into the same few pixels and the middle of it fills solid.
///
/// Three, measured over `.index/full` at the two reaches the picture was
/// judged at — a hundred light years, which reads well and must not
/// change, and five hundred, which was filling solid:
///
/// | stacked | 100 ly, of 8,113 | 500 ly, of 116,511 |
/// |---|---|---|
/// | 1 | 5,727 | 10,713 |
/// | 2 | 7,391 | 18,965 |
/// | **3** | **7,822** | **25,744** |
/// | 4 | 7,936 | 31,473 |
/// | none | 7,970 | 66,114 |
///
/// Three keeps 98% of the near view — which is the one that was already
/// right — and takes two thirds off the crowded one. One would cost the
/// near view a quarter of itself to save a further fifth of the far,
/// which is the wrong trade: the whole point of a lattice in the galaxy
/// is that a system standing alone is drawn.
const STACKED: u8 = 3;

/// One tile of the screen: what landed on it, and the best thing there is
/// to light it with if nothing did.
#[derive(Clone, Copy, Default)]
struct Tile {
    /// Marks the ordinary share already puts here.
    drew: u64,
    /// The largest subtree offered here, and which offer it was.
    stands_for: u64,
    offer: u32,
}

/// The tiles of the frame that draw nothing, and the merged mark each one
/// would light if it could
///
/// **A void and an empty patch of sky are different things, and the share
/// alone cannot tell them apart.** The thinning is there because marks
/// collide: the frame carries [`frame_marks`] of them and the sky in reach
/// holds millions, so a cell is cut to the fraction of the screen it can
/// have. But a cell standing alone off the galactic plane collides with
/// nothing — there is no second mark competing for its pixel — and cutting
/// it to a thousandth of a mark draws nothing at all where there is
/// something. Measured over `.index/full` from sixty thousand light years
/// out, a pixel of the plane has some twenty-three frontier cells stacked
/// behind it and one three thousand light years up has three hundredths of
/// one: an eight-hundred-fold difference in *screen* crowding, against a
/// three-and-a-half-fold difference in what any one cell holds. No rule
/// reading a cell's own contents can tell those apart; this reads the
/// screen.
///
/// **It can only ever light what was dark.** A tile the share already draws
/// into is left exactly as it was, so nothing visible is moved, rebalanced
/// or taken away, and the plane — where the share is what binds — is
/// untouched to the mark. What it changes is the band between nothing and
/// one mark a tile, and one mark to [`TILE_PX`] squared is the faintest
/// thing the frame can say.
///
/// **Either kind of cell.** A merged mark is drawn off its aggregate and
/// costs nothing but the mark; a cell above the frontier draws the head of
/// its own payload, which is the brightest thing it holds, and that
/// payload is in hand — every marked cell in reach is read to the client's
/// `READ_LEAST` whatever its share. Merged marks alone were tried and it
/// is not enough: the one inhabited system more than two thousand light
/// years off the galactic plane in `.index/full`, `HIP 58832`, sits in a
/// level 5 cell holding two systems that the walk answers as a *mark* and
/// not a blob, so blobs-only left it — the only thing up there — undrawn.
pub struct Empty {
    tiles: Vec<Tile>,
    across: usize,
    down: usize,
}

impl Empty {
    /// A grid over `view`, one entry to [`TILE_PX`] squared.
    pub fn over(view: &View) -> Empty {
        let [width, height] = frame(view);
        let across = (width / TILE_PX).ceil().max(1.0) as usize;
        let down = (height / TILE_PX).ceil().max(1.0) as usize;
        Empty { tiles: vec![Tile::default(); across * down], across, down }
    }

    /// Which tile a place on screen falls in, or [`None`] off the frame.
    fn tile(&self, view: &View, at: [f64; 3]) -> Option<usize> {
        let [x, y] = view.project(at)?;
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let (column, row) = ((x / TILE_PX) as usize, (y / TILE_PX) as usize);
        (column < self.across && row < self.down)
            .then_some(row * self.across + column)
    }
    /// What the ordinary share draws at `at`, which is what stops a tile
    /// being empty.
    ///
    /// Marks read out of a cell's payload say so here too, even though they
    /// are never promoted: a coarse cell drawing its brightest few over a
    /// patch of sky is a patch that is not dark, and lighting it again
    /// would be a mark nobody asked for.
    pub fn drew(&mut self, view: &View, at: [f64; 3], marks: usize) {
        if let Some(tile) = self.tile(view, at) {
            self.tiles[tile].drew += marks as u64;
        }
    }

    /// A merged mark that would draw nothing, offered as the thing to light
    /// its tile with.
    ///
    /// `offer` is the caller's own numbering of its blobs, handed back by
    /// [`Empty::lit`]. The largest subtree in a tile wins it: of the things
    /// standing in an empty patch of sky, the one worth saying is the one
    /// standing for the most.
    pub fn offered(
        &mut self,
        view: &View,
        at: [f64; 3],
        stands_for: u64,
        offer: u32,
    ) {
        let Some(tile) = self.tile(view, at) else { return };
        let tile = &mut self.tiles[tile];
        if tile.stands_for < stands_for.max(1) {
            tile.stands_for = stands_for.max(1);
            tile.offer = offer;
        }
    }

    /// The offers that draw: one per tile that nothing else reaches, in the
    /// caller's own numbering and in it.
    ///
    /// Sorted, so a caller walking its blobs in order walks this alongside
    /// them rather than looking each one up.
    pub fn lit(&self) -> Vec<u32> {
        let mut lit: Vec<u32> = self
            .tiles
            .iter()
            .filter(|tile| tile.drew == 0 && tile.stands_for > 0)
            .map(|tile| tile.offer)
            .collect();
        lit.sort_unstable();
        lit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::StarKind;
    use crate::tree::{BuildParams, Snapshot, System};

    /// A sky of hand-placed systems, built as the galaxy is: one system a
    /// cell and one a slice, so the tree is as deep as the positions allow.
    pub(super) fn sky(at: &[[f64; 3]]) -> Snapshot {
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

    /// A frame of the given size, seen through the map's own lens.
    fn framed(wide: f32, high: f32) -> View {
        View {
            eye: [0.; 3],
            forward: [0., 0., -1.],
            up: [0., 1., 0.],
            fov_y: 0.785,
            viewport_height: high,
            aspect: wide / high,
        }
    }

    /// The share is of population, so a region with more systems draws more
    /// marks — the density the sky has, not one the frame imposes
    ///
    /// **What this is guarding against is a flat answer.** A share of a
    /// cell's footprint *area* draws the same number of marks over the same
    /// patch of screen whatever is in it, so the core, the arms and the
    /// voids all come out at one density and the galaxy reads as a uniform
    /// ball. Ten times the systems must draw ten times the marks.
    #[test]
    fn the_share_follows_the_population() {
        let view = framed(1280., 720.);
        let capacity = frame_marks(&view);
        assert!(
            (capacity - 57_600.).abs() < 1.,
            "a 1280x720 frame carries {capacity} marks at {MERGE_PX} px"
        );

        // A tenth of the sky in reach can be drawn: a sparse cell of ten
        // draws one and a dense one of ten thousand draws a thousand.
        let tenth = share(capacity as u64 * 10, capacity);
        assert!((tenth - 0.1).abs() < 1e-9, "the share came out at {tenth}");
        let sparse = CellId::of_point([0., 0., -100.], 8);
        let dense = CellId::of_point([0., 0., -200.], 8);
        assert_eq!(wanted(tenth, 10_000, dense), 1_000);
        assert!(
            wanted(tenth, 10, sparse) <= 2,
            "a cell of ten drew {} at a tenth",
            wanted(tenth, 10, sparse)
        );

        // Nothing is ever asked for more than it holds, and a frame with
        // room for everything draws everything.
        let whole = share(100, capacity);
        assert_eq!(whole, 1., "a sky the frame can hold was thinned");
        assert_eq!(wanted(whole, 40, dense), 40, "a cell was over-asked");
    }

    /// A cell wanting less than one mark draws one in that fraction of the
    /// places rather than nowhere at all
    ///
    /// At a wide zoom the share runs to thousandths and nearly every cell
    /// wants a fraction of a mark. Rounding those down empties the sparse
    /// sky outright; rounding them up hands a mark to each of hundreds of
    /// thousands of cells. Dithered against the cell's own address, the
    /// count comes out right in aggregate and is the same answer for the
    /// same cell every frame.
    #[test]
    fn a_fraction_of_a_mark_is_dithered_over_the_cells() {
        let cells: Vec<CellId> = (0..20_000u32)
            .map(|n| CellId {
                level: 12,
                x: n % 40,
                y: (n / 40) % 25,
                z: n / 1_000,
            })
            .collect();
        for share in [0.02_f64, 0.25, 0.7] {
            let drew: usize =
                cells.iter().map(|&id| wanted(share, 1, id)).sum();
            let rate = drew as f64 / cells.len() as f64;
            assert!(
                (rate / share - 1.).abs() < 0.1,
                "a share of {share} drew {rate} of the cells"
            );
        }
        // And the same cell answers the same way twice.
        for &id in cells.iter().take(64) {
            assert_eq!(wanted(0.3, 7, id), wanted(0.3, 7, id));
        }
    }


    /// Marks merge by where they stand in the galaxy, not by where they
    /// land on the frame
    ///
    /// Two things a screen grid gets wrong, and this holds against
    /// both. It turns with the camera, so its boundaries sweep the sky
    /// and the drawn set churns while the eye rotates. And it has no
    /// depth: two systems in line with one another merge however far
    /// apart they stand, so a volume reads as a mosaic on a sphere.
    #[test]
    fn marks_merge_where_they_stand() {
        // A sky of a thousand, spread over a few degrees at a thousand
        // light years: close enough together that most of them collide.
        let sky: Vec<[f64; 3]> = (0..1_000)
            .map(|n| {
                let turn = f64::from(n) * 0.61;
                let out = 3. + f64::from(n % 37);
                [out * turn.cos(), out * turn.sin(), 1_000.]
            })
            .collect();

        let claimed = |view: &View, sky: &[[f64; 3]]| -> Vec<bool> {
            let mut crowded = Crowded::over(view);
            sky.iter().map(|&at| crowded.claim(at)).collect()
        };

        let looking = |up: [f64; 3], forward: [f64; 3]| View {
            eye: [0.0; 3],
            forward,
            up,
            fov_y: std::f32::consts::FRAC_PI_4,
            viewport_height: 720.0,
            aspect: 16.0 / 9.0,
        };

        let straight = claimed(&looking([0., 1., 0.], [0., 0., 1.]), &sky);
        assert!(
            straight.iter().filter(|it| **it).count() < sky.len(),
            "nothing collided, so nothing is being tested",
        );
        // Rolled, and then turned away: the same eye, pointed
        // differently, merges the same marks.
        assert_eq!(
            straight,
            claimed(&looking([1., 1., 0.], [0., 0., 1.]), &sky),
            "a roll changed which marks were merged",
        );
        assert_eq!(
            straight,
            claimed(&looking([0., 1., 0.], [0.3, 0.1, 1.]), &sky),
            "turning the eye changed which marks were merged",
        );

        // Depth tells two systems apart, however exactly one stands
        // behind the other: a mark is a mark's width of galaxy, and a
        // hundred light years is many marks at this distance.
        let inline = vec![[0., 0., 1_000.], [0., 0., 1_100.]];
        assert_eq!(
            claimed(&looking([0., 1., 0.], [0., 0., 1.]), &inline),
            vec![true, true],
            "one system was merged into another standing in front of it",
        );
        // And two a mark apart at that distance are one.
        let view = looking([0., 1., 0.], [0., 0., 1.]);
        let mark = 1_000. * MERGE_PX / view.pixels_per_radian();
        let touching = vec![[0., 0., 1_000.], [mark / 8., 0., 1_000.]];
        assert_eq!(
            claimed(&view, &touching),
            vec![true, false],
            "two marks within a mark of each other were both drawn",
        );

        // A line of sight carries a few marks and not a crowd: four
        // systems strung out behind one another give three marks, the
        // fourth being the one the sheet would have been made of.
        let strung: Vec<[f64; 3]> = (0..4)
            .map(|n| [0., 0., 1_000. + f64::from(n) * 100.])
            .collect();
        assert_eq!(
            claimed(&view, &strung),
            vec![true, true, true, false],
            "a line of sight carried {STACKED} marks or none",
        );

        // Travelling changes it, and must: what a mark covers of the
        // galaxy depends on how far off the galaxy is.
        let moved = View { eye: [0., 0., 900.], ..view };
        assert_ne!(straight, claimed(&moved, &sky), "travelling changed nothing");
    }

    /// A view a thousand light years back from the origin, looking at it.
    fn looking() -> View {
        View {
            eye: [0.0, 0.0, -1000.0],
            forward: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
            fov_y: std::f32::consts::FRAC_PI_4,
            viewport_height: 720.0,
            aspect: 1280.0 / 720.0,
        }
    }

    /// What the eye is pointed at lands in the middle of the frame, and
    /// what is behind it lands nowhere.
    #[test]
    fn the_projection_is_the_view_it_was_given() {
        let view = looking();
        let middle = view.project([0.0; 3]).expect("the origin is ahead");
        assert!((middle[0] - 640.0).abs() < 1e-9, "{middle:?}");
        assert!((middle[1] - 360.0).abs() < 1e-9, "{middle:?}");
        assert_eq!(view.project([0.0, 0.0, -2000.0]), None);
        let up = view.project([0.0, 100.0, 0.0]).expect("ahead");
        assert!(up[1] < middle[1], "up on screen is up: {up:?}");
    }

    /// A tile the share already draws into is left alone, and one it does
    /// not is lit by the largest thing standing in it.
    #[test]
    fn only_the_dark_tiles_light() {
        let view = looking();
        let mut empty = Empty::over(&view);
        // Two offers in the middle tile, which something already draws in.
        empty.drew(&view, [0.0; 3], 1);
        empty.offered(&view, [0.0; 3], 500, 7);
        // And two in a tile far off to one side, which nothing draws in.
        let aside = [200.0, 0.0, 0.0];
        empty.offered(&view, aside, 10, 1);
        empty.offered(&view, aside, 400, 2);
        assert_eq!(empty.lit(), vec![2], "the largest offer, and only it");
    }

    /// Nothing off the frame is ever lit: a tile has to be on screen to be
    /// dark.
    #[test]
    fn what_is_off_the_frame_lights_nothing() {
        let view = looking();
        let mut empty = Empty::over(&view);
        empty.offered(&view, [0.0, 0.0, -2000.0], 100, 1);
        empty.offered(&view, [900_000.0, 0.0, 0.0], 100, 2);
        assert!(empty.lit().is_empty());
    }

    /// At most one mark a tile, whatever is offered: the promise is bounded
    /// by the frame and not by the tree.
    #[test]
    fn the_frame_bounds_what_is_lit() {
        let view = looking();
        let mut empty = Empty::over(&view);
        let tiles = empty.tiles.len();
        assert_eq!(tiles, 40 * 23, "1280x720 at {TILE_PX} pixels");
        for offer in 0..10_000u32 {
            let across = f64::from(offer % 100) * 8.0 - 400.0;
            let down = f64::from(offer / 100) * 8.0 - 400.0;
            empty.offered(&view, [across, down, 0.0], 1, offer);
        }
        let lit = empty.lit();
        assert!(lit.len() <= tiles, "{} lit of {tiles} tiles", lit.len());
        assert!(lit.len() > 1, "something was lit: {}", lit.len());
        assert!(lit.windows(2).all(|two| two[0] < two[1]), "sorted, unique");
    }
}

/// What the rule comes to over a whole sky, which is the thing the client
/// and the offline renderer each assemble out of the pieces above.
#[cfg(test)]
mod drawing {
    use super::tests::sky;
    use super::*;
    use crate::walk::{Index, Mode, View};

    /// Where the test skies hang: the galactic centre, so the cells are the
    /// ones a real sky would land in.
    const HERE: [f64; 3] = [0.0, 900.0, 24400.0];

    /// An eye `out_ly` back from `HERE`, looking at it.
    fn eye(out_ly: f64) -> View {
        View {
            eye: [HERE[0], HERE[1], HERE[2] - out_ly],
            forward: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
            fov_y: std::f32::consts::FRAC_PI_4,
            viewport_height: 180.0,
            aspect: 16.0 / 9.0,
        }
    }

    /// A crowd at `HERE` and one small clump set off to one side of it,
    /// which is a galactic plane and something standing off the plane.
    fn plane_and_clump(off_ly: f64) -> Vec<[f64; 3]> {
        let mut at = Vec::new();
        for n in 0..20_000 {
            let turn = n as f64 / 40.0;
            let out = 100.0 + (n % 400) as f64 * 5.0;
            at.push([
                HERE[0] + out * turn.cos(),
                HERE[1] + (n % 7) as f64 - 3.0,
                HERE[2] + out * turn.sin(),
            ]);
        }
        for n in 0..6 {
            at.push([
                HERE[0] + n as f64,
                HERE[1] + off_ly,
                HERE[2] + (n % 3) as f64,
            ]);
        }
        at
    }

    /// What one frame draws: marks off the read cells, merged marks, and
    /// the merged marks lighting tiles the rest of the frame left dark.
    ///
    /// The client's own assembly, in the order it runs it.
    fn drawn(index: &Index, view: &View) -> (Vec<[f64; 3]>, Vec<[f64; 3]>) {
        let needed = index.needed(view, Mode::Shell, None);
        let population: u64 = needed
            .marks
            .iter()
            .map(|mark| u64::from(mark.slice))
            .chain(needed.blobs.iter().map(|blob| blob.count))
            .sum();
        let share = share(population, frame_marks(view));

        // One pass to settle which patches of sky the frame leaves dark,
        // over both kinds of cell, and then the draw.
        let mut lighting = Empty::over(view);
        for (offer, mark) in needed.marks.iter().enumerate() {
            match wanted(share, mark.slice as usize, mark.id) {
                0 => lighting.offered(
                    view,
                    mark.at,
                    u64::from(mark.slice),
                    offer as u32,
                ),
                take => lighting.drew(view, mark.at, take),
            }
        }
        for (offer, blob) in needed.blobs.iter().enumerate() {
            let offer = (needed.marks.len() + offer) as u32;
            match wanted(share * blob.blend, blob.count as usize, blob.id) {
                0 => lighting.offered(view, blob.at, blob.count, offer),
                _ => lighting.drew(view, blob.at, 1),
            }
        }
        let lit = lighting.lit();

        let mut marks = Vec::new();
        for mark in &needed.marks {
            for _ in 0..wanted(share, mark.slice as usize, mark.id) {
                marks.push(mark.at);
            }
        }
        for blob in &needed.blobs {
            if wanted(share * blob.blend, blob.count as usize, blob.id) > 0 {
                marks.push(blob.at);
            }
        }
        let lit = lit
            .into_iter()
            .map(|offer| match needed.marks.get(offer as usize) {
                Some(mark) => mark.at,
                None => needed.blobs[offer as usize - needed.marks.len()].at,
            })
            .collect();
        (marks, lit)
    }

    /// One system on its own draws a mark, whichever kind of cell the walk
    /// answers it as
    ///
    /// **This is the case merged marks alone did not cover.** A cell stays
    /// above the frontier by being wider than a mark, so the reasoning
    /// went that what stands in an empty patch of sky is always a blob. It
    /// is not: measured over `.index/full`, the one inhabited system more
    /// than two thousand light years off the galactic plane sits in a cell
    /// the walk answers as a mark, and lighting blobs alone left the only
    /// thing up there undrawn.
    #[test]
    fn one_system_alone_out_there_is_drawn() {
        let mut at = plane_and_clump(1200.0);
        at.truncate(20_000);
        at.push([HERE[0], HERE[1] + 1200.0, HERE[2]]);
        let built = sky(&at);
        let index = Index::from_cells(built.index.cells().cloned());
        let view = eye(9000.0);

        let (marks, lit) = drawn(&index, &view);
        let high = |at: &[f64; 3]| at[1] - HERE[1] > 600.0;
        assert_eq!(
            marks.iter().filter(|at| high(at)).count(),
            0,
            "the share alone should draw nothing out there",
        );
        assert!(lit.iter().any(high), "the one system up there went undrawn");
    }

    /// A clump standing on its own draws a mark, at a zoom where the share
    /// alone draws it nothing
    ///
    /// **The thinning is there because marks collide.** From far enough out
    /// the frame carries fewer marks than the sky holds systems, so every
    /// cell is cut to a fraction of one — and a clump sitting on its own
    /// off the plane is cut to nothing, though nothing else is competing
    /// for its pixel. What the frame draws there is the difference between
    /// an empty patch of sky and a patch with something in it.
    #[test]
    fn something_alone_out_there_is_drawn() {
        let built = sky(&plane_and_clump(1200.0));
        let index = Index::from_cells(built.index.cells().cloned());
        let view = eye(9000.0);

        let (marks, lit) = drawn(&index, &view);
        let high = |at: &[f64; 3]| at[1] - HERE[1] > 600.0;
        assert_eq!(
            marks.iter().filter(|at| high(at)).count(),
            0,
            "the share alone should draw nothing out there, or this test \
             is not testing anything",
        );
        assert!(
            lit.iter().any(high),
            "nothing was drawn where the clump stands: {} lit",
            lit.len(),
        );
    }

    /// Lighting the dark tiles leaves every mark the share drew exactly
    /// where it was
    ///
    /// The rule may only ever add to a tile nothing reached, so the plane —
    /// where the share is what binds — comes out to the mark as it did
    /// before, and what is added is bounded by the frame rather than by the
    /// tree: one to a tile.
    #[test]
    fn what_was_drawn_is_untouched() {
        let built = sky(&plane_and_clump(1200.0));
        let index = Index::from_cells(built.index.cells().cloned());
        let view = eye(9000.0);
        let (marks, lit) = drawn(&index, &view);

        let tiles = Empty::over(&view);
        assert!(!marks.is_empty(), "the plane drew nothing at all");
        assert!(
            lit.len() <= tiles.tiles.len(),
            "{} lit over {} tiles",
            lit.len(),
            tiles.tiles.len(),
        );
        // Nothing lit stands in a tile a mark already reached.
        let mut drew = Empty::over(&view);
        for at in &marks {
            drew.drew(&view, *at, 1);
        }
        for at in &lit {
            let tile = drew.tile(&view, *at).expect("lit means on screen");
            assert_eq!(
                drew.tiles[tile].drew, 0,
                "a tile that already drew was lit again",
            );
        }
    }
}
