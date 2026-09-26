//! What a cell carries about the systems anybody lives in.
//!
//! [`Aggregate`](crate::core::aggregate::Aggregate) stands for every system in a
//! subtree, and its two weightings are the stellar ones: flux for the glow,
//! count for the density. Neither answers where the *inhabited* systems sit,
//! and the two are nothing alike — one 256 ly cell holds a few thousand
//! governed systems among some hundred thousand neighbours, so a political
//! field laid at the count-weighted centroid with the count-weighted spread
//! draws the colonized filaments as a blob over the whole cell and loses the
//! shape that was the picture.
//!
//! So this is a third weighting, kept beside the other two rather than folded
//! into them: **one unit of weight for every system somebody lives in, and
//! none for the rest**. The centroid is then where the colonies are and the
//! spread is how far they reach, both of which are what a political splat is
//! drawn from. Inhabited is exactly `population > 0` and is asked per system,
//! never reconstructed from a sum — a summed population says somebody lives
//! under a cell and never how many systems do.
//!
//! The political histograms ride here rather than in their own record because
//! they have the same support: only a system on the populated table has an
//! allegiance, a government or a security rating to count. They are counts and
//! not fractions, because a count composes exactly and a `u8` share of a total
//! does not — merging two shares needs their weights back, and the rounding
//! accumulates through every level of the rollup until [`Inhabited::remove`]
//! stops being the inverse of [`Inhabited::merge`].
//!
//! What the counts are *worth* is not stored. A view that wants unaligned
//! systems to weigh less than aligned ones applies its gain where it draws,
//! which keeps the gain a tunable rather than a format decision and keeps a
//! residual correct under any gain: bake a weight into the stored sum and a
//! residual taken under one gain is wrong under another.
//!
//! Derived rather than published today. [`Inhabitance::of`] rolls the resident
//! `populated.bin` up a tree the client already holds, which is the whole
//! column for the price of one pass over a table that is resident anyway. The
//! record carries its own codec so the builder can publish it as
//! `agg/inhabited.bin` once cells have a stable order, and nothing reading it
//! has to change when they do.

use crate::core::codec::{Decode, Encode, FixedCodec, record};
use crate::core::geometry::CellId;
use crate::core::moments::Moments;
use crate::read::index::Index;
use crate::records::PopulatedSystem;
use elite_journal::prelude::{Allegiance, Government, Security};
use std::collections::HashMap;

/// Buckets the allegiance histogram counts in: one a variant, plus one for a
/// system whose allegiance nothing has reported.
///
/// Bucket zero is that unknown, and it is not the same fact as
/// [`Allegiance::None`], which is the game saying a populated system answers
/// to nobody. Both draw grey and the two are kept apart anyway, for the same
/// reason an absent `factions.bin` is not an empty one.
pub(crate) const ALLEGIANCE_BUCKETS: usize = 11;

/// Buckets the government histogram counts in, plus one unknown at zero.
pub(crate) const GOVERNMENT_BUCKETS: usize = 18;

/// Buckets the security histogram counts in, plus one unknown at zero.
pub(crate) const SECURITY_BUCKETS: usize = 6;

/// Which bucket a system's allegiance counts in.
///
/// A `match` and never a comparison: `Allegiance` carries a hand-written
/// `PartialEq` under which `None != None`, so `==` answers falsely for the one
/// variant a histogram most needs to place.
pub(crate) fn allegiance_bucket(allegiance: Option<Allegiance>) -> usize {
    match allegiance {
        None => 0,
        Some(Allegiance::Alliance) => 1,
        Some(Allegiance::Empire) => 2,
        Some(Allegiance::Federation) => 3,
        Some(Allegiance::Guardian) => 4,
        Some(Allegiance::Independent) => 5,
        Some(Allegiance::PilotsFederation) => 6,
        Some(Allegiance::PlayerPilots) => 7,
        Some(Allegiance::Thargoid) => 8,
        Some(Allegiance::FrontlineSolutions) => 9,
        Some(Allegiance::None) => 10,
    }
}

/// The allegiance a bucket counts: the inverse of [`allegiance_bucket`], so a
/// view can name the colour a bucket is drawn in.
pub fn allegiance_at(bucket: usize) -> Option<Allegiance> {
    match bucket {
        1 => Some(Allegiance::Alliance),
        2 => Some(Allegiance::Empire),
        3 => Some(Allegiance::Federation),
        4 => Some(Allegiance::Guardian),
        5 => Some(Allegiance::Independent),
        6 => Some(Allegiance::PilotsFederation),
        7 => Some(Allegiance::PlayerPilots),
        8 => Some(Allegiance::Thargoid),
        9 => Some(Allegiance::FrontlineSolutions),
        10 => Some(Allegiance::None),
        _ => None,
    }
}

/// Which bucket a system's government counts in.
pub(crate) fn government_bucket(government: Option<Government>) -> usize {
    match government {
        None => 0,
        Some(Government::Anarchy) => 1,
        Some(Government::Communism) => 2,
        Some(Government::Confederacy) => 3,
        Some(Government::Cooperative) => 4,
        Some(Government::Corporate) => 5,
        Some(Government::Democracy) => 6,
        Some(Government::Dictatorship) => 7,
        Some(Government::Feudal) => 8,
        Some(Government::Patronage) => 9,
        Some(Government::Prison) => 10,
        Some(Government::PrisonColony) => 11,
        Some(Government::Theocracy) => 12,
        Some(Government::Engineer) => 13,
        Some(Government::Carrier) => 14,
        Some(Government::Megaconstruction) => 15,
        Some(Government::PrivateOwnership) => 16,
        Some(Government::None) => 17,
    }
}

/// The government a bucket counts: the inverse of [`government_bucket`].
pub fn government_at(bucket: usize) -> Option<Government> {
    match bucket {
        1 => Some(Government::Anarchy),
        2 => Some(Government::Communism),
        3 => Some(Government::Confederacy),
        4 => Some(Government::Cooperative),
        5 => Some(Government::Corporate),
        6 => Some(Government::Democracy),
        7 => Some(Government::Dictatorship),
        8 => Some(Government::Feudal),
        9 => Some(Government::Patronage),
        10 => Some(Government::Prison),
        11 => Some(Government::PrisonColony),
        12 => Some(Government::Theocracy),
        13 => Some(Government::Engineer),
        14 => Some(Government::Carrier),
        15 => Some(Government::Megaconstruction),
        16 => Some(Government::PrivateOwnership),
        17 => Some(Government::None),
        _ => None,
    }
}

/// Which bucket a system's security rating counts in.
pub(crate) fn security_bucket(security: Option<Security>) -> usize {
    match security {
        None => 0,
        Some(Security::High) => 1,
        Some(Security::Medium) => 2,
        Some(Security::Low) => 3,
        Some(Security::Anarchy) => 4,
        Some(Security::None) => 5,
    }
}

/// The security rating a bucket counts: the inverse of [`security_bucket`].
pub fn security_at(bucket: usize) -> Option<Security> {
    match bucket {
        1 => Some(Security::High),
        2 => Some(Security::Medium),
        3 => Some(Security::Low),
        4 => Some(Security::Anarchy),
        5 => Some(Security::None),
        _ => None,
    }
}

/// What a cell carries about the inhabited systems in its whole subtree.
///
/// Built from single systems with [`of_system`](Self::of_system), rolled up
/// with [`merge`](Self::merge) and drawn over its own loaded slice through
/// [`remove`](Self::remove). Every field is a sum, so a set split any way and
/// rejoined is the same record and there is no non-additive key to answer on
/// the total instead of the residual — which [`Aggregate`](crate::core::aggregate::Aggregate)
/// needs for `m_min` and this does not need at all.
///
/// The prune key is `count > 0`: a subtree with nobody in it cannot matter to
/// a political view, and that is free to ask.
///
/// [`Default`] is [`ZERO`](Self::ZERO): a record of nobody is the identity of
/// [`merge`](Self::merge), so the two cannot mean different things.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Inhabited {
    /// How many systems in the subtree anybody lives in.
    count: u64,
    /// Position moments in that weight, one unit a system: the centroid the
    /// political field splats from and the spread of its footprint.
    settled: Moments,
    /// Inhabited systems per allegiance bucket. Sums to `count`.
    allegiance: [u32; ALLEGIANCE_BUCKETS],
    /// Inhabited systems per government bucket. Sums to `count`.
    government: [u32; GOVERNMENT_BUCKETS],
    /// Inhabited systems per security bucket. Sums to `count`.
    security: [u32; SECURITY_BUCKETS],
}

impl Inhabited {
    /// The empty record, the identity of [`merge`](Self::merge).
    pub const ZERO: Inhabited = Inhabited {
        count: 0,
        settled: Moments::ZERO,
        allegiance: [0; ALLEGIANCE_BUCKETS],
        government: [0; GOVERNMENT_BUCKETS],
        security: [0; SECURITY_BUCKETS],
    };

    /// One inhabited system's contribution: one unit of weight at its
    /// position, and one count in each axis's bucket.
    ///
    /// The caller owes the predicate. Only a system somebody lives in belongs
    /// here — `population > 0` — because the weight is what makes the centroid
    /// the colonies' own, and an empty system contributing would pull it back
    /// toward the count centroid this exists to differ from.
    pub fn of_system(
        position: [f64; 3],
        allegiance: Option<Allegiance>,
        government: Option<Government>,
        security: Option<Security>,
    ) -> Inhabited {
        let mut a = [0; ALLEGIANCE_BUCKETS];
        a[allegiance_bucket(allegiance)] = 1;
        let mut g = [0; GOVERNMENT_BUCKETS];
        g[government_bucket(government)] = 1;
        let mut s = [0; SECURITY_BUCKETS];
        s[security_bucket(security)] = 1;
        Inhabited {
            count: 1,
            settled: Moments::point(1.0, position),
            allegiance: a,
            government: g,
            security: s,
        }
    }

    /// Roll two records into one. Commutative and associative, so a subtree
    /// rolls up the same however its children are ordered.
    pub fn merge(self, other: Inhabited) -> Inhabited {
        let mut allegiance = self.allegiance;
        let mut government = self.government;
        let mut security = self.security;
        for (a, o) in allegiance.iter_mut().zip(other.allegiance) {
            *a += o;
        }
        for (g, o) in government.iter_mut().zip(other.government) {
            *g += o;
        }
        for (s, o) in security.iter_mut().zip(other.security) {
            *s += o;
        }
        Inhabited {
            count: self.count + other.count,
            settled: self.settled.merge(other.settled),
            allegiance,
            government,
            security,
        }
    }

    /// The residual of this total less a slice that was part of it: what a
    /// cell splats once some of its systems have loaded and are drawn as
    /// themselves.
    ///
    /// Every field subtracts exactly, being the inverse of
    /// [`merge`](Self::merge), the moments included.
    pub fn remove(self, slice: Inhabited) -> Inhabited {
        let mut allegiance = self.allegiance;
        let mut government = self.government;
        let mut security = self.security;
        for (a, s) in allegiance.iter_mut().zip(slice.allegiance) {
            *a -= s;
        }
        for (g, s) in government.iter_mut().zip(slice.government) {
            *g -= s;
        }
        for (x, s) in security.iter_mut().zip(slice.security) {
            *x -= s;
        }
        Inhabited {
            count: self.count - slice.count,
            settled: self.settled.remove(slice.settled),
            allegiance,
            government,
            security,
        }
    }

    /// How many systems in the subtree anybody lives in, exact. Zero is the
    /// prune key: nothing political is under this cell at all.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Where the political field splats from: the centre of the inhabited
    /// systems alone, or [`None`] where there are none.
    ///
    /// [`None`] is load-bearing and must not fall back on the count centroid.
    /// A cell with nobody in it has no political place, and drawing one at the
    /// centre of its empty systems is how a colony appears where there is not
    /// one.
    pub fn centroid(&self) -> Option<[f64; 3]> {
        self.settled.centroid()
    }

    /// The footprint of that field: the RMS radius of the inhabited systems
    /// about their own centroid, in light years.
    pub fn spread(&self) -> f64 {
        self.settled.rms_radius()
    }

    /// Inhabited systems per allegiance bucket, which a political view resolves
    /// its colour from. Index with [`allegiance_bucket`], name with
    /// [`allegiance_at`].
    pub fn allegiance(&self) -> &[u32; ALLEGIANCE_BUCKETS] {
        &self.allegiance
    }

    /// Inhabited systems per government bucket.
    pub fn government(&self) -> &[u32; GOVERNMENT_BUCKETS] {
        &self.government
    }

    /// Inhabited systems per security bucket.
    pub fn security(&self) -> &[u32; SECURITY_BUCKETS] {
        &self.security
    }
}

record! {
    Inhabited {
        count: u64,
        settled: Moments,
        allegiance: [u32; ALLEGIANCE_BUCKETS],
        government: [u32; GOVERNMENT_BUCKETS],
        security: [u32; SECURITY_BUCKETS],
    }
}

impl FromIterator<Inhabited> for Inhabited {
    fn from_iter<I: IntoIterator<Item = Inhabited>>(iter: I) -> Inhabited {
        iter.into_iter().fold(Inhabited::ZERO, Inhabited::merge)
    }
}

/// The inhabited aggregation over a whole tree, one record a cell.
///
/// Keyed by address and never by ordinal, which is the same rule the published
/// per-column files are held to: `index.bin` has no stable cell order, so
/// anything keyed by file position is rewritten whole whenever the tree's shape
/// moves.
///
/// Sparse on purpose. Only the cells with somebody under them get a record —
/// tens of thousands of a few hundred thousand — and a cell absent from here
/// reads as [`Inhabited::ZERO`], which is what it is.
#[derive(Clone, Debug, Default)]
pub struct Inhabitance(HashMap<CellId, Inhabited>);

impl Inhabitance {
    /// Roll every inhabited system in `rows` up the tree it falls in.
    ///
    /// A system contributes to every cell on its path from the root, which is
    /// what makes a cell's record the total over its whole subtree and what
    /// makes a colony's colour reach every level above it. Cells the tree does
    /// not hold are not invented: the descent follows the index's own children,
    /// so a row lands on exactly the cells that stand over it.
    ///
    /// Rows with nobody living in them are skipped. `populated.bin` is written
    /// from a projection already gated on population, so this is a guard and
    /// not a filter — but the weight is the whole point of the record, and a
    /// silent zero-population row would flatten the centroid it exists to
    /// sharpen.
    ///
    /// Positions come off the populated row as `f32`, which is a thirtieth of
    /// a light year out at the galaxy's edge and can put a system the wrong
    /// side of a cell boundary it sits exactly on. That moves one count between
    /// two neighbouring cells of a density field and is not worth a wider
    /// column to prevent.
    pub fn of<'a>(
        index: &Index,
        rows: impl IntoIterator<Item = &'a PopulatedSystem>,
    ) -> Inhabitance {
        let mut held: HashMap<CellId, Inhabited> = HashMap::new();
        for row in rows {
            if row.population == 0 {
                continue;
            }
            let position = [
                row.position[0] as f64,
                row.position[1] as f64,
                row.position[2] as f64,
            ];
            let one = Inhabited::of_system(
                position,
                row.allegiance,
                row.government,
                row.security,
            );
            index.descend(position, |id| {
                let at = held.entry(id).or_insert(Inhabited::ZERO);
                *at = at.merge(one);
            });
        }
        Inhabitance(held)
    }

    /// What a cell carries, or [`None`] where nobody lives under it.
    pub fn get(&self, id: CellId) -> Option<&Inhabited> {
        self.0.get(&id)
    }

    /// How many cells have anybody under them.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no cell does.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::aggregate::{Aggregate, Cell};
    use crate::core::name::SystemName;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn close3(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(a, b)| close(*a, *b))
    }

    /// An inhabited system at a place, with an allegiance and nothing else.
    fn row(
        address: i64,
        at: [f64; 3],
        population: u64,
        allegiance: Option<Allegiance>,
    ) -> PopulatedSystem {
        PopulatedSystem {
            address,
            name: SystemName::new("SOL"),
            position: [at[0] as f32, at[1] as f32, at[2] as f32],
            population,
            security: None,
            government: None,
            allegiance,
            primary_economy: None,
            secondary_economy: None,
            factions: Vec::new(),
            body_count: None,
            non_body_count: None,
        }
    }

    fn of(at: [f64; 3], allegiance: Option<Allegiance>) -> Inhabited {
        Inhabited::of_system(at, allegiance, None, None)
    }

    #[test]
    fn a_system_is_its_own_record() {
        let a = of([1.0, 2.0, 3.0], Some(Allegiance::Empire));
        assert_eq!(a.count(), 1);
        assert!(close3(a.centroid().unwrap(), [1.0, 2.0, 3.0]));
        assert!(close(a.spread(), 0.0));
        assert_eq!(
            a.allegiance()[allegiance_bucket(Some(Allegiance::Empire))],
            1
        );
        assert_eq!(a.allegiance().iter().sum::<u32>(), 1);
        assert_eq!(a.government().iter().sum::<u32>(), 1);
        assert_eq!(a.security().iter().sum::<u32>(), 1);
    }

    /// Every bucket names exactly the value that counts in it, and the widths
    /// are tight: no variant shares a bucket and no bucket goes unused.
    #[test]
    fn the_buckets_round_trip() {
        for bucket in 0..ALLEGIANCE_BUCKETS {
            assert_eq!(allegiance_bucket(allegiance_at(bucket)), bucket);
        }
        for bucket in 0..GOVERNMENT_BUCKETS {
            assert_eq!(government_bucket(government_at(bucket)), bucket);
        }
        for bucket in 0..SECURITY_BUCKETS {
            assert_eq!(security_bucket(security_at(bucket)), bucket);
        }
    }

    /// The game saying "no allegiance" is not the same fact as nothing having
    /// been reported, and the histogram keeps them apart. Neither can be found
    /// by comparison: `Allegiance::None != Allegiance::None`.
    #[test]
    fn unreported_and_unaligned_are_different_buckets() {
        assert_ne!(
            allegiance_bucket(None),
            allegiance_bucket(Some(Allegiance::None))
        );
        let unreported = of([0.0; 3], None);
        let unaligned = of([0.0; 3], Some(Allegiance::None));
        assert_ne!(unreported.allegiance(), unaligned.allegiance());
    }

    /// A set split any way and rejoined is the same record, moments included.
    #[test]
    fn a_split_conserves_the_subtree() {
        let systems = [
            ([0.0, 0.0, 0.0], Some(Allegiance::Federation)),
            ([10.0, 0.0, 0.0], Some(Allegiance::Empire)),
            ([0.0, 10.0, 0.0], Some(Allegiance::Federation)),
            ([-5.0, 2.0, 8.0], None),
        ];
        let whole: Inhabited = systems.iter().map(|&(p, a)| of(p, a)).collect();
        let left: Inhabited =
            systems[..2].iter().map(|&(p, a)| of(p, a)).collect();
        let right: Inhabited =
            systems[2..].iter().map(|&(p, a)| of(p, a)).collect();

        assert_eq!(whole.count(), left.merge(right).count());
        assert_eq!(whole.allegiance(), left.merge(right).allegiance());
        assert!(close3(
            whole.centroid().unwrap(),
            left.merge(right).centroid().unwrap()
        ));
        assert!(close(whole.spread(), left.merge(right).spread()));
        // And the other order, since a rollup does not fix one.
        assert!(close(whole.spread(), right.merge(left).spread()));
    }

    /// `remove` is the exact inverse of `merge`: the residual of a total less
    /// a loaded slice is the rest, so nothing is counted twice.
    #[test]
    fn remove_leaves_the_residual() {
        let slice: Inhabited = [
            ([1.0, 0.0, 0.0], Some(Allegiance::Alliance)),
            ([2.0, 1.0, 0.0], Some(Allegiance::Empire)),
        ]
        .iter()
        .map(|&(p, a)| of(p, a))
        .collect();
        let rest: Inhabited = [
            ([20.0, 5.0, 5.0], Some(Allegiance::Federation)),
            ([18.0, 4.0, 7.0], None),
            ([25.0, 9.0, 1.0], Some(Allegiance::None)),
        ]
        .iter()
        .map(|&(p, a)| of(p, a))
        .collect();
        let residual = slice.merge(rest).remove(slice);

        assert_eq!(residual.count(), rest.count());
        assert_eq!(residual.allegiance(), rest.allegiance());
        assert_eq!(residual.government(), rest.government());
        assert_eq!(residual.security(), rest.security());
        assert!(close3(residual.centroid().unwrap(), rest.centroid().unwrap()));
        assert!(close(residual.spread(), rest.spread()));
    }

    #[test]
    fn zero_is_the_identity() {
        let a = of([1.0, 2.0, 3.0], Some(Allegiance::Empire));
        assert_eq!(a.merge(Inhabited::ZERO), a);
        assert_eq!(Inhabited::ZERO.merge(a), a);
        assert_eq!(Inhabited::ZERO.count(), 0);
    }

    /// An empty record has no political place, and says so rather than
    /// answering with the origin.
    #[test]
    fn nobody_home_has_no_centroid() {
        assert_eq!(Inhabited::ZERO.centroid(), None);
        assert!(close(Inhabited::ZERO.spread(), 0.0));
    }

    #[test]
    fn a_record_round_trips() {
        let held = of([1.0, 2.0, 3.0], Some(Allegiance::Empire))
            .merge(of([-4.0, 5.0, 6.0], None));
        let mut buf = Vec::new();
        held.encode(&mut buf);
        assert_eq!(buf.len(), Inhabited::LEN);
        let mut cur = buf.as_slice();
        assert_eq!(Inhabited::decode(&mut cur), Some(held));
        assert!(cur.is_empty());
    }

    /// A tree of one root cell and its eight children, so a descent has
    /// somewhere to go.
    fn tree(at: [f64; 3]) -> Index {
        let leaf = CellId::of_point(at, 3);
        let mut cells = vec![Cell {
            id: CellId::ROOT,
            rank_lo: 0,
            rank_hi: 0,
            child_mask: 0,
            aggregate: Aggregate::ZERO,
        }];
        // The chain of cells over `at`, each naming the next as its child.
        let mut path = vec![CellId::ROOT];
        for level in 1..=leaf.level {
            path.push(CellId::of_point(at, level));
        }
        cells.clear();
        for (depth, id) in path.iter().enumerate() {
            let mask = match path.get(depth + 1) {
                Some(kid) => {
                    let kids = id.children();
                    let octant =
                        kids.iter().position(|k| k == kid).unwrap() as u8;
                    1u8 << octant
                }
                None => 0,
            };
            cells.push(Cell {
                id: *id,
                rank_lo: 0,
                rank_hi: 0,
                child_mask: mask,
                aggregate: Aggregate::ZERO,
            });
        }
        Index::from_cells(cells)
    }

    /// Every cell over a system counts it, so a colony's colour reaches the
    /// root and a coarse view is the sum of the fine ones under it.
    #[test]
    fn a_system_counts_in_every_cell_over_it() {
        let at = [100.0, 20.0, 24_000.0];
        let index = tree(at);
        let held = Inhabitance::of(
            &index,
            [&row(1, at, 5_000, Some(Allegiance::Empire))],
        );

        let root = held.get(CellId::ROOT).expect("the root counts it");
        assert_eq!(root.count(), 1);
        assert!(close3(root.centroid().unwrap(), at));
        for level in 1..=3u8 {
            let id = CellId::of_point(at, level);
            assert_eq!(
                held.get(id).map(Inhabited::count),
                Some(1),
                "level {level} lost the system"
            );
        }
    }

    /// Nobody living there is not a colony, whatever the table says.
    #[test]
    fn an_empty_system_is_not_counted() {
        let at = [100.0, 20.0, 24_000.0];
        let index = tree(at);
        let held = Inhabitance::of(&index, [&row(1, at, 0, None)]);
        assert!(held.is_empty());
        assert_eq!(held.get(CellId::ROOT), None);
    }

    /// The point of the record: the inhabited centre is not the count centre.
    ///
    /// Two colonies at one end of a cell and a crowd of empty systems at the
    /// other. The count centroid sits with the crowd and the political field
    /// laid there would draw the colonies where they are not; this centroid
    /// sits on the colonies.
    #[test]
    fn the_settled_centre_is_not_the_count_centre() {
        let colonies = [[10.0, 0.0, 24_000.0], [12.0, 0.0, 24_000.0]];
        let index = tree(colonies[0]);
        let held = Inhabitance::of(
            &index,
            [
                &row(1, colonies[0], 100, Some(Allegiance::Empire)),
                &row(2, colonies[1], 100, Some(Allegiance::Empire)),
            ],
        );
        let settled = held.get(CellId::ROOT).unwrap().centroid().unwrap();

        // The stellar count centroid of the same region, pulled far off by a
        // hundred empty systems nowhere near the colonies.
        let mut mass = Aggregate::ZERO;
        for i in 0..100 {
            mass = mass.merge(Aggregate::of_system(
                [-20_000.0 + i as f64, 0.0, 24_000.0],
                8.0,
                5000.0,
                0,
            ));
        }
        for at in colonies {
            mass = mass.merge(Aggregate::of_system(at, 8.0, 5000.0, 0));
        }
        let count = mass.count_centroid().unwrap();

        assert!(close(settled[0], 11.0), "settled centre moved: {settled:?}");
        assert!(
            (count[0] - settled[0]).abs() > 1_000.0,
            "the two centres did not diverge: {count:?} vs {settled:?}"
        );
    }
}
