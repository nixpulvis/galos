//! How bright, and how hot: what the realistic view asks of a system and of a
//! cell, kept apart from what every view asks.
//!
//! A cell's [`Aggregate`](crate::core::aggregate::Aggregate) is what the map
//! reads of every cell it draws — the count, where the systems sit, how old
//! the news of them is, what stars they arrive at. Light is not in it. Only
//! the realistic view weighs a star by how much light reaches the eye, so
//! only the realistic view reads this: [`Photometry`] over a whole subtree,
//! and [`Lit`] for each system of a cell's payload. Both are served beside
//! the cells rather than in them — `photometry.bin` beside `index.bin`, and a
//! file under `photometry/` beside each cell's payload — so a view that never
//! asks about light never reads any.
//!
//! The split is also what keeps brightness from ordering anything else. A
//! cell's slice used to be its subtree's brightest, because the magnitude was
//! there in the record to be sorted by; see [`crate::core::standing`] for the
//! order now, and what it cost the views that drew from it.
//!
//! [`Photometry`] composes as the aggregate does: sums, and `m_min`, which
//! takes the smaller of two and is stored because a summed flux has lost the
//! single brightest star. It answers the photometric cull on the stored
//! total, never on a residual, so [`Photometry::remove`] leaves it be.

use crate::core::moments::Moments;
use crate::system::System;
use galos_photometry::Magnitude;

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

/// One system's light, as the photometry sidecar holds it beside its cell's
/// payload: the combined absolute magnitude narrowed to `f32`, and the
/// blackbody tint already binned so a reader needs no per-star join.
///
/// Neither is fit to build an aggregate or an order from; that is
/// [`System`]'s, at full precision.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Lit {
    pub magnitude: f32,
    pub temp_bucket: TempBucket,
}

impl Lit {
    /// A system's light, packed for the sidecar: the one place its precision
    /// is given up.
    pub fn of(system: &System) -> Lit {
        Lit {
            magnitude: system.absolute_magnitude as f32,
            temp_bucket: TempBucket::of(system.temperature),
        }
    }
}

/// The light a cell's whole subtree gives off.
///
/// Built from single systems with [`of_system`](Self::of_system), rolled up
/// with [`merge`](Self::merge), and drawn over a loaded slice through
/// [`remove`](Self::remove). Everything but `m_min` is a sum, so a set split
/// any way and rejoined is the same total.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Photometry {
    /// Brightest absolute magnitude in the subtree, the smallest number, or
    /// [`None`] for an empty one.
    pub(crate) m_min: Option<f32>,
    /// Linear flux per temperature bucket, summed.
    pub(crate) flux: [f64; TempBucket::COUNT],
    /// Position moments weighted by flux, for the glow's centroid and spread.
    pub(crate) light: Moments,
}

impl Photometry {
    /// No light, the identity of [`merge`](Self::merge).
    pub const ZERO: Photometry = Photometry {
        m_min: None,
        flux: [0.0; TempBucket::COUNT],
        light: Moments::ZERO,
    };

    /// One system's light.
    ///
    /// Its flux is `10^(-0.4*M)`, the linear form magnitudes sum in, dropped
    /// into the bucket its temperature falls in; its position enters the glow
    /// weighted by that flux; and its magnitude is the brightest the total has
    /// seen until something brighter merges in.
    pub fn of_system(
        position: [f64; 3],
        absolute_magnitude: f64,
        temperature: f64,
    ) -> Photometry {
        let f = Magnitude(absolute_magnitude).flux().0;
        let mut flux = [0.0; TempBucket::COUNT];
        flux[TempBucket::of(temperature).index()] = f;
        Photometry {
            m_min: Some(absolute_magnitude as f32),
            flux,
            light: Moments::point(f, position),
        }
    }

    /// One [`System`]'s light, off the record the build holds.
    pub fn of(system: &System) -> Photometry {
        Photometry::of_system(
            system.position,
            system.absolute_magnitude,
            system.temperature,
        )
    }

    /// Roll two totals into one. Commutative and associative, so a subtree
    /// rolls up the same however its children are ordered.
    pub fn merge(self, other: Photometry) -> Photometry {
        let mut flux = self.flux;
        for (f, o) in flux.iter_mut().zip(other.flux) {
            *f += o;
        }
        Photometry {
            m_min: min_opt(self.m_min, other.m_min),
            flux,
            light: self.light.merge(other.light),
        }
    }

    /// The residual of this total less a slice that was part of it.
    ///
    /// The sums subtract exactly, being the inverse of
    /// [`merge`](Self::merge). `m_min` is left untouched: a residual is never
    /// culled on it, and the brightest single star cannot be recovered from a
    /// summed flux.
    pub fn remove(self, slice: Photometry) -> Photometry {
        let mut flux = self.flux;
        for (f, s) in flux.iter_mut().zip(slice.flux) {
            *f -= s;
        }
        Photometry {
            m_min: self.m_min,
            flux,
            light: self.light.remove(slice.light),
        }
    }

    /// The brightest absolute magnitude in the subtree, what the photometric
    /// walk prunes on.
    pub fn m_min(&self) -> Option<f32> {
        self.m_min
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
}

impl FromIterator<Photometry> for Photometry {
    fn from_iter<I: IntoIterator<Item = Photometry>>(iter: I) -> Photometry {
        iter.into_iter().fold(Photometry::ZERO, Photometry::merge)
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

    /// One system is its own light: its magnitude, its flux in one bucket,
    /// and the glow's centre on its position.
    #[test]
    fn a_system_is_its_own_photometry() {
        let a = Photometry::of_system([1.0, 2.0, 3.0], 4.83, 5772.0);
        assert_eq!(a.m_min(), Some(4.83));
        assert!(close(a.total_flux(), Magnitude(4.83).flux().0));
        assert!(close3(a.luminosity_centroid().unwrap(), [1.0, 2.0, 3.0]));
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
        let dim = Photometry::of_system([0.0; 3], 10.0, 3400.0);
        let bright = Photometry::of_system([1.0; 3], -2.0, 20000.0);
        assert_eq!(dim.merge(bright).m_min(), Some(-2.0));
        assert_eq!(bright.merge(dim).m_min(), Some(-2.0));
    }

    /// A hot star and a cool one land in different buckets, and merging sums
    /// the buckets rather than blending them.
    #[test]
    fn flux_stays_in_its_temperature_bucket() {
        let cool = Photometry::of_system([0.0; 3], 5.0, 3000.0);
        let hot = Photometry::of_system([0.0; 3], 5.0, 25000.0);
        let cool_b = TempBucket::of(3000.0).index();
        let hot_b = TempBucket::of(25000.0).index();
        assert_ne!(cool_b, hot_b);
        let both = cool.merge(hot);
        assert!(close(both.flux()[cool_b], Magnitude(5.0).flux().0));
        assert!(close(both.flux()[hot_b], Magnitude(5.0).flux().0));
    }

    /// The glow leans toward the light: a bright star and a dim one meet near
    /// the bright one, where a count would put them in the middle.
    #[test]
    fn the_glow_leans_toward_the_light() {
        let bright = Photometry::of_system([0.0, 0.0, 0.0], -1.0, 15000.0);
        let dim = Photometry::of_system([10.0, 0.0, 0.0], 9.0, 3400.0);
        let both = bright.merge(dim);
        assert!(both.luminosity_centroid().unwrap()[0] < 0.1);
    }

    /// A set split any way and rejoined is the same light.
    #[test]
    fn a_split_conserves_the_light() {
        let systems = [
            ([10.0, 0.0, 0.0], 3.0, 6000.0),
            ([0.0, 10.0, 0.0], 7.0, 3500.0),
            ([0.0, 0.0, 10.0], -1.0, 20000.0),
            ([-5.0, -5.0, -5.0], 5.0, 4800.0),
        ];
        let of =
            |&(p, m, t): &([f64; 3], f64, f64)| Photometry::of_system(p, m, t);
        let whole: Photometry = systems.iter().map(of).collect();
        let left: Photometry = systems[..2].iter().map(of).collect();
        let right: Photometry = systems[2..].iter().map(of).collect();
        let rejoined = left.merge(right);

        assert_eq!(rejoined.m_min(), whole.m_min());
        assert!(close(rejoined.total_flux(), whole.total_flux()));
        assert!(close(rejoined.luminosity_spread(), whole.luminosity_spread()));
        assert!(close3(
            rejoined.luminosity_centroid().unwrap(),
            whole.luminosity_centroid().unwrap()
        ));
    }

    /// Removing a slice from a total leaves exactly the rest.
    #[test]
    fn remove_leaves_the_residual() {
        let of =
            |&(p, m, t): &([f64; 3], f64, f64)| Photometry::of_system(p, m, t);
        let slice: Photometry =
            [([1.0, 0.0, 0.0], 2.0, 6000.0), ([0.0, 1.0, 0.0], 4.0, 4000.0)]
                .iter()
                .map(of)
                .collect();
        let rest: Photometry = [
            ([5.0, 5.0, 5.0], 6.0, 3500.0),
            ([-2.0, 3.0, 1.0], 8.0, 3200.0),
            ([0.0, 0.0, 9.0], 1.0, 12000.0),
        ]
        .iter()
        .map(of)
        .collect();
        let residual = slice.merge(rest).remove(slice);
        assert!(close(residual.total_flux(), rest.total_flux()));
        assert!(close3(
            residual.luminosity_centroid().unwrap(),
            rest.luminosity_centroid().unwrap()
        ));
    }

    /// No light changes nothing it merges with.
    #[test]
    fn zero_is_the_identity() {
        let a = Photometry::of_system([1.0, 2.0, 3.0], 5.0, 5000.0);
        assert_eq!(a.merge(Photometry::ZERO), a);
        assert_eq!(Photometry::ZERO.merge(a), a);
        assert_eq!(Photometry::ZERO.m_min(), None);
    }
}
