//! What the client holds loaded, and the set arithmetic the three consumers do.
//!
//! The walk says what the view needs; the cache says what is here. Between
//! them fall the three consumers: drawing takes what is needed and resident,
//! loading fetches what is needed and absent, and eviction drops what is
//! resident and no longer needed. Each is one set operation
//! against a [`Needed`], and they are the whole of the client's fetch loop.
//!
//! A cell arrives faint and brightens in, rather than appearing, so an arriving
//! payload does not pop against a still sky. Each resident cell carries a
//! presence that ramps from zero to one over a couple of hundred milliseconds,
//! which is multiplied into flux; at the visibility floor a payload
//! lands at, that fade is close to physically honest.
//!
//! The residual a cell splats over its drawn slice, and the field it resolves
//! into, are the next step's work; this holds the payload, the ramp, and the
//! bookkeeping the loop turns on.

use crate::geometry::CellId;
use crate::aggregate::temp_bucket;
use crate::walk::Needed;
use std::collections::{HashMap, HashSet};

/// How long a payload takes to ramp fully in, in seconds.
pub const PRESENCE_RAMP: f32 = 0.2;

/// One system as the payload carries it: its id, its exact position, and the
/// two photometric bytes.
///
/// Position is three `f64` in light years, the system's own galactic
/// coordinates carried through unchanged, so a system is drawn exactly where
/// it sits however coarse the cell that owns it. The magnitude is the system's
/// combined absolute magnitude, which its flux and the ordering are read from,
/// and the temperature bucket is the blackbody tint, already binned so the
/// client needs no per-star join.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Point {
    pub id64: u64,
    pub pos: [f64; 3],
    pub magnitude: f32,
    pub temp_bucket: u8,
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
    ) -> Point {
        Point {
            id64,
            pos: position,
            magnitude: magnitude as f32,
            temp_bucket: temp_bucket(temperature) as u8,
        }
    }
}

/// A cell whose payload has loaded: its systems and how far it has ramped in.
#[derive(Clone, Debug, PartialEq)]
pub struct ResidentCell {
    pub points: Box<[Point]>,
    /// Zero to one, multiplied into flux so the cell fades in rather than pops.
    pub presence: f32,
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
    /// A cell's payload arrives, starting its presence ramp at zero.
    pub fn insert(&mut self, id: CellId, points: Vec<Point>) {
        self.cells.insert(
            id,
            ResidentCell { points: points.into_boxed_slice(), presence: 0.0 },
        );
    }

    /// The payload held for a cell, if any.
    pub fn get(&self, id: CellId) -> Option<&ResidentCell> {
        self.cells.get(&id)
    }

    /// Whether a cell's payload is held.
    pub fn contains(&self, id: CellId) -> bool {
        self.cells.contains_key(&id)
    }

    /// Drop a cell's payload, returning it if it was held.
    pub fn remove(&mut self, id: CellId) -> Option<ResidentCell> {
        self.cells.remove(&id)
    }

    /// How many payloads are held.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Advance every ramp by `dt` seconds, toward a full presence of one.
    pub fn advance(&mut self, dt: f32) {
        let step = dt / PRESENCE_RAMP;
        for cell in self.cells.values_mut() {
            cell.presence = (cell.presence + step).min(1.0);
        }
    }

    /// The needed marks whose payloads are resident, ready to draw.
    pub fn drawable(&self, needed: &Needed) -> Vec<CellId> {
        needed.marks.iter().copied().filter(|&id| self.contains(id)).collect()
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
        Point { id64: id, pos: [1.0, 2.0, 3.0], magnitude: 4.0, temp_bucket: 2 }
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
        assert_eq!(cache.get(id).unwrap().points.len(), 2);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.remove(id).unwrap().points.len(), 2);
        assert!(cache.is_empty());
    }

    /// A payload arrives at zero presence and ramps to full over the ramp time,
    /// then holds there.
    #[test]
    fn presence_ramps_in_and_clamps() {
        let mut cache = Resident::default();
        let id = at(3, 1);
        cache.insert(id, vec![point(1)]);
        assert_eq!(cache.get(id).unwrap().presence, 0.0);
        cache.advance(PRESENCE_RAMP / 2.0);
        assert!((cache.get(id).unwrap().presence - 0.5).abs() < 1e-6);
        cache.advance(PRESENCE_RAMP);
        assert_eq!(cache.get(id).unwrap().presence, 1.0);
    }

    /// The three consumers split the world cleanly: what is drawn is needed and
    /// resident, what is fetched is needed and absent, what is evicted is
    /// resident and unneeded.
    #[test]
    fn the_three_consumers_partition_the_marks() {
        let (a, b, c) = (at(4, 0), at(4, 1), at(4, 2));
        let mut cache = Resident::default();
        cache.insert(a, vec![point(1)]); // needed and resident
        cache.insert(c, vec![point(3)]); // resident but not needed
        // b is needed but absent.
        let needed = Needed { mode: Mode::Shell, marks: vec![a, b], splats: vec![] };

        assert_eq!(ids(cache.drawable(&needed)), ids(vec![a]));
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
        let needed =
            Needed { mode: Mode::Real, marks: vec![], splats: vec![SplatRef { id: s, blend: 1.0 }] };
        assert!(cache.missing(&needed).is_empty());
        assert_eq!(ids(cache.stale(&needed)), ids(vec![s]));
    }
}
