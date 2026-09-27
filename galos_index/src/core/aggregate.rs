//! What a cell carries for its whole subtree, held so it composes exactly.
//!
//! A cell stands for every system beneath it, and says something true about
//! them whether or not their records are loaded: the count, the flux, the two
//! weighted centroids, the brightest magnitude — the aggregate `T(c)`. With
//! the payload absent the total is drawn as it stands; with the payload
//! present the residual, the total less the moments of the slice that
//! arrived, is drawn instead, so no system counts twice.
//!
//! Everything additive is kept as a sum, sums composing where the things read
//! off them do not: flux per temperature bucket, counts, age buckets. The
//! centroids and spreads come out of [`Moments`], which keeps the moments
//! they are read from rather than the answers. Two weightings run at once and
//! diverge wherever the bright stars sit off centre: the glow follows the
//! light and the density follows the count.
//!
//! `m_min`, the brightest absolute magnitude in the subtree, composes by
//! taking the smaller of two rather than by adding, and is stored because a
//! summed flux has lost the single brightest star. It answers the photometric
//! cull on the stored total, never on a residual, so [`Aggregate::remove`]
//! leaves it be.

use crate::core::codec::{Decode, Encode, FixedCodec, record};
use crate::core::moments::Moments;
use galos_photometry::Magnitude;

/// Age buckets for the Recency axis, which a prefix sum answers any span from.
pub const AGE_BUCKETS: usize = 8;

/// The temperature range the buckets span, log-spaced between them: the
/// coolest star worth coloring and the hottest whose blue has stopped moving.
/// [`TempBucket::of`] bins the range and [`TempBucket::temperature`] names a
/// point back out of a bucket.
const TEMP_LO: f64 = 2000.0;
const TEMP_HI: f64 = 50000.0;

/// Which temperature bucket a star falls in, log-spaced across the stellar
/// range: what the glow keeps its color structure in, a warm bulge and blue
/// arms without a temperature stored per star.
///
/// One byte on the wire, and always one of the [`Self::COUNT`] buckets: both
/// ways in clamp.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TempBucket(u8);

impl TempBucket {
    /// How many buckets the range is split into.
    pub const COUNT: usize = 6;

    /// The bucket at `index`, clamped to the hottest.
    pub const fn new(index: u8) -> TempBucket {
        let last = (TempBucket::COUNT - 1) as u8;
        TempBucket(if index > last { last } else { index })
    }

    /// The bucket a star of `temperature_k` falls in, clamped at both ends.
    /// The buckets are even in log temperature, which is where color is even.
    pub fn of(temperature_k: f64) -> TempBucket {
        let t = temperature_k.clamp(TEMP_LO, TEMP_HI);
        let f = (t.ln() - TEMP_LO.ln()) / (TEMP_HI.ln() - TEMP_LO.ln());
        TempBucket::new((f * TempBucket::COUNT as f64) as u8)
    }

    /// Every bucket, coolest first.
    pub fn all() -> impl Iterator<Item = TempBucket> {
        (0..TempBucket::COUNT as u8).map(TempBucket)
    }

    /// Where the bucket stands among [`Self::COUNT`], for indexing a
    /// per-bucket array.
    pub fn index(self) -> usize {
        usize::from(self.0)
    }

    /// A representative temperature for the bucket, kelvin: the inverse of
    /// [`Self::of`].
    ///
    /// The geometric centre of the bucket's log-temperature span, so
    /// `TempBucket::of(b.temperature()) == b` for every bucket, and the color
    /// a bucket is drawn in is the blackbody tint at that centre.
    pub fn temperature(self) -> f64 {
        let f = (f64::from(self.0) + 0.5) / TempBucket::COUNT as f64;
        (TEMP_LO.ln() + f * (TEMP_HI.ln() - TEMP_LO.ln())).exp()
    }
}

impl Encode for TempBucket {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}

impl Decode for TempBucket {
    fn decode(cur: &mut &[u8]) -> Option<TempBucket> {
        Some(TempBucket::new(u8::decode(cur)?))
    }
}

impl FixedCodec for TempBucket {
    const LEN: usize = 1;
}

/// The totals a cell carries over its whole subtree.
///
/// Built from single systems with [`of_system`](Self::of_system), rolled up
/// with [`merge`](Self::merge), and drawn over its own loaded slice through
/// [`remove`](Self::remove). Everything but `m_min` is a sum, so a set split
/// any way and rejoined is the same aggregate.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aggregate {
    /// Brightest absolute magnitude in the subtree, the smallest number, or
    /// [`None`] for an empty aggregate.
    m_min: Option<f32>,
    /// How many systems the subtree holds.
    count: u64,
    /// Linear flux per temperature bucket, summed.
    flux: [f64; TempBucket::COUNT],
    /// Position moments weighted by flux, for the glow's centroid and spread.
    light: Moments,
    /// Position moments weighted by count, for the count-weighted centroid
    /// and extent.
    mass: Moments,
    /// Counts per age bucket, a column of the record so a Recency span can be
    /// answered by prefix sum off the aggregates alone. Every build writes it
    /// and nothing reads it back yet: the Recency filter asks per system, off
    /// the `updated_at` the payload carries, which is the same clock binned
    /// finer.
    ///
    /// `u32`, and exact: a bucket counts systems and the buckets of a cell
    /// sum to its `count`, so four billion is ample over the galaxy's 129
    /// million, where a `u16` share of `count` would round the smallest
    /// bucket — the recently-changed one the axis exists to show — away.
    aged: [u32; AGE_BUCKETS],
}

impl Aggregate {
    /// The empty aggregate, the identity of [`merge`](Self::merge).
    pub const ZERO: Aggregate = Aggregate {
        m_min: None,
        count: 0,
        flux: [0.0; TempBucket::COUNT],
        light: Moments::ZERO,
        mass: Moments::ZERO,
        aged: [0; AGE_BUCKETS],
    };

    /// One system's contribution.
    ///
    /// Its flux is `10^(-0.4*M)`, the linear form magnitudes sum in, dropped
    /// into the bucket its temperature falls in. Its position enters the glow
    /// weighted by that flux and the density weighted by one, and its
    /// magnitude is the brightest the aggregate has seen until something
    /// brighter merges in.
    pub fn of_system(
        position: [f64; 3],
        absolute_magnitude: f64,
        temperature: f64,
        age_bucket: u32,
    ) -> Aggregate {
        let f = Magnitude(absolute_magnitude).flux().0;
        let mut flux_by_bucket = [0.0; TempBucket::COUNT];
        flux_by_bucket[TempBucket::of(temperature).index()] = f;
        let mut aged = [0; AGE_BUCKETS];
        if (age_bucket as usize) < AGE_BUCKETS {
            aged[age_bucket as usize] = 1;
        }
        Aggregate {
            m_min: Some(absolute_magnitude as f32),
            count: 1,
            flux: flux_by_bucket,
            light: Moments::point(f, position),
            mass: Moments::point(1.0, position),
            aged,
        }
    }

    /// Roll two aggregates into one. Commutative and associative, so a subtree
    /// rolls up the same however its children are ordered.
    pub fn merge(self, other: Aggregate) -> Aggregate {
        let mut flux = self.flux;
        let mut aged = self.aged;
        for (f, o) in flux.iter_mut().zip(other.flux) {
            *f += o;
        }
        for (a, o) in aged.iter_mut().zip(other.aged) {
            *a += o;
        }
        Aggregate {
            m_min: min_opt(self.m_min, other.m_min),
            count: self.count + other.count,
            flux,
            light: self.light.merge(other.light),
            mass: self.mass.merge(other.mass),
            aged,
        }
    }

    /// The residual of this total less a slice that was part of it: what a
    /// cell splats once some of its systems have loaded and are drawn as
    /// themselves.
    ///
    /// The additive fields subtract exactly, being the inverse of
    /// [`merge`](Self::merge). `m_min` is left untouched: a residual splat is
    /// never culled on it, and the brightest single star cannot be recovered
    /// from a summed flux.
    pub fn remove(self, slice: Aggregate) -> Aggregate {
        let mut flux = self.flux;
        let mut aged = self.aged;
        for (f, s) in flux.iter_mut().zip(slice.flux) {
            *f -= s;
        }
        for (a, s) in aged.iter_mut().zip(slice.aged) {
            *a -= s;
        }
        Aggregate {
            m_min: self.m_min,
            count: self.count - slice.count,
            flux,
            light: self.light.remove(slice.light),
            mass: self.mass.remove(slice.mass),
            aged,
        }
    }

    /// The brightest absolute magnitude in the subtree, what the photometric
    /// walk prunes on.
    pub fn m_min(&self) -> Option<f32> {
        self.m_min
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
    /// The column has been written by every build since the format existed and
    /// had no reader.
    pub fn aged(&self) -> &[u32; AGE_BUCKETS] {
        &self.aged
    }

    /// The total linear flux across every temperature bucket.
    pub fn total_flux(&self) -> f64 {
        self.flux.iter().sum()
    }

    /// The flux in each temperature bucket, which is what the glow's color is
    /// resolved from.
    pub fn flux(&self) -> &[f64; TempBucket::COUNT] {
        &self.flux
    }

    /// Where the glow splats from: the flux-weighted centre of the subtree.
    pub fn luminosity_centroid(&self) -> Option<[f64; 3]> {
        self.light.centroid()
    }

    /// The glow's Gaussian footprint: the flux-weighted RMS radius.
    pub fn luminosity_spread(&self) -> f64 {
        self.light.rms_radius()
    }

    /// The count-weighted centre of a cell, the centre of the subtree by count,
    /// which diverges from the glow's wherever the bright stars sit off centre.
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

/// A brightest magnitude on the wire, with `NaN` standing for none: a real
/// magnitude is never NaN, so the sentinel cannot collide with a value.
struct BrightestMag(f32);

impl Encode for BrightestMag {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}

impl Decode for BrightestMag {
    fn decode(cur: &mut &[u8]) -> Option<BrightestMag> {
        Some(BrightestMag(f32::decode(cur)?))
    }
}

impl FixedCodec for BrightestMag {
    const LEN: usize = f32::LEN;
}

impl From<Option<f32>> for BrightestMag {
    fn from(m: Option<f32>) -> BrightestMag {
        BrightestMag(m.unwrap_or(f32::NAN))
    }
}

impl From<BrightestMag> for Option<f32> {
    fn from(m: BrightestMag) -> Option<f32> {
        (!m.0.is_nan()).then_some(m.0)
    }
}

record! {
    Aggregate {
        m_min: Option<f32> as BrightestMag,
        count: u64,
        flux: [f64; TempBucket::COUNT],
        light: Moments,
        mass: Moments,
        aged: [u32; AGE_BUCKETS],
    }
}

impl FromIterator<Aggregate> for Aggregate {
    fn from_iter<I: IntoIterator<Item = Aggregate>>(iter: I) -> Aggregate {
        iter.into_iter().fold(Aggregate::ZERO, Aggregate::merge)
    }
}

/// The smaller of two magnitudes, the brighter star, ignoring an empty side.
fn min_opt(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, b) => b,
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

    /// One system is itself: one count, its own magnitude, its flux in one
    /// bucket, and both centroids on its position.
    #[test]
    fn a_system_is_its_own_aggregate() {
        let a = Aggregate::of_system([1.0, 2.0, 3.0], 4.83, 5772.0, 0);
        assert_eq!(a.count(), 1);
        assert_eq!(a.m_min(), Some(4.83));
        assert!(close(a.total_flux(), Magnitude(4.83).flux().0));
        assert!(close3(a.luminosity_centroid().unwrap(), [1.0, 2.0, 3.0]));
        assert!(close3(a.count_centroid().unwrap(), [1.0, 2.0, 3.0]));
        assert!(close(a.luminosity_spread(), 0.0));
    }

    /// The temperature buckets span the range, cool at the bottom and hot at
    /// the top, and never leave it.
    #[test]
    fn temperature_buckets_span_the_range() {
        let last = TempBucket::new(TempBucket::COUNT as u8 - 1);
        assert_eq!(TempBucket::of(1000.0), TempBucket::new(0));
        assert_eq!(TempBucket::of(2000.0), TempBucket::new(0));
        assert_eq!(TempBucket::of(60000.0), last);
        assert!(TempBucket::of(30000.0) > TempBucket::of(4000.0));
        // Monotonic non-decreasing across the range.
        let mut was = TempBucket::new(0);
        for k in (2000..=50000).step_by(1000) {
            let b = TempBucket::of(k as f64);
            assert!(b >= was);
            was = b;
        }
    }

    /// A bucket's representative temperature falls back in that same bucket, so
    /// the color drawn for a bucket is the tint of a star that would land in
    /// it, and the centres climb with the bucket.
    #[test]
    fn bucket_temperature_round_trips() {
        let mut last = f64::NEG_INFINITY;
        for bucket in TempBucket::all() {
            let t = bucket.temperature();
            assert_eq!(TempBucket::of(t), bucket, "{bucket:?} centre");
            assert!(t > last, "centres climb with the bucket");
            last = t;
        }
    }

    /// The brightest magnitude is the smallest, and merging keeps it.
    #[test]
    fn m_min_keeps_the_brightest() {
        let dim = Aggregate::of_system([0.0; 3], 10.0, 3400.0, 0);
        let bright = Aggregate::of_system([1.0; 3], -2.0, 20000.0, 0);
        assert_eq!(dim.merge(bright).m_min(), Some(-2.0));
        assert_eq!(bright.merge(dim).m_min(), Some(-2.0));
    }

    /// A hot star and a cool one land in different buckets, and merging sums
    /// the buckets rather than blending them.
    #[test]
    fn flux_stays_in_its_temperature_bucket() {
        let cool = Aggregate::of_system([0.0; 3], 5.0, 3000.0, 0);
        let hot = Aggregate::of_system([0.0; 3], 5.0, 25000.0, 0);
        let cool_b = TempBucket::of(3000.0).index();
        let hot_b = TempBucket::of(25000.0).index();
        assert_ne!(cool_b, hot_b);
        let both = cool.merge(hot);
        assert!(close(both.flux()[cool_b], Magnitude(5.0).flux().0));
        assert!(close(both.flux()[hot_b], Magnitude(5.0).flux().0));
    }

    /// The two centroids diverge: a bright star and a dim one meet in the
    /// middle by count, where the glow leans toward the bright one.
    #[test]
    fn the_glow_and_the_map_centre_differ() {
        let bright = Aggregate::of_system([0.0, 0.0, 0.0], -1.0, 15000.0, 0);
        let dim = Aggregate::of_system([10.0, 0.0, 0.0], 9.0, 3400.0, 0);
        let a = bright.merge(dim);
        // Count weights them equally: the midpoint.
        assert!(close3(a.count_centroid().unwrap(), [5.0, 0.0, 0.0]));
        // Flux weights the bright one far more, so the glow sits near it.
        assert!(a.luminosity_centroid().unwrap()[0] < 0.1);
    }

    /// A set split any way and rejoined is the same aggregate: count, flux,
    /// both centroids and both spreads all conserve.
    #[test]
    fn a_split_conserves_the_subtree() {
        let systems = [
            ([10.0, 0.0, 0.0], 3.0, 6000.0, 1u32),
            ([0.0, 10.0, 0.0], 7.0, 3500.0, 2),
            ([0.0, 0.0, 10.0], -1.0, 20000.0, 0),
            ([-5.0, -5.0, -5.0], 5.0, 4800.0, 3),
        ];
        let whole: Aggregate = systems
            .iter()
            .map(|&(p, m, t, a)| Aggregate::of_system(p, m, t, a))
            .collect();
        let left: Aggregate = systems[..2]
            .iter()
            .map(|&(p, m, t, a)| Aggregate::of_system(p, m, t, a))
            .collect();
        let right: Aggregate = systems[2..]
            .iter()
            .map(|&(p, m, t, a)| Aggregate::of_system(p, m, t, a))
            .collect();
        let rejoined = left.merge(right);

        assert_eq!(rejoined.count(), whole.count());
        assert_eq!(rejoined.m_min(), whole.m_min());
        assert!(close(rejoined.total_flux(), whole.total_flux()));
        assert!(close(rejoined.luminosity_spread(), whole.luminosity_spread()));
        assert!(close(rejoined.count_extent(), whole.count_extent()));
        assert!(close3(
            rejoined.luminosity_centroid().unwrap(),
            whole.luminosity_centroid().unwrap()
        ));
        assert!(close3(
            rejoined.count_centroid().unwrap(),
            whole.count_centroid().unwrap()
        ));
    }

    /// Removing a slice from a total leaves exactly the rest, the residual the
    /// field splats over what has loaded.
    #[test]
    fn remove_leaves_the_residual() {
        let slice: Aggregate = [
            ([1.0, 0.0, 0.0], 2.0, 6000.0, 0u32),
            ([0.0, 1.0, 0.0], 4.0, 4000.0, 1),
        ]
        .iter()
        .map(|&(p, m, t, a)| Aggregate::of_system(p, m, t, a))
        .collect();
        let rest: Aggregate = [
            ([5.0, 5.0, 5.0], 6.0, 3500.0, 2u32),
            ([-2.0, 3.0, 1.0], 8.0, 3200.0, 3),
            ([0.0, 0.0, 9.0], 1.0, 12000.0, 0),
        ]
        .iter()
        .map(|&(p, m, t, a)| Aggregate::of_system(p, m, t, a))
        .collect();
        let total = slice.merge(rest);
        let residual = total.remove(slice);

        assert_eq!(residual.count(), rest.count());
        assert!(close(residual.total_flux(), rest.total_flux()));
        assert!(close3(
            residual.luminosity_centroid().unwrap(),
            rest.luminosity_centroid().unwrap()
        ));
        assert!(close(residual.count_extent(), rest.count_extent()));
        assert_eq!(residual.aged, rest.aged);
    }

    /// The empty aggregate changes nothing it merges with.
    #[test]
    fn zero_is_the_identity() {
        let a = Aggregate::of_system([1.0, 2.0, 3.0], 5.0, 5000.0, 0);
        assert_eq!(a.merge(Aggregate::ZERO), a);
        assert_eq!(Aggregate::ZERO.merge(a), a);
        assert_eq!(Aggregate::ZERO.count(), 0);
        assert_eq!(Aggregate::ZERO.m_min(), None);
    }
}
