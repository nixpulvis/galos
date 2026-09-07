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
use crate::walk::Needed;
use std::collections::{HashMap, HashSet};

/// One system as the payload carries it: its id, its exact position, the two
/// photometric bytes, and when it was last updated.
///
/// Position is three `f64` in light years, the system's own galactic
/// coordinates carried through unchanged, so a system is drawn exactly where
/// it sits however coarse the cell that owns it. The magnitude is the system's
/// combined absolute magnitude, which its flux and the ordering are read from,
/// and the temperature bucket is the blackbody tint, already binned so the
/// client needs no per-star join.
///
/// `updated_at` is Unix seconds, and the one field here that is not about
/// where a system is or what it looks like. It is what the Recency filter
/// asks: which systems have been heard from lately. A cell's aggregate
/// answers that at a distance, counting systems per age bucket, but a bucket
/// is a day at its finest and the filter's shortest span is a minute, so the
/// per-system answer has to come from here. Four bytes on a record of
/// thirty-five,
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
    ) -> Point {
        Point {
            id64,
            pos: position,
            magnitude: magnitude as f32,
            temp_bucket: temp_bucket(temperature) as u8,
            updated_at,
        }
    }
}

/// A cell whose payload has loaded, and the systems it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct ResidentCell {
    pub points: Box<[Point]>,
}

/// The payloads the client holds, keyed by cell.
///
/// The index of aggregates is always resident and lives beside this; what this
/// holds is the per-system payloads, which come and go as the view moves.
#[derive(Clone, Debug, Default)]
pub struct Resident {
    cells: HashMap<CellId, ResidentCell>,
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

    /// Every resident cell and its payload, for a draw that reads the whole set
    /// to decide how much of each to show.
    pub fn iter(&self) -> impl Iterator<Item = (CellId, &ResidentCell)> {
        self.cells.iter().map(|(&id, cell)| (id, cell))
    }

    /// What the loader fetches: the needed marks not yet resident.
    pub fn missing(&self, needed: &Needed) -> Vec<CellId> {
        needed.marks.iter().copied().filter(|&id| !self.contains(id)).collect()
    }

    /// What the evictor drops: resident payloads the walk no longer asks for.
    ///
    /// Only the marks want a payload (a splat draws from the index alone), so a
    /// held payload outside the needed marks is what the evictor takes. The
    /// margin the doc calls for is applied by widening the walk before this, so
    /// the set arithmetic stays plain.
    pub fn stale(&self, needed: &Needed) -> Vec<CellId> {
        let wanted: HashSet<CellId> = needed.marks.iter().copied().collect();
        self.cells.keys().copied().filter(|id| !wanted.contains(id)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::{Mode, SplatRef};

    fn point(id: u64) -> Point {
        Point {
            id64: id,
            pos: [1.0, 2.0, 3.0],
            magnitude: 4.0,
            temp_bucket: 2,
            updated_at: 1_757_260_000,
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

    /// The set arithmetic splits the marks cleanly: what is fetched is needed
    /// and absent, what is evicted is resident and unneeded, and what is left
    /// over — needed and resident — is what the draw reads off `iter`.
    #[test]
    fn the_fetch_and_evict_sets_split_the_marks() {
        let (a, b, c) = (at(4, 0), at(4, 1), at(4, 2));
        let mut cache = Resident::default();
        cache.insert(a, vec![point(1)]); // needed and resident
        cache.insert(c, vec![point(3)]); // resident but not needed
        // b is needed but absent.
        let needed =
            Needed { mode: Mode::Shell, marks: vec![a, b], splats: vec![] };

        assert_eq!(ids(cache.missing(&needed)), ids(vec![b]));
        assert_eq!(ids(cache.stale(&needed)), ids(vec![c]));
    }

    /// A splat cell wants no payload, so holding one for a cell that is only
    /// splatted counts as stale.
    #[test]
    fn a_splat_only_cell_is_not_kept_resident() {
        let s = at(2, 1);
        let mut cache = Resident::default();
        cache.insert(s, vec![point(1)]);
        let needed = Needed {
            mode: Mode::Real,
            marks: vec![],
            splats: vec![SplatRef { id: s, blend: 1.0 }],
        };
        assert!(cache.missing(&needed).is_empty());
        assert_eq!(ids(cache.stale(&needed)), ids(vec![s]));
    }
}
