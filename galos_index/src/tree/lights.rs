//! Every cell's light: the index half of the photometry sidecar.
//!
//! A [`Photometry`] a cell, keyed by its address, for the cells the
//! [`Index`] holds. It is served as its own file beside `index.bin` and read
//! only by what asks how bright the sky is — the realistic view's cut and the
//! status report — so a reader drawing anything else never loads it. The
//! payload half, each system's [`Lit`](crate::core::photometry::Lit), is a
//! file beside each cell's payload; see [`crate::codec::lights`].

use crate::core::geometry::{CellId, CellMap};
use crate::core::photometry::Photometry;
use crate::tree::index::Index;

/// Every cell's [`Photometry`], over the whole of its subtree.
///
/// A cell with none recorded answers [`Photometry::ZERO`]: no light, which
/// is what a cell holding nothing gives off.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lights {
    cells: CellMap<Photometry>,
}

impl Lights {
    /// The light of the subtree under `id`.
    pub fn get(&self, id: CellId) -> Photometry {
        self.cells.get(&id).copied().unwrap_or(Photometry::ZERO)
    }

    /// Record a cell's light, in place of whatever it had.
    pub fn insert(&mut self, id: CellId, photometry: Photometry) {
        self.cells.insert(id, photometry);
    }

    /// Forget a cell's light, the cell having gone.
    pub fn remove(&mut self, id: CellId) {
        self.cells.remove(&id);
    }

    /// How many cells have a light recorded.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether none has.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Every cell's light, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (CellId, &Photometry)> {
        self.cells.iter().map(|(&id, light)| (id, light))
    }

    /// The light of the whole galaxy: the root's.
    pub fn root(&self) -> Photometry {
        self.get(CellId::ROOT)
    }

    /// Only the cells `index` holds, so the sidecar names exactly the tree it
    /// stands beside.
    pub fn over(mut self, index: &Index) -> Lights {
        self.cells.retain(|id, _| index.get(*id).is_some());
        self
    }
}

impl FromIterator<(CellId, Photometry)> for Lights {
    fn from_iter<I: IntoIterator<Item = (CellId, Photometry)>>(
        iter: I,
    ) -> Lights {
        Lights { cells: iter.into_iter().collect() }
    }
}

impl Extend<(CellId, Photometry)> for Lights {
    fn extend<I: IntoIterator<Item = (CellId, Photometry)>>(
        &mut self,
        iter: I,
    ) {
        self.cells.extend(iter);
    }
}
