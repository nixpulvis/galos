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
//! Two profile shapes, chosen through [`Kernel`]: a [`Moffat`],
//! `(1 + (r/α)²)^(−β)`, a bright core with power-law wings, and a [`Gaussian`],
//! `exp(−r²/2σ²)`. The Moffat is the default and the truer of the two — a real
//! star has the halo its wings give and the Gaussian has none — but the
//! Gaussian is the standard seeing approximation and cheaper to reason about,
//! so both are offered and a renderer picks.
//!
//! A single profile is not the whole of a real star, though. Its core is the
//! seeing disc, but a bright star also wears a broad, faint *aureole* — the
//! halo scattering in the air and the optics throws around it, falling off far
//! more slowly than any core. One profile cannot be both a tight core and a
//! broad halo at once, so a [`Psf`] is a *stack* of [`Layer`]s: a base kernel
//! and however many broader ones laid behind it, each carrying a share of the
//! light. [`AUREOLE_WEIGHT`] and its neighbours are the first such layer,
//! measured from a real photograph; a renderer adds more until the star reads
//! right.
//!
//! # Why bright stars are bigger
//!
//! Nothing here scales a radius by brightness. The radius falls out of the
//! energy: the wings fall off at a fixed rate, so a star carrying more energy
//! has wings that stay above a floor further out. Sirius draws as a disc and a
//! magnitude-six star as a dot for the same reason they look that way through a
//! telescope, rather than because a table said so. The Moffat's wings are
//! heavier than a Gaussian's — its radius grows as a power of the energy where
//! the Gaussian's grows as the square root of its log — which is the halo a
//! bright star really has, and the reason [`MAX_RADIUS`] is a guard for the
//! Moffat rather than a formality.

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

/// The wing index `β` of a star's aureole: the broad halo, far shallower than
/// the seeing core's [`STELLAR_BETA`].
///
/// Fitting the halo of a bright star in a real wide-field photograph gives a
/// falloff near `r^-2` — a Moffat `β` near one. This sits above that at one and
/// a half: heavy enough to read as a broad halo, steep enough that
/// [`MAX_RADIUS`] does not truncate too much of its light, and well over one so
/// the aureole's share stays finite, the same condition [`STELLAR_BETA`] meets
/// for the core.
pub const AUREOLE_BETA: f64 = 1.5;

/// How much broader the aureole's core is than the seeing disc, as a multiple
/// of the seeing width. The halo is a wide, soft thing, so it starts several
/// times the core out and its wings carry from there.
pub const AUREOLE_WIDTH: f64 = 4.5;

/// The share of a star's light in its aureole rather than its core.
///
/// About a quarter: enough that a bright star's halo climbs well clear of the
/// display floor and reads as a real glow, and little enough that the core
/// still dominates the centre and a faint star — a quarter of almost nothing —
/// stays a bare point. Fitted to the reference photograph's brightest stars.
pub const AUREOLE_WEIGHT: f64 = 0.25;

/// Which point-spread profile an instrument wears.
///
/// The shape a renderer deposits, chosen through [`Kernel::new`]. Both are
/// energy-conserving and share [`MAX_RADIUS`]; they differ in the wings, which
/// is what a bright star's halo is made of.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Profile {
    /// `exp(−r²/2σ²)`: a core and effectively no wings, the standard seeing
    /// approximation.
    Gaussian,
    /// `(1 + (r/α)²)^(−β)`: a core with heavy power-law wings, the fit a real
    /// star wears. The default.
    #[default]
    Moffat,
}

impl Profile {
    /// Both profiles, the default first, for a caller offering the choice.
    pub const ALL: [Profile; 2] = [Profile::Moffat, Profile::Gaussian];

    /// The profile's name, for a label or a command line.
    pub fn name(self) -> &'static str {
        match self {
            Profile::Gaussian => "Gaussian",
            Profile::Moffat => "Moffat",
        }
    }
}

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

/// A Gaussian point-spread function of a given width.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Gaussian {
    /// The standard deviation `σ`, in whatever unit the caller measures its
    /// image in — pixels, for the renderers here. A star's disc visibly ends
    /// at about three of these.
    pub sigma: f64,
}

impl Gaussian {
    /// A PSF of the given width.
    pub fn new(sigma: f64) -> Gaussian {
        Gaussian { sigma: sigma.max(1e-3) }
    }

    /// The profile's shape, normalized to unit peak; see [`Moffat::shape`].
    pub fn shape(&self, distance: f64) -> f64 {
        let s = distance / self.sigma;
        (-0.5 * s * s).exp()
    }

    /// The peak value at the centre of a star carrying `energy`.
    ///
    /// A two-dimensional Gaussian integrates to `2π σ²`, so dividing by that
    /// conserves the energy put in; see [`Moffat::peak`] for why this is the
    /// figure two renderers must share.
    pub fn peak(&self, energy: f64) -> f64 {
        energy / (std::f64::consts::TAU * self.sigma * self.sigma)
    }

    /// The value `distance` from the centre for a star carrying `energy`.
    pub fn at(&self, energy: f64, distance: f64) -> f64 {
        self.peak(energy) * self.shape(distance)
    }

    /// How far out this star is still worth drawing; see [`Moffat::radius`].
    ///
    /// The Gaussian falls to `floor` at `σ·√(2·ln(peak/floor))`, which grows as
    /// the square root of the log of the energy — self-limiting, so the cap
    /// rarely binds.
    pub fn radius(&self, energy: f64, floor: f64) -> Option<f64> {
        let peak = self.peak(energy);
        if !(floor > 0.0) || peak <= floor {
            return None;
        }
        let r = self.sigma * (2.0 * (peak / floor).ln()).sqrt();
        Some(r.min(MAX_RADIUS))
    }
}

/// One layer's shape: a single normalized profile of either [`Profile`].
///
/// The old whole of a point spread, now one voice in it. It dispatches the
/// shared quantities — [`shape`](Self::shape), [`peak`](Self::peak),
/// [`at`](Self::at), [`radius`](Self::radius) — to the profile it wears, and a
/// [`Psf`] is one or more of these stacked. A renderer that wants a plain
/// seeing disc holds a [`Psf`] of a single kernel; one that wants a star's real
/// halo layers a broad one behind a tight one.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Kernel {
    Gaussian(Gaussian),
    Moffat(Moffat),
}

impl Kernel {
    /// A kernel of the given profile and width, in pixels.
    ///
    /// The width is the Gaussian's `σ` or the Moffat's `α`; both are the core's
    /// scale, so one "seeing" dial drives either. The Moffat takes the shared
    /// [`STELLAR_BETA`] for its wings.
    pub fn new(profile: Profile, width: f64) -> Kernel {
        match profile {
            Profile::Gaussian => Kernel::Gaussian(Gaussian::new(width)),
            Profile::Moffat => Kernel::Moffat(Moffat::new(width, STELLAR_BETA)),
        }
    }

    /// A Moffat kernel of an explicit width and wing index — the form a broad,
    /// heavy-winged aureole is built with, where [`STELLAR_BETA`] is too steep.
    pub fn moffat(alpha: f64, beta: f64) -> Kernel {
        Kernel::Moffat(Moffat::new(alpha, beta))
    }

    /// Which profile this is.
    pub fn profile(&self) -> Profile {
        match self {
            Kernel::Gaussian(_) => Profile::Gaussian,
            Kernel::Moffat(_) => Profile::Moffat,
        }
    }

    /// The unit-peak shape at `distance`.
    pub fn shape(&self, distance: f64) -> f64 {
        match self {
            Kernel::Gaussian(g) => g.shape(distance),
            Kernel::Moffat(m) => m.shape(distance),
        }
    }

    /// The peak of a star carrying `energy`.
    pub fn peak(&self, energy: f64) -> f64 {
        match self {
            Kernel::Gaussian(g) => g.peak(energy),
            Kernel::Moffat(m) => m.peak(energy),
        }
    }

    /// The value `distance` from the centre of a star carrying `energy`.
    pub fn at(&self, energy: f64, distance: f64) -> f64 {
        match self {
            Kernel::Gaussian(g) => g.at(energy, distance),
            Kernel::Moffat(m) => m.at(energy, distance),
        }
    }

    /// The radius a star carrying `energy` clears above `floor`.
    pub fn radius(&self, energy: f64, floor: f64) -> Option<f64> {
        match self {
            Kernel::Gaussian(g) => g.radius(energy, floor),
            Kernel::Moffat(m) => m.radius(energy, floor),
        }
    }
}

/// One layer of a point spread: a [`Kernel`] carrying a share of the light.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Layer {
    /// The shape this layer deposits.
    pub kernel: Kernel,
    /// Its weight, relative to the other layers. A [`Psf`] reads these as
    /// proportions — a base of one and an aureole of `0.06` puts a sixteenth of
    /// the light in the halo — and normalizes by their sum when it draws, so
    /// the value is a ratio and the order layers were added in does not matter.
    pub weight: f64,
}

/// A point spread built from one or more [`Layer`]s.
///
/// A real star is not one profile. Its core is the seeing disc — a tight
/// [`Moffat`] — but a bright one also wears a broad, faint *aureole*, the
/// light scattering in the air and the optics throws into a halo that falls off
/// far more slowly than any single core. One Moffat cannot be both: a wing
/// index steep enough for the core is far too steep for the halo, and one
/// shallow enough for the halo carries infinite energy. So a [`Psf`] is a
/// *stack* — a base profile and however many broader layers are laid behind it
/// until the star reads right — each carrying a share of the flux and each
/// conserving its own share, so the whole still conserves energy.
///
/// Weights are relative and normalized by their sum at every query, so
/// [`with_layer`](Self::with_layer) composes in any order and adding one layer
/// never rescales another's stored weight. Everything stays linear in energy,
/// so the stack is still separable: [`at`] is [`peak`] times a fixed [`shape`]
/// exactly as a single kernel is, which is what lets a GPU still bake one
/// texture from [`shape`] and scale it by [`peak`].
///
/// [`at`]: Self::at
/// [`peak`]: Self::peak
/// [`shape`]: Self::shape
#[derive(Clone, Debug, PartialEq)]
pub struct Psf {
    layers: Vec<Layer>,
}

impl Psf {
    /// A single-layer PSF: a plain seeing disc of the given profile and width.
    ///
    /// The whole of the old behaviour, and what a renderer that wants no halo
    /// still holds.
    pub fn new(profile: Profile, width: f64) -> Psf {
        Psf::of(Kernel::new(profile, width))
    }

    /// A single-layer PSF around an explicit kernel.
    pub fn of(kernel: Kernel) -> Psf {
        Psf { layers: vec![Layer { kernel, weight: 1.0 }] }
    }

    /// Lay another kernel behind this one, carrying `weight` of the light
    /// relative to the layers already present.
    ///
    /// The base is weight one, so `with_layer(halo, 0.06)` puts `0.06 / 1.06` of
    /// the light — about a sixteenth — in the halo and leaves the rest in the
    /// core, and a second `with_layer` behind that shares out against the same
    /// running total rather than rescaling what came before. This is the "layer
    /// until happy" seam.
    pub fn with_layer(mut self, kernel: Kernel, weight: f64) -> Psf {
        self.layers.push(Layer { kernel, weight: weight.max(0.0) });
        self
    }

    /// The layers this PSF is built from, base first. Their weights are
    /// relative; divide by their sum for each one's share.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// The sum the relative weights are read against.
    fn total_weight(&self) -> f64 {
        self.layers.iter().map(|l| l.weight).sum()
    }

    /// The base layer's profile — which profile the core wears.
    pub fn profile(&self) -> Profile {
        self.layers[0].kernel.profile()
    }

    /// The unit-peak shape at `distance`: the whole stack, normalized so the
    /// centre is one. Separable from energy, so a texture can bake it once.
    pub fn shape(&self, distance: f64) -> f64 {
        let numerator: f64 = self
            .layers
            .iter()
            .map(|l| l.weight * l.kernel.peak(1.0) * l.kernel.shape(distance))
            .sum();
        let denominator: f64 =
            self.layers.iter().map(|l| l.weight * l.kernel.peak(1.0)).sum();
        if denominator > 0.0 { numerator / denominator } else { 0.0 }
    }

    /// The peak at the centre of a star carrying `energy`: the layers' peaks,
    /// each for its share, summed.
    pub fn peak(&self, energy: f64) -> f64 {
        let total = self.total_weight();
        if total <= 0.0 {
            return 0.0;
        }
        self.layers.iter().map(|l| l.kernel.peak(l.weight / total * energy)).sum()
    }

    /// The value `distance` from the centre of a star carrying `energy`: the
    /// layers added, since incoherent light sums.
    pub fn at(&self, energy: f64, distance: f64) -> f64 {
        let total = self.total_weight();
        if total <= 0.0 {
            return 0.0;
        }
        self.layers
            .iter()
            .map(|l| l.kernel.at(l.weight / total * energy, distance))
            .sum()
    }

    /// How far out the star is still worth drawing: the furthest any layer
    /// reaches above `floor`, since past that even the broadest is invisible.
    ///
    /// The broadest layer sets the edge and the tighter ones have long since
    /// fallen to nothing there, so their contribution at that radius is below a
    /// pixel's worth and the furthest single layer is the edge to a fraction of
    /// one. [`None`] when no layer clears the floor — the visibility cut.
    pub fn radius(&self, energy: f64, floor: f64) -> Option<f64> {
        let total = self.total_weight();
        if total <= 0.0 {
            return None;
        }
        self.layers
            .iter()
            .filter_map(|l| l.kernel.radius(l.weight / total * energy, floor))
            .fold(None, |acc, r| Some(acc.map_or(r, |a: f64| a.max(r))))
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

    /// The Gaussian holds the light put into it too, and needs no deep floor to
    /// do it: its wings fall off fast enough that the tail past the radius is
    /// nothing.
    #[test]
    fn a_gaussian_disc_conserves_energy() {
        let psf = Gaussian::new(2.0);
        let energy = 10.0;
        let r = psf.radius(energy, FLOOR).expect("visible");
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
        assert!((total - energy).abs() / energy < 0.01, "summed {total}");
    }

    /// A [`Psf`] wears the profile it is handed and dispatches to it. The two
    /// concentrate a star's light differently at one width, so their peaks
    /// differ — which is the whole of why the choice is worth offering.
    #[test]
    fn a_psf_wears_the_profile_it_is_given() {
        let g = Psf::new(Profile::Gaussian, 2.0);
        let m = Psf::new(Profile::Moffat, 2.0);
        assert_eq!(g.profile(), Profile::Gaussian);
        assert_eq!(m.profile(), Profile::Moffat);
        assert!(g.peak(10.0) != m.peak(10.0), "the profiles concentrate alike");
        assert!((g.shape(0.0) - 1.0).abs() < 1e-12);
        assert!((m.shape(0.0) - 1.0).abs() < 1e-12);
        assert!(g.shape(5.0) > 0.0 && m.shape(5.0) > 0.0);
    }

    /// A stacked PSF still holds the light put into it: the layers split the
    /// energy and each conserves its share, so the whole conserves it. Uses an
    /// explicit light aureole rather than the shipping default so it measures
    /// the mechanism; a heavy halo's wing loses a few percent more past
    /// [`MAX_RADIUS`], which the tolerance leaves room for.
    #[test]
    fn a_layered_psf_conserves_energy() {
        let psf = Psf::new(Profile::Moffat, 4.0)
            .with_layer(Kernel::moffat(20.0, 1.4), 0.1);
        let energy = 100.0;
        let r = psf.radius(energy, 1e-6).expect("a bright star has a radius");
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
        assert!((total - energy).abs() / energy < 0.04, "summed {total}");
    }

    /// The point of layering: an aureole is a halo the core does not have. Far
    /// out, where a bare Moffat core has fallen to almost nothing, the stack is
    /// several times brighter — that extra light is the halo — while at the
    /// centre the two are all but identical, since the aureole carries little
    /// and spreads it wide.
    #[test]
    fn the_aureole_is_a_halo_the_core_lacks() {
        let core = Psf::new(Profile::Moffat, 4.0);
        let stack = core.clone().with_layer(Kernel::moffat(20.0, 1.4), 0.1);
        let energy = 100.0;
        assert!(
            (stack.peak(energy) - core.peak(energy)).abs() / core.peak(energy)
                < 0.1,
            "the centre barely moves"
        );
        assert!(
            stack.at(energy, 60.0) > 2.0 * core.at(energy, 60.0),
            "far out the stack is a halo the core lacks"
        );
    }

    /// The aureole decouples core from halo: it broadens a bright star's reach
    /// but leaves a faint one's alone, because a few percent of almost nothing
    /// stays under the floor and never draws.
    #[test]
    fn the_aureole_reaches_only_for_the_bright() {
        let core = Psf::new(Profile::Moffat, 4.0);
        let stack = core.clone().with_layer(Kernel::moffat(20.0, 1.4), 0.1);
        let bright = 100.0;
        assert!(
            stack.radius(bright, FLOOR).unwrap()
                > core.radius(bright, FLOOR).unwrap(),
            "a bright star grows a halo"
        );
        let faint = 1.0;
        let (sf, cf) = (
            stack.radius(faint, FLOOR).unwrap(),
            core.radius(faint, FLOOR).unwrap(),
        );
        assert!((sf - cf).abs() < 1.0, "a faint star is unchanged: {sf} vs {cf}");
    }

    /// A stack is still linear in energy, so it stays separable: the value at a
    /// distance is the peak times a fixed shape, which is what lets a GPU bake
    /// one texture from [`Psf::shape`] and scale it by [`Psf::peak`].
    #[test]
    fn a_stack_stays_separable() {
        let psf = Psf::new(Profile::Moffat, 3.0)
            .with_layer(Kernel::moffat(12.0, AUREOLE_BETA), 0.1);
        for &d in &[0.0, 2.5, 9.0, 25.0] {
            let expected = psf.peak(10.0) * psf.shape(d);
            assert!((psf.at(10.0, d) - expected).abs() < 1e-9, "at {d}");
        }
    }

    /// Weights are relative shares: the base stays one whatever is layered on,
    /// and a layer's fraction of the light is its weight over their sum — so a
    /// `0.05` aureole behind a base of one carries `0.05 / 1.05` of the flux.
    #[test]
    fn weights_are_relative_shares() {
        let psf = Psf::new(Profile::Moffat, 4.0)
            .with_layer(Kernel::moffat(16.0, AUREOLE_BETA), 0.05);
        assert_eq!(psf.layers()[0].weight, 1.0, "the base is untouched");
        assert_eq!(psf.layers()[1].weight, 0.05);
        // The halo's share of the energy is its weight over the total.
        let share = psf.layers()[1].weight
            / psf.layers().iter().map(|l| l.weight).sum::<f64>();
        assert!((share - 0.05 / 1.05).abs() < 1e-12);
    }
}
