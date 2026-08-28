//! What a catalog says about itself twice, and whether the two agree.
//!
//! A survey measures a parallax and writes down a distance. It also writes down
//! cartesian coordinates, which it computed from that distance and a bearing.
//! The two are the same fact recorded twice, so the length of the coordinates
//! should be the distance — and where it is not, the difference is rounding,
//! a revision applied to one column and not the other, or a mistake.
//!
//! It is a small number and it matters for a reason out of proportion to its
//! size: **it is the noise floor under every comparison this crate can make.**
//! A cross-check against another dataset that reports disagreements smaller than
//! this is reporting the catalog's own arithmetic back at itself. Knowing where
//! the floor sits is what says which findings are real.
//!
//! For HYG the answer is a median of about two parts in a billion, a 99th
//! percentile of one part in ninety thousand, and a worst row at one and a half
//! parts in a thousand. So anything above a tenth of a per cent is signal, and
//! the fifteen per cent by which Hipparcos and the modern value disagree about
//! the Pleiades is a mountain above it.
//!
//! The shape of the worst rows says what causes them, and it is not rounding.
//! HYG writes six decimal places of a parsec, which for a star ten light years
//! away is a part in ten million; the worst rows are out by a part in a
//! thousand, four thousand times that. They are also, conspicuously, *nearby*
//! stars and close pairs — Ross 248, EZ Aqr, two components sharing a distance
//! at 8.57 light years. That is the signature of a distance and a set of
//! coordinates computed from **different parallaxes**: one column revised, or
//! taken from the system and the other from the component, rather than one
//! number rounded twice.

use crate::Star;

/// One star's disagreement with itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Discrepancy {
    /// The star's name, where it has one.
    pub name: Option<String>,
    /// Its id within its source.
    pub id: u64,
    /// The distance the catalog records, light years.
    pub recorded: f64,
    /// The length of the coordinates it also records, light years.
    pub derived: f64,
}

impl Discrepancy {
    /// The difference as a fraction of the recorded distance.
    pub fn fraction(&self) -> f64 {
        if self.recorded > 0.0 {
            (self.derived - self.recorded).abs() / self.recorded
        } else {
            0.0
        }
    }

    /// The difference in light years.
    pub fn light_years(&self) -> f64 {
        (self.derived - self.recorded).abs()
    }
}

/// How far a catalog is from agreeing with itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Consistency {
    /// How many stars were examined.
    pub stars: usize,
    /// The median fractional disagreement.
    pub median: f64,
    /// The 99th percentile of it.
    pub p99: f64,
    /// The worst rows, worst first.
    pub worst: Vec<Discrepancy>,
}

impl Consistency {
    /// The largest fractional disagreement in the catalog.
    pub fn max(&self) -> f64 {
        self.worst.first().map_or(0.0, Discrepancy::fraction)
    }
}

/// Measure a catalog against itself.
///
/// `keep` is how many of the worst rows to hand back by name, for a report that
/// wants to point at them rather than only quantify them.
pub fn consistency(stars: &[Star], keep: usize) -> Consistency {
    let mut all: Vec<Discrepancy> = stars
        .iter()
        .filter(|s| s.distance > 0.0)
        .map(|s| Discrepancy {
            name: s.name.clone(),
            id: s.id,
            recorded: s.distance,
            derived: (s.position.iter().map(|c| c * c).sum::<f64>()).sqrt(),
        })
        .collect();

    all.sort_by(|a, b| a.fraction().total_cmp(&b.fraction()));
    let count = all.len();
    let at = |q: f64| {
        if count == 0 {
            0.0
        } else {
            all[((count as f64 * q) as usize).min(count - 1)].fraction()
        }
    };
    let (median, p99) = (at(0.5), at(0.99));

    all.reverse();
    all.truncate(keep);
    Consistency { stars: count, median, p99, worst: all }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyg;

    fn stars() -> Vec<Star> {
        hyg::read(include_str!("../data/bright.csv").as_bytes())
            .expect("a HYG catalog")
            .stars
    }

    /// **The noise floor is real and small.** Both halves matter: if the two
    /// columns agreed exactly there would be nothing to measure, and if they
    /// disagreed by a per cent no cross-check against another dataset would
    /// mean anything.
    #[test]
    fn the_catalog_nearly_agrees_with_itself() {
        let c = consistency(&stars(), 5);
        assert!(c.stars > 200);
        assert!(c.median > 0.0, "exact agreement would need no check");
        assert!(c.max() < 1e-2, "worst was {:e}", c.max());
        assert!(c.median <= c.p99 && c.p99 <= c.max());
    }

    /// The worst rows come back worst first, and named, so a report can point
    /// at them.
    #[test]
    fn the_worst_rows_come_back_in_order() {
        let c = consistency(&stars(), 5);
        assert_eq!(c.worst.len(), 5);
        for pair in c.worst.windows(2) {
            assert!(pair[0].fraction() >= pair[1].fraction());
        }
    }

    /// A star whose columns are made to disagree is found, and by the amount
    /// it was made to disagree by.
    #[test]
    fn a_planted_disagreement_is_found() {
        let mut stars = stars();
        // What this star already disagrees with itself by, before anything is
        // done to it. A planted error rides on top of the floor rather than
        // replacing it, which is the module's whole point and would make an
        // exact assertion here a lie.
        let floor = consistency(&stars[..1], 1).worst[0].fraction();

        let victim = &mut stars[0];
        // Push the coordinates ten per cent out without touching the distance.
        for c in &mut victim.position {
            *c *= 1.1;
        }
        let name = victim.name.clone();

        let c = consistency(&stars, 1);
        assert_eq!(c.worst[0].name, name);
        let found = c.worst[0].fraction();
        assert!(
            (found - 0.1).abs() <= floor * 1.1 + 1e-12,
            "planted 0.1, found {found}, on a floor of {floor:e}",
        );
        assert!(c.worst[0].light_years() > 0.0);
    }

    /// An empty catalog is zeroes rather than a panic on an empty slice.
    #[test]
    fn nothing_to_check_is_not_a_crash() {
        let c = consistency(&[], 5);
        assert_eq!(c.stars, 0);
        assert_eq!(c.max(), 0.0);
        assert!(c.worst.is_empty());
    }
}
