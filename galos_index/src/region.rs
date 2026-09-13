//! Raising the tree a region at a time, so a galaxy is never held whole.
//!
//! [`Snapshot::build`] wants every system at once, and what does not fit is
//! the build rather than the input. So the galaxy is cut into regions, each
//! built alone, and the pieces joined. See `TODO-scale.md`.
//!
//! A region is a cell and the systems inside it, not a level: one cell at
//! level 2 can hold half the galaxy. A cut is any set of cells, at any
//! levels, where no cell is inside another and every system is inside one.
//! The caller picks them by counting, which [`crate::cold`] does from a
//! cheap pass over positions.
//!
//! ## Why a region can be built alone
//!
//! A cell owns the brightest systems of its subtree that its ancestors have
//! not already claimed. Inside a region that rule is local. Across regions
//! it is not: the cells *above* the cut — the **crown** — are shared, and
//! the brightest systems anywhere may be owned up there.
//!
//! The coupling is bounded. A region's crown cells are its own ancestors,
//! `region.level` of them, each owning at most
//! [`BuildParams::internal_slice`], so a region can lose only its brightest
//! `region.level × internal_slice` systems, and it is enough to
//! [offer](Offer::of) exactly those. Nothing fainter can be claimed: a
//! candidate that finds no room proves the crown was full, and the crown
//! never empties, so everything fainter falls to the region.
//!
//! ## The shape of a build
//!
//! 1. **Offer.** Each region streams its systems once, keeping its
//!    brightest few and its total in bounded memory.
//! 2. **Crown.** [`Crown::over`] places those candidates in the cells above
//!    the cut, as a whole build would, and answers which it claimed.
//! 3. **Build.** Each region is a [`Snapshot::of_region`] over its own
//!    systems minus what the crown took; its payloads are written and only
//!    its cells are kept.
//! 4. **Join.** The crown's cells and every region's are one [`Index`],
//!    which is the index file.
//!
//! The result is the tree [`Snapshot::build`] would have built, cell for
//! cell and payload for payload — what
//! `a_regional_build_is_the_whole_build` holds it to.
//!
//! [`crate::cold`] is that sequence, run over a source that streams.

use crate::aggregate::{Aggregate, Cell};
use crate::cache::Point;
use crate::geometry::CellId;
use crate::tree::{BuildParams, Snapshot, System};
use crate::walk::Index;
use std::collections::{HashMap, HashSet};

/// Which cells a galaxy is built a region at a time from.
///
/// A set of cells where no cell is inside another and every system is
/// inside one. Not any such set: **a region's ancestors must each hold more
/// than [`BuildParams::leaf_cap`] systems.** A whole build stops splitting
/// where a cell holds no more than a leaf's worth, so a cut taken below
/// such a cell asks for cells the whole build would never have raised, and
/// the two builds stop agreeing.
///
/// [`Cut::of`] cannot produce one: it only ever divides a cell holding more
/// than the budget, and the budget is at least a leaf's worth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cut {
    /// The regions, coarsest first.
    regions: Vec<CellId>,
    /// The same, for asking whether a cell is one.
    held: HashSet<CellId>,
    /// The deepest region, which bounds the walk in [`Cut::region_of`].
    deepest: u8,
}

impl Cut {
    /// Divide the galaxy until no region holds more than `budget` systems.
    ///
    /// `counted` is how many systems fall in each cell of some fine grid —
    /// a level a few deeper than the cut will land, cheap to gather in one
    /// streaming pass over positions. Everything coarser is summed from it,
    /// and the descent from the root emits a cell as soon as it is small
    /// enough to build alone.
    ///
    /// The budget is systems, not bytes, because that is what the caller
    /// can count before it has read anything.
    ///
    /// A grid cell over budget on its own is emitted anyway: the counts
    /// cannot say how to divide it further, and a region too large is a
    /// build that needs more memory rather than a build that is wrong.
    pub fn of(
        counted: impl IntoIterator<Item = (CellId, u64)>,
        budget: u64,
        params: &BuildParams,
    ) -> Cut {
        let budget = budget.max(params.leaf_cap as u64);
        let mut total: HashMap<CellId, u64> = HashMap::new();
        let mut deepest = 0;
        for (cell, count) in counted {
            deepest = deepest.max(cell.level);
            let mut at = cell;
            *total.entry(at).or_insert(0) += count;
            while let Some(parent) = at.parent() {
                *total.entry(parent).or_insert(0) += count;
                at = parent;
            }
        }

        let mut cut = Vec::new();
        let mut frontier = vec![CellId::ROOT];
        while let Some(cell) = frontier.pop() {
            let count = total.get(&cell).copied().unwrap_or(0);
            if count == 0 {
                continue;
            }
            if count <= budget || cell.level >= deepest {
                cut.push(cell);
                continue;
            }
            for octant in 0..8 {
                frontier.push(cell.child(octant));
            }
        }
        Cut::over(cut)
    }

    /// A cut over cells a caller chose some other way.
    ///
    /// The caller owes what [`Cut::of`] proves: the cells are disjoint,
    /// every system is inside one, and each one's ancestors hold more than
    /// [`BuildParams::leaf_cap`] systems.
    pub fn over(regions: impl IntoIterator<Item = CellId>) -> Cut {
        let mut regions: Vec<CellId> = regions.into_iter().collect();
        regions.sort_by_key(|cell| (cell.level, cell.morton()));
        regions.dedup();
        let held: HashSet<CellId> = regions.iter().copied().collect();
        let deepest = regions.iter().map(|c| c.level).max().unwrap_or(0);
        Cut { regions, held, deepest }
    }

    /// The regions, coarsest first.
    pub fn regions(&self) -> &[CellId] {
        &self.regions
    }

    /// Which region a position falls in, or [`None`] where the cut was made
    /// over a galaxy this position is not in.
    ///
    /// From the root down, and the first hit wins: the regions are disjoint
    /// and no region is inside another, so a position meets at most one of
    /// them. At most `deepest + 1` hash lookups.
    pub fn region_of(&self, position: [f64; 3]) -> Option<CellId> {
        (0..=self.deepest)
            .map(|level| CellId::of_point(position, level))
            .find(|cell| self.held.contains(cell))
    }
}

/// What one region hands the cells above it.
///
/// Its brightest few systems, which are the only ones the crown could take,
/// and the total of everything in it, which is what the crown's aggregates
/// are rolled up from. Both are gathered in one streaming pass.
#[derive(Clone, Debug)]
pub struct Offer {
    /// The cell this is an offer for.
    pub region: CellId,
    /// Its brightest systems, brightest first, at most as many as the crown
    /// could possibly claim.
    brightest: Vec<System>,
    /// Everything in the region, rolled up.
    total: Aggregate,
}

impl Offer {
    /// Read a region's systems and keep what the crown may need.
    ///
    /// `systems` is every system inside `region`, in any order. What is
    /// kept is `region.level × internal_slice` of them, plus one aggregate.
    pub fn of(
        region: CellId,
        systems: impl IntoIterator<Item = System>,
        params: &BuildParams,
    ) -> Offer {
        let room = region.level as usize * params.internal_slice;
        let mut brightest: Vec<System> = Vec::with_capacity(room + 1);
        let mut total = Aggregate::ZERO;

        for system in systems {
            total = total.merge(Aggregate::of_system(
                system.position,
                system.absolute_magnitude,
                system.temperature,
                system.age_bucket,
            ));
            if room == 0 {
                continue;
            }
            // Held sorted rather than heaped: the insert is a memmove of
            // `room`, and it only happens for a system brighter than the
            // faintest already kept.
            if brightest.len() == room
                && !brighter(&system, &brightest[room - 1])
            {
                continue;
            }
            let at = brightest.partition_point(|held| brighter(held, &system));
            brightest.insert(at, system);
            brightest.truncate(room);
        }

        Offer { region, brightest, total }
    }

    /// How many systems the region holds.
    pub fn count(&self) -> u64 {
        self.total.count()
    }
}

/// Brightest first, ties by id: the order the build settles ownership in.
fn brighter(a: &System, b: &System) -> bool {
    a.absolute_magnitude
        .total_cmp(&b.absolute_magnitude)
        .then(a.id64.cmp(&b.id64))
        .is_lt()
}

/// The cells above the cut, and what they took from the regions below.
///
/// A [`Snapshot`] like any other — cells and payloads, written the same way
/// — beside the set of systems it claimed, which is what each region must
/// be told before it can be built.
pub struct Crown {
    built: Snapshot,
    claimed: HashSet<u64>,
}

impl Crown {
    /// Settle the cells above the cut over what the regions offered.
    ///
    /// The candidates are taken brightest first across *all* regions, which
    /// is the order a whole build would have reached them in, and each is
    /// placed at the shallowest crown cell on its path with room. A
    /// candidate that finds none is left to its region.
    pub fn over(offers: &[Offer], params: &BuildParams) -> Crown {
        // Every cell strictly above the cut, and which octants of it have
        // something beneath them.
        let mut child_mask: HashMap<CellId, u8> = HashMap::new();
        let mut total: HashMap<CellId, Aggregate> = HashMap::new();
        for offer in offers {
            let mut child = offer.region;
            while let Some(parent) = child.parent() {
                *child_mask.entry(parent).or_insert(0) |= 1 << child.octant();
                let at = total.entry(parent).or_insert(Aggregate::ZERO);
                *at = at.merge(offer.total);
                child = parent;
            }
        }

        // The candidates, brightest first across every region. Each carries
        // the level its region sits at, which is where the crown stops.
        let mut candidates: Vec<(&System, u8)> = offers
            .iter()
            .flat_map(|offer| {
                offer.brightest.iter().map(|s| (s, offer.region.level))
            })
            .collect();
        candidates.sort_by(|(a, _), (b, _)| {
            a.absolute_magnitude
                .total_cmp(&b.absolute_magnitude)
                .then(a.id64.cmp(&b.id64))
        });

        let mut payloads: HashMap<CellId, Vec<Point>> = HashMap::new();
        let mut owned: HashMap<CellId, usize> = HashMap::new();
        let mut rank_lo: HashMap<CellId, u64> = HashMap::new();
        let mut claimed = HashSet::new();

        for (system, region_level) in candidates {
            // Every crown cell is internal — it has a region beneath it — so
            // the slice width is the internal one at every level.
            let mut taken = None;
            for level in 0..region_level {
                let cid = CellId::of_point(system.position, level);
                let count = owned.entry(cid).or_insert(0);
                if *count < params.internal_slice {
                    *count += 1;
                    taken = Some(level);
                    payloads.entry(cid).or_default().push(Point::new(
                        system.id64,
                        system.position,
                        system.absolute_magnitude,
                        system.temperature,
                        system.updated_at,
                    ));
                    break;
                }
            }
            let Some(taken) = taken else { continue };
            claimed.insert(system.id64);
            // The crown cells below the one that took it hold it and do not
            // own it. The cells inside its region are told the same thing by
            // being handed [`Crown::claimed`].
            for level in (taken + 1)..region_level {
                let cid = CellId::of_point(system.position, level);
                *rank_lo.entry(cid).or_insert(0) += 1;
            }
        }

        let cells = child_mask.keys().map(|&id| {
            let lo = rank_lo.get(&id).copied().unwrap_or(0);
            let slice = owned.get(&id).copied().unwrap_or(0) as u64;
            Cell {
                id,
                rank_lo: lo,
                rank_hi: lo + slice,
                child_mask: child_mask[&id],
                aggregate: total.get(&id).copied().unwrap_or(Aggregate::ZERO),
            }
        });

        Crown {
            built: Snapshot { index: Index::from_cells(cells), payloads },
            claimed,
        }
    }

    /// The systems the crown took, which their regions must not own.
    pub fn claimed(&self) -> &HashSet<u64> {
        &self.claimed
    }

    /// The crown itself: its cells, for the index, and its payloads, to be
    /// written like any others.
    pub fn built(&self) -> &Snapshot {
        &self.built
    }
}

/// The index of a whole galaxy, from the crown and every region's cells.
///
/// The one thing that has to be held to the end of a build, a cell being a
/// couple of hundred bytes where the systems it stands for are gigabytes.
pub fn joined<'a>(
    crown: &'a Crown,
    regions: impl IntoIterator<Item = &'a Index>,
) -> Index {
    let cells = crown
        .built()
        .index
        .cells()
        .chain(regions.into_iter().flat_map(|index| index.cells()))
        .copied();
    Index::from_cells(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn coord(&mut self) -> f64 {
            (self.next() % 40_000) as f64 - 20_000.0
        }
    }

    /// A galaxy with the lumpiness the real one has: most systems in a few
    /// places, the rest scattered, so a cut by count is not a cut by level.
    fn galaxy(n: u64) -> Vec<System> {
        let mut rng = Rng(0x5EED);
        let clumps: Vec<[f64; 3]> =
            (0..6).map(|_| [rng.coord(), rng.coord(), rng.coord()]).collect();
        (1..=n)
            .map(|id| {
                let near = clumps[(rng.next() % clumps.len() as u64) as usize];
                let spread = if id % 5 == 0 { 4_000.0 } else { 200.0 };
                let off = |rng: &mut Rng| {
                    (rng.next() % 2_000) as f64 / 1_000.0 * spread
                        - spread / 2.0
                };
                System {
                    id64: id,
                    position: [
                        near[0] + off(&mut rng),
                        near[1] + off(&mut rng),
                        near[2] + off(&mut rng),
                    ],
                    absolute_magnitude: (rng.next() % 2_000) as f64 / 100.0
                        - 5.0,
                    temperature: 3_000.0 + (rng.next() % 20_000) as f64,
                    age_bucket: (rng.next() % 8) as u32,
                    updated_at: 1_700_000_000 + (id as u32 % 1_000),
                }
            })
            .collect()
    }

    /// A cut of `systems` under `budget`, the way a caller makes one: count
    /// a fine grid, then divide from the root until every region fits.
    fn cut(systems: &[System], budget: u64, params: &BuildParams) -> Cut {
        const GRID: u8 = 6;
        let mut counted: HashMap<CellId, u64> = HashMap::new();
        for s in systems {
            *counted.entry(CellId::of_point(s.position, GRID)).or_insert(0) +=
                1;
        }
        Cut::of(counted, budget, params)
    }

    /// The systems of one region, which is what a caller reads out of a
    /// region's spill file.
    fn inside(systems: &[System], region: CellId) -> Vec<System> {
        systems
            .iter()
            .filter(|s| CellId::of_point(s.position, region.level) == region)
            .copied()
            .collect()
    }

    /// A galaxy built a region at a time is the galaxy built whole: the same
    /// cells, owning the same systems in the same order, with the same ranks.
    ///
    /// Checked at three budgets, since the crown's work grows as the cut
    /// deepens: one cutting near the root, one cutting deep, and one that
    /// reaches different depths in the dense and sparse parts of the same
    /// galaxy.
    ///
    /// Aggregates are compared for their counts and not their sums: a
    /// rolled-up `f64` depends on the order the merges happened in, which a
    /// regional build changes by construction. See `TODO-source-sink.md`
    /// item 5.
    #[test]
    fn a_regional_build_is_the_whole_build() {
        let params = BuildParams::default();
        let systems = galaxy(40_000);
        let whole = Snapshot::build(&systems, &params);

        for budget in [20_000u64, 6_000, 1_500] {
            let cut = cut(&systems, budget, &params);
            let offers: Vec<Offer> = cut
                .regions()
                .iter()
                .map(|&region| {
                    Offer::of(region, inside(&systems, region), &params)
                })
                .collect();
            let crown = Crown::over(&offers, &params);

            let depths: HashSet<u8> =
                cut.regions().iter().map(|c| c.level).collect();
            println!(
                "budget {budget}: {} regions over levels {:?}",
                cut.regions().len(),
                {
                    let mut d: Vec<u8> = depths.iter().copied().collect();
                    d.sort();
                    d
                },
            );
            assert!(
                !crown.claimed().is_empty(),
                "a cut under {budget} left the crown owning nothing, so \
                 this proves only that regions do not interfere",
            );

            let mut payloads = crown.built().payloads.clone();
            let mut indexes = Vec::new();
            for &region in cut.regions() {
                let built = Snapshot::of_region(
                    region,
                    &inside(&systems, region),
                    crown.claimed(),
                    &params,
                );
                payloads.extend(built.payloads.clone());
                indexes.push(built.index);
            }
            let index = joined(&crown, indexes.iter());

            assert_eq!(
                index.len(),
                whole.index.len(),
                "a cut under {budget} built a different set of cells",
            );
            for cell in whole.index.cells() {
                let built = index
                    .get(cell.id)
                    .unwrap_or_else(|| panic!("{:?} is missing", cell.id));
                assert_eq!(
                    (built.rank_lo, built.rank_hi, built.child_mask),
                    (cell.rank_lo, cell.rank_hi, cell.child_mask),
                    "{:?} differs under a budget of {budget}",
                    cell.id,
                );
                assert_eq!(
                    built.aggregate.count(),
                    cell.aggregate.count(),
                    "{:?} holds a different number under {budget}",
                    cell.id,
                );
                assert_eq!(
                    payloads.get(&cell.id).map_or(&[][..], Vec::as_slice),
                    whole.payload(cell.id),
                    "{:?} owns different systems under {budget}",
                    cell.id,
                );
            }
        }
    }

    /// A region offers exactly what the crown could take from it, and no
    /// more: the bound is what keeps a region's memory its own.
    #[test]
    fn an_offer_is_bounded_by_what_the_crown_can_claim() {
        let params = BuildParams::default();
        let systems = galaxy(20_000);
        for budget in [20_000u64, 5_000] {
            for region in
                cut(&systems, budget, &params).regions().iter().copied()
            {
                let level = region.level;
                let held = inside(&systems, region);
                let offer = Offer::of(region, held.clone(), &params);
                let room = level as usize * params.internal_slice;
                assert!(
                    offer.brightest.len() <= room.min(held.len()),
                    "a region at level {level} offered {} of {}",
                    offer.brightest.len(),
                    held.len(),
                );
                assert_eq!(offer.count(), held.len() as u64);
            }
        }
    }

    /// A cut places every system in exactly one region, and places it in
    /// the region a build would spill it to.
    #[test]
    fn a_cut_places_every_system_once() {
        let params = BuildParams::default();
        let systems = galaxy(20_000);
        let cut = cut(&systems, 3_000, &params);
        for s in &systems {
            let region = cut
                .region_of(s.position)
                .unwrap_or_else(|| panic!("{} fell outside the cut", s.id64));
            assert_eq!(
                CellId::of_point(s.position, region.level),
                region,
                "{} was placed in a region it is not inside",
                s.id64,
            );
        }
    }

    /// What a region offers is its brightest, in the order the build settles
    /// ownership in. A build that took them in any other order would hand
    /// the crown the wrong systems.
    #[test]
    fn an_offer_is_the_brightest_in_order() {
        let params = BuildParams::default();
        let systems = galaxy(5_000);
        let region = cut(&systems, 2_000, &params).regions()[0];
        let held = inside(&systems, region);
        let offer = Offer::of(region, held.clone(), &params);

        let mut wanted = held.clone();
        wanted.sort_by(|a, b| {
            a.absolute_magnitude
                .total_cmp(&b.absolute_magnitude)
                .then(a.id64.cmp(&b.id64))
        });
        wanted.truncate(offer.brightest.len());
        assert_eq!(offer.brightest, wanted);
    }
}
