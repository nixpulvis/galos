//! How a point of light lands on a detector.
//!
//! A star is a point and a picture of one is not. Every optic and every eye
//! smears a point into a small disc, and the shape of that smear is the
//! point-spread function. It belongs here rather than in a renderer for the
//! same reason [`relative_exposure`](crate::relative_exposure) does: it decides
//! **how much light lands and where**, which is the quantity two renderers of
//! the same sky have to agree on exactly. A rasterizer and a shader that
//! normalize a Gaussian differently put different amounts of flux in the same
//! star, and a comparison between their pictures then measures the gap between
//! two instruments rather than between two renderings.
//!
//! The seam is drawn there and not at the crate boundary. What a renderer
//! keeps is how it *deposits* the profile — a loop over pixels, or a quad and
//! a fragment shader — and how it compresses the result for a display. What it
//! may not keep is its own idea of the profile's shape or its normalization.
//!
//! A Gaussian here, the standard approximation to atmospheric seeing and close
//! enough to a defocused optic.
//!
//! # Why bright stars are bigger
//!
//! Nothing here scales a radius by brightness. The radius falls out of the
//! energy: a Gaussian's wings fall off at a fixed rate, so a star carrying a
//! thousand times the energy has wings that stay above a floor about
//! `sqrt(2*ln(1000))` sigmas further out. Sirius draws as a disc and a
//! magnitude-six star as a dot for the same reason they look that way through
//! a telescope, rather than because a table said so.

/// The largest radius a single star may spread over, in the PSF's own units.
///
/// A cap on cost rather than on physics, and one that rarely binds: a
/// Gaussian's radius grows as the square root of the log of its energy, so the
/// disc is nearly self-limiting. At a width of two pixels even `1e30` units of
/// energy reaches only twenty-five, and no representable `f64` reaches this cap
/// at all.
///
/// It binds where the PSF is wide. At a width of twenty pixels a bright star
/// passes it easily, and without it one overexposed star would cost more than
/// the rest of the catalog together — a loop over pixels and a shader's quad
/// alike.
pub const MAX_RADIUS: f64 = 96.0;

/// A Gaussian point-spread function of a given width.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Gaussian {
    /// The standard deviation, in whatever unit the caller measures its image
    /// in — pixels, for both renderers here. A star's disc visibly ends at
    /// about three of these.
    pub sigma: f64,
}

impl Gaussian {
    /// A PSF of the given width.
    pub fn new(sigma: f64) -> Gaussian {
        Gaussian { sigma: sigma.max(1e-3) }
    }

    /// The peak value at the centre of a star carrying `energy`.
    ///
    /// The normalization that conserves it: a two-dimensional Gaussian
    /// integrates to `2*pi*sigma^2`, so dividing by that makes the whole disc
    /// sum to the energy put in rather than to some multiple of it that
    /// depends on how wide the PSF happens to be. **This is the figure two
    /// renderers must share.** Get it wrong in one of them and every star
    /// carries a different amount of light there, by a factor no comparison
    /// can see past.
    pub fn peak(&self, energy: f64) -> f64 {
        energy / (std::f64::consts::TAU * self.sigma * self.sigma)
    }

    /// The value `distance` from the centre.
    pub fn at(&self, energy: f64, distance: f64) -> f64 {
        let s = distance / self.sigma;
        self.peak(energy) * (-0.5 * s * s).exp()
    }

    /// How far out this star is still worth drawing.
    ///
    /// Where the Gaussian falls to `floor` — the energy per pixel below which
    /// the caller's display shows nothing. [`None`] when even the star's own
    /// centre is below it, which is the visibility cut: at a given exposure
    /// there is a magnitude past which a star contributes nothing any pixel can
    /// show, and finding it is a comparison rather than a table.
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
        let r = self.sigma * (2.0 * (peak / floor).ln()).sqrt();
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
    /// holds, and so the single most load-bearing property here.
    #[test]
    fn the_disc_conserves_energy() {
        let psf = Gaussian::new(2.0);
        let energy = 10.0;
        let r = psf.radius(energy, FLOOR).expect("a bright star has a radius");
        let mut total = 0.0;
        let n = r.ceil() as i64;
        for y in -n..=n {
            for x in -n..=n {
                let d = ((x * x + y * y) as f64).sqrt();
                total += psf.at(energy, d);
            }
        }
        assert!(
            (total - energy).abs() / energy < 0.01,
            "summed {total}, put in {energy}"
        );
    }

    /// Conservation does not depend on how wide the PSF is, which is what
    /// makes seeing a free dial rather than a brightness control.
    #[test]
    fn energy_is_conserved_at_every_width() {
        for sigma in [1.0, 2.0, 4.0, 8.0] {
            let psf = Gaussian::new(sigma);
            let energy = 10.0;
            let r = psf.radius(energy, FLOOR).expect("visible");
            let n = r.ceil() as i64;
            let mut total = 0.0;
            for y in -n..=n {
                for x in -n..=n {
                    total += psf.at(energy, ((x * x + y * y) as f64).sqrt());
                }
            }
            assert!(
                (total - energy).abs() / energy < 0.02,
                "sigma {sigma} summed {total}"
            );
        }
    }

    /// A brighter star spreads further, without anything scaling a radius by
    /// brightness: it is the wings clearing the floor for longer.
    #[test]
    fn brighter_stars_draw_wider() {
        let psf = Gaussian::new(1.5);
        let dim = psf.radius(0.1, FLOOR).expect("visible");
        let bright = psf.radius(100.0, FLOOR).expect("visible");
        assert!(bright > dim, "{bright} should exceed {dim}");
    }

    /// Past some faintness a star cannot be shown at all, and that is a
    /// comparison against the floor rather than a magnitude in a table.
    #[test]
    fn a_star_below_the_floor_has_no_disc() {
        let psf = Gaussian::new(1.5);
        assert_eq!(psf.radius(1e-9, FLOOR), None);
    }

    /// A renderer that shows fainter light draws every star wider, which is
    /// the whole of what the floor being a parameter buys.
    #[test]
    fn a_lower_floor_draws_wider() {
        let psf = Gaussian::new(2.0);
        let shallow = psf.radius(1.0, 1e-2).expect("visible");
        let deep = psf.radius(1.0, 1e-6).expect("visible");
        assert!(deep > shallow, "{deep} should exceed {shallow}");
    }

    /// A nonsensical floor is no disc rather than a NaN radius.
    #[test]
    fn a_floor_at_or_below_zero_draws_nothing() {
        let psf = Gaussian::new(2.0);
        assert_eq!(psf.radius(1.0, 0.0), None);
        assert_eq!(psf.radius(1.0, -1.0), None);
    }

    /// However bright, one star's cost is bounded — where the cap binds at
    /// all, which is at wide seeing.
    #[test]
    fn the_radius_is_capped_at_wide_seeing() {
        let psf = Gaussian::new(20.0);
        assert_eq!(psf.radius(1e12, FLOOR), Some(MAX_RADIUS));
    }

    /// At ordinary seeing the cap is unreachable, because the radius grows
    /// only as the square root of the log of the energy. Worth pinning: it
    /// says the disc size is self-limiting and the cap is a guard rather than
    /// a shape anybody sees.
    #[test]
    fn at_ordinary_seeing_the_disc_limits_itself() {
        let psf = Gaussian::new(2.0);
        let huge = psf.radius(1e300, FLOOR).expect("visible");
        assert!(huge < MAX_RADIUS, "{huge}");
        // Three hundred orders of magnitude of energy buys a factor of
        // eleven in size, which is the whole point.
        let ratio = huge / psf.radius(1.0, FLOOR).expect("visible");
        assert!(ratio < 12.0, "{ratio}");
    }
}
