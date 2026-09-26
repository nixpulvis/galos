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
//! **The packing is now the published file.** [`galos_index::names`] writes
//! those same arrays as the sections of a generation — `addr.bin`,
//! `span.bin`, `text.bin`, and a `byname.bin` the resident packing never
//! had, and no positions at all since a name became a function of an
//! address — so the map maps them rather than building them. Opening the
//! table is five `mmap` calls and nothing is resident: a lookup
//! by address faults the eight bytes a step it binary-searches and no name
//! it is not going to answer with, and resolving a route's endpoint by name
//! is a search of `byname.bin` rather than the 11.3 s scan of the galaxy
//! this module's blob had to do. What the map holds is
//! [`galos_index::Names`]; [`crate::map::index::Names`] is the wrapper that puts it
//! beside the reaches below.
//!
//! The reaches are still packed here, because they are still read whole.
//! They are published as one MessagePack table (`reaches.bin`) and not as a
//! mapped one, and they cover a fifth of the index rather than all of it.

use galos_index::meta::SystemReach;

/// How far each scanned system reaches, in metres, by address.
///
/// Sorted and packed for the reasons the names were: a fifth of the galaxy
/// has a reach on record, which is 76 M rows at 200 M systems, and a
/// `HashMap<i64, f32>` over them is about 2 GB against 912 MB here.
#[derive(Debug, Default)]
pub struct Reaches {
    addresses: Box<[i64]>,
    metres: Box<[f32]>,
}

impl Reaches {
    /// Pack the published table.
    pub fn of(mut rows: Vec<SystemReach>) -> Reaches {
        rows.sort_by_key(|it| it.address);
        rows.dedup_by_key(|it| it.address);
        let mut addresses = Vec::with_capacity(rows.len());
        let mut metres = Vec::with_capacity(rows.len());
        for row in &rows {
            addresses.push(row.address);
            metres.push(row.reach);
        }
        Reaches {
            addresses: addresses.into_boxed_slice(),
            metres: metres.into_boxed_slice(),
        }
    }

    /// How far `address` reaches, where anything in it has been scanned.
    pub fn get(&self, address: i64) -> Option<f32> {
        self.addresses.binary_search(&address).ok().map(|at| self.metres[at])
    }

    /// How many systems have a reach on record.
    pub fn len(&self) -> usize {
        self.addresses.len()
    }

    /// Whether none do.
    pub fn is_empty(&self) -> bool {
        self.addresses.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reach is found by address, and an absence is an absence
    ///
    /// Which is how the map tells "small" from "not on record": a system
    /// with nothing scanned has no row here and is drawn at the stand-in.
    #[test]
    fn a_reach_is_found_or_is_not_on_record() {
        let reaches = Reaches::of(vec![
            SystemReach { address: 9, reach: 400. },
            SystemReach { address: 3, reach: 100. },
        ]);

        assert_eq!(reaches.len(), 2);
        assert_eq!(reaches.get(3), Some(100.));
        assert_eq!(reaches.get(9), Some(400.));
        assert_eq!(reaches.get(5), None, "a system with nothing scanned");
    }
}
