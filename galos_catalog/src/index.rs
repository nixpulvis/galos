//! Catalog stars as index systems: the bridge that makes this a peer of the
//! bake.
//!
//! `galos_db/src/index.rs` reads Postgres and hands back
//! `Vec<galos_index::System>`. This does the same from a catalog file, and the
//! point of it is that everything downstream — the tree, the walks, the cells,
//! the client — cannot tell which one it got. A tree of real stars drawn by
//! the map is both a thing worth seeing and the sharpest test that
//! `galos_index` is not quietly shaped around Elite.
//!
//! Two fields have no catalog meaning and are given one here.
//!
//! - **`id64`** is Elite's system address, large and structured. A catalog's
//!   ids are small integers and would collide with nothing in practice, but
//!   "in practice" is not a guarantee, so they are tagged into a high range
//!   rather than passed through. Two trees can then be held side by side
//!   without either's ids being mistaken for the other's.
//! - **`age_bucket`** is how long ago a row was last written, and it feeds
//!   only the Recency colouring. A catalog has no such notion — Hipparcos was
//!   published once — so every star sits in bucket zero. That the index's
//!   input vocabulary carries a field of pure presentation is visible here
//!   precisely because a second dataset has nothing to say about it.

use crate::Star;
use galos_index::System;

/// The high bit that marks an id as a catalog's rather than a system address.
///
/// Elite's addresses are 64-bit but built from bounded boxel and body fields
/// and do not reach here, so a tagged catalog id cannot be mistaken for one.
pub const CATALOG_ID_TAG: u64 = 1 << 62;

impl Star {
    /// This star as the index's input record.
    ///
    /// The three fields that survive are the position, the absolute magnitude
    /// and the temperature — the name, the measured apparent magnitude, the
    /// colour index and the spectral type do not, which is what makes `System`
    /// the lossy projection and [`Star`] the fuller record.
    pub fn to_system(&self) -> System {
        System {
            id64: CATALOG_ID_TAG | self.id,
            position: self.position,
            absolute_magnitude: self.absolute_magnitude,
            temperature: self.temperature(),
            age_bucket: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyg;
    use galos_index::{BuildParams, Snapshot};

    fn bright() -> Vec<Star> {
        hyg::read(include_str!("../data/bright.csv").as_bytes())
            .expect("the fixture is a HYG catalog")
            .stars
    }

    /// A catalog id is tagged, so it can never be read as an Elite system
    /// address and two trees can be held side by side.
    #[test]
    fn a_catalog_id_is_tagged_out_of_elites_range() {
        let star = &bright()[0];
        assert!(star.to_system().id64 >= CATALOG_ID_TAG);
        assert!(star.id < CATALOG_ID_TAG);
    }

    /// The photometry crosses the bridge unchanged: what the tree orders by is
    /// the magnitude the catalog measured, not something recomputed.
    #[test]
    fn the_photometry_crosses_unchanged() {
        for star in bright() {
            let system = star.to_system();
            assert_eq!(system.absolute_magnitude, star.absolute_magnitude);
            assert_eq!(system.temperature, star.temperature());
            assert_eq!(system.position, star.position);
        }
    }

    /// **A tree of real stars.** The whole catalog builds through
    /// `galos_index` end to end, and every star lands in exactly one cell —
    /// which is the test that the index is not shaped around Elite, since
    /// nothing in this tree came from it.
    #[test]
    fn a_catalog_builds_a_tree() {
        let stars = bright();
        let systems: Vec<_> = stars.iter().map(Star::to_system).collect();
        let built = Snapshot::build(&systems, &BuildParams::default());

        let placed: usize =
            built.payloads.values().map(|points| points.len()).sum();
        assert_eq!(placed, systems.len());

        let root = built.index.root().expect("a built tree has a root");
        assert_eq!(root.aggregate.count(), systems.len() as u64);
    }
}
