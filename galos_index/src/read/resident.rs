//! What a reader holds loaded, and the fetch set the loader asks of it.
//!
//! The walk says what the view needs; the cache says what is here. Between
//! them fall the three consumers: drawing takes what is needed and resident,
//! loading fetches what is needed and absent, and eviction drops what is
//! resident and no longer needed. Only the fetch is a set operation here,
//! [`Resident::missing`] against a [`Needed`]; the draw reads
//! [`Resident::iter`], and when a payload stops being needed is the reader's
//! policy and not this cache's.
//!
//! The residual a cell splats over its drawn slice, and the field it resolves
//! into, are the reader's work; this holds the payload and the bookkeeping
//! the loop turns on.

use crate::core::geometry::{CellId, CellMap};
use crate::read::walk::Needed;
use crate::tree::cell::CellSystem;

/// A cell whose payload has loaded, and the systems it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct ResidentCell {
    pub points: Box<[CellSystem]>,
}

/// The payloads a reader holds, keyed by cell.
///
/// The index of aggregates is always resident and lives beside this; what this
/// holds is the per-system payloads, which come and go as the view moves.
#[derive(Clone, Debug, Default)]
pub struct Resident {
    cells: CellMap<ResidentCell>,
}

impl Resident {
    /// A cell's payload arrives, replacing anything held for that cell.
    pub fn insert(&mut self, id: CellId, points: Vec<CellSystem>) {
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
    /// comes back for it: the map queues a point that way rather than
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
    use crate::core::star::StarKind;
    use crate::read::walk::{BlobRef, MarkRef, Mode, SplatRef};

    fn point(id: u64) -> CellSystem {
        CellSystem {
            id64: id,
            position: [1.0, 2.0, 3.0],
            magnitude: 4.0,
            temp_bucket: crate::core::aggregate::TempBucket::new(2),
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
    /// dropped again is the reader's policy and not this cache's: see
    /// `galos_map`'s `map::galaxy::walk::evict_payloads`.
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
                aged: [1; crate::core::aggregate::AGE_BUCKETS],
                m_min: None,
            }],
            splats: vec![SplatRef { id: s, blend: 1.0 }],
        };
        assert!(cache.missing(&needed).is_empty());
    }
}
