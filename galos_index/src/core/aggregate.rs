//! What a cell carries for its whole subtree, held so it composes exactly.
//!
//! A cell stands for every system beneath it, and says something true about
//! them whether or not their records are loaded: the count, where they sit,
//! how lately each was heard from, what stars they arrive at — the aggregate
//! `T(c)`. With the payload absent the total is drawn as it stands; with the
//! payload present the residual, the total less the moments of the slice
//! that arrived, is drawn instead, so no system counts twice.
//!
//! Everything is kept as a sum, sums composing where the things read off
//! them do not: counts, age buckets, star kinds. The centroid and spread come
//! out of [`Moments`], which keeps the moments they are read from rather than
//! the answers.
//!
//! Light is not here. What a subtree gives off is the realistic view's
//! question alone, and is [`crate::core::photometry::Photometry`], served
//! beside the cells rather than in them.

use crate::core::moments::Moments;
use crate::core::star::StarKind;

/// Age buckets for the Recency axis, which a prefix sum answers any span from.
pub const AGE_BUCKETS: usize = 8;

/// The totals a cell carries over its whole subtree.
///
/// Built from single systems with [`of_system`](Self::of_system), rolled up
/// with [`merge`](Self::merge), and drawn over its own loaded slice through
/// [`remove`](Self::remove). Everything is a sum, so a set split any way and
/// rejoined is the same aggregate.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aggregate {
    /// How many systems the subtree holds.
    pub(crate) count: u64,
    /// Position moments weighted by count, for the count-weighted centroid
    /// and extent.
    pub(crate) mass: Moments,
    /// Counts per age bucket, a column of the record so a Recency span can be
    /// answered by prefix sum off the aggregates alone. Every build writes it,
    /// and a span asked of a merged mark or of the field is answered from it,
    /// through [`Self::aged`]; a mark drawn as itself is asked off the
    /// `updated_at` the payload carries, which is the same clock binned finer.
    ///
    /// `u32`, and exact: a bucket counts systems and the buckets of a cell
    /// sum to its `count`, so four billion is ample over the galaxy's 200
    /// million, where a `u16` share of `count` would round the smallest
    /// bucket — the recently-changed one the axis exists to show — away.
    pub(crate) aged: [u32; AGE_BUCKETS],
    /// Counts per arrival star kind, indexed by [`StarKind::code`]: what the
    /// map colors a cell by when it colors every system by its star rather
    /// than the colonies by their politics. Sums to `count`, exact for the
    /// reason `aged` is.
    pub(crate) kinds: [u32; StarKind::COUNT],
}

impl Aggregate {
    /// The empty aggregate, the identity of [`merge`](Self::merge).
    pub const ZERO: Aggregate = Aggregate {
        count: 0,
        mass: Moments::ZERO,
        aged: [0; AGE_BUCKETS],
        kinds: [0; StarKind::COUNT],
    };

    /// One system's contribution: one count, at its position, in its age
    /// bucket and its star kind.
    pub fn of_system(
        position: [f64; 3],
        age_bucket: u32,
        kind: StarKind,
    ) -> Aggregate {
        let mut aged = [0; AGE_BUCKETS];
        if (age_bucket as usize) < AGE_BUCKETS {
            aged[age_bucket as usize] = 1;
        }
        let mut kinds = [0; StarKind::COUNT];
        kinds[usize::from(kind.code())] = 1;
        Aggregate { count: 1, mass: Moments::point(1.0, position), aged, kinds }
    }

    /// One [`System`](crate::system::System)'s contribution, off the record
    /// the build holds.
    pub fn of(system: &crate::system::System) -> Aggregate {
        Aggregate::of_system(system.position, system.age_bucket, system.kind)
    }

    /// Roll two aggregates into one. Commutative and associative, so a subtree
    /// rolls up the same however its children are ordered.
    pub fn merge(self, other: Aggregate) -> Aggregate {
        let mut aged = self.aged;
        let mut kinds = self.kinds;
        for (a, o) in aged.iter_mut().zip(other.aged) {
            *a += o;
        }
        for (k, o) in kinds.iter_mut().zip(other.kinds) {
            *k += o;
        }
        Aggregate {
            count: self.count + other.count,
            mass: self.mass.merge(other.mass),
            aged,
            kinds,
        }
    }

    /// The residual of this total less a slice that was part of it: what a
    /// cell splats once some of its systems have loaded and are drawn as
    /// themselves.
    ///
    /// Every field subtracts exactly, being the inverse of
    /// [`merge`](Self::merge).
    pub fn remove(self, slice: Aggregate) -> Aggregate {
        let mut aged = self.aged;
        let mut kinds = self.kinds;
        for (a, s) in aged.iter_mut().zip(slice.aged) {
            *a -= s;
        }
        for (k, s) in kinds.iter_mut().zip(slice.kinds) {
            *k -= s;
        }
        Aggregate {
            count: self.count - slice.count,
            mass: self.mass.remove(slice.mass),
            aged,
            kinds,
        }
    }

    /// How many systems the subtree holds.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// How many systems of the subtree fall in each Recency bucket
    ///
    /// **What lets a Recency span be answered about a cell rather than about a
    /// system.** The buckets are [`crate::records::derive::AGE_EDGES`], so the
    /// systems a span admits are a prefix of them and the count is a prefix sum
    /// — the same question the filter asks of a payload point, answered to the
    /// day instead of to the second, and answered for a whole subtree at once.
    pub fn aged(&self) -> &[u32; AGE_BUCKETS] {
        &self.aged
    }

    /// How many systems of the subtree arrive at each kind of star, indexed
    /// by [`StarKind::code`]
    pub fn kinds(&self) -> &[u32; StarKind::COUNT] {
        &self.kinds
    }

    /// The count-weighted centre of a cell, the centre of the subtree by count,
    /// which diverges from the glow's wherever the bright stars sit off centre:
    /// see [`crate::core::photometry::Photometry::luminosity_centroid`].
    pub fn count_centroid(&self) -> Option<[f64; 3]> {
        self.mass.centroid()
    }

    /// The count-weighted extent of a cell: the RMS radius by count.
    pub fn count_extent(&self) -> f64 {
        self.mass.rms_radius()
    }

    /// The count-weighted moments themselves, one unit a system.
    ///
    /// The centroid and the extent above are what most readers want; this is
    /// for the one that has to take a slice back out. A field drawing a cell
    /// whose systems have partly loaded draws the residual — the total less
    /// what is already drawn as itself — and that subtraction is
    /// [`Moments::remove`] against the moments of the drawn set, which cannot
    /// be done through a centroid and a radius because neither composes.
    pub fn mass(&self) -> Moments {
        self.mass
    }
}

impl FromIterator<Aggregate> for Aggregate {
    fn from_iter<I: IntoIterator<Item = Aggregate>>(iter: I) -> Aggregate {
        iter.into_iter().fold(Aggregate::ZERO, Aggregate::merge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn close3(a: [f64; 3], b: [f64; 3]) -> bool {
        close(a[0], b[0]) && close(a[1], b[1]) && close(a[2], b[2])
    }

    /// One system is itself: one count, its centroid on its position, in its
    /// age bucket and its kind.
    #[test]
    fn a_system_is_its_own_aggregate() {
        let a = Aggregate::of_system([1.0, 2.0, 3.0], 2, StarKind::G);
        assert_eq!(a.count(), 1);
        assert!(close3(a.count_centroid().unwrap(), [1.0, 2.0, 3.0]));
        assert!(close(a.count_extent(), 0.0));
        assert_eq!(a.aged()[2], 1);
        assert_eq!(a.kinds()[usize::from(StarKind::G.code())], 1);
    }

    /// A set split any way and rejoined is the same aggregate: count, the
    /// centroid, the spread and the star kinds all conserve.
    #[test]
    fn a_split_conserves_the_subtree() {
        let systems = [
            ([10.0, 0.0, 0.0], 1u32, StarKind::F),
            ([0.0, 10.0, 0.0], 2, StarKind::M),
            ([0.0, 0.0, 10.0], 0, StarKind::Neutron),
            ([-5.0, -5.0, -5.0], 3, StarKind::M),
        ];
        let of = |&(p, a, k): &([f64; 3], u32, StarKind)| {
            Aggregate::of_system(p, a, k)
        };
        let whole: Aggregate = systems.iter().map(of).collect();
        let left: Aggregate = systems[..2].iter().map(of).collect();
        let right: Aggregate = systems[2..].iter().map(of).collect();
        let rejoined = left.merge(right);

        assert_eq!(rejoined.count(), whole.count());
        assert!(close(rejoined.count_extent(), whole.count_extent()));
        assert!(close3(
            rejoined.count_centroid().unwrap(),
            whole.count_centroid().unwrap()
        ));
        assert_eq!(rejoined.kinds(), whole.kinds());
        let mut wanted = [0u32; StarKind::COUNT];
        wanted[usize::from(StarKind::F.code())] = 1;
        wanted[usize::from(StarKind::M.code())] = 2;
        wanted[usize::from(StarKind::Neutron.code())] = 1;
        assert_eq!(*whole.kinds(), wanted, "a kind landed in the wrong slot");
    }

    /// Removing a slice from a total leaves exactly the rest, the residual the
    /// field splats over what has loaded.
    #[test]
    fn remove_leaves_the_residual() {
        let of = |&(p, a, k): &([f64; 3], u32, StarKind)| {
            Aggregate::of_system(p, a, k)
        };
        let slice: Aggregate = [
            ([1.0, 0.0, 0.0], 0u32, StarKind::G),
            ([0.0, 1.0, 0.0], 1, StarKind::K),
        ]
        .iter()
        .map(of)
        .collect();
        let rest: Aggregate = [
            ([5.0, 5.0, 5.0], 2u32, StarKind::M),
            ([-2.0, 3.0, 1.0], 3, StarKind::G),
            ([0.0, 0.0, 9.0], 0, StarKind::Unknown),
        ]
        .iter()
        .map(of)
        .collect();
        let total = slice.merge(rest);
        let residual = total.remove(slice);

        assert_eq!(residual.count(), rest.count());
        assert!(close3(
            residual.count_centroid().unwrap(),
            rest.count_centroid().unwrap()
        ));
        assert!(close(residual.count_extent(), rest.count_extent()));
        assert_eq!(residual.aged, rest.aged);
        assert_eq!(residual.kinds(), rest.kinds());
        assert_eq!(
            residual.kinds().iter().map(|&n| u64::from(n)).sum::<u64>(),
            residual.count(),
            "the kinds stopped summing to the count",
        );
    }

    /// The empty aggregate changes nothing it merges with.
    #[test]
    fn zero_is_the_identity() {
        let a = Aggregate::of_system([1.0, 2.0, 3.0], 0, StarKind::K);
        assert_eq!(a.merge(Aggregate::ZERO), a);
        assert_eq!(Aggregate::ZERO.merge(a), a);
        assert_eq!(Aggregate::ZERO.count(), 0);
    }
}
