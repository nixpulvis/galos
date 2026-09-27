//! One node of the served tree, and the record its payload packs each of its
//! systems as.
//!
//! A [`Cell`] is a node: its address, the magnitude-ordered slice of systems
//! it owns, which children it has, and the totals over its whole subtree. A
//! [`CellSystem`] is one system in that slice, packed at the precision a
//! reader needs. Both are read-side and fixed: the tree a feed edits is
//! [`crate::Tree`], which publishes into these.

use crate::core::aggregate::{Aggregate, TempBucket};
use crate::core::codec::{Decode, Encode, FixedCodec, record};
use crate::core::geometry::CellId;
use crate::core::star::StarKind;
use crate::system::System;

/// One node of the tree: its address, the magnitude-ordered slice it owns, the
/// children it has, and the totals it stands for.
///
/// A node at level `L` owns ranks `[rank_lo, rank_hi)` of its subtree's
/// magnitude order, holding only what its ancestors did not, so drawing a
/// node with its loaded ancestors is exactly the union with no system twice.
/// The `aggregate` is the total over the whole subtree, not the slice; with
/// the slice absent it is drawn as it stands, and with the slice present the
/// residual is drawn instead.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Cell {
    /// Where the cell sits in the tree.
    pub id: CellId,
    /// The first rank of the subtree's magnitude order this cell owns.
    pub rank_lo: u64,
    /// One past the last rank this cell owns.
    pub rank_hi: u64,
    /// Which of the eight children exist, one bit each, in octant order.
    pub child_mask: u8,
    /// The totals over the whole subtree.
    pub aggregate: Aggregate,
}

impl Cell {
    /// How many systems this cell owns in its own slice, the width of its rank
    /// range.
    pub fn slice_len(&self) -> u64 {
        self.rank_hi - self.rank_lo
    }

    /// Whether the cell has a child in the given octant, `0..8`.
    pub fn has_child(&self, octant: u8) -> bool {
        self.child_mask & (1 << octant) != 0
    }

    /// Whether the cell is a leaf, with no children to refine into.
    pub fn is_leaf(&self) -> bool {
        self.child_mask == 0
    }

    /// The contents' spread in light years: the count-weighted RMS radius,
    /// floored at the mean spacing so a cell of one or a few systems still
    /// resolves as the camera closes rather than staying a zero-extent point
    /// forever.
    pub(crate) fn contents_extent(&self) -> f64 {
        let count = self.aggregate.count().max(1) as f64;
        let spacing = self.id.edge_ly() / count.cbrt();
        self.aggregate.count_extent().max(spacing)
    }

    /// Where the contents sit: the count-weighted centroid, or the box centre
    /// where the aggregate carries no weight of its own.
    pub(crate) fn contents_center(&self) -> [f64; 3] {
        self.aggregate
            .count_centroid()
            .unwrap_or_else(|| self.id.bounds().center())
    }

    /// How wide the contents are in light years, before the children are
    /// rolled into the answer: the RMS radius read as the span of an even
    /// spread.
    ///
    /// **A spread is not a width, and the merge rule needs the width.** A cell
    /// carries one scalar about how far its systems sit from their centroid,
    /// the RMS radius `r`. For a set spread evenly along anything — a box, a
    /// line, a sheet — the distance between its two furthest members is
    /// `2·sqrt(3)·r`: a uniform segment of length `L` has `r = L/sqrt(12)`,
    /// and a uniform cube of edge `e` has `r = e/2` against a diagonal of
    /// `e·sqrt(3)`. So the span is the radius times [`UNIFORM_SPAN`], and
    /// reading the radius itself as a width understates a cell by three and a
    /// half — which is a filament of three marks merged into one and gone from
    /// the picture.
    ///
    /// Unfloored, where [`Self::contents_extent`] floors at the mean spacing.
    /// The floor is the field's: a lone system must still splat as something.
    /// Here the honest answer for one system is zero width, and what keeps it
    /// drawn as itself rather than merged into a blob is
    /// `crate::read::walk::splitting`'s rule that a cell holding one system is
    /// never merged.
    ///
    /// This is the seed [`crate::tree::index`]'s `widen` rolls up. Where a
    /// cell has children their centroids say far more about its shape than
    /// this does, and the larger of the two answers stands.
    pub(crate) fn contents_width(&self) -> f64 {
        UNIFORM_SPAN * self.aggregate.count_extent()
    }
}

/// How many RMS radii across an evenly spread set is: `2·sqrt(3)`.
pub const UNIFORM_SPAN: f64 = 3.464_101_615_137_754_6;

record! {
    Cell {
        id: CellId,
        rank_lo: u64,
        rank_hi: u64,
        child_mask: u8,
        aggregate: Aggregate,
    }
}

/// One system as the index is built *into*: packed for a cell's payload, with
/// its id, its exact position, the two photometric fields at the precision a
/// reader needs, when it was last updated, and its arrival star's kind. What
/// the map draws and the router measures; made only by [`CellSystem::of`].
///
/// **The served half of two records of a system.** [`System`] is the whole
/// record the build works in — see its table for what this one drops and
/// why. Nothing turns a [`CellSystem`] back into a [`System`]: a resume point
/// keeps the [`System`]s themselves.
///
/// Position is three `f64` in light years, the system's own galactic
/// coordinates carried through unchanged, so a system is drawn exactly where
/// it sits however coarse the cell that owns it. The magnitude is the
/// system's combined absolute magnitude narrowed to `f32`, which its flux is
/// drawn from, and the temperature bucket is the blackbody tint, already
/// binned so a reader needs no per-star join. Neither is fit to build an
/// aggregate or an order from; that is [`System`]'s.
///
/// `updated_at` is Unix seconds, and the one field here that is not about
/// where a system is or what it looks like. It is what the Recency filter
/// asks: which systems have been heard from lately. A cell's aggregate
/// answers that at a distance, counting systems per age bucket, but a bucket
/// is a day at its finest and the filter's shortest span is a minute, so the
/// per-system answer has to come from here. Four bytes a system in the
/// payload, which is the one table a reader is served that
/// rewrites per system: the cell a report moves is a file of tens of
/// kilobytes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CellSystem {
    pub id64: u64,
    pub position: [f64; 3],
    pub magnitude: f32,
    pub temp_bucket: TempBucket,
    pub updated_at: u32,
    /// What kind of star a ship arrives at
    ///
    /// **Here because the router reads it per expansion.** Whether a ship
    /// can refuel and whether it can supercharge are both this one byte,
    /// and a route over the galaxy asks it of every system it reaches — so
    /// it rides beside the position, which that same loop has already
    /// faulted, rather than in a table of ninety-five million rows. See
    /// [`crate::core::star::StarKind`].
    pub kind: StarKind,
}

impl CellSystem {
    /// Pack a system for the payload: the one place precision is given up.
    ///
    /// The position and `updated_at` are carried through whole; the
    /// magnitude narrows to `f32`, the temperature to its bucket, and
    /// `age_bucket` is dropped, the aggregates having already counted it.
    pub fn of(system: &System) -> CellSystem {
        CellSystem {
            id64: system.id64,
            position: system.position,
            magnitude: system.absolute_magnitude as f32,
            temp_bucket: TempBucket::of(system.temperature),
            updated_at: system.updated_at,
            kind: system.kind,
        }
    }
}

record! {
    CellSystem {
        id64: u64,
        position: [f64; 3],
        magnitude: f32,
        temp_bucket: TempBucket,
        updated_at: u32,
        kind: StarKind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell reads its slice width and its children off the record.
    #[test]
    fn a_cell_reads_its_slice_and_children() {
        let cell = Cell {
            id: CellId::ROOT,
            rank_lo: 0,
            rank_hi: 512,
            child_mask: 0b0000_0101,
            aggregate: Aggregate::ZERO,
        };
        assert_eq!(cell.slice_len(), 512);
        assert!(cell.has_child(0));
        assert!(!cell.has_child(1));
        assert!(cell.has_child(2));
        assert!(!cell.is_leaf());
        assert!(Cell { child_mask: 0, ..cell }.is_leaf());
    }

    /// A cell's whole index record survives the round trip exactly, aggregate
    /// and all; the moments are `f64` and lose nothing.
    #[test]
    fn a_cell_record_round_trips() {
        let agg = Aggregate::of_system([1.0, 2.0, 3.0], 4.83, 5772.0, 2)
            .merge(Aggregate::of_system([5.0, 6.0, 7.0], -1.0, 12000.0, 5));
        let cell = Cell {
            id: CellId { level: 3, x: 5, y: 6, z: 7 },
            rank_lo: 512,
            rank_hi: 1024,
            child_mask: 0b1010_0001,
            aggregate: agg,
        };
        let mut buf = Vec::new();
        cell.encode(&mut buf);
        assert_eq!(buf.len(), Cell::LEN);
        let mut cur = &buf[..];
        assert_eq!(Cell::decode(&mut cur), Some(cell));
    }

    fn point(id: u64, mag: f32) -> CellSystem {
        CellSystem {
            id64: id,
            position: [10.5, -40000.25, 65535.0],
            magnitude: mag,
            temp_bucket: TempBucket::new(3),
            updated_at: 1_757_260_000,
            kind: StarKind::G,
        }
    }

    /// A system survives the round trip through its bytes exactly, every
    /// field, the magnitude included.
    #[test]
    fn a_point_round_trips() {
        for mag in [-6.0, -1.5, 0.0, 4.83, 4.831_234_5, 15.0] {
            let p = point(42, mag);
            let mut buf = Vec::new();
            p.encode(&mut buf);
            assert_eq!(buf.len(), CellSystem::LEN);
            let mut cur = &buf[..];
            let back = CellSystem::decode(&mut cur).unwrap();
            assert_eq!(back.id64, p.id64);
            assert_eq!(back.position, p.position);
            assert_eq!(back.temp_bucket, p.temp_bucket);
            assert_eq!(back.updated_at, p.updated_at);
            assert_eq!(back.magnitude, p.magnitude);
        }
    }
}
