//! Weighted moments that compose by addition and survive galaxy-scale offsets.
//!
//! A cell stands for the systems in its subtree, and what it stands for has to
//! survive being split and rejoined. Descending replaces a parent's one splat
//! with its children's slices plus their residual splats, and the two have to
//! integrate to the same totals or the galaxy would brighten and dim as the
//! view moved. So a cell does not store a centroid and a radius, which do not
//! add, but moments they are read from, which do.
//!
//! The obvious moments, the weighted sum of positions and of squared distances
//! from the origin, add cleanly but cancel badly. A system sixty thousand
//! light years out has a squared distance near four billion, and the spread
//! within its cell is a few hundred at most, so reading the spread as the mean
//! squared distance less the squared centroid subtracts two near-equal billions
//! and keeps only noise. That is the same precision loss the cell-relative
//! payload exists to avoid, and it is avoided here the same way: by never
//! forming a quantity about the far origin.
//!
//! So the moments are kept about the running centroid, as Welford and Chan
//! compose a variance: the weight, the weighted mean, and `m2`, the weighted
//! sum of squared deviations from that mean. Merging shifts the mean by the
//! weighted gap between the two and corrects `m2` by the same gap: the
//! parallel axis theorem, but applied to the small distance between two
//! centroids rather than to the large distance to the origin. Removing a slice
//! is the exact inverse.
//!
//! The weight is whatever the walk weights by. The overview weights by count,
//! per system, and reads a count-weighted centroid and extent. The glow weights
//! by luminosity and reads a luminosity-weighted centroid and spread. It is the
//! same arithmetic; only the weight differs.

use crate::serialization::{FixedCodec, Decode, Encode, record};

/// The weighted moments of a set of points in three dimensions.
///
/// Held about the running centroid rather than the origin, so a set far from
/// the origin keeps the precision of its own spread. [`merge`](Self::merge)
/// combines two sets and [`remove`](Self::remove) takes one back out: the
/// residual of a total minus a slice that was part of it, which is what a cell
/// drawn over its own loaded systems subtracts so no system is counted twice.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Moments {
    /// Total weight, `sum(w_i)`.
    weight: f64,
    /// The weighted mean position, the centroid the deviations are about.
    mean: [f64; 3],
    /// Weighted sum of squared deviations from the mean, `sum(w_i * |p_i - mean|^2)`.
    m2: f64,
}

impl Moments {
    /// The empty set, the identity of [`merge`](Self::merge).
    pub const ZERO: Moments = Moments { weight: 0.0, mean: [0.0; 3], m2: 0.0 };

    /// One point of weight `w` at `p`. A single point is its own mean and has
    /// no deviation from it, so `m2` is zero exactly, whatever `p` is.
    pub fn point(w: f64, p: [f64; 3]) -> Moments {
        Moments { weight: w, mean: p, m2: 0.0 }
    }

    /// The total weight of the set: its count under count weighting, its flux
    /// under luminosity weighting.
    pub fn weight(&self) -> f64 {
        self.weight
    }

    /// The weighted mean position, or [`None`] where there is no weight to take
    /// a mean of.
    pub fn centroid(&self) -> Option<[f64; 3]> {
        (self.weight > 0.0).then_some(self.mean)
    }

    /// The RMS distance of the set from its own centroid.
    ///
    /// Zero for an empty set and, exactly, for a single point. Floored at zero
    /// so rounding in a residual can never ask for the square root of a small
    /// negative.
    pub fn rms_radius(&self) -> f64 {
        if self.weight <= 0.0 {
            return 0.0;
        }
        (self.m2 / self.weight).max(0.0).sqrt()
    }

    /// Combine two sets into one. Commutative and associative, so neighbours
    /// sum into a field with no seam and no dependence on draw order.
    ///
    /// The mean moves toward the heavier set by the weighted gap between the
    /// two, and `m2` gains that gap squared scaled by the reduced weight: the
    /// parallel axis correction, worked on the distance between centroids.
    pub fn merge(self, other: Moments) -> Moments {
        let weight = self.weight + other.weight;
        if weight <= 0.0 {
            return Moments::ZERO;
        }
        let delta = [
            other.mean[0] - self.mean[0],
            other.mean[1] - self.mean[1],
            other.mean[2] - self.mean[2],
        ];
        let share = other.weight / weight;
        let mean = [
            self.mean[0] + delta[0] * share,
            self.mean[1] + delta[1] * share,
            self.mean[2] + delta[2] * share,
        ];
        let delta_sq = delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2];
        let m2 = self.m2 + other.m2 + delta_sq * (self.weight * other.weight / weight);
        Moments { weight, mean, m2 }
    }

    /// Take `other`'s contribution back out: the residual of a total less a
    /// slice.
    ///
    /// The exact inverse of [`merge`](Self::merge): `total.remove(slice)` is the
    /// rest exactly when `slice.merge(rest)` was the total. This is how a cell
    /// whose systems have loaded draws only what its ancestors did not,
    /// subtracting the moments of the arrived slice from the stored total. Only
    /// meaningful where `other` was part of `self`.
    pub fn remove(self, other: Moments) -> Moments {
        let weight = self.weight - other.weight;
        if weight <= 0.0 {
            return Moments::ZERO;
        }
        let mean = [
            (self.weight * self.mean[0] - other.weight * other.mean[0]) / weight,
            (self.weight * self.mean[1] - other.weight * other.mean[1]) / weight,
            (self.weight * self.mean[2] - other.weight * other.mean[2]) / weight,
        ];
        let delta = [
            other.mean[0] - mean[0],
            other.mean[1] - mean[1],
            other.mean[2] - mean[2],
        ];
        let delta_sq = delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2];
        let m2 = self.m2 - other.m2 - delta_sq * (weight * other.weight / self.weight);
        Moments { weight, mean, m2 }
    }
}

record! {
    Moments {
        weight: f64,
        mean: [f64; 3],
        m2: f64,
    }
}

impl FromIterator<Moments> for Moments {
    fn from_iter<I: IntoIterator<Item = Moments>>(iter: I) -> Moments {
        iter.into_iter().fold(Moments::ZERO, Moments::merge)
    }
}

impl std::iter::Sum for Moments {
    fn sum<I: Iterator<Item = Moments>>(iter: I) -> Moments {
        iter.fold(Moments::ZERO, Moments::merge)
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

    fn close_moments(a: Moments, b: Moments) -> bool {
        close(a.weight, b.weight) && close3(a.mean, b.mean) && close(a.m2, b.m2)
    }

    /// A lone point sits at itself and has no spread, exactly zero however far
    /// from the origin it is, which is the whole reason for the deviation form.
    #[test]
    fn a_point_is_its_own_centroid() {
        let m = Moments::point(1.0, [3.0, -4.0, 12.0]);
        assert!(close3(m.centroid().unwrap(), [3.0, -4.0, 12.0]));
        assert_eq!(m.rms_radius(), 0.0);
        assert!(close(m.weight(), 1.0));
    }

    /// A faint point sixty thousand light years out still has exactly zero
    /// spread, where an origin-referenced sum would show a light year of noise.
    #[test]
    fn a_far_faint_point_has_no_spurious_spread() {
        let m = Moments::point(0.0116, [60000.0, -900.0, 40000.0]);
        assert_eq!(m.rms_radius(), 0.0);
    }

    /// Nothing has no centroid and no radius.
    #[test]
    fn the_empty_set_has_no_centroid() {
        assert_eq!(Moments::ZERO.centroid(), None);
        assert!(close(Moments::ZERO.rms_radius(), 0.0));
    }

    /// Two equal points meet in the middle, each half the span away, so the RMS
    /// radius is half the distance between them.
    #[test]
    fn two_equal_points_meet_in_the_middle() {
        let m = Moments::point(1.0, [0.0, 0.0, 0.0])
            .merge(Moments::point(1.0, [2.0, 0.0, 0.0]));
        assert!(close3(m.centroid().unwrap(), [1.0, 0.0, 0.0]));
        assert!(close(m.rms_radius(), 1.0));
        assert!(close(m.weight(), 2.0));
    }

    /// Weight pulls the centroid: three parts at the origin against one part
    /// out at four leaves the mean one unit out, not two.
    #[test]
    fn weight_pulls_the_centroid() {
        let m = Moments::point(3.0, [0.0, 0.0, 0.0])
            .merge(Moments::point(1.0, [4.0, 0.0, 0.0]));
        assert!(close3(m.centroid().unwrap(), [1.0, 0.0, 0.0]));
    }

    /// Merging is order-independent, which is what lets neighbours accumulate
    /// into one field with no seam.
    #[test]
    fn merging_is_commutative_and_associative() {
        let a = Moments::point(1.0, [1.0, 2.0, 3.0]);
        let b = Moments::point(2.0, [-4.0, 0.5, 6.0]);
        let c = Moments::point(0.5, [7.0, -3.0, 1.0]);
        assert!(close_moments(a.merge(b), b.merge(a)));
        assert!(close_moments(a.merge(b).merge(c), a.merge(b.merge(c))));
    }

    /// The empty set changes nothing it is merged with.
    #[test]
    fn zero_is_the_identity() {
        let a = Moments::point(2.5, [1.0, -2.0, 3.0]);
        assert!(close_moments(a.merge(Moments::ZERO), a));
        assert!(close_moments(Moments::ZERO.merge(a), a));
    }

    /// Remove undoes merge: a total less a slice is exactly the rest. This is
    /// the residual subtraction the field leans on.
    #[test]
    fn remove_is_the_inverse_of_merge() {
        let slice = Moments::point(1.0, [2.0, 0.0, 0.0])
            .merge(Moments::point(1.0, [0.0, 2.0, 0.0]));
        let rest = Moments::point(3.0, [-1.0, -1.0, 5.0])
            .merge(Moments::point(0.5, [8.0, 1.0, -2.0]));
        let total = slice.merge(rest);
        assert!(close_moments(total.remove(slice), rest));
        assert!(close_moments(total.remove(rest), slice));
    }

    /// Conservation: a set split any way and its parts recombined is the same
    /// set, so the centroid and radius of the whole match those of the union
    /// of its pieces. This is what makes a coarse cell and its refined children
    /// integrate to the same totals.
    #[test]
    fn a_split_conserves_the_whole() {
        let points = [
            (1.0, [10.0, 0.0, 0.0]),
            (2.0, [0.0, 10.0, 0.0]),
            (1.5, [0.0, 0.0, 10.0]),
            (0.5, [-10.0, -10.0, -10.0]),
            (3.0, [4.0, 4.0, 4.0]),
        ];
        let whole: Moments =
            points.iter().map(|&(w, p)| Moments::point(w, p)).collect();

        let left: Moments =
            points[..2].iter().map(|&(w, p)| Moments::point(w, p)).collect();
        let right: Moments =
            points[2..].iter().map(|&(w, p)| Moments::point(w, p)).collect();

        assert!(close_moments(left.merge(right), whole));
        assert!(close(left.merge(right).rms_radius(), whole.rms_radius()));
        assert!(close3(
            left.merge(right).centroid().unwrap(),
            whole.centroid().unwrap()
        ));
    }

    /// The spread is right even when the whole set sits far from the origin:
    /// two points a known distance apart out in the galaxy keep their true
    /// half-span, not a subtraction of billions.
    #[test]
    fn spread_holds_far_from_the_origin() {
        let base = [40000.0, -900.0, 24400.0];
        let a = Moments::point(1.0, base);
        let b = Moments::point(1.0, [base[0] + 300.0, base[1], base[2]]);
        let m = a.merge(b);
        assert!(close(m.rms_radius(), 150.0));
        assert!(close3(m.centroid().unwrap(), [base[0] + 150.0, base[1], base[2]]));
    }

    /// The iterator forms fold through merge, so summing a column of moments is
    /// the same as merging them by hand.
    #[test]
    fn summing_folds_through_merge() {
        let ms = [
            Moments::point(1.0, [1.0, 0.0, 0.0]),
            Moments::point(2.0, [0.0, 3.0, 0.0]),
            Moments::point(1.0, [0.0, 0.0, 5.0]),
        ];
        let by_hand = ms[0].merge(ms[1]).merge(ms[2]);
        let collected: Moments = ms.into_iter().collect();
        let summed: Moments = ms.into_iter().sum();
        assert!(close_moments(collected, by_hand));
        assert!(close_moments(summed, by_hand));
    }
}
