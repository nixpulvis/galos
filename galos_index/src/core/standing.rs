//! The order a cell's slice is taken in: where a system stands among the
//! systems it shares a subtree with.
//!
//! A cell owns the first of its subtree that its ancestors did not, so this
//! order decides which systems a coarse cell holds, and so what every view
//! drawing from coarse cells sees. It used to be brightness, which is the
//! realistic view's question and nobody else's: a coarse cell held the
//! brightest stars of the region below it, and every other view drew the
//! remnants a magnitude of thirteen put ahead of the brown dwarfs a
//! magnitude of eighteen put last, whatever the region was made of.
//!
//! So the order is none of the things a system is. It is a hash of the
//! address, which no view asks about: the first `n` of any subtree are a
//! uniform sample of it, and a coarse cell holds every kind of star, every
//! allegiance and every age in the proportion its region does. Each view
//! orders what a cell holds by its own question after that — star class by
//! its classes in proportion, the realistic view by brightness out of the
//! photometry sidecar — over a sample that is already even.
//!
//! Deterministic, so a system stands in the same place on every build and
//! every edit: the order is a function of the address alone, and the same
//! systems build the same tree however they arrive.

/// Where a system stands in its subtree's order, the lowest first
///
/// SplitMix64's finalizer over the address. Elite's addresses are anything
/// but uniform — a sector, a mass code and a boxel packed into bit fields —
/// so the address itself would sort by where a system is; the mix spreads
/// every bit of it over every bit of the key.
pub fn standing(id64: u64) -> u64 {
    let mut z = id64.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The whole key a slice is ordered by: the standing, ties by address, so two
/// systems never compare equal and the order is total.
pub fn key(id64: u64) -> (u64, u64) {
    (standing(id64), id64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key is a bijection's image, so no two addresses tie on it, and it
    /// does not follow the address: neighbouring addresses land far apart.
    #[test]
    fn neighbouring_addresses_stand_apart() {
        let keys: Vec<u64> = (0..1_000u64).map(standing).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "two addresses tied");
        let ascending = keys.windows(2).filter(|w| w[0] < w[1]).count();
        assert!(
            (400..600).contains(&ascending),
            "{ascending} of 999 neighbours in order: the key follows the address"
        );
    }

    /// The first of a set by standing are an even sample of it: a class that
    /// is a fifth of the set is about a fifth of its first tenth, whatever
    /// else the class has in common.
    #[test]
    fn the_first_are_an_even_sample() {
        // Every fifth address is one class, the rest another: the class is
        // as structured as an address field, which is the hard case.
        let mut ids: Vec<u64> = (0..50_000u64).collect();
        ids.sort_unstable_by_key(|&id| key(id));
        let first = &ids[..5_000];
        let rare = first.iter().filter(|&&id| id % 5 == 0).count();
        assert!((900..1_100).contains(&rare), "{rare} of the first 5,000");
    }
}
