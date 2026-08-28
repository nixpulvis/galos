//! How one star's light is spread across pixels.
//!
//! A star is a point. A picture of one is not, because every optic and every
//! eye smears a point into a small disc, and the shape of that smear is the
//! point-spread function. Drawing stars as single pixels gives a picture that
//! aliases into confetti when the camera turns and in which every star, from
//! Sirius to the faintest, is exactly as bright as every other. The PSF is
//! what makes a magnitude visible as a size.
//!
//! A Gaussian here, which is the standard approximation to atmospheric seeing
//! and close enough to a defocused optic. The one thing it must do is conserve
//! energy: a star's whole [`relative_exposure`](galos_photometry::relative_exposure)
//! is spread across the disc and none of it is created, so that two renderers
//! drawing the same sky can be compared by how much light landed and where.
//!
//! # Why bright stars are bigger
//!
//! Nothing here scales a radius by brightness. The radius falls out of the
//! energy: a Gaussian's wings fall off at a fixed rate, so a star carrying a
//! thousand times the energy has wings that stay above the floor about
//! `sqrt(2*ln(1000))` sigmas further out. Sirius draws as a disc and a
//! magnitude-six star as a dot for the same reason they look that way through
//! a telescope, rather than because a table said so.

/// The energy per pixel below which nothing is worth depositing.
///
/// A pixel holding less than this tone-maps to under one part in 255 and
/// cannot be seen, so the disc is cut where it falls below. Small enough that
/// the truncated tail is a fraction of a percent of a star's light, which is
/// what keeps the sum over an image close to the sum over the stars in it.
pub const FLOOR: f64 = 1e-4;

/// The largest radius a single star may spread over, pixels.
///
/// A cap on cost rather than on physics, and one that rarely binds: a
/// Gaussian's radius grows as the square root of the log of its energy, so
/// the disc is nearly self-limiting. At a seeing of two pixels even `1e30`
/// units of energy reaches only twenty-five pixels, and no representable
/// `f64` reaches this cap at all.
///
/// It binds where the PSF is wide. At a seeing of twenty pixels a bright star
/// passes it easily, and without it one overexposed star would cost more than
/// the rest of the catalog together.
pub const MAX_RADIUS: f64 = 96.0;

/// A Gaussian point-spread function of a given width.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Gaussian {
    /// The standard deviation, in pixels. About the radius at which a star's
    /// disc visibly ends is three of these.
    pub sigma: f64,
}

impl Gaussian {
    /// A PSF of the given width in pixels.
    pub fn new(sigma: f64) -> Gaussian {
        Gaussian { sigma: sigma.max(1e-3) }
    }

    /// The peak value at the centre of a star carrying `energy`.
    ///
    /// The normalization that conserves it: a two-dimensional Gaussian
    /// integrates to `2*pi*sigma^2`, so dividing by that makes the whole disc
    /// sum to the energy put in rather than to some multiple of it that
    /// depends on how wide the PSF happens to be.
    pub fn peak(&self, energy: f64) -> f64 {
        energy / (std::f64::consts::TAU * self.sigma * self.sigma)
    }

    /// The value `distance` pixels from the centre.
    pub fn at(&self, energy: f64, distance: f64) -> f64 {
        let s = distance / self.sigma;
        self.peak(energy) * (-0.5 * s * s).exp()
    }

    /// How far out this star is still worth drawing, pixels.
    ///
    /// Where the Gaussian falls to [`FLOOR`]. [`None`] when even the star's
    /// own centre is below the floor, which is the visibility cut: at a given
    /// exposure there is a magnitude past which a star contributes nothing any
    /// pixel can show, and finding it is a comparison rather than a table.
    pub fn radius(&self, energy: f64) -> Option<f64> {
        let peak = self.peak(energy);
        if peak <= FLOOR {
            return None;
        }
        let r = self.sigma * (2.0 * (peak / FLOOR).ln()).sqrt();
        Some(r.min(MAX_RADIUS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The disc holds the light put into it. Summed over a grid fine enough to
    /// catch the wings, a star's whole energy is there and no more — which is
    /// what lets two pictures of one sky be compared by how much light each
    /// holds.
    #[test]
    fn the_disc_conserves_energy() {
        let psf = Gaussian::new(2.0);
        let energy = 10.0;
        let r = psf.radius(energy).expect("a bright star has a radius");
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

    /// A brighter star spreads further, without anything scaling a radius by
    /// brightness: it is the wings clearing the floor for longer.
    #[test]
    fn brighter_stars_draw_wider() {
        let psf = Gaussian::new(1.5);
        let dim = psf.radius(0.1).expect("visible");
        let bright = psf.radius(100.0).expect("visible");
        assert!(bright > dim, "{bright} should exceed {dim}");
    }

    /// Past some faintness a star cannot be shown at all, and that is a
    /// comparison against the floor rather than a magnitude in a table.
    #[test]
    fn a_star_below_the_floor_has_no_disc() {
        let psf = Gaussian::new(1.5);
        assert_eq!(psf.radius(1e-9), None);
    }

    /// However bright, one star's cost is bounded — where the cap binds at
    /// all, which is at wide seeing.
    #[test]
    fn the_radius_is_capped_at_wide_seeing() {
        let psf = Gaussian::new(20.0);
        assert_eq!(psf.radius(1e12), Some(MAX_RADIUS));
    }

    /// At ordinary seeing the cap is unreachable, because the radius grows
    /// only as the square root of the log of the energy. Worth pinning: it
    /// says the disc size is self-limiting and the cap is a guard rather than
    /// a shape anybody sees.
    #[test]
    fn at_ordinary_seeing_the_disc_limits_itself() {
        let psf = Gaussian::new(2.0);
        let huge = psf.radius(1e300).expect("visible");
        assert!(huge < MAX_RADIUS, "{huge}");
        // Three hundred orders of magnitude of energy buys a factor of
        // eleven in size, which is the whole point.
        let ratio = huge / psf.radius(1.0).expect("visible");
        assert!(ratio < 12.0, "{ratio}");
    }
}
