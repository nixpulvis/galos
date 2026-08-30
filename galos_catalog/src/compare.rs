//! Comparing a catalog against another dataset's idea of the same stars.
//!
//! Two surveys of one sky disagree in three ways, and telling them apart is the
//! whole job.
//!
//! - **Distance.** The measurement that is hard, the one that gets revised, and
//!   the one a parallax is. If Elite's Pleiades sit at 118 parsecs where the
//!   modern value is 136, that is not a bug in either dataset, it is a date
//!   stamp on the older one's import.
//! - **Direction.** The measurement that is easy and does not get revised.
//!   Where two datasets disagree on a bearing they disagree about which star
//!   they are talking about, or one of them placed it procedurally.
//! - **Frame.** Not a disagreement at all, but it looks exactly like one until
//!   it is divided out. Two datasets can agree perfectly and share no axes.
//!
//! # Why the frame is fitted rather than assumed
//!
//! Nothing here is told how to rotate one dataset onto the other. The rotation
//! is **recovered from the matched stars**, by the orthogonal matrix that best
//! carries one set of bearings onto the other, and what is reported is the
//! residual after it. That is the honest order: a comparison that assumed a
//! transform would be testing the assumption, and a wrong assumption would show
//! up as every star being wrong rather than as a wrong matrix.
//!
//! It also answers a question in its own right, since the fitted rotation *is*
//! the transform between the two frames, derived rather than guessed. And its
//! determinant says something no rotation can hide: a best fit that comes back
//! a reflection means the two datasets differ in handedness, which is a fact
//! about a coordinate convention rather than about the sky.
//!
//! # Distance is compared without any of that
//!
//! Distance from the origin is invariant under rotation and reflection alike,
//! so [`Match::distance_error`] needs no frame and cannot be contaminated by a
//! bad fit. It is the figure to trust first and the one the interesting
//! question rests on.
//!
//! Both sides of it are the length of a **position**, including the catalog's,
//! rather than the catalog's own distance column. The two are not quite the
//! same number — across HYG's 109,400 stars they disagree by a median of 2e-9,
//! a 99th percentile of 1.1e-5 and as much as 1.5e-3 in the worst row, which
//! `galos-catalog consistency` will print. Far below anything a comparison
//! between two datasets is looking for, but not zero, and taking one side's
//! distance from a column and the other's from a position would put the
//! catalog's own inconsistency into every row of the report as though it were a
//! disagreement with the other dataset. Position against position; the noise
//! floor is then the other dataset's alone.

use crate::frame::rotate;
use crate::Star;

/// The fewest matched stars a rotation can be fitted from.
///
/// Three non-collinear bearings pin an orientation: two leave a spin about
/// their common axis undetermined and one determines nothing at all. With
/// fewer than this [`fit_frame`] returns [`None`] rather than a rotation it
/// cannot stand behind, and the distance comparison — which needs no frame —
/// carries on without one.
const MIN_PAIRS_TO_FIT: usize = 3;

/// How many Newton steps the polar decomposition is allowed before it gives up.
///
/// The iteration `X <- (X + X^-T) / 2` converges on the orthogonal factor in a
/// handful of steps for any well-conditioned matrix, so this is a generous
/// ceiling that only bites on a degenerate fit: a safety stop against a matrix
/// that will not converge, not a knob that shapes a good one.
const MAX_POLAR_ITERATIONS: usize = 64;

/// When the polar iteration has converged: the largest a single matrix entry
/// may still move between steps before the result is taken as orthogonal.
///
/// Set near the floor of `f64` precision, since the iteration converges
/// quadratically and there is no cost to letting a good fit reach its last few
/// bits.
const POLAR_CONVERGENCE: f64 = 1e-14;

/// Below this, a 3x3 matrix's determinant is treated as zero and the matrix as
/// singular rather than inverted.
///
/// The correlation matrix of degenerate bearings — all collinear, say — lands
/// here, and inverting it would feed the iteration nonsense. Loose enough to
/// catch a near-singular matrix, far below any determinant a real fit produces.
const SINGULAR_DETERMINANT: f64 = 1e-12;

/// What another dataset says about a star, in its own frame and units.
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    /// The name it is known by there, matched against the catalog's.
    pub name: String,
    /// Its position in that dataset's frame, light years, origin at Sol.
    pub position: [f64; 3],
}

impl Reference {
    /// How far from the origin, light years.
    pub fn distance(&self) -> f64 {
        length(self.position)
    }
}

/// One star both datasets know about.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// The name matched on.
    pub name: String,
    /// What the catalog measures, light years.
    pub catalog_distance: f64,
    /// What the other dataset holds, light years.
    pub reference_distance: f64,
    /// How far apart the two bearings are after the fitted rotation, degrees.
    ///
    /// [`None`] until a frame has been fitted, and small for everything once
    /// one has — a bearing is the easy measurement. A large value here is a
    /// mismatched name or a star somebody placed rather than measured.
    pub bearing_error: Option<f64>,
}

/// How far a position is from the origin, light years.
fn length(v: [f64; 3]) -> f64 {
    (v.iter().map(|c| c * c).sum::<f64>()).sqrt()
}

impl Match {
    /// Signed difference in distance, light years: positive where the other
    /// dataset puts the star further away than the catalog does.
    pub fn distance_error(&self) -> f64 {
        self.reference_distance - self.catalog_distance
    }

    /// The same as a fraction of the catalog's distance.
    pub fn distance_error_fraction(&self) -> f64 {
        if self.catalog_distance > 0.0 {
            self.distance_error() / self.catalog_distance
        } else {
            0.0
        }
    }
}

/// What a comparison found.
#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    /// Every star both datasets carry, worst distance disagreement first.
    pub matches: Vec<Match>,
    /// Catalog stars the other dataset had no row for.
    pub unmatched: Vec<String>,
    /// The rotation carrying catalog bearings onto the other dataset's, if
    /// enough stars matched to fit one.
    pub frame: Option<Frame>,
}

/// The transform between two datasets' axes, recovered from matched stars.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Frame {
    /// The fitted matrix, as three row vectors.
    pub rotation: [[f64; 3]; 3],
    /// Its determinant: `+1` for a rotation, `-1` for a reflection.
    ///
    /// A reflection is not an error. It says the two datasets use opposite
    /// handedness, which is a convention rather than a disagreement about
    /// anything in the sky.
    pub determinant: f64,
    /// Median bearing disagreement after the fit, degrees. The figure that
    /// says whether the fit means anything.
    pub median_error: f64,
    /// How many stars it was fitted from.
    pub from: usize,
}

impl Frame {
    /// Whether the two datasets differ in handedness.
    pub fn is_mirrored(&self) -> bool {
        self.determinant < 0.0
    }
}

impl Comparison {
    /// The median absolute fractional disagreement in distance.
    ///
    /// The single figure for "how far apart are these two datasets", and it
    /// needs no frame: distance is invariant under any rotation or reflection
    /// between them.
    pub fn median_distance_error(&self) -> Option<f64> {
        median(self.matches.iter().map(|m| m.distance_error_fraction().abs()))
    }
}

/// Compare a catalog against another dataset, matching by name.
///
/// Matching is case- and space-insensitive and nothing cleverer: a star that
/// both datasets name the same way is the same star, and one that is not named
/// the same way is left unmatched rather than guessed at. A wrong match would
/// be indistinguishable in the output from a real disagreement, which is the
/// one failure this must not have.
pub fn compare(catalog: &[Star], reference: &[Reference]) -> Comparison {
    let key = |name: &str| name.trim().to_ascii_lowercase().replace(' ', "");
    let mut matches = Vec::new();
    let mut unmatched = Vec::new();
    let mut pairs = Vec::new();

    for star in catalog {
        let Some(name) = star.name.as_deref() else { continue };
        let found = reference.iter().find(|r| key(&r.name) == key(name));
        let Some(found) = found else {
            unmatched.push(name.to_owned());
            continue;
        };
        // The length of the catalog's own position, not its distance column,
        // so the report's noise floor is the other dataset's alone. See the
        // module docs.
        let catalog_distance = length(star.position);
        let reference_distance = found.distance();
        if catalog_distance <= 0.0 || reference_distance <= 0.0 {
            continue;
        }
        pairs.push((
            unit(star.position, catalog_distance),
            unit(found.position, reference_distance),
        ));
        matches.push(Match {
            name: name.to_owned(),
            catalog_distance,
            reference_distance,
            bearing_error: None,
        });
    }

    // The frame first, since every bearing error is measured after it.
    let frame = fit_frame(&pairs);
    if let Some(frame) = &frame {
        for (m, (from, to)) in matches.iter_mut().zip(&pairs) {
            m.bearing_error =
                Some(angle_between(rotate(&frame.rotation, *from), *to));
        }
    }

    matches.sort_by(|a, b| {
        b.distance_error_fraction()
            .abs()
            .total_cmp(&a.distance_error_fraction().abs())
    });
    Comparison { matches, unmatched, frame }
}

/// The orthogonal matrix that best carries the first bearings onto the second.
///
/// Wahba's problem. The correlation matrix `H = sum(to * from^T)` carries the
/// answer, and the orthogonal factor of its polar decomposition is it —
/// obtained here by Newton's iteration, `X <- (X + X^-T) / 2`, which converges
/// on the orthogonal part of any non-singular matrix in a handful of steps and
/// needs no singular value decomposition and so no dependency.
///
/// [`None`] under three matched stars, which cannot determine an orientation,
/// or where the iteration will not converge because the bearings are degenerate
/// — all collinear, say.
fn fit_frame(pairs: &[([f64; 3], [f64; 3])]) -> Option<Frame> {
    if pairs.len() < MIN_PAIRS_TO_FIT {
        return None;
    }
    let mut h = [[0.0; 3]; 3];
    for (from, to) in pairs {
        for r in 0..3 {
            for c in 0..3 {
                h[r][c] += to[r] * from[c];
            }
        }
    }

    let mut x = h;
    for _ in 0..MAX_POLAR_ITERATIONS {
        let inverse = invert(&x)?;
        let mut next = [[0.0; 3]; 3];
        let mut delta = 0.0f64;
        for r in 0..3 {
            for c in 0..3 {
                // The transpose of the inverse: index it the other way round.
                next[r][c] = 0.5 * (x[r][c] + inverse[c][r]);
                delta = delta.max((next[r][c] - x[r][c]).abs());
            }
        }
        x = next;
        if delta < POLAR_CONVERGENCE {
            break;
        }
    }

    let errors: Vec<f64> = pairs
        .iter()
        .map(|(from, to)| angle_between(rotate(&x, *from), *to))
        .collect();
    Some(Frame {
        rotation: x,
        determinant: determinant(&x),
        median_error: median(errors.iter().copied()).unwrap_or(f64::NAN),
        from: pairs.len(),
    })
}

fn unit(v: [f64; 3], length: f64) -> [f64; 3] {
    [v[0] / length, v[1] / length, v[2] / length]
}

/// The angle between two vectors, degrees.
fn angle_between(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dot: f64 = (0..3).map(|i| a[i] * b[i]).sum();
    let la = (a.iter().map(|c| c * c).sum::<f64>()).sqrt();
    let lb = (b.iter().map(|c| c * c).sum::<f64>()).sqrt();
    if la == 0.0 || lb == 0.0 {
        return f64::NAN;
    }
    (dot / (la * lb)).clamp(-1.0, 1.0).acos().to_degrees()
}

fn determinant(m: &[[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn invert(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = determinant(m);
    if det.abs() < SINGULAR_DETERMINANT {
        return None;
    }
    let c = |r: usize, k: usize| {
        let rows: Vec<usize> = (0..3).filter(|&i| i != r).collect();
        let cols: Vec<usize> = (0..3).filter(|&i| i != k).collect();
        let minor = m[rows[0]][cols[0]] * m[rows[1]][cols[1]]
            - m[rows[0]][cols[1]] * m[rows[1]][cols[0]];
        if (r + k) % 2 == 0 { minor } else { -minor }
    };
    let mut out = [[0.0; 3]; 3];
    for r in 0..3 {
        for k in 0..3 {
            // Adjugate is the transpose of the cofactors.
            out[r][k] = c(k, r) / det;
        }
    }
    Some(out)
}

fn median(values: impl IntoIterator<Item = f64>) -> Option<f64> {
    let mut v: Vec<f64> =
        values.into_iter().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{EQUATORIAL_TO_GALACTIC, rotate as turn};
    use crate::hyg;

    fn stars() -> Vec<Star> {
        hyg::read(include_str!("../data/bright.csv").as_bytes())
            .expect("a HYG catalog")
            .stars
    }

    /// Build a reference dataset from the catalog itself, put through a
    /// transform and a per-star distance change. The catalog is then being
    /// compared against a known distortion of itself, which is the only way to
    /// check that the comparison recovers what was done to it.
    fn distorted(
        stars: &[Star],
        matrix: [[f64; 3]; 3],
        scale: impl Fn(&Star) -> f64,
    ) -> Vec<Reference> {
        stars
            .iter()
            .filter_map(|s| {
                let name = s.name.clone()?;
                let turned = turn(&matrix, s.position);
                let k = scale(s);
                Some(Reference {
                    name,
                    position: [turned[0] * k, turned[1] * k, turned[2] * k],
                })
            })
            .collect()
    }

    const IDENTITY: [[f64; 3]; 3] =
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    /// **The frame is recovered, not assumed.** Rotate a whole catalog by a
    /// known matrix, hand both to the comparison, and it works out the matrix
    /// — leaving no bearing error behind, because there was no real
    /// disagreement to find.
    #[test]
    fn a_known_rotation_is_recovered_from_the_stars() {
        let stars = stars();
        let reference = distorted(&stars, EQUATORIAL_TO_GALACTIC, |_| 1.0);
        let comparison = compare(&stars, &reference);
        let frame = comparison.frame.expect("enough stars to fit a frame");

        for r in 0..3 {
            for c in 0..3 {
                assert!(
                    (frame.rotation[r][c] - EQUATORIAL_TO_GALACTIC[r][c]).abs()
                        < 1e-9,
                    "row {r} col {c}: {} against {}",
                    frame.rotation[r][c],
                    EQUATORIAL_TO_GALACTIC[r][c],
                );
            }
        }
        assert!(frame.median_error < 1e-9, "{}", frame.median_error);
        assert!(!frame.is_mirrored());
        assert!((frame.determinant - 1.0).abs() < 1e-9);
    }

    /// A dataset of the opposite handedness fits as a reflection, and that is
    /// reported rather than smeared into the residual. Elite's frame is
    /// left-handed, so this is the case the tool will actually meet.
    #[test]
    fn opposite_handedness_shows_up_as_a_reflection() {
        let stars = stars();
        // Flip one axis: a mirror, not a rotation.
        let mirror = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]];
        let comparison = compare(&stars, &distorted(&stars, mirror, |_| 1.0));
        let frame = comparison.frame.expect("a frame");
        assert!(frame.is_mirrored(), "determinant {}", frame.determinant);
        assert!((frame.determinant + 1.0).abs() < 1e-9);
        // And it still fits: a mirrored sky is not a disagreeing sky.
        assert!(frame.median_error < 1e-9);
    }

    /// **Distance disagreement survives any frame.** The same distance error,
    /// measured against a reference that has been rotated and one that has
    /// not, comes out identical — which is what makes distance the figure to
    /// trust when the frame is unknown.
    #[test]
    fn distance_error_does_not_depend_on_the_frame() {
        let stars = stars();
        let shrink = |s: &Star| if s.distance > 50.0 { 0.8 } else { 1.0 };
        let plain = compare(&stars, &distorted(&stars, IDENTITY, shrink));
        let turned =
            compare(&stars, &distorted(&stars, EQUATORIAL_TO_GALACTIC, shrink));

        assert_eq!(plain.matches.len(), turned.matches.len());
        // Paired by name rather than by position in the list: turning a
        // vector changes its length in the last bits, which is enough to
        // reorder stars whose disagreement is otherwise identical. That
        // reordering is the sort being honest about a tie, not a discrepancy.
        for a in &plain.matches {
            let b = turned
                .matches
                .iter()
                .find(|m| m.name == a.name)
                .expect("the same stars match either way");
            // Relative, not absolute: turning a 700 light year vector moves
            // its length by tens of nanometres of light year purely in the
            // arithmetic. Frame invariance holds to nine significant figures,
            // which is the claim worth making.
            let drift = (a.distance_error() - b.distance_error()).abs()
                / a.catalog_distance;
            assert!(
                drift < 1e-9,
                "{}: {} against {} ({drift:e} of the distance)",
                a.name,
                a.distance_error(),
                b.distance_error(),
            );
        }
    }

    /// A star moved in distance sorts to the top, and its error reads the way
    /// it was made: twenty per cent nearer than the catalog says.
    #[test]
    fn the_worst_disagreement_comes_first() {
        let stars = stars();
        let reference = distorted(&stars, IDENTITY, |s| {
            if s.name.as_deref() == Some("Rigel") { 0.8 } else { 1.0 }
        });
        let comparison = compare(&stars, &reference);
        let worst = &comparison.matches[0];
        assert_eq!(worst.name, "Rigel");
        assert!((worst.distance_error_fraction() + 0.2).abs() < 1e-9);
        assert!(worst.distance_error() < 0.0, "the reference put it nearer");
        // And it is a distance disagreement, not a bearing one.
        assert!(worst.bearing_error.expect("fitted") < 1e-9);
    }

    /// Where the two datasets agree, they agree: no disagreement is invented
    /// out of floating point.
    #[test]
    fn identical_datasets_disagree_about_nothing() {
        let stars = stars();
        let comparison = compare(&stars, &distorted(&stars, IDENTITY, |_| 1.0));
        assert!(comparison.unmatched.is_empty());
        assert!(comparison.median_distance_error().expect("matches") < 1e-12);
        for m in &comparison.matches {
            assert!(m.bearing_error.expect("fitted") < 1e-9);
        }
    }

    /// Names match across case and spacing, since two catalogs spell a star
    /// however they like, and a star the other dataset does not carry is
    /// reported as unmatched rather than guessed at. A wrong match would be
    /// indistinguishable in the output from a real disagreement.
    #[test]
    fn names_match_loosely_and_misses_are_reported() {
        let stars = stars();
        let reference = vec![
            Reference {
                name: "  sirius ".into(),
                position: named(&stars, "Sirius").position,
            },
            Reference {
                name: "RIGILKENTAURUS".into(),
                position: named(&stars, "Rigil Kentaurus").position,
            },
        ];
        let comparison = compare(&stars, &reference);
        let matched: Vec<&str> =
            comparison.matches.iter().map(|m| m.name.as_str()).collect();
        assert!(matched.contains(&"Sirius"));
        assert!(matched.contains(&"Rigil Kentaurus"));
        assert!(comparison.unmatched.contains(&"Vega".to_string()));
    }

    /// Two stars cannot say which way a sky is turned, so no frame is claimed
    /// from them — and the distance comparison still works without one.
    #[test]
    fn too_few_stars_fit_no_frame() {
        let stars = stars();
        let two: Vec<Star> =
            ["Sirius", "Vega"].iter().map(|n| named(&stars, n)).collect();
        let comparison = compare(&two, &distorted(&two, IDENTITY, |_| 1.0));
        assert!(comparison.frame.is_none());
        assert_eq!(comparison.matches.len(), 2);
        assert!(comparison.matches.iter().all(|m| m.bearing_error.is_none()));
    }

    /// The catalog's coordinates and its distance column are separately
    /// rounded and do not quite agree. Pinned because it is the noise floor
    /// under any comparison, and because it is the reason both sides of a
    /// distance error are measured off positions.
    #[test]
    fn the_catalogs_own_columns_disagree_a_little() {
        let stars = stars();
        let worst = stars
            .iter()
            .map(|s| ((length(s.position) - s.distance) / s.distance).abs())
            .fold(0.0f64, f64::max);
        assert!(worst > 0.0, "they would not need reconciling if they agreed");
        assert!(worst < 1e-2, "but they agree to within a per cent: {worst:e}");
    }

    fn named(stars: &[Star], name: &str) -> Star {
        stars
            .iter()
            .find(|s| s.name.as_deref() == Some(name))
            .expect("in the fixture")
            .clone()
    }
}
