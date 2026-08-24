//! The cube, the cells that tile it, and the coordinates that address them.
//!
//! One cube stands over the whole galaxy, and every cell is a power-of-two
//! subdivision of it. The cube is 131,072 ly on a side, `2^17`, the smallest
//! power of two that holds the galaxy's extent, centred where the galaxy sits
//! rather than on the origin, since the disc is thin in `y` and offset in `z`
//! and a cube on the origin would waste most of itself. Level zero is the cube;
//! each level halves the edge, so a cell at level `L` is `131072 / 2^L` ly
//! across, down to a 16 ly leaf at level 13 and finer where the bubble is
//! dense.
//!
//! A [`CellId`] is the address, `level` and integer `x, y, z` at that level.
//! Its [`morton`](CellId::morton) key interleaves the three coordinates so that
//! cells near in space are near in the key, which is the order the builder
//! sorts and the files are laid out in. A system's position inside its cell is
//! kept as three `u16`, cell-relative, since an absolute float this far from
//! the origin has an ulp of hundreds of AU while a `u16` in a 16 ly leaf
//! resolves about fifteen, smaller and in half the bytes.

use crate::serialization::{Decode, Encode, FixedCodec};

/// The edge of the root cube, in light years: `2^17`, the smallest power of two
/// that holds the galaxy's extent.
pub const ROOT_EDGE_LY: f64 = 131072.0;

/// Where the root cube is centred, in light years. The galaxy's disc is thin in
/// `y` and pushed out in `z`, so the cube is placed over it rather than on the
/// origin.
pub const ROOT_CENTER_LY: [f64; 3] = [0.0, 900.0, 24400.0];

/// The low corner of the root cube, from which every cell's origin is measured.
pub const ROOT_MIN_LY: [f64; 3] = [
    ROOT_CENTER_LY[0] - ROOT_EDGE_LY / 2.0,
    ROOT_CENTER_LY[1] - ROOT_EDGE_LY / 2.0,
    ROOT_CENTER_LY[2] - ROOT_EDGE_LY / 2.0,
];

/// The deepest level the Morton key can carry, being 21 bits per axis. Far
/// below it a cell is a fraction of a light year, so this is a ceiling on the
/// encoding rather than a limit anything reaches.
pub const MAX_LEVEL: u8 = 21;

/// How many positions a `u16` axis divides its cell into.
const QUANTA: f64 = 65536.0;

/// The edge of a cell at `level`, in light years.
pub const fn edge_ly(level: u8) -> f64 {
    ROOT_EDGE_LY / (1u64 << level) as f64
}

/// An axis-aligned box in light years, what a cell occupies and what the walks
/// measure the eye against.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb {
    /// The centre of the box.
    pub fn center(&self) -> [f64; 3] {
        [
            0.5 * (self.min[0] + self.max[0]),
            0.5 * (self.min[1] + self.max[1]),
            0.5 * (self.min[2] + self.max[2]),
        ]
    }

    /// Whether a point lies within the box, edges included.
    pub fn contains(&self, p: [f64; 3]) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }

    /// The distance from a point to the nearest point of the box, zero inside.
    ///
    /// This is the `d_min` the photometric walk needs: measured to the nearest
    /// corner, the visibility test can never drop a star the cell might hold.
    pub fn distance_to(&self, p: [f64; 3]) -> f64 {
        let mut sq = 0.0;
        for ((p, min), max) in p.iter().zip(&self.min).zip(&self.max) {
            let d = if p < min {
                min - p
            } else if p > max {
                p - max
            } else {
                0.0
            };
            sq += d * d;
        }
        sq.sqrt()
    }
}

/// The address of one cell: its level and its integer coordinates at that level.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CellId {
    pub level: u8,
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

impl CellId {
    /// The root cube, level zero.
    pub const ROOT: CellId = CellId { level: 0, x: 0, y: 0, z: 0 };

    /// The edge of this cell, in light years.
    pub fn edge_ly(&self) -> f64 {
        edge_ly(self.level)
    }

    /// The low corner of this cell, in light years.
    pub fn min_ly(&self) -> [f64; 3] {
        let edge = self.edge_ly();
        [
            ROOT_MIN_LY[0] + self.x as f64 * edge,
            ROOT_MIN_LY[1] + self.y as f64 * edge,
            ROOT_MIN_LY[2] + self.z as f64 * edge,
        ]
    }

    /// The box this cell occupies, in light years.
    pub fn bounds(&self) -> Aabb {
        let min = self.min_ly();
        let edge = self.edge_ly();
        Aabb { min, max: [min[0] + edge, min[1] + edge, min[2] + edge] }
    }

    /// The cell at `level` that a point falls in.
    ///
    /// A point outside the cube clamps to the nearest edge cell rather than
    /// wrapping or overflowing; nothing on record sits outside, but a caller
    /// should not have to prove it before asking.
    pub fn of_point(p: [f64; 3], level: u8) -> CellId {
        let edge = edge_ly(level);
        let last = ((1u64 << level) - 1) as f64;
        let idx = |v: f64, min: f64| {
            ((v - min) / edge).floor().clamp(0.0, last) as u32
        };
        CellId {
            level,
            x: idx(p[0], ROOT_MIN_LY[0]),
            y: idx(p[1], ROOT_MIN_LY[1]),
            z: idx(p[2], ROOT_MIN_LY[2]),
        }
    }

    /// The cell one level up that holds this one, or [`None`] at the root.
    pub fn parent(&self) -> Option<CellId> {
        (self.level > 0).then(|| CellId {
            level: self.level - 1,
            x: self.x >> 1,
            y: self.y >> 1,
            z: self.z >> 1,
        })
    }

    /// The eight cells one level down that tile this one.
    pub fn children(&self) -> [CellId; 8] {
        let mut kids = [CellId::ROOT; 8];
        for (octant, kid) in kids.iter_mut().enumerate() {
            let o = octant as u32;
            *kid = CellId {
                level: self.level + 1,
                x: (self.x << 1) | (o & 1),
                y: (self.y << 1) | ((o >> 1) & 1),
                z: (self.z << 1) | ((o >> 2) & 1),
            };
        }
        kids
    }

    /// Which of its parent's eight octants this cell fills, `0..8`, in the
    /// order [`children`](Self::children) lays them out: `x` in the low bit,
    /// then `y`, then `z`. Meaningless at the root.
    pub fn octant(&self) -> u8 {
        ((self.x & 1) | ((self.y & 1) << 1) | ((self.z & 1) << 2)) as u8
    }

    /// The child in a given octant, `0..8`.
    pub fn child(&self, octant: u8) -> CellId {
        self.children()[octant as usize]
    }

    /// The Morton key of this cell's coordinates, near in the key where the cell
    /// is near in space. Unique among cells at one level; carry `level` beside
    /// it to tell levels apart.
    pub fn morton(&self) -> u64 {
        morton_encode(self.x, self.y, self.z)
    }

    /// A cell's coordinates from a Morton key and a level.
    pub fn from_morton(level: u8, key: u64) -> CellId {
        let (x, y, z) = morton_decode(key);
        CellId { level, x, y, z }
    }

    /// A point's position within this cell as three cell-relative `u16`.
    ///
    /// Zero is the low corner and 65535 the far one, so the whole span of the
    /// cell is spent on the axis and the resolution follows the cell's own
    /// size. A point on or past the far edge clamps to the last quantum.
    pub fn quantize(&self, p: [f64; 3]) -> [u16; 3] {
        let min = self.min_ly();
        let edge = self.edge_ly();
        let q = |v: f64, lo: f64| {
            ((v - lo) / edge * QUANTA).floor().clamp(0.0, QUANTA - 1.0) as u16
        };
        [q(p[0], min[0]), q(p[1], min[1]), q(p[2], min[2])]
    }

    /// The light-year position a quantized triple stands for, taken at the
    /// centre of its quantum so the round-trip error is halved.
    pub fn dequantize(&self, q: [u16; 3]) -> [f64; 3] {
        let min = self.min_ly();
        let edge = self.edge_ly();
        let d = |qi: u16, lo: f64| lo + (qi as f64 + 0.5) / QUANTA * edge;
        [d(q[0], min[0]), d(q[1], min[1]), d(q[2], min[2])]
    }
}

impl Encode for CellId {
    fn encode(&self, out: &mut Vec<u8>) {
        self.level.encode(out);
        self.morton().encode(out);
    }
}

impl Decode for CellId {
    fn decode(cur: &mut &[u8]) -> Option<CellId> {
        let level = u8::decode(cur)?;
        Some(CellId::from_morton(level, u64::decode(cur)?))
    }
}

impl FixedCodec for CellId {
    const LEN: usize = u8::LEN + u64::LEN;
}

/// Spread the low 21 bits of `v` out to every third bit, the one-axis half of a
/// 3D Morton interleave.
fn split3(v: u32) -> u64 {
    let mut x = v as u64 & 0x1f_ffff;
    x = (x | x << 32) & 0x001f_0000_0000_ffff;
    x = (x | x << 16) & 0x001f_0000_ff00_00ff;
    x = (x | x << 8) & 0x100f_00f0_0f00_f00f;
    x = (x | x << 4) & 0x10c3_0c30_c30c_30c3;
    x = (x | x << 2) & 0x1249_2492_4924_9249;
    x
}

/// Gather every third bit of `m` back into the low 21 bits, the inverse of
/// [`split3`].
fn compact3(m: u64) -> u32 {
    let mut x = m & 0x1249_2492_4924_9249;
    x = (x | x >> 2) & 0x10c3_0c30_c30c_30c3;
    x = (x | x >> 4) & 0x100f_00f0_0f00_f00f;
    x = (x | x >> 8) & 0x001f_0000_ff00_00ff;
    x = (x | x >> 16) & 0x001f_0000_0000_ffff;
    x = (x | x >> 32) & 0x1f_ffff;
    x as u32
}

/// Interleave three coordinates into one Morton key, `x` in the low bit of each
/// triple, then `y`, then `z`.
pub fn morton_encode(x: u32, y: u32, z: u32) -> u64 {
    split3(x) | split3(y) << 1 | split3(z) << 2
}

/// Recover three coordinates from a Morton key, the inverse of
/// [`morton_encode`].
pub fn morton_decode(m: u64) -> (u32, u32, u32) {
    (compact3(m), compact3(m >> 1), compact3(m >> 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// The cube halves at every level: 131,072 ly at the root, 16 at level 13,
    /// one at level 17.
    #[test]
    fn each_level_halves_the_edge() {
        assert!(close(edge_ly(0), 131072.0));
        assert!(close(edge_ly(13), 16.0));
        assert!(close(edge_ly(17), 1.0));
    }

    /// The cube is placed over the galaxy, so its low corner is the centre less
    /// half an edge on every axis.
    #[test]
    fn the_root_sits_over_the_galaxy() {
        assert!(close(ROOT_MIN_LY[0], -65536.0));
        assert!(close(ROOT_MIN_LY[1], 900.0 - 65536.0));
        assert!(close(ROOT_MIN_LY[2], 24400.0 - 65536.0));
        // The root holds its own centre and Sol at the origin.
        assert!(CellId::ROOT.bounds().contains(ROOT_CENTER_LY));
        assert!(CellId::ROOT.bounds().contains([0.0, 0.0, 0.0]));
    }

    /// A point lands in a cell that contains it, at every level.
    #[test]
    fn a_point_lands_in_a_cell_that_holds_it() {
        for p in [
            [0.0, 0.0, 0.0],
            [1234.0, 5678.0, -9012.0],
            [-41974.0, 5319.0, 65630.0],
        ] {
            for level in [0, 5, 13, 17] {
                let cell = CellId::of_point(p, level);
                assert!(
                    cell.bounds().contains(p),
                    "level {level} cell {cell:?} does not hold {p:?}"
                );
            }
        }
    }

    /// A point outside the cube clamps to an edge cell instead of overflowing.
    #[test]
    fn a_point_outside_the_cube_clamps() {
        let cell = CellId::of_point([1e9, -1e9, 1e9], 13);
        let last = (1u32 << 13) - 1;
        assert_eq!((cell.x, cell.y, cell.z), (last, 0, last));
    }

    /// A cell is one of its parent's children, and its parent is one level up.
    #[test]
    fn parents_and_children_agree() {
        let cell = CellId::of_point([1234.0, 5678.0, -9012.0], 13);
        let parent = cell.parent().unwrap();
        assert_eq!(parent.level, 12);
        assert!(parent.children().contains(&cell));
        assert_eq!(CellId::ROOT.parent(), None);
    }

    /// The eight children tile the parent: distinct, one level down, and each
    /// held by the parent's box.
    #[test]
    fn children_tile_the_parent() {
        let parent = CellId { level: 4, x: 3, y: 5, z: 6 };
        let kids = parent.children();
        for kid in kids {
            assert_eq!(kid.level, 5);
            assert_eq!(kid.parent(), Some(parent));
        }
        for i in 0..8 {
            for j in (i + 1)..8 {
                assert_ne!(kids[i], kids[j]);
            }
        }
    }

    /// Encoding and decoding a Morton key are inverses.
    #[test]
    fn morton_round_trips() {
        for coords in [
            (0, 0, 0),
            (1, 2, 3),
            (0x1f_ffff, 0, 0),
            (0, 0x1f_ffff, 0),
            (12345, 54321, 9999),
        ] {
            let m = morton_encode(coords.0, coords.1, coords.2);
            assert_eq!(morton_decode(m), coords);
        }
    }

    /// Distinct coordinates give distinct keys, so the key can stand for the
    /// cell within a level.
    #[test]
    fn morton_separates_the_axes() {
        let x = morton_encode(1, 0, 0);
        let y = morton_encode(0, 1, 0);
        let z = morton_encode(0, 0, 1);
        assert_ne!(x, y);
        assert_ne!(y, z);
        assert_ne!(x, z);
    }

    /// SOL round trips
    #[test]
    fn quantization_round_trips_0() {
        let origin = [0.0, 0.0, 0.0];
        let cell = CellId::of_point(origin, 0);
        let result = cell.dequantize(cell.quantize(origin));
        assert_eq!(result, origin);
    }

    /// Quantizing a position and reading it back lands within one quantum of the
    /// cell's edge, which at a 16 ly leaf is about fifteen AU.
    #[test]
    fn quantization_round_trips_within_a_quantum() {
        let cell = CellId::of_point([1234.0, 5678.0, -9012.0], 13);
        let min = cell.min_ly();
        let edge = cell.edge_ly();
        let tolerance = edge / QUANTA;
        for frac in [0.0, 0.1, 0.5, 0.75, 0.999] {
            let p = [
                min[0] + frac * edge,
                min[1] + (1.0 - frac) * edge,
                min[2] + 0.25 * edge,
            ];
            let back = cell.dequantize(cell.quantize(p));
            for i in 0..3 {
                assert!((p[i] - back[i]).abs() <= tolerance);
            }
        }
    }

    /// A quantized triple survives the round trip through a position unchanged,
    /// including the extremes of the range.
    #[test]
    fn dequantize_then_quantize_is_stable() {
        let cell = CellId { level: 10, x: 200, y: 300, z: 400 };
        for q in
            [[0, 0, 0], [1, 2, 3], [65535, 0, 32768], [65535, 65535, 65535]]
        {
            assert_eq!(cell.quantize(cell.dequantize(q)), q);
        }
    }

    /// The box is zero distance from a point inside it and the straight-line
    /// distance from one outside.
    #[test]
    fn distance_is_zero_inside_and_true_outside() {
        let cell = CellId { level: 4, x: 3, y: 5, z: 6 };
        assert!(close(cell.bounds().distance_to(cell.bounds().center()), 0.0));
        let min = cell.min_ly();
        // Three-four-five out from the low corner, one axis at a time then two.
        assert!(close(
            cell.bounds().distance_to([min[0] - 3.0, min[1], min[2]]),
            3.0
        ));
        assert!(close(
            cell.bounds().distance_to([min[0] - 3.0, min[1] - 4.0, min[2]]),
            5.0
        ));
    }
}
