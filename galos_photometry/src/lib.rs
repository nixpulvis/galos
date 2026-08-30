//! The physics of how bright a star is and what colour it comes to.
//!
//! Everything the galaxy is drawn from rests on one ordering, by absolute
//! magnitude, and one colour, from temperature. Both are physics rather than
//! taste: a bright giant five thousand light years out belongs in the sky
//! while a hundred dim dwarfs nearby do not, and a star's tint is the tint of
//! a blackbody at its surface heat. So the quantities here are claims about the
//! world, each a named type with a test that reads as one.
//!
//! The vocabulary is the physics. A [`Magnitude`] is a brightness on the
//! logarithmic scale the sky is spoken in; a [`Flux`] is the linear light it
//! stands for, the thing that adds when sources combine; a [`Temperature`] is a
//! surface heat, which fixes a [`Color`]; a [`Distance`] carries its own unit so
//! the light-year map and the parsec physics meet without a bare conversion at
//! every site. The bare numbers stay at the edges — a caller wraps an `f64`
//! going in and reads the field coming out — and in between the units cannot be
//! confused for one another.
//!
//! Two claims the map cannot compute for itself. Ordering by absolute magnitude
//! needs a magnitude for every system, including the two thirds that carry no
//! scanned star, and colour needs a temperature. Where a scan is absent the
//! class the system is named for is all there is, so [`ClassLight::of`] turns
//! that class into a typical magnitude and heat. It is the last link in the
//! bake's fallback chain — scanned stars first, then this — and it is what lets
//! a system with nothing recorded but its primary's letter still take its place
//! in the ordering.
//!
//! Nothing here knows about the database or the renderer. A [`Color`] is linear
//! RGB as `[f32; 3]`, and the caller converts to whatever it draws in.
//!
//! # The instrument
//!
//! Three of these are not about the light but about what receives it:
//! [`Magnitude::EYE_LIMIT`], a detector's faintest; [`Magnitude::exposure`],
//! how much of a star's flux an exposure collects; and [`psf`], where on the
//! detector it lands. They are here rather than in a renderer because they
//! decide **how much light lands and where**, and that is the one quantity two
//! renderers of the same sky must agree on exactly — a rasterizer and a shader
//! with different normalizations put different amounts of flux in the same star,
//! and no comparison between their pictures can see past it.
//!
//! What stays with a renderer is how it *deposits* the profile — a loop over
//! pixels, or a quad and a fragment shader — and how it compresses the result
//! for a display. Those legitimately differ, which is why the tone curve is not
//! here and why the PSF takes its cutoff as a parameter rather than a
//! constant.

pub mod psf;

/// Light years to a parsec.
///
/// The definition of the parsec, `648000 / PI` astronomical units, worked
/// through the AU and the light year in metres. Photometry is written in
/// parsecs because the distance modulus is, and the map stands in light years,
/// so one or the other is always being converted.
pub const LY_PER_PARSEC: f64 = 3.261_563_777_167_433;

/// Parsecs to a light year, the reciprocal of [`LY_PER_PARSEC`].
pub const PARSECS_PER_LY: f64 = 1.0 / LY_PER_PARSEC;

/// A distance to a star, carrying its own unit.
///
/// The map holds light years everywhere and the distance modulus is written in
/// parsecs, so one or the other is always being converted. Naming the unit at
/// the point a distance is made — [`Distance::light_years`] or
/// [`Distance::parsecs`] — folds that conversion into one place and keeps a
/// light-year figure from being read as a parsec one at the call that would
/// most quietly go wrong.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Distance(f64);

impl Distance {
    /// A distance given in parsecs, the unit the distance modulus reads.
    pub const fn parsecs(pc: f64) -> Distance {
        Distance(pc)
    }

    /// A distance given in light years, the unit the map stands in.
    pub const fn light_years(ly: f64) -> Distance {
        Distance(ly * PARSECS_PER_LY)
    }

    /// This distance in parsecs.
    pub const fn as_parsecs(self) -> f64 {
        self.0
    }

    /// This distance in light years.
    pub const fn as_light_years(self) -> f64 {
        self.0 * LY_PER_PARSEC
    }
}

/// A brightness on the magnitude scale: logarithmic, and inverted, so a smaller
/// number is a brighter star.
///
/// The one ordering the whole galaxy is drawn from. Magnitudes are what do not
/// add — combining light means going through [`flux`](Magnitude::flux) — so this
/// carries the scale the sky is spoken in and hands off to [`Flux`] wherever
/// light has to be summed.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Magnitude(pub f64);

impl Magnitude {
    /// The faintest a dark-adapted eye reaches, in space with no air to dim it.
    ///
    /// The cut that decides how many stars a sky holds, roughly ten to fifty
    /// thousand across a whole one. Below it a star is there in the record and
    /// in the glow but is not drawn as itself.
    pub const EYE_LIMIT: Magnitude = Magnitude(8.0);

    /// A magnitude so faint no eye or sensor reads it as anything, its flux the
    /// sky rounds to nothing. What a body with no visible light comes to — a
    /// black hole, whose temperature is zero — and the floor any darkened scan
    /// lands on.
    pub const DARK: Magnitude = Magnitude(40.0);

    /// The Sun's absolute visual magnitude, the zero the sequence is hung from.
    pub const SOLAR_ABSOLUTE: Magnitude = Magnitude(4.83);

    /// The relative flux of this magnitude, with magnitude zero as one unit.
    ///
    /// `10^(-0.4*m)`, the inverse of the magnitude scale being logarithmic:
    /// five magnitudes is a factor of a hundred, so a magnitude-five star
    /// carries a hundredth the flux of a magnitude-zero one. Flux is what adds,
    /// and magnitudes are what do not, so anything that combines light converts
    /// to this first.
    pub fn flux(self) -> Flux {
        Flux(10f64.powf(-POGSON_EXPONENT * self.0))
    }

    /// How bright this absolute magnitude looks from `distance` away.
    ///
    /// The distance modulus, `m = M + 5*log10(d/10)`: a star seen from ten
    /// parsecs looks its absolute magnitude, and every tenfold further off is
    /// five magnitudes fainter. This is the one place distance enters the sky,
    /// and it enters as a logarithm, which is why a giant far out can still
    /// outshine a dwarf near. The [`Distance`] carries its own unit, so the
    /// light-year map and the parsec formula meet without a conversion left at
    /// the call site.
    pub fn apparent(self, distance: Distance) -> Magnitude {
        Magnitude(self.0 + 5.0 * (distance.as_parsecs() / 10.0).log10())
    }

    /// The one magnitude a set of unresolved sources comes to together.
    ///
    /// Their light adds because it is incoherent, so the fluxes sum and the
    /// total is turned back into a magnitude:
    /// `-2.5*log10(sum of 10^(-0.4*m_i))`. Two equal stars are about three
    /// quarters of a magnitude brighter than either alone, and the combined
    /// source is always brighter — a smaller magnitude — than its brightest
    /// member.
    ///
    /// Fed absolute magnitudes it returns a combined absolute magnitude, which
    /// is how the bake collapses a system's scanned stars into the one figure
    /// it orders by; fed apparent ones it returns a combined apparent
    /// magnitude, which is what an unresolved pair looks like from where it is
    /// seen. It is the same sum either way. Empty in, [`None`] out: no light is
    /// not a magnitude.
    pub fn combine(
        magnitudes: impl IntoIterator<Item = Magnitude>,
    ) -> Option<Magnitude> {
        let total: Flux = magnitudes.into_iter().map(Magnitude::flux).sum();
        (total.0 > 0.0).then(|| total.magnitude())
    }

    /// The energy this magnitude lands on a detector, relative to one exposed
    /// for `zero_point`.
    ///
    /// The exposure law, and the one figure two renderers of the same sky must
    /// agree on exactly or every comparison between them measures the gap
    /// between two laws rather than between two pictures. It is
    /// [`flux`](Self::flux) with the exposure folded in —
    /// `10^(-0.4*(m - zero_point))` — so a star exactly at the zero point
    /// returns one, a magnitude brighter returns about 2.5, and each five
    /// magnitudes fainter is another hundredth.
    ///
    /// The dial is a magnitude rather than a multiplier because that is the
    /// unit the decision is made in: `zero_point` is the magnitude that
    /// saturates a pixel, so setting it to 1.0 means Sirius and Canopus blow
    /// out and Vega sits just under, which is a sentence about a picture rather
    /// than about a number.
    ///
    /// What is deliberately *not* here is the point-spread function and the
    /// tone curve. Those are how a renderer spends the energy across pixels and
    /// then compresses it for a display, and they differ legitimately between a
    /// rasterizer and a shader. This is the part that may not differ.
    pub fn exposure(self, zero_point: Magnitude) -> Flux {
        Magnitude(self.0 - zero_point.0).flux()
    }

    /// The visual absolute magnitude of a blackbody whose *bolometric* absolute
    /// magnitude is `self` and whose surface is `temperature`.
    ///
    /// Elite's scanned magnitude is bolometric: the whole of a star's output,
    /// `4πR²σT⁴`, turned into a magnitude as though all of it were visible. The
    /// eye and the map see only the V band, and the fraction of a blackbody's
    /// light that lands there peaks near the Sun's heat and falls away to
    /// either side — a hot O star pours most of its into the ultraviolet, a cool
    /// T dwarf into the infrared — so both come out fainter in the visible than
    /// their total says. The correction is `−2.5 log₁₀` of that fraction
    /// against the Sun's, which leaves a Sun-like star untouched, dims the
    /// extremes, and sends a white dwarf down only as far as its own heat
    /// warrants — keeping the star-to-star variation a single class figure
    /// would flatten.
    ///
    /// It carries the compact remnants with no special case of their own. A
    /// neutron star's millions of kelvin leave all but nothing in the visible,
    /// so the correction runs to tens of magnitudes and it drops below any eye.
    /// A black hole has no temperature and no light at all: zero or below
    /// returns [`Magnitude::DARK`], the bolometric figure a scan carries for it
    /// being a placeholder with nothing behind it.
    pub fn visual(self, temperature: Temperature) -> Magnitude {
        if temperature.0 <= 0.0 {
            return Magnitude::DARK;
        }
        // The visible fraction is f_V(T)/f_bol(T), and f_bol ∝ T⁴, so relative
        // to the Sun it is (shape(T)/T⁴) over the Sun's same ratio. The
        // magnitude is dimmed by how much smaller than one that comes to.
        let fraction = (temperature.v_band_shape() / temperature.0.powi(4))
            / (*SOLAR_V_BAND_SHAPE / Temperature::SOLAR.0.powi(4));
        Magnitude(self.0 - POGSON_RATIO * fraction.log10())
    }
}

/// Relative flux: linear light, with magnitude zero as one unit.
///
/// What adds when light combines, where a [`Magnitude`] does not — the fluxes
/// of unresolved sources sum, and the total goes back to a magnitude through
/// [`magnitude`](Self::magnitude). It is also the energy an exposure collects,
/// which is why [`Magnitude::exposure`] returns one.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Flux(pub f64);

impl Flux {
    /// The magnitude this flux comes to, the inverse of [`Magnitude::flux`].
    ///
    /// `-2.5*log10(flux)`. Once fluxes have been summed this turns the total
    /// back into the scale the rest of the sky is spoken in.
    pub fn magnitude(self) -> Magnitude {
        Magnitude(-POGSON_RATIO * self.0.log10())
    }
}

impl std::iter::Sum for Flux {
    fn sum<I: Iterator<Item = Flux>>(iter: I) -> Flux {
        Flux(iter.map(|f| f.0).sum())
    }
}

/// The Pogson ratio: 2.5 magnitudes to a factor of ten in flux, from the
/// definition that five magnitudes is exactly a hundredfold. It carries a flux
/// ratio back into the magnitude scale the rest of the sky is spoken in.
const POGSON_RATIO: f64 = 2.5;

/// The reciprocal Pogson exponent, `1 / 2.5 = 0.4`: the power of ten a
/// magnitude carries in flux. Named apart from [`POGSON_RATIO`] so each
/// direction of the conversion reads as one factor rather than a bare decimal.
const POGSON_EXPONENT: f64 = 0.4;

/// A surface temperature, in kelvin, which fixes a star's colour.
///
/// A star radiates as a blackbody, so its heat is the whole of its tint: this
/// carries into [`color`](Self::color) and nothing else is needed for it. A
/// measured survey records a colour index rather than a temperature, so
/// [`from_color_index`](Self::from_color_index) is the second way in.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Temperature(pub f64);

/// The second radiation constant `hc/k`, in metre-kelvin, the one physical
/// constant the Planck curve still needs once its leading factor has cancelled.
const RADIATION_C2: f64 = 1.438_776_877e-2;

/// The Johnson V passband as a Gaussian: its centre and standard deviation in
/// metres, the width taken from the band's 88-nanometre full width at half
/// maximum. This is the band the eye's daylight response sits closest to and
/// the one a visual magnitude is spoken in.
const V_BAND_CENTER_M: f64 = 550e-9;
const V_BAND_SIGMA_M: f64 = 88e-9 / 2.354_820_045_030_949_3;

/// The wavelengths the V-band integral runs between, in metres, and how many
/// samples it takes across them. The span reaches far enough into either wing
/// that the Gaussian has fallen to nothing, and the count is well past where
/// the result stops moving — the ratio it is used in cancels what little error
/// the trapezoid leaves in any case.
const V_BAND_LO_M: f64 = 350e-9;
const V_BAND_HI_M: f64 = 800e-9;
const V_BAND_SAMPLES: usize = 129;

/// The Sun's [`v_band_shape`](Temperature::v_band_shape), computed once and
/// kept. It is the zero the correction hangs from: the Sun loses nothing
/// crossing into the visible, so every other star is weighed against it.
static SOLAR_V_BAND_SHAPE: std::sync::LazyLock<f64> =
    std::sync::LazyLock::new(|| Temperature::SOLAR.v_band_shape());

impl Temperature {
    /// The Sun's effective temperature in kelvin.
    pub const SOLAR: Temperature = Temperature(5772.0);

    /// The effective temperature a `B-V` colour index implies.
    ///
    /// Ballesteros' formula, which treats the two photometric bands as
    /// blackbody samples and solves for the temperature that would produce
    /// their ratio: `T = 4600 * (1/(0.92*BV + 1.7) + 1/(0.92*BV + 0.62))`. It
    /// reads the Sun's 0.656 back as about 5750 K against a true 5772, and
    /// Sirius' 0.009 as about 10100 against 9940.
    ///
    /// This is the second way into [`color`](Self::color), and the important
    /// one for a real catalog: measured surveys record a colour index, not a
    /// temperature, so without this nothing outside the game can be drawn or
    /// checked at all. Where a scanned star already carries a temperature that
    /// figure is better and this is not consulted.
    ///
    /// The fit runs cool at the hot end — it reads a B3 star at about 12,600 K
    /// against a true 15,000 — and its second term diverges as `BV` approaches
    /// -0.674, so the input is clamped to `-0.4..2.0`. Above roughly 20,000 K
    /// the class is the better source and [`ClassLight::of`] is what to ask.
    pub fn from_color_index(b_v: f64) -> Temperature {
        let bv = b_v.clamp(COLOR_INDEX_MIN, COLOR_INDEX_MAX);
        Temperature(
            4600.0 * (1.0 / (0.92 * bv + 1.7) + 1.0 / (0.92 * bv + 0.62)),
        )
    }

    /// The linear-RGB colour of a blackbody at this temperature.
    ///
    /// A star radiates as a blackbody, so its tint is fixed by its surface heat
    /// and nothing else: cool stars are red, the Sun is a warm white, and the
    /// hottest are blue. The channels are linear rather than gamma-encoded, so
    /// the caller can multiply flux straight into them, and the three carry a
    /// fixed Rec. 709 luminance — this carries chroma, not brightness, which
    /// flux carries. A saturated hue may take a channel past one to hold that
    /// luminance.
    ///
    /// The chromaticity is Kim et al.'s cubic fit to the Planckian locus, valid
    /// from 1667 to 25000 K and clamped either side, which covers everything
    /// with visible flux: below the floor are brown dwarfs the eye cannot see
    /// and above the ceiling the colour has already gone as blue as it goes.
    pub fn color(self) -> Color {
        let (x, y) = self.planckian_locus();
        Color(xy_to_linear_srgb(x, y))
    }

    /// The V band's slice of a blackbody's Planck curve at this temperature, in
    /// units where the curve's leading `2hc^2` is dropped since only ratios of
    /// this are ever taken. The Planck radiance `1 / (λ^5 (e^(c2/λT) − 1))` is
    /// weighted by the Johnson V response and summed across the band by the
    /// trapezoid rule.
    fn v_band_shape(self) -> f64 {
        let step = (V_BAND_HI_M - V_BAND_LO_M) / (V_BAND_SAMPLES - 1) as f64;
        let mut sum = 0.0;
        for i in 0..V_BAND_SAMPLES {
            let lambda = V_BAND_LO_M + step * i as f64;
            let response = (-0.5
                * ((lambda - V_BAND_CENTER_M) / V_BAND_SIGMA_M).powi(2))
            .exp();
            let planck = 1.0
                / (lambda.powi(5)
                    * (RADIATION_C2 / (lambda * self.0)).exp_m1());
            let end = i == 0 || i == V_BAND_SAMPLES - 1;
            sum += if end { 0.5 } else { 1.0 } * response * planck;
        }
        sum * step
    }

    /// The chromaticity `(x, y)` of the Planckian locus at this temperature.
    ///
    /// Kim, Weyrich and Kautz (2002), the cubic-spline approximation astronomy
    /// and colour tooling both cite. `x` is fit in two temperature ranges and
    /// `y` as a cubic in `x`; outside 1667..25000 K the temperature is clamped,
    /// since the fit is undefined there and the colour has stopped moving in any
    /// case.
    fn planckian_locus(self) -> (f64, f64) {
        let t = self.0.clamp(PLANCKIAN_LOCUS_MIN_K, PLANCKIAN_LOCUS_MAX_K);
        let (t2, t3) = (t * t, t * t * t);

        let x = if t < PLANCKIAN_X_BREAK_K {
            -0.266_123_9e9 / t3 - 0.234_358_9e6 / t2 + 0.877_695_6e3 / t
                + 0.179_910
        } else {
            -3.025_846_9e9 / t3 + 2.107_037_9e6 / t2 + 0.222_634_7e3 / t
                + 0.240_390
        };
        let (x2, x3) = (x * x, x * x * x);

        let y = if t < PLANCKIAN_Y_BREAK_K {
            -1.106_381_4 * x3 - 1.348_110_20 * x2 + 2.185_558_32 * x
                - 0.202_196_83
        } else if t < PLANCKIAN_X_BREAK_K {
            -0.954_947_6 * x3 - 1.374_185_93 * x2 + 2.091_370_15 * x
                - 0.167_488_67
        } else {
            3.081_758_0 * x3 - 5.873_386_70 * x2 + 3.751_129_97 * x
                - 0.370_014_83
        };

        (x, y)
    }
}

/// A linear-RGB tint: chroma at unit luminance, so flux alone sets brightness.
///
/// What a [`Temperature`] comes to through [`Temperature::color`]. The channels
/// are linear rather than gamma-encoded, so a caller multiplies flux straight
/// into them, and they carry a fixed [`Luminance`] so the hue is all this holds
/// and the brightness is the flux's to set.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Color(pub [f32; 3]);

impl Color {
    /// The Rec. 709 luminance of this tint. A blackbody colour is normalized to
    /// unit luminance, so this is one for anything [`Temperature::color`]
    /// returns; it is the measure that normalization is held to.
    pub fn luminance(self) -> Luminance {
        Luminance::of([self.0[0] as f64, self.0[1] as f64, self.0[2] as f64])
    }
}

impl std::ops::Index<usize> for Color {
    type Output = f32;

    fn index(&self, channel: usize) -> &f32 {
        &self.0[channel]
    }
}

/// The Rec. 709 perceived brightness of a linear-RGB triple.
///
/// The one brightness the crate's colours are held to. A blackbody tint carries
/// hue at unit luminance so flux alone sets how bright a star is drawn; this
/// measures that unit.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Luminance(pub f64);

impl Luminance {
    /// The Rec. 709 luminance weights for a linear-sRGB triple: how much red,
    /// green and blue each count toward perceived brightness, the same
    /// primaries and D65 white [`Temperature::color`] targets.
    /// They sum to one, so a neutral triple of value `v` has luminance `v`.
    pub const REC709_WEIGHTS: [f64; 3] = [0.2126, 0.7152, 0.0722];

    /// The Rec. 709 luminance of a linear-sRGB triple.
    pub fn of(rgb: [f64; 3]) -> Luminance {
        Luminance(
            Self::REC709_WEIGHTS[0] * rgb[0]
                + Self::REC709_WEIGHTS[1] * rgb[1]
                + Self::REC709_WEIGHTS[2] * rgb[2],
        )
    }
}

/// The temperature range Kim et al.'s Planckian-locus fit is defined over, in
/// kelvin. Outside it the fit is undefined and the chromaticity has stopped
/// moving, so the temperature is clamped to these bounds before it is read.
const PLANCKIAN_LOCUS_MIN_K: f64 = 1667.0;
const PLANCKIAN_LOCUS_MAX_K: f64 = 25000.0;

/// The piecewise breakpoints of Kim et al.'s cubic fit: the temperature where
/// it switches the coefficients it reads `x` with, and the lower one where it
/// switches those it reads `y` with.
const PLANCKIAN_X_BREAK_K: f64 = 4000.0;
const PLANCKIAN_Y_BREAK_K: f64 = 2222.0;

/// The XYZ→linear-sRGB matrix under a D65 white point, one row per output
/// channel — the standard sRGB primaries. Each row dotted with an `[X, Y, Z]`
/// triple gives that channel before any gamut clamp.
const XYZ_TO_LINEAR_SRGB_D65: [[f64; 3]; 3] = [
    [3.240_625_5, -1.537_208_0, -0.498_628_6],
    [-0.968_930_7, 1.875_756_1, 0.041_517_5],
    [0.055_710_1, -0.204_021_1, 1.056_995_9],
];

/// A CIE XYZ triple as linear sRGB (D65), before any gamut clamp: the
/// [`XYZ_TO_LINEAR_SRGB_D65`] matrix applied as one named step.
fn xyz_to_linear_srgb(xyz: [f64; 3]) -> [f64; 3] {
    let m = XYZ_TO_LINEAR_SRGB_D65;
    [
        m[0][0] * xyz[0] + m[0][1] * xyz[1] + m[0][2] * xyz[2],
        m[1][0] * xyz[0] + m[1][1] * xyz[1] + m[1][2] * xyz[2],
        m[2][0] * xyz[0] + m[2][1] * xyz[1] + m[2][2] * xyz[2],
    ]
}

/// A chromaticity `(x, y)` as linear sRGB at unit luminance.
///
/// Through XYZ at unit luminance, then the sRGB primaries under D65. A colour
/// off the sRGB gamut lands a channel below zero, which is clamped up, so the
/// deepest reds and blues sit at the edge of what the display can show rather
/// than turning inside out. The result is then scaled to unit Rec. 709
/// luminance, so the tint carries hue at a fixed brightness and flux alone says
/// how bright a star is drawn: a saturated red or blue, whose light piles into
/// one channel, is no dimmer than a white of the same flux. A channel may run
/// past one to hold that luminance in a saturated hue, which the linear HDR path
/// the callers feed takes without clipping.
fn xy_to_linear_srgb(x: f64, y: f64) -> [f32; 3] {
    // xyY to XYZ at unit luminance, Y = 1.
    let big_x = x / y;
    let big_z = (1.0 - x - y) / y;

    // XYZ to linear sRGB, D65, then clamp any out-of-gamut channel up to zero.
    let [r, g, b] = xyz_to_linear_srgb([big_x, 1.0, big_z]);
    let r = r.max(0.0);
    let g = g.max(0.0);
    let b = b.max(0.0);
    // Divide out the Rec. 709 luminance so the tint carries hue at unit
    // brightness and only flux moves how bright a star is drawn.
    let luminance = Luminance::of([r, g, b]).0;
    if luminance > 0.0 {
        [(r / luminance) as f32, (g / luminance) as f32, (b / luminance) as f32]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// A typical absolute magnitude and temperature for a class of star.
///
/// What [`ClassLight::of`] answers: the two numbers the bake needs where no
/// scan gives them, standing in for the whole class rather than any one member
/// of it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClassLight {
    /// Typical absolute visual magnitude for the class.
    pub absolute_magnitude: Magnitude,
    /// Typical effective temperature.
    pub temperature: Temperature,
}

impl ClassLight {
    const fn new(absolute_magnitude: f64, temperature: f64) -> Self {
        ClassLight {
            absolute_magnitude: Magnitude(absolute_magnitude),
            temperature: Temperature(temperature),
        }
    }

    /// What an unrecognized or missing class comes to.
    ///
    /// An M dwarf, because that is what most of the galaxy is: red dwarfs
    /// outnumber everything else together, so a system with nothing on record
    /// but its existence is more likely one of those than not, and guessing dim
    /// keeps an unknown from crowding into a sky it has no claim to.
    pub const DEFAULT: ClassLight = ClassLight::new(10.0, 3400.0);

    /// A typical absolute magnitude and temperature for an Elite star class.
    ///
    /// The last link in the bake's fallback chain, reached only where a system
    /// carries no scanned star to sum. It takes the class the system is named
    /// for — the game's `StarClass`, a letter or two and nothing finer — and
    /// answers the pair the ordering and the glow need.
    ///
    /// The figures are representative values for each family, drawn from the
    /// standard dwarf sequence (Pecaut & Mamajek 2013) for the main one and
    /// from the character of each remnant and oddity for the rest, and are
    /// meant to be tuned against the bake once it runs rather than settled to a
    /// decimal now. What matters at this stage is the ordering they impose: hot
    /// before cool, bright before dim, remnants and brown dwarfs down where
    /// their flux really sits.
    ///
    /// Matching runs specific before general. The pairs that begin with a
    /// letter another family also begins with — `MS` and `S` for the S-type
    /// giants, `TTS` for the T Tauri stars — are caught whole before the bare
    /// letter is read as a main-sequence class, and the white dwarf, Wolf-Rayet
    /// and carbon families are taken by their leading letter since nothing else
    /// wears it.
    pub fn of(class: &str) -> ClassLight {
        let class = class.trim().to_ascii_uppercase();
        let c = class.as_str();

        // The lightless remnants and the anomalies. Their flux is negligible,
        // so where they land in the ordering matters more than the exact
        // figure; the temperature is a nominal one that never draws.
        if c == "H" || c.starts_with("SUPERMASSIVE") {
            // Black holes give off no light. A magnitude this faint is flux the
            // eye and the sensor both read as nothing.
            return ClassLight::new(Magnitude::DARK.0, 0.0);
        }
        if c == "N" {
            // A neutron star's thermal output is almost all in X-rays; what
            // leaks into the visible is next to nothing.
            return ClassLight::new(16.0, 30000.0);
        }
        if c == "X" {
            return ClassLight::DEFAULT;
        }

        // Pre-main-sequence, caught before the bare letters they share a lead
        // with.
        if c.starts_with("TTS") {
            // T Tauri: young, cool and variable, roughly a dim K–M dwarf.
            return ClassLight::new(6.0, 4000.0);
        }
        if c.starts_with("AEBE") {
            // Herbig Ae/Be: the hotter, brighter pre-main-sequence stars.
            return ClassLight::new(1.0, 10000.0);
        }

        // The families named by a leading letter no ordinary class wears.
        if c.starts_with('D') {
            // White dwarfs: hot but tiny, so faint for their heat.
            return ClassLight::new(13.0, 12000.0);
        }
        if c.starts_with('W') {
            // Wolf-Rayet: extreme, hot and very luminous.
            return ClassLight::new(-4.0, 60000.0);
        }
        if c.starts_with('C') {
            // Carbon stars: cool, red and luminous AGB giants.
            return ClassLight::new(0.0, 3000.0);
        }

        // S-type giants, whose tokens collide with the main sequence.
        if c == "S" || c == "MS" {
            return ClassLight::new(0.0, 3200.0);
        }

        // The main sequence and the brown dwarfs, by leading letter, hottest
        // and brightest to coolest and dimmest.
        match class.chars().next() {
            Some('O') => ClassLight::new(-4.0, 38000.0),
            Some('B') => ClassLight::new(-1.5, 18000.0),
            Some('A') => ClassLight::new(1.3, 8700.0),
            Some('F') => ClassLight::new(3.1, 6700.0),
            Some('G') => ClassLight::new(4.9, 5700.0),
            Some('K') => ClassLight::new(6.7, 4600.0),
            Some('M') => ClassLight::new(10.0, 3400.0),
            Some('L') => ClassLight::new(18.0, 1800.0),
            Some('T') => ClassLight::new(22.0, 1000.0),
            Some('Y') => ClassLight::new(25.0, 500.0),
            _ => ClassLight::DEFAULT,
        }
    }
}

/// The `B-V` range Ballesteros' fit is trusted over. Its second term diverges
/// as `B-V` nears −0.674, and past the red end the class is the better source,
/// so the index is clamped to these bounds before the formula reads it.
const COLOR_INDEX_MIN: f64 = -0.4;
const COLOR_INDEX_MAX: f64 = 2.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// A parsec is that many light years, and the two conversions undo each
    /// other, which is the only thing a caller relies on.
    #[test]
    fn a_parsec_is_over_three_light_years() {
        assert!(close(LY_PER_PARSEC, 3.261_563_777_167_433));
        assert!(close(PARSECS_PER_LY * LY_PER_PARSEC, 1.0));
    }

    /// A distance names its unit, and the two forms agree once one is converted
    /// into the other.
    #[test]
    fn a_distance_reads_the_same_in_either_unit() {
        let d = Distance::light_years(42.0);
        assert!(close(d.as_light_years(), 42.0));
        assert!(close(d.as_parsecs(), 42.0 * PARSECS_PER_LY));
        assert!(close(Distance::parsecs(1.0).as_light_years(), LY_PER_PARSEC));
    }

    /// Ten parsecs is the zero of the distance modulus: a star seen from there
    /// looks exactly its absolute magnitude.
    #[test]
    fn ten_parsecs_shows_a_star_at_its_absolute_magnitude() {
        let m = Magnitude(4.83).apparent(Distance::parsecs(10.0));
        assert!(close(m.0, 4.83));
    }

    /// Every tenfold further off is five magnitudes fainter.
    #[test]
    fn ten_times_the_distance_is_five_magnitudes_fainter() {
        assert!(close(
            Magnitude(0.0).apparent(Distance::parsecs(100.0)).0,
            5.0
        ));
        assert!(close(
            Magnitude(0.0).apparent(Distance::parsecs(1000.0)).0,
            10.0
        ));
    }

    /// The two distance units reach the same magnitude, the light-year form
    /// only folding the conversion in.
    #[test]
    fn the_two_distance_units_reach_the_same_magnitude() {
        let ly = 42.0;
        assert!(close(
            Magnitude(3.0).apparent(Distance::light_years(ly)).0,
            Magnitude(3.0).apparent(Distance::parsecs(ly * PARSECS_PER_LY)).0,
        ));
    }

    /// Magnitude zero is one unit of flux, and five magnitudes is a factor of
    /// a hundred, in both directions.
    #[test]
    fn five_magnitudes_is_a_hundredfold_in_flux() {
        assert!(close(Magnitude(0.0).flux().0, 1.0));
        assert!(close(Magnitude(5.0).flux().0, 0.01));
        assert!(close(Magnitude(-5.0).flux().0, 100.0));
    }

    /// Flux and magnitude are inverses, so a round trip is the identity.
    #[test]
    fn flux_and_magnitude_undo_each_other() {
        for m in [-5.0, -1.0, 0.0, 3.7, 8.0, 15.0] {
            assert!(close(Magnitude(m).flux().magnitude().0, m));
        }
    }

    /// Two equal stars are about three quarters of a magnitude brighter
    /// together than either alone, the `2.5*log10(2)` the physics gives.
    #[test]
    fn two_equal_stars_add_by_log_two() {
        let combined =
            Magnitude::combine([Magnitude(0.0), Magnitude(0.0)]).unwrap();
        assert!(close(combined.0, -POGSON_RATIO * 2f64.log10()));
    }

    /// A combined source is brighter — a smaller magnitude — than its
    /// brightest member, never dimmer.
    #[test]
    fn combining_never_dims() {
        let ms = [2.0, 4.0, 7.5];
        let combined = Magnitude::combine(ms.map(Magnitude)).unwrap();
        let brightest = ms.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(combined.0 < brightest);
    }

    /// No light is no magnitude.
    #[test]
    fn no_sources_have_no_magnitude() {
        assert_eq!(Magnitude::combine(std::iter::empty()), None);
    }

    /// Every blackbody colour is a real one: each channel in gamut above zero,
    /// and the triple carrying a fixed luminance, since it stands for hue alone
    /// and flux carries the brightness.
    #[test]
    fn a_blackbody_colour_is_normalized_and_in_range() {
        for t in [1000.0, 3000.0, 5772.0, 10000.0, 30000.0] {
            let c = Temperature(t).color();
            assert!(c.0.iter().all(|&ch| ch >= 0.0));
            let luminance = c.luminance().0;
            assert!(
                (luminance - 1.0).abs() < 1e-4,
                "at {t} K luminance is {luminance}"
            );
        }
    }

    /// Cool stars are red: at three thousand kelvin the red channel leads the
    /// blue.
    #[test]
    fn a_cool_star_is_red() {
        let c = Temperature(3000.0).color();
        assert!(c[0] > c[2]);
    }

    /// Hot stars are blue: at twenty thousand kelvin the blue channel leads the
    /// red.
    #[test]
    fn a_hot_star_is_blue() {
        let c = Temperature(20000.0).color();
        assert!(c[2] > c[0]);
    }

    /// The Sun is a warm white: no channel starved, and red over blue.
    #[test]
    fn the_sun_is_a_warm_white() {
        let c = Temperature::SOLAR.color();
        assert!(c.0.iter().all(|&ch| ch > 0.6));
        assert!(c[0] > c[2]);
    }

    /// The class table is ordered the way the sky is: hotter classes are
    /// hotter and brighter classes are brighter, down the whole main sequence.
    #[test]
    fn the_main_sequence_orders_hot_and_bright_together() {
        let seq = ["O", "B", "A", "F", "G", "K", "M"];
        for pair in seq.windows(2) {
            let hot = ClassLight::of(pair[0]);
            let cool = ClassLight::of(pair[1]);
            assert!(
                hot.temperature > cool.temperature,
                "{} should be hotter than {}",
                pair[0],
                pair[1]
            );
            assert!(
                hot.absolute_magnitude < cool.absolute_magnitude,
                "{} should be brighter than {}",
                pair[0],
                pair[1]
            );
        }
    }

    /// A G star is Sun-like, which is the one anchor a reader can check by eye.
    #[test]
    fn a_g_star_is_sun_like() {
        let g = ClassLight::of("G");
        assert!(
            (g.absolute_magnitude.0 - Magnitude::SOLAR_ABSOLUTE.0).abs() < 1.0
        );
        assert!((g.temperature.0 - Temperature::SOLAR.0).abs() < 500.0);
    }

    /// Class is read case- and space-insensitively, as the feeds spell it
    /// however they like.
    #[test]
    fn class_ignores_case_and_padding() {
        assert_eq!(ClassLight::of(" g "), ClassLight::of("G"));
        assert_eq!(ClassLight::of("da"), ClassLight::of("DA"));
    }

    /// The S-type tokens are giants, not the main-sequence letters they lead
    /// with: `MS` is not an M dwarf and `S` is not read as one either.
    #[test]
    fn s_type_giants_are_not_the_main_sequence() {
        assert_ne!(ClassLight::of("MS"), ClassLight::of("M"));
        assert_ne!(ClassLight::of("S"), ClassLight::of("M"));
        assert_eq!(ClassLight::of("MS"), ClassLight::of("S"));
    }

    /// T Tauri is caught whole, not read as a T dwarf, which would be far
    /// cooler and far fainter.
    #[test]
    fn t_tauri_is_not_a_brown_dwarf() {
        assert_ne!(ClassLight::of("TTS"), ClassLight::of("T"));
        assert!(
            ClassLight::of("TTS").temperature > ClassLight::of("T").temperature
        );
    }

    /// A white dwarf's subtype rides the same family figures as the bare `D`.
    #[test]
    fn white_dwarf_subtypes_share_the_family() {
        for sub in ["D", "DA", "DB", "DC", "DQ", "DAV", "DX"] {
            assert_eq!(ClassLight::of(sub), ClassLight::of("D"));
        }
    }

    /// A black hole gives off no light, so its flux is nothing the sky ever
    /// draws — a magnitude far below any limit.
    #[test]
    fn a_black_hole_is_dark() {
        let h = ClassLight::of("H");
        assert!(
            h.absolute_magnitude.apparent(Distance::parsecs(10.0))
                > Magnitude::EYE_LIMIT
        );
        assert!(h.absolute_magnitude.flux().0 < 1e-10);
        assert_eq!(ClassLight::of("SupermassiveBlackHole"), h);
    }

    /// An unrecognized class falls to the default rather than to nothing.
    #[test]
    fn an_unknown_class_defaults() {
        assert_eq!(ClassLight::of("ZZZ"), ClassLight::DEFAULT);
        assert_eq!(ClassLight::of(""), ClassLight::DEFAULT);
    }

    /// The Sun loses nothing crossing into the visible: the V band is where its
    /// light already is, so its visual magnitude is its bolometric one.
    #[test]
    fn the_sun_keeps_its_magnitude_in_the_visible() {
        assert!(close(
            Magnitude::SOLAR_ABSOLUTE.visual(Temperature::SOLAR).0,
            Magnitude::SOLAR_ABSOLUTE.0,
        ));
    }

    /// A blackbody hotter or cooler than the Sun spills more of its light out
    /// of the visible band, so it is fainter there than its total output says,
    /// and the further from the Sun's heat the fainter it grows.
    #[test]
    fn the_visible_band_dims_both_extremes() {
        let sun = Magnitude(0.0).visual(Temperature::SOLAR);
        let hot = Magnitude(0.0).visual(Temperature(40_000.0));
        let hotter = Magnitude(0.0).visual(Temperature(100_000.0));
        let cool = Magnitude(0.0).visual(Temperature(3000.0));
        let cooler = Magnitude(0.0).visual(Temperature(1500.0));
        assert!(hot > sun && hotter > hot, "a hotter star dims further");
        assert!(cool > sun && cooler > cool, "a cooler star dims further");
    }

    /// A neutron star's heat is almost all in X-rays, so next to none of it
    /// reaches the visible: a scan near naked-eye brightness corrects to tens
    /// of magnitudes below any eye.
    #[test]
    fn a_neutron_star_all_but_vanishes_in_the_visible() {
        let corrected = Magnitude(5.0).visual(Temperature(5_950_000.0));
        assert!(
            corrected.0 > Magnitude::EYE_LIMIT.0 + 10.0,
            "a neutron star is no naked-eye star: {corrected:?}"
        );
    }

    /// A black hole has no temperature and no light: whatever placeholder
    /// magnitude a scan carries, it comes to the dark figure and no flux.
    #[test]
    fn a_black_hole_has_no_visible_light() {
        assert_eq!(Magnitude(20.0).visual(Temperature(0.0)), Magnitude::DARK);
        assert!(Magnitude(20.0).visual(Temperature(0.0)).flux().0 < 1e-10);
    }

    /// A white dwarf keeps the brightness its scan gives, shifted only a little
    /// by its heat, so one white dwarf still differs from another — the
    /// variation a flat class figure would erase.
    #[test]
    fn a_white_dwarf_keeps_its_scanned_variation() {
        // Two white dwarfs the game scans at one bolometric magnitude but
        // different heat stay distinct, the hotter the fainter in the visible.
        let cool = Magnitude(12.0).visual(Temperature(7000.0));
        let hot = Magnitude(12.0).visual(Temperature(16_000.0));
        assert!(hot > cool, "the hotter white dwarf is fainter in V");
        // Neither is thrown far from its scanned figure.
        assert!((cool.0 - 12.0).abs() < 0.5);
        assert!((hot.0 - 12.0).abs() < 1.5);
    }

    /// The Sun's colour index reads its temperature back, which is the one
    /// anchor for this fit a reader can check by eye.
    #[test]
    fn the_suns_colour_index_gives_the_suns_temperature() {
        let t = Temperature::from_color_index(0.656);
        assert!((t.0 - Temperature::SOLAR.0).abs() < 100.0, "{t:?}");
    }

    /// Bluer is hotter, the whole content of a colour index, and it holds
    /// across the range the fit is defined on.
    #[test]
    fn a_bluer_colour_index_is_hotter() {
        let seq = [-0.3, 0.0, 0.3, 0.656, 1.0, 1.5, 2.0];
        for pair in seq.windows(2) {
            assert!(
                Temperature::from_color_index(pair[0])
                    > Temperature::from_color_index(pair[1]),
                "{} should be hotter than {}",
                pair[0],
                pair[1]
            );
        }
    }

    /// The formula's second term diverges near -0.674, so the clamp holds the
    /// answer finite and positive however far out of range the input is.
    #[test]
    fn an_out_of_range_colour_index_stays_finite() {
        for bv in [-100.0, -0.674, -0.5, 3.0, 100.0] {
            let t = Temperature::from_color_index(bv).0;
            assert!(t.is_finite() && t > 0.0, "{bv} gave {t}");
        }
    }

    /// A colour index and a class agree on a Sun-like star, which is the check
    /// that the two independent routes into a temperature meet.
    #[test]
    fn the_two_routes_to_a_temperature_agree_on_a_g_star() {
        let from_colour = Temperature::from_color_index(0.656);
        let from_class = ClassLight::of("G").temperature;
        assert!((from_colour.0 - from_class.0).abs() < 500.0);
    }

    /// A star at the zero point is one unit of exposure, and the scale stays
    /// the magnitude scale either side of it.
    #[test]
    fn the_zero_point_is_one_unit_of_exposure() {
        assert!(close(Magnitude(1.0).exposure(Magnitude(1.0)).0, 1.0));
        assert!(close(Magnitude(6.0).exposure(Magnitude(1.0)).0, 0.01));
        assert!(close(Magnitude(-4.0).exposure(Magnitude(1.0)).0, 100.0));
    }

    /// Turning the exposure up by a magnitude brightens everything by the same
    /// factor, which is what makes it one dial rather than a per-star one.
    #[test]
    fn exposure_scales_every_star_alike() {
        let ms = [-1.4, 0.03, 4.83, 8.0];
        let ratios: Vec<f64> = ms
            .iter()
            .map(|&m| {
                Magnitude(m).exposure(Magnitude(2.0)).0
                    / Magnitude(m).exposure(Magnitude(1.0)).0
            })
            .collect();
        for r in &ratios {
            assert!(close(*r, ratios[0]));
        }
    }
}
