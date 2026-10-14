//! How far each scanned system reaches, packed — and where the names went.
//!
//! This module used to be the names table. The map read the published
//! chunks — 8.70 GiB of MessagePack over 3,053 files — decoded them across
//! the task pool, and packed the rows into runs of four arrays: addresses
//! sorted for the binary search, positions, offsets, and every name's bytes
//! in one blob. Measured against `.index/full`'s 200,071,629 systems that
//! was 7.9 GB resident and 33 s before the window drew anything, which was
//! itself the answer to the 47 GB of `Vec<NameEntry>` and
//! `HashMap<i64, usize>` it replaced.
//!
//! **The packing is now the published file.** [`galos_index::codec::names`] writes
//! those same arrays as the sections of a generation — `addr.bin`,
//! `span.bin`, `text.bin`, and a `byname.bin` the resident packing never
//! had, and no positions at all since a name became a function of an
//! address — so the map maps them rather than building them. Opening the
//! table is five `mmap` calls and nothing is resident: a lookup
//! by address faults the eight bytes a step it binary-searches and no name
//! it is not going to answer with, and resolving a route's endpoint by name
//! is a search of `byname.bin` rather than the 11.3 s scan of the galaxy
//! this module's blob had to do. What the map holds is
//! [`galos_index::prelude::Names`]; [`crate::map::index::Names`] is the wrapper
//! that puts it beside the reaches below.
//!
//! The reaches are held here, a cell at a time. Each cell publishes its
//! systems' reaches in a file beside its payload, in the payload's order,
//! and the map reads them only for the cells near the camera, which are the
//! only ones whose systems are drawn large enough for a reach to show. See
//! [`crate::map::index::reach`].

use galos_index::prelude::{CellId, Stamp};
use rustc_hash::FxHashMap;
use std::sync::RwLock;

/// How far the systems of the cells near the camera reach, in metres, by
/// address
///
/// Filled a cell at a time ([`Self::put`]) as the map reads each cell's
/// reaches, and let go of a cell at a time as the map lets go of its
/// payload. Behind a lock rather than replaced whole: a fetch task holds the
/// [`crate::map::index::Names`] it names systems with, and a cell's reaches
/// landing is a few hundred rows, not a table.
///
/// A system absent here is drawn at the stand-in, whether nothing in it has
/// been scanned or its cell's reaches have not been read: up close the
/// second lasts the frames a read takes, and further out a reach changes
/// nothing that is drawn.
#[derive(Debug, Default)]
pub struct Reaches {
    held: RwLock<Held>,
}

/// What [`Reaches`] keeps behind its lock.
#[derive(Debug, Default)]
struct Held {
    /// Each cell read: the stamp it was read under, its column in its
    /// payload's order, and the addresses that column has been laid against.
    cells: FxHashMap<CellId, Cell>,
    /// Every reach laid against an address, over every cell held.
    by_address: FxHashMap<i64, f32>,
}

/// One cell's reaches as the map holds them.
#[derive(Debug, Default)]
struct Cell {
    stamp: Option<Stamp>,
    column: Vec<Option<f32>>,
    /// The addresses the column was laid against, which are the payload's
    /// first so many: as many of the payload as the map held when it was.
    laid: Vec<i64>,
}

impl Reaches {
    /// Rows a caller has in hand, held as if they were one cell's: what a
    /// test names its systems' reaches with, there being no directory to
    /// read them a cell at a time from.
    pub fn of(rows: Vec<galos_index::records::SystemReach>) -> Reaches {
        let reaches = Reaches::default();
        if !rows.is_empty() {
            let addresses: Vec<i64> = rows.iter().map(|it| it.address).collect();
            let column = rows.iter().map(|it| Some(it.reach)).collect();
            reaches.put(CellId::ROOT, None, column, &addresses);
        }
        reaches
    }

    /// How far `address` reaches, where its cell's reaches are held and
    /// anything in it has been scanned.
    pub fn get(&self, address: i64) -> Option<f32> {
        self.held.read().expect("the reaches lock").by_address.get(&address).copied()
    }

    /// How many systems have a reach held.
    pub fn len(&self) -> usize {
        self.held.read().expect("the reaches lock").by_address.len()
    }

    /// Whether none do.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `id`'s reaches have been read.
    pub fn holds(&self, id: CellId) -> bool {
        self.held.read().expect("the reaches lock").cells.contains_key(&id)
    }

    /// Every cell held, with the stamp it was read under, for a refresh to
    /// ask after.
    pub fn stamps(&self) -> Vec<(CellId, Option<Stamp>)> {
        let held = self.held.read().expect("the reaches lock");
        held.cells.iter().map(|(&id, cell)| (id, cell.stamp)).collect()
    }

    /// How many of `id`'s systems its reaches have been laid against, for a
    /// caller holding more of its payload than that.
    pub fn laid(&self, id: CellId) -> Option<usize> {
        let held = self.held.read().expect("the reaches lock");
        held.cells.get(&id).map(|cell| cell.laid.len())
    }

    /// Take `id`'s reaches, read under `stamp`, laid against `addresses` —
    /// the payload's first so many, as the map holds it — in place of any
    /// held for it. Answers the addresses whose reach this moved.
    pub fn put(
        &self,
        id: CellId,
        stamp: Option<Stamp>,
        column: Vec<Option<f32>>,
        addresses: &[i64],
    ) -> Vec<i64> {
        let mut held = self.held.write().expect("the reaches lock");
        let mut moved = Vec::new();
        let gone = held.cells.remove(&id).map(|cell| cell.laid).unwrap_or_default();
        for address in gone {
            if held.by_address.remove(&address).is_some() {
                moved.push(address);
            }
        }
        let laid: Vec<i64> =
            addresses.iter().copied().take(column.len()).collect();
        for (&address, metres) in laid.iter().zip(&column) {
            if let Some(metres) = *metres {
                held.by_address.insert(address, metres);
                moved.push(address);
            }
        }
        held.cells.insert(id, Cell { stamp, column, laid });
        moved.sort_unstable();
        moved.dedup();
        moved
    }

    /// Lay `id`'s reaches against more of its payload than they were, the
    /// map holding more of it now. Answers the addresses newly laid.
    pub fn lay(&self, id: CellId, addresses: &[i64]) -> Vec<i64> {
        let mut held = self.held.write().expect("the reaches lock");
        let Held { cells, by_address } = &mut *held;
        let Some(cell) = cells.get_mut(&id) else { return Vec::new() };
        let from = cell.laid.len();
        let mut moved = Vec::new();
        for (at, &address) in
            addresses.iter().enumerate().take(cell.column.len()).skip(from)
        {
            cell.laid.push(address);
            if let Some(metres) = cell.column[at] {
                by_address.insert(address, metres);
                moved.push(address);
            }
        }
        moved
    }

    /// Let go of `id`'s reaches, the map having let go of its payload.
    pub fn forget(&self, id: CellId) {
        let mut held = self.held.write().expect("the reaches lock");
        if let Some(cell) = held.cells.remove(&id) {
            for address in cell.laid {
                held.by_address.remove(&address);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: CellId = CellId { level: 4, x: 1, y: 2, z: 3 };

    /// A reach is found by address once its cell's are read, and an absence
    /// is an absence
    ///
    /// Which is how the map tells "small" from "not on record": a system
    /// with nothing scanned has no reach here and is drawn at the stand-in.
    #[test]
    fn a_reach_is_found_or_is_not_on_record() {
        let reaches = Reaches::default();
        assert_eq!(reaches.get(3), None, "nothing read yet");

        let moved =
            reaches.put(CELL, Some(1), vec![Some(100.), None, Some(400.)], &[3, 5, 9]);
        assert_eq!(moved, [3, 9]);
        assert_eq!(reaches.len(), 2);
        assert_eq!(reaches.get(3), Some(100.));
        assert_eq!(reaches.get(9), Some(400.));
        assert_eq!(reaches.get(5), None, "a system with nothing scanned");
    }

    /// Reaches laid against a prefix are laid against more of the payload
    /// as more of it is held, and a cell let go takes its reaches with it.
    #[test]
    fn a_cells_reaches_follow_what_is_held_of_it() {
        let reaches = Reaches::default();
        reaches.put(CELL, None, vec![Some(1.), Some(2.), Some(3.)], &[7]);
        assert_eq!((reaches.get(7), reaches.get(8)), (Some(1.), None));
        assert_eq!(reaches.laid(CELL), Some(1));

        assert_eq!(reaches.lay(CELL, &[7, 8, 9, 10]), [8, 9]);
        assert_eq!(reaches.get(9), Some(3.));
        assert_eq!(reaches.laid(CELL), Some(3));

        // Read again, one system's reach gone: it is gone here too.
        let moved = reaches.put(CELL, Some(2), vec![Some(1.), None, Some(3.)], &[7, 8, 9]);
        assert!(moved.contains(&8));
        assert_eq!(reaches.get(8), None);

        reaches.forget(CELL);
        assert!(reaches.is_empty());
        assert!(!reaches.holds(CELL));
    }
}
