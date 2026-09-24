//! What the client holds loaded, and the set arithmetic the three consumers do.
//!
//! The walk says what the view needs; the cache says what is here. Between
//! them fall the three consumers: drawing takes what is needed and resident,
//! loading fetches what is needed and absent, and eviction drops what is
//! resident and no longer needed. Each is one set operation
//! against a [`Needed`], and they are the whole of the client's fetch loop.
//!
//! The residual a cell splats over its drawn slice, and the field it resolves
//! into, are the next step's work; this holds the payload and the bookkeeping
//! the loop turns on.

use crate::aggregate::temp_bucket;
use crate::geometry::CellId;
use crate::meta::StarKind;
use crate::walk::Needed;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// One system as the payload carries it: its id, its exact position, the
/// two photometric fields, and when it was last updated.
///
/// Position is three `f64` in light years, the system's own galactic
/// coordinates carried through unchanged, so a system is drawn exactly where
/// it sits however coarse the cell that owns it. The magnitude is the
/// system's combined absolute magnitude, carried at the `f32` the catalogue
/// holds it at, which its flux and the ordering are read from, and the
/// temperature bucket is the blackbody tint, already binned so the client
/// needs no per-star join.
///
/// `updated_at` is Unix seconds, and the one field here that is not about
/// where a system is or what it looks like. It is what the Recency filter
/// asks: which systems have been heard from lately. A cell's aggregate
/// answers that at a distance, counting systems per age bucket, but a bucket
/// is a day at its finest and the filter's shortest span is a minute, so the
/// per-system answer has to come from here. Four bytes on a record of
/// thirty-seven,
/// and the only table on the client's side of the wire that already rewrites
/// per system rather than per chunk: the cell a report moves is a file of tens
/// of kilobytes, where the names table's chunk is three megabytes and would go
/// dirty for every system reported.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Point {
    pub id64: u64,
    pub pos: [f64; 3],
    pub magnitude: f32,
    pub temp_bucket: u8,
    pub updated_at: u32,
    /// What kind of star a ship arrives at
    ///
    /// **Here because the router reads it per expansion.** Whether a ship
    /// can refuel and whether it can supercharge are both this one byte,
    /// and a route over the galaxy asks it of every system it reaches — so
    /// it rides beside the position, which that same loop has already
    /// faulted, rather than in a table of ninety-five million rows. See
    /// [`crate::meta::StarKind`].
    pub kind: StarKind,
}

impl Point {
    /// A system packed into a payload point, its position carried through at
    /// full precision. The one place a record becomes a point, so the
    /// temperature bucketing lives here rather than at each caller that emits a
    /// payload.
    pub fn new(
        id64: u64,
        position: [f64; 3],
        magnitude: f64,
        temperature: f64,
        updated_at: u32,
        kind: StarKind,
    ) -> Point {
        Point {
            id64,
            pos: position,
            magnitude: magnitude as f32,
            temp_bucket: temp_bucket(temperature) as u8,
            updated_at,
            kind,
        }
    }
}

/// A cell whose payload has loaded, and the systems it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct ResidentCell {
    pub points: Box<[Point]>,
}

/// A multiply-shift hash over an address, for the one map a frame asks
/// tens of thousands of times
///
/// **SipHash is most of what a lookup costs here.** An address is a level
/// and three grid coordinates, thirteen bytes of integer that are already
/// well spread, and the draw asks this map once per marked cell per pass:
/// measured over `.index/full` at a wide zoom, 77,773 of them a frame.
/// Measured in `tests/zooming.rs` over the same addresses, SipHash is
/// 15 ns against 3 for a multiply-shift, which at that count is four
/// milliseconds a frame against under one.
///
/// Fine to be weak. Nothing adversarial reaches this — the keys are the
/// map's own tree addresses — and the mix below is the same one the
/// aggregates' own moments are dithered with.
#[derive(Default)]
pub(crate) struct Quick(u64);

impl Hasher for Quick {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u8(byte);
        }
    }

    fn write_u8(&mut self, byte: u8) {
        self.write_u64(u64::from(byte));
    }

    fn write_u32(&mut self, word: u32) {
        self.write_u64(u64::from(word));
    }

    fn write_u64(&mut self, word: u64) {
        let mixed = (self.0 ^ word).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 = mixed.rotate_left(29);
    }

    fn finish(&self) -> u64 {
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// The payloads the client holds, keyed by cell.
///
/// The index of aggregates is always resident and lives beside this; what this
/// holds is the per-system payloads, which come and go as the view moves.
#[derive(Clone, Debug, Default)]
pub struct Resident {
    cells: HashMap<CellId, ResidentCell, BuildHasherDefault<Quick>>,
}

impl Resident {
    /// A cell's payload arrives, replacing anything held for that cell.
    pub fn insert(&mut self, id: CellId, points: Vec<Point>) {
        self.cells
            .insert(id, ResidentCell { points: points.into_boxed_slice() });
    }

    /// Whether a cell's payload is held.
    pub fn contains(&self, id: CellId) -> bool {
        self.cells.contains_key(&id)
    }

    /// Drop a cell's payload, returning it if it was held.
    pub fn remove(&mut self, id: CellId) -> Option<ResidentCell> {
        self.cells.remove(&id)
    }

    /// One cell's payload, where it is held
    ///
    /// For a reader that has noted *which point of which cell* it wants and
    /// comes back for it: the client queues a point that way rather than
    /// building a system out of it, most of what it queues never being
    /// drawn.
    pub fn cell(&self, id: CellId) -> Option<&ResidentCell> {
        self.cells.get(&id)
    }

    /// Every resident cell and its payload, for a draw that reads the whole set
    /// to decide how much of each to show.
    pub fn iter(&self) -> impl Iterator<Item = (CellId, &ResidentCell)> {
        self.cells.iter().map(|(&id, cell)| (id, cell))
    }

    /// How many cells are held, for a caller that says so in a diagnostic.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// What the loader fetches: the needed marks not yet resident.
    pub fn missing(&self, needed: &Needed) -> Vec<CellId> {
        needed
            .marks
            .iter()
            .map(|mark| mark.id)
            .filter(|&id| !self.contains(id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::{BlobRef, MarkRef, Mode, SplatRef};

    fn point(id: u64) -> Point {
        Point {
            id64: id,
            pos: [1.0, 2.0, 3.0],
            magnitude: 4.0,
            temp_bucket: 2,
            updated_at: 1_757_260_000,
            kind: StarKind::G,
        }
    }

    fn ids(mut v: Vec<CellId>) -> Vec<CellId> {
        v.sort_by_key(|c| (c.level, c.x, c.y, c.z));
        v
    }

    fn at(level: u8, x: u32) -> CellId {
        CellId { level, x, y: 0, z: 0 }
    }

    /// A payload goes in, reads back, and comes out.
    #[test]
    fn payloads_come_and_go() {
        let mut cache = Resident::default();
        let id = at(3, 1);
        assert!(!cache.contains(id));
        cache.insert(id, vec![point(1), point(2)]);
        assert!(cache.contains(id));
        let (held, cell) = cache.iter().next().unwrap();
        assert_eq!(held, id);
        assert_eq!(cell.points.len(), 2);
        assert_eq!(cache.iter().count(), 1);
        assert_eq!(cache.remove(id).unwrap().points.len(), 2);
        assert!(!cache.contains(id));
        assert_eq!(cache.iter().count(), 0);
    }

    /// What is fetched is needed and absent, and what is needed and resident
    /// is what the draw reads off `iter`. Which of the resident cells are
    /// dropped again is the client's policy and not this cache's: see
    /// `galos_map`'s `systems::bounded::evict_payloads`.
    #[test]
    fn the_fetch_set_is_what_is_needed_and_absent() {
        let (a, b, c) = (at(4, 0), at(4, 1), at(4, 2));
        let mut cache = Resident::default();
        cache.insert(a, vec![point(1)]); // needed and resident
        cache.insert(c, vec![point(3)]); // resident but not needed
        // b is needed but absent.
        let needed = Needed {
            mode: Mode::Shell,
            marks: vec![
                MarkRef { id: a, slice: 1, at: [0.; 3] },
                MarkRef { id: b, slice: 1, at: [0.; 3] },
            ],
            blobs: vec![],
            splats: vec![],
        };

        assert_eq!(ids(cache.missing(&needed)), ids(vec![b]));
    }

    /// A cell the field or a merged mark draws wants no payload, so one is
    /// never fetched for it: a cell's aggregate stands for its whole
    /// subtree.
    #[test]
    fn an_aggregate_only_cell_is_never_fetched() {
        let s = at(2, 1);
        let mut cache = Resident::default();
        cache.insert(s, vec![point(1)]);
        let needed = Needed {
            mode: Mode::Real { limit: 8.0 },
            marks: vec![],
            blobs: vec![BlobRef {
                id: s,
                count: 1,
                blend: 1.0,
                at: [0.; 3],
                aged: [1; crate::aggregate::AGE_BUCKETS],
                m_min: None,
            }],
            splats: vec![SplatRef { id: s, blend: 1.0 }],
        };
        assert!(cache.missing(&needed).is_empty());
    }
}
