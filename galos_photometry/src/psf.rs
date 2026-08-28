//! How a point of light lands on a detector.
//!
//! A star is a point and a picture of one is not. Every optic and every eye
//! smears a point into a small disc, and the shape of that smear is the
//! point-spread function. It belongs here rather than in a renderer for the
//! same reason [`relative_exposure`](crate::relative_exposure) does: it decides
//! **how much light lands and where**, which is the quantity two renderers of
//! the same sky have to agree on exactly. A rasterizer and a shader that
//! normalize a profile differently put different amounts of flux in the same
//! star, and a comparison between their pictures then measures the gap between
//! two instruments rather than between two renderings.
//!
//! The seam is drawn there and not at the crate boundary. What a renderer
//! keeps is how it *deposits* the profile — a loop over pixels, or a quad and a
//! fragment shader, or a texture baked once from [`Moffat::shape`] — and how it
//! compresses the result for a display. What it may not keep is its own idea of
//! the profile's shape or its normalization.
//!
//! A Moffat profile here, `(1 + (r/α)²)^(−β)`: a bright core with power-law
//! wings, the standard fit to a seeing-limited star. Closer to a real star
//! than a Gaussian, which falls off too fast to have the halo a bright star has.
//!
//! # Why bright stars are bigger
//!
//! Nothing here scales a radius by brightness. The radius falls out of the
//! energy: the wings fall off at a fixed rate, so a star carrying more energy
//! has wings that stay above a floor further out. Sirius draws as a disc and a
//! magnitude-six star as a dot for the same reason they look that way through a
//! telescope, rather than because a table said so. The Moffat's wings are
//! heavier than a Gaussian's — the radius grows as a power of the energy rather
//! than the square root of its log — which is the halo a bright star really has,
//! and the reason [`MAX_RADIUS`] is a guard rather than a formality.

/// The wing index `β` of the stellar point spread, shared so both renderers
/// wear one profile.
///
/// The `β` of `(1 + (r/α)²)^(−β)`: how heavy the wings are, lower spreading
/// more of a bright star's light into its halo. Two is the usual fit to a
/// seeing-limited star, and keeps the disc's energy finite (`β > 1`), which is
/// what lets [`Moffat::peak`] conserve it.
pub const STELLAR_BETA: f64 = 2.0;

/// The largest radius a single star may spread over, in the PSF's own units.
///
/// A cap on cost rather than on physics. The Moffat's radius grows as a power
/// of the energy — `(peak/floor)^(1/(2β))` in units of `α` — so unlike a
/// Gaussian it is not self-limiting, and one pathologically bright star over a
/// shallow floor would draw an unbounded disc without this. It does not bind
/// for real stars at ordinary settings; it is the guard that keeps a single
/// overexposed source from costing more than the rest of the catalog together,
/// a loop over pixels and a shader's quad alike.
pub const MAX_RADIUS: f64 = 96.0;

/// A Moffat point-spread function: a core of width `alpha` and wings of index
/// `beta`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Moffat {
    /// The core width `α`, in whatever unit the caller measures its image in —
    /// pixels, for the renderers here. The profile is at a quarter of its peak
    /// about `α` out, and its wings carry on well past that.
    pub alpha: f64,
    /// The wing index `β`; see [`STELLAR_BETA`]. Held above one so the disc
    /// holds finite energy.
    pub beta: f64,
}

impl Moffat {
    /// A PSF of the given core width and wing index.
    pub fn new(alpha: f64, beta: f64) -> Moffat {
        Moffat { alpha: alpha.max(1e-3), beta: beta.max(1.0 + 1e-6) }
    }

    /// The profile's shape, normalized to unit peak: one at the centre, falling
    /// away to nothing in the wings.
    ///
    /// The shape alone, with no energy in it — what a renderer bakes into a
    /// texture or evaluates in a shader. [`at`](Self::at) is this times the
    /// [`peak`](Self::peak) the energy comes to, and is where the shared
    /// normalization enters; this carries the profile's form and nothing else.
    pub fn shape(&self, distance: f64) -> f64 {
        let s = distance / self.alpha;
        (1.0 + s * s).powf(-self.beta)
    }

    /// The peak value at the centre of a star carrying `energy`.
    ///
    /// The normalization that conserves it: a Moffat integrates over the plane
    /// to `π α² / (β − 1)`, so dividing by that makes the whole disc sum to the
    /// energy put in rather than to some multiple of it that depends on how wide
    /// the PSF happens to be. **This is the figure two renderers must share.**
    /// Get it wrong in one of them and every star carries a different amount of
    /// light there, by a factor no comparison can see past.
    pub fn peak(&self, energy: f64) -> f64 {
        energy * (self.beta - 1.0)
            / (std::f64::consts::PI * self.alpha * self.alpha)
    }

    /// The value `distance` from the centre for a star carrying `energy`.
    pub fn at(&self, energy: f64, distance: f64) -> f64 {
        self.peak(energy) * self.shape(distance)
    }

    /// How far out this star is still worth drawing.
    ///
    /// Where the profile falls to `floor` — the energy per pixel below which the
    /// caller's display shows nothing. [`None`] when even the star's own centre
    /// is below it, which is the visibility cut: at a given exposure there is a
    /// magnitude past which a star contributes nothing any pixel can show, and
    /// finding it is a comparison rather than a table.
    ///
    /// The floor is a parameter rather than a constant because it is the one
    /// part of this that is legitimately the renderer's. It follows from the
    /// tone curve, and a CPU rasterizer with a film response and a GPU pipeline
    /// with its own tonemapper do not have the same one. The profile above is
    /// shared; where each stops drawing it is not.
    pub fn radius(&self, energy: f64, floor: f64) -> Option<f64> {
        let peak = self.peak(energy);
        if !(floor > 0.0) || peak <= floor {
            return None;
        }
        // shape = floor/peak at the edge, so (1 + (r/α)²) = (peak/floor)^(1/β).
        let ratio = (peak / floor).powf(1.0 / self.beta);
        let r = self.alpha * (ratio - 1.0).sqrt();
        Some(r.min(MAX_RADIUS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOOR: f64 = 1e-4;

    /// The disc holds the light put into it. Summed over a grid fine enough to
    /// catch the wings, a star's whole energy is there and no more — which is
    /// what lets two pictures of one sky be compared by how much light each
    /// holds, and so the single most load-bearing property here. The floor is
    /// set deep because the Moffat's power-law wings carry a little energy a
    /// long way out, and the tail past the radius is what a shallow floor would
    /// truncate.
    #[test]
    fn the_disc_conserves_energy() {
        let psf = Moffat::new(4.0, STELLAR_BETA);
        let energy = 10.0;
        let deep = 1e-6;
        let r = psf.radius(energy, deep).expect("a bright star has a radius");
        let mut total = 0.0;
        let n = r.ceil() as i64;
        for y in -n..=n {
            for x in -n..=n {
                let d = ((x * x + y * y) as f64).sqrt();
                if d <= r {
                    total += psf.at(energy, d);
                }
            }
        }
        assert!(
            (total - energy).abs() / energy < 0.02,
            "summed {total}, put in {energy}"
        );
    }

    /// Conservation does not depend on how wide the PSF is, which is what makes
    /// seeing a free dial rather than a brightness control.
    #[test]
    fn energy_is_conserved_at_every_width() {
        for alpha in [3.0, 5.0, 8.0] {
            let psf = Moffat::new(alpha, STELLAR_BETA);
            let energy = 10.0;
            let deep = 1e-6;
            let r = psf.radius(energy, deep).expect("visible");
            let n = r.ceil() as i64;
            let mut total = 0.0;
            for y in -n..=n {
                for x in -n..=n {
                    let d = ((x * x + y * y) as f64).sqrt();
                    if d <= r {
                        total += psf.at(energy, d);
                    }
                }
            }
            assert!(
                (total - energy).abs() / energy < 0.02,
                "alpha {alpha} summed {total}"
            );
        }
    }

    /// The shape carries the profile's form and no energy: unit at the centre,
    /// monotone outward, gone in the far wings. It is what a texture bakes.
    #[test]
    fn the_shape_is_unit_peak_and_falls_away() {
        let psf = Moffat::new(2.0, STELLAR_BETA);
        assert!((psf.shape(0.0) - 1.0).abs() < 1e-12);
        assert!(psf.shape(1.0) > psf.shape(3.0));
        assert!(psf.shape(3.0) > psf.shape(10.0));
        assert!(psf.shape(1e6) < 1e-6);
    }

    /// A brighter star spreads further, without anything scaling a radius by
    /// brightness: it is the wings clearing the floor for longer.
    #[test]
    fn brighter_stars_draw_wider() {
        let psf = Moffat::new(1.5, STELLAR_BETA);
        let dim = psf.radius(0.1, FLOOR).expect("visible");
        let bright = psf.radius(100.0, FLOOR).expect("visible");
        assert!(bright > dim, "{bright} should exceed {dim}");
    }

    /// Past some faintness a star cannot be shown at all, and that is a
    /// comparison against the floor rather than a magnitude in a table.
    #[test]
    fn a_star_below_the_floor_has_no_disc() {
        let psf = Moffat::new(1.5, STELLAR_BETA);
        assert_eq!(psf.radius(1e-9, FLOOR), None);
    }

    /// A renderer that shows fainter light draws every star wider, which is the
    /// whole of what the floor being a parameter buys.
    #[test]
    fn a_lower_floor_draws_wider() {
        let psf = Moffat::new(2.0, STELLAR_BETA);
        let shallow = psf.radius(1.0, 1e-2).expect("visible");
        let deep = psf.radius(1.0, 1e-6).expect("visible");
        assert!(deep > shallow, "{deep} should exceed {shallow}");
    }

    /// A nonsensical floor is no disc rather than a NaN radius.
    #[test]
    fn a_floor_at_or_below_zero_draws_nothing() {
        let psf = Moffat::new(2.0, STELLAR_BETA);
        assert_eq!(psf.radius(1.0, 0.0), None);
        assert_eq!(psf.radius(1.0, -1.0), None);
    }

    /// However bright, one star's cost is bounded by the cap. Unlike a
    /// Gaussian, the Moffat's power-law wings are not self-limiting, so an
    /// extreme energy reaches the cap and the guard is what holds it.
    #[test]
    fn the_radius_is_capped_for_extreme_energy() {
        let psf = Moffat::new(2.0, STELLAR_BETA);
        assert_eq!(psf.radius(1e300, FLOOR), Some(MAX_RADIUS));
    }

    /// At ordinary brightness the disc is a handful of pixels, well under the
    /// cap, so the cap is a guard rather than a shape anybody sees.
    #[test]
    fn an_ordinary_star_is_a_small_disc() {
        let psf = Moffat::new(2.0, STELLAR_BETA);
        let r = psf.radius(10.0, FLOOR).expect("visible");
        assert!(r < 30.0, "{r}");
    }
}
