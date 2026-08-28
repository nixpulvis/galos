//! The physics of how bright a star is and what colour it comes to.
//!
//! Everything the galaxy is drawn from rests on one ordering, by absolute
//! magnitude, and one colour, from temperature. Both are physics rather than
//! taste: a bright giant five thousand light years out belongs in the sky
//! while a hundred dim dwarfs nearby do not, and a star's tint is the tint of
//! a blackbody at its surface heat. So the functions here are claims about the
//! world, each with a test that reads as one.
//!
//! Two of them the map cannot compute for itself. Ordering by absolute
//! magnitude needs a magnitude for every system, including the two thirds that
//! carry no scanned star, and colour needs a temperature. Where a scan is
//! absent the class the system is named for is all there is, so
//! [`class_light`] turns that class into a typical magnitude and heat. It is
//! the last link in the bake's fallback chain — scanned stars first, then this
//! — and it is what lets a system with nothing recorded but its primary's
//! letter still take its place in the ordering.
//!
//! Nothing here knows about the database or the renderer. Magnitudes are plain
//! [`f64`], colours are linear RGB as `[f32; 3]`, and distances are named in
//! their unit at each call. The caller converts to whatever it draws in.

/// Light years to a parsec.
///
/// The definition of the parsec, `648000 / PI` astronomical units, worked
/// through the AU and the light year in metres. Photometry is written in
/// parsecs because the distance modulus is, and the map stands in light years,
/// so one or the other is always being converted.
pub const LY_PER_PARSEC: f64 = 3.261_563_777_167_433;

/// Parsecs to a light year, the reciprocal of [`LY_PER_PARSEC`].
pub const PARSECS_PER_LY: f64 = 1.0 / LY_PER_PARSEC;

/// The faintest a dark-adapted eye reaches, in space with no air to dim it.
///
/// The cut that decides how many stars a sky holds, roughly ten to fifty
/// thousand across a whole one. Below it a star is there in the record and in
/// the glow but is not drawn as itself.
pub const EYE_LIMIT: f64 = 8.0;

/// A magnitude so faint no eye or sensor reads it as anything, its flux the
/// sky rounds to nothing. What a body with no visible light comes to — a black
/// hole, whose temperature is zero — and the floor any darkened scan lands on.
pub const DARK_MAGNITUDE: f64 = 40.0;

/// The Sun's absolute visual magnitude, the zero the sequence is hung from.
pub const SOLAR_ABSOLUTE_MAGNITUDE: f64 = 4.83;

/// The Sun's effective temperature in kelvin.
pub const SOLAR_TEMPERATURE: f64 = 5772.0;

/// How bright a star of absolute magnitude `absolute` looks from `distance_pc`
/// parsecs away.
///
/// The distance modulus, `m = M + 5*log10(d/10)`: a star seen from ten parsecs
/// looks its absolute magnitude, and every tenfold further off is five
/// magnitudes fainter. This is the one place distance enters the sky, and it
/// enters as a logarithm, which is why a giant far out can still outshine a
/// dwarf near.
pub fn apparent_magnitude(absolute: f64, distance_pc: f64) -> f64 {
    absolute + 5.0 * (distance_pc / 10.0).log10()
}

/// [`apparent_magnitude`] for a distance already in light years.
///
/// The map holds light years everywhere, so this is the form it calls, the
/// parsec conversion folded in rather than left at every site.
pub fn apparent_magnitude_ly(absolute: f64, distance_ly: f64) -> f64 {
    apparent_magnitude(absolute, distance_ly * PARSECS_PER_LY)
}

/// The relative flux of a magnitude, with magnitude zero as one unit.
///
/// `10^(-0.4*m)`, the inverse of the magnitude scale being logarithmic: five
/// magnitudes is a factor of a hundred, so a magnitude-five star carries a
/// hundredth the flux of a magnitude-zero one. Flux is what adds, and
/// magnitudes are what do not, so anything that combines light converts to
/// this first.
pub fn flux(magnitude: f64) -> f64 {
    10f64.powf(-0.4 * magnitude)
}

/// The magnitude a relative flux comes to, the inverse of [`flux`].
///
/// `-2.5*log10(flux)`. Once fluxes have been summed this turns the total back
/// into the scale the rest of the sky is spoken in.
pub fn magnitude(flux: f64) -> f64 {
    -2.5 * flux.log10()
}

/// The one magnitude a set of unresolved sources comes to together.
///
/// Their light adds because it is incoherent, so the fluxes sum and the total
/// is turned back into a magnitude:
/// `-2.5*log10(sum of 10^(-0.4*m_i))`. Two equal stars are about three
/// quarters of a magnitude brighter than either alone, and the combined source
/// is always brighter — a smaller magnitude — than its brightest member.
///
/// Fed absolute magnitudes it returns a combined absolute magnitude, which is
/// how the bake collapses a system's scanned stars into the one figure it
/// orders by; fed apparent ones it returns a combined apparent magnitude,
/// which is what an unresolved pair looks like from where it is seen. It is the
/// same sum either way. Empty in, [`None`] out: no light is not a magnitude.
pub fn combined_magnitude(
    magnitudes: impl IntoIterator<Item = f64>,
) -> Option<f64> {
    let total: f64 = magnitudes.into_iter().map(flux).sum();
    (total > 0.0).then(|| magnitude(total))
}

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

/// The V band's slice of a blackbody's Planck curve at `temperature_k`, in
/// units where the curve's leading `2hc^2` is dropped since only ratios of this
/// are ever taken. The Planck radiance `1 / (λ^5 (e^(c2/λT) − 1))` is weighted
/// by the Johnson V response and summed across the band by the trapezoid rule.
fn v_band_shape(temperature_k: f64) -> f64 {
    let step = (V_BAND_HI_M - V_BAND_LO_M) / (V_BAND_SAMPLES - 1) as f64;
    let mut sum = 0.0;
    for i in 0..V_BAND_SAMPLES {
        let lambda = V_BAND_LO_M + step * i as f64;
        let response =
            (-0.5 * ((lambda - V_BAND_CENTER_M) / V_BAND_SIGMA_M).powi(2)).exp();
        let planck = 1.0
            / (lambda.powi(5)
                * (RADIATION_C2 / (lambda * temperature_k)).exp_m1());
        let end = i == 0 || i == V_BAND_SAMPLES - 1;
        sum += if end { 0.5 } else { 1.0 } * response * planck;
    }
    sum * step
}

/// The Sun's [`v_band_shape`], computed once and kept. It is the zero the
/// correction hangs from: the Sun loses nothing crossing into the visible, so
/// every other star is weighed against it.
static SOLAR_V_BAND_SHAPE: std::sync::LazyLock<f64> =
    std::sync::LazyLock::new(|| v_band_shape(SOLAR_TEMPERATURE));

/// The visual absolute magnitude of a blackbody whose *bolometric* absolute
/// magnitude is `bolometric` and whose surface is `temperature_k` kelvin.
///
/// Elite's scanned magnitude is bolometric: the whole of a star's output,
/// `4πR²σT⁴`, turned into a magnitude as though all of it were visible. The eye
/// and the map see only the V band, and the fraction of a blackbody's light
/// that lands there peaks near the Sun's heat and falls away to either side — a
/// hot O star pours most of its into the ultraviolet, a cool T dwarf into the
/// infrared — so both come out fainter in the visible than their total says.
/// The correction is `−2.5 log₁₀` of that fraction against the Sun's, which
/// leaves a Sun-like star untouched, dims the extremes, and sends a white dwarf
/// down only as far as its own heat warrants — keeping the star-to-star
/// variation a single class figure would flatten.
///
/// It carries the compact remnants with no special case of their own. A neutron
/// star's millions of kelvin leave all but nothing in the visible, so the
/// correction runs to tens of magnitudes and it drops below any eye. A black
/// hole has no temperature and no light at all: zero or below returns
/// [`DARK_MAGNITUDE`], the bolometric figure a scan carries for it being a
/// placeholder with nothing behind it.
pub fn visual_magnitude(bolometric: f64, temperature_k: f64) -> f64 {
    if temperature_k <= 0.0 {
        return DARK_MAGNITUDE;
    }
    // The visible fraction is f_V(T)/f_bol(T), and f_bol ∝ T⁴, so relative to
    // the Sun it is (shape(T)/T⁴) over the Sun's same ratio. The magnitude is
    // dimmed by how much smaller than one that comes to.
    let fraction = (v_band_shape(temperature_k) / temperature_k.powi(4))
        / (*SOLAR_V_BAND_SHAPE / SOLAR_TEMPERATURE.powi(4));
    bolometric - 2.5 * fraction.log10()
}

/// The linear-RGB colour of a blackbody at `temperature_k` kelvin.
///
/// A star radiates as a blackbody, so its tint is fixed by its surface heat and
/// nothing else: cool stars are red, the Sun is a warm white, and the hottest
/// are blue. The channels are linear rather than gamma-encoded, so the caller
/// can multiply flux straight into them, and the brightest of the three is one
/// — this carries chroma, not brightness, which flux carries.
///
/// The chromaticity is Kim et al.'s cubic fit to the Planckian locus, valid
/// from 1667 to 25000 K and clamped either side, which covers everything with
/// visible flux: below the floor are brown dwarfs the eye cannot see and above
/// the ceiling the colour has already gone as blue as it goes.
pub fn blackbody_color(temperature_k: f64) -> [f32; 3] {
    let (x, y) = planckian_locus(temperature_k);
    xy_to_linear_srgb(x, y)
}

/// The chromaticity `(x, y)` of the Planckian locus at a temperature.
///
/// Kim, Weyrich and Kautz (2002), the cubic-spline approximation astronomy and
/// colour tooling both cite. `x` is fit in two temperature ranges and `y` as a
/// cubic in `x`; outside 1667..25000 K the temperature is clamped, since the
/// fit is undefined there and the colour has stopped moving in any case.
fn planckian_locus(temperature_k: f64) -> (f64, f64) {
    let t = temperature_k.clamp(1667.0, 25000.0);
    let (t2, t3) = (t * t, t * t * t);

    let x = if t < 4000.0 {
        -0.266_123_9e9 / t3 - 0.234_358_9e6 / t2 + 0.877_695_6e3 / t + 0.179_910
    } else {
        -3.025_846_9e9 / t3 + 2.107_037_9e6 / t2 + 0.222_634_7e3 / t + 0.240_390
    };
    let (x2, x3) = (x * x, x * x * x);

    let y = if t < 2222.0 {
        -1.106_381_4 * x3 - 1.348_110_20 * x2 + 2.185_558_32 * x - 0.202_196_83
    } else if t < 4000.0 {
        -0.954_947_6 * x3 - 1.374_185_93 * x2 + 2.091_370_15 * x - 0.167_488_67
    } else {
        3.081_758_0 * x3 - 5.873_386_70 * x2 + 3.751_129_97 * x - 0.370_014_83
    };

    (x, y)
}

/// A chromaticity `(x, y)` as linear sRGB, normalized so the brightest channel
/// is one.
///
/// Through XYZ at unit luminance, then the sRGB primaries under D65. A colour
/// off the sRGB gamut lands a channel below zero, which is clamped up, so the
/// deepest reds and blues sit at the edge of what the display can show rather
/// than turning inside out. The result is scaled to unit peak because it stands
/// for hue alone; how bright a star is drawn is its flux, applied by the
/// caller.
fn xy_to_linear_srgb(x: f64, y: f64) -> [f32; 3] {
    // xyY to XYZ at unit luminance, Y = 1.
    let big_x = x / y;
    let big_z = (1.0 - x - y) / y;

    // XYZ to linear sRGB, D65.
    let r = 3.240_625_5 * big_x - 1.537_208_0 - 0.498_628_6 * big_z;
    let g = -0.968_930_7 * big_x + 1.875_756_1 + 0.041_517_5 * big_z;
    let b = 0.055_710_1 * big_x - 0.204_021_1 + 1.056_995_9 * big_z;

    let r = r.max(0.0);
    let g = g.max(0.0);
    let b = b.max(0.0);
    let peak = r.max(g).max(b);
    if peak > 0.0 {
        [(r / peak) as f32, (g / peak) as f32, (b / peak) as f32]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// A typical absolute magnitude and temperature for a class of star.
///
/// What [`class_light`] answers: the two numbers the bake needs where no scan
/// gives them, standing in for the whole class rather than any one member of
/// it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClassLight {
    /// Typical absolute visual magnitude for the class.
    pub absolute_magnitude: f64,
    /// Typical effective temperature, kelvin.
    pub temperature: f64,
}

impl ClassLight {
    const fn new(absolute_magnitude: f64, temperature: f64) -> Self {
        ClassLight { absolute_magnitude, temperature }
    }
}

/// What an unrecognized or missing class comes to.
///
/// An M dwarf, because that is what most of the galaxy is: red dwarfs
/// outnumber everything else together, so a system with nothing on record but
/// its existence is more likely one of those than not, and guessing dim keeps
/// an unknown from crowding into a sky it has no claim to.
pub const DEFAULT_CLASS_LIGHT: ClassLight = ClassLight::new(10.0, 3400.0);

/// A typical absolute magnitude and temperature for an Elite star class.
///
/// The last link in the bake's fallback chain, reached only where a system
/// carries no scanned star to sum. It takes the class the system is named for
/// — the game's `StarClass`, a letter or two and nothing finer — and answers
/// the pair the ordering and the glow need.
///
/// The figures are representative values for each family, drawn from the
/// standard dwarf sequence (Pecaut & Mamajek 2013) for the main one and from
/// the character of each remnant and oddity for the rest, and are meant to be
/// tuned against the bake once it runs rather than settled to a decimal now.
/// What matters at this stage is the ordering they impose: hot before cool,
/// bright before dim, remnants and brown dwarfs down where their flux really
/// sits.
///
/// Matching runs specific before general. The pairs that begin with a letter
/// another family also begins with — `MS` and `S` for the S-type giants, `TTS`
/// for the T Tauri stars — are caught whole before the bare letter is read as
/// a main-sequence class, and the white dwarf, Wolf-Rayet and carbon families
/// are taken by their leading letter since nothing else wears it.
pub fn class_light(class: &str) -> ClassLight {
    let class = class.trim().to_ascii_uppercase();
    let c = class.as_str();

    // The lightless remnants and the anomalies. Their flux is negligible, so
    // where they land in the ordering matters more than the exact figure; the
    // temperature is a nominal one that never draws.
    if c == "H" || c.starts_with("SUPERMASSIVE") {
        // Black holes give off no light. A magnitude this faint is flux the
        // eye and the sensor both read as nothing.
        return ClassLight::new(DARK_MAGNITUDE, 0.0);
    }
    if c == "N" {
        // A neutron star's thermal output is almost all in X-rays; what leaks
        // into the visible is next to nothing.
        return ClassLight::new(16.0, 30000.0);
    }
    if c == "X" {
        return DEFAULT_CLASS_LIGHT;
    }

    // Pre-main-sequence, caught before the bare letters they share a lead with.
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

    // The main sequence and the brown dwarfs, by leading letter, hottest and
    // brightest to coolest and dimmest.
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
        _ => DEFAULT_CLASS_LIGHT,
    }
}

/// The effective temperature a `B-V` colour index implies, in kelvin.
///
/// Ballesteros' formula, which treats the two photometric bands as blackbody
/// samples and solves for the temperature that would produce their ratio:
/// `T = 4600 * (1/(0.92*BV + 1.7) + 1/(0.92*BV + 0.62))`. It reads the Sun's
/// 0.656 back as about 5750 K against a true 5772, and Sirius' 0.009 as about
/// 10100 against 9940.
///
/// This is the second way into [`blackbody_color`], and the important one for
/// a real catalog: measured surveys record a colour index, not a temperature,
/// so without this nothing outside the game can be drawn or checked at all.
/// Where a scanned star already carries a temperature that figure is better
/// and this is not consulted.
///
/// The fit runs cool at the hot end — it reads a B3 star at about 12,600 K
/// against a true 15,000 — and its second term diverges as `BV` approaches
/// -0.674, so the input is clamped to `-0.4..2.0`. Above roughly 20,000 K the
/// class is the better source and [`class_light`] is what to ask.
pub fn color_index_to_temperature(b_v: f64) -> f64 {
    let bv = b_v.clamp(-0.4, 2.0);
    4600.0 * (1.0 / (0.92 * bv + 1.7) + 1.0 / (0.92 * bv + 0.62))
}

/// The energy a magnitude lands on a detector, relative to one exposed for
/// `zero_point`.
///
/// The exposure law, and the one figure two renderers of the same sky must
/// agree on exactly or every comparison between them measures the gap between
/// two laws rather than between two pictures. It is [`flux`] with the exposure
/// folded in — `10^(-0.4*(m - zero_point))` — so a star exactly at the zero
/// point returns one, a magnitude brighter returns about 2.5, and each five
/// magnitudes fainter is another hundredth.
///
/// The dial is a magnitude rather than a multiplier because that is the unit
/// the decision is made in: `zero_point` is the magnitude that saturates a
/// pixel, so setting it to 1.0 means Sirius and Canopus blow out and Vega sits
/// just under, which is a sentence about a picture rather than about a number.
///
/// What is deliberately *not* here is the point-spread function and the tone
/// curve. Those are how a renderer spends the energy across pixels and then
/// compresses it for a display, and they differ legitimately between a
/// rasterizer and a shader. This is the part that may not differ.
pub fn relative_exposure(magnitude: f64, zero_point: f64) -> f64 {
    flux(magnitude - zero_point)
}

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

    /// Ten parsecs is the zero of the distance modulus: a star seen from there
    /// looks exactly its absolute magnitude.
    #[test]
    fn ten_parsecs_shows_a_star_at_its_absolute_magnitude() {
        assert!(close(apparent_magnitude(4.83, 10.0), 4.83));
    }

    /// Every tenfold further off is five magnitudes fainter.
    #[test]
    fn ten_times_the_distance_is_five_magnitudes_fainter() {
        assert!(close(apparent_magnitude(0.0, 100.0), 5.0));
        assert!(close(apparent_magnitude(0.0, 1000.0), 10.0));
    }

    /// The light-year form is the parsec one with the unit folded in, and
    /// nothing else.
    #[test]
    fn the_light_year_form_only_converts_the_distance() {
        let ly = 42.0;
        assert!(close(
            apparent_magnitude_ly(3.0, ly),
            apparent_magnitude(3.0, ly * PARSECS_PER_LY),
        ));
    }

    /// Magnitude zero is one unit of flux, and five magnitudes is a factor of
    /// a hundred, in both directions.
    #[test]
    fn five_magnitudes_is_a_hundredfold_in_flux() {
        assert!(close(flux(0.0), 1.0));
        assert!(close(flux(5.0), 0.01));
        assert!(close(flux(-5.0), 100.0));
    }

    /// Flux and magnitude are inverses, so a round trip is the identity.
    #[test]
    fn flux_and_magnitude_undo_each_other() {
        for m in [-5.0, -1.0, 0.0, 3.7, 8.0, 15.0] {
            assert!(close(magnitude(flux(m)), m));
        }
    }

    /// Two equal stars are about three quarters of a magnitude brighter
    /// together than either alone, the `2.5*log10(2)` the physics gives.
    #[test]
    fn two_equal_stars_add_by_log_two() {
        let combined = combined_magnitude([0.0, 0.0]).unwrap();
        assert!(close(combined, -2.5 * 2f64.log10()));
    }

    /// A combined source is brighter — a smaller magnitude — than its
    /// brightest member, never dimmer.
    #[test]
    fn combining_never_dims() {
        let ms = [2.0, 4.0, 7.5];
        let combined = combined_magnitude(ms).unwrap();
        let brightest = ms.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(combined < brightest);
    }

    /// No light is no magnitude.
    #[test]
    fn no_sources_have_no_magnitude() {
        assert_eq!(combined_magnitude(std::iter::empty()), None);
    }

    /// Every blackbody colour is a real one: each channel in gamut and the
    /// brightest of the three riding the ceiling, since it carries hue alone.
    #[test]
    fn a_blackbody_colour_is_normalized_and_in_range() {
        for t in [1000.0, 3000.0, 5772.0, 10000.0, 30000.0] {
            let c = blackbody_color(t);
            assert!(c.iter().all(|&ch| (0.0..=1.0).contains(&ch)));
            assert!(close(
                c.iter().cloned().fold(0.0f32, f32::max) as f64,
                1.0
            ));
        }
    }

    /// Cool stars are red: at three thousand kelvin the red channel leads the
    /// blue.
    #[test]
    fn a_cool_star_is_red() {
        let c = blackbody_color(3000.0);
        assert!(c[0] > c[2]);
    }

    /// Hot stars are blue: at twenty thousand kelvin the blue channel leads the
    /// red.
    #[test]
    fn a_hot_star_is_blue() {
        let c = blackbody_color(20000.0);
        assert!(c[2] > c[0]);
    }

    /// The Sun is a warm white: no channel starved, and red over blue.
    #[test]
    fn the_sun_is_a_warm_white() {
        let c = blackbody_color(SOLAR_TEMPERATURE);
        assert!(c.iter().all(|&ch| ch > 0.6));
        assert!(c[0] > c[2]);
    }

    /// The class table is ordered the way the sky is: hotter classes are
    /// hotter and brighter classes are brighter, down the whole main sequence.
    #[test]
    fn the_main_sequence_orders_hot_and_bright_together() {
        let seq = ["O", "B", "A", "F", "G", "K", "M"];
        for pair in seq.windows(2) {
            let hot = class_light(pair[0]);
            let cool = class_light(pair[1]);
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
        let g = class_light("G");
        assert!((g.absolute_magnitude - SOLAR_ABSOLUTE_MAGNITUDE).abs() < 1.0);
        assert!((g.temperature - SOLAR_TEMPERATURE).abs() < 500.0);
    }

    /// Class is read case- and space-insensitively, as the feeds spell it
    /// however they like.
    #[test]
    fn class_ignores_case_and_padding() {
        assert_eq!(class_light(" g "), class_light("G"));
        assert_eq!(class_light("da"), class_light("DA"));
    }

    /// The S-type tokens are giants, not the main-sequence letters they lead
    /// with: `MS` is not an M dwarf and `S` is not read as one either.
    #[test]
    fn s_type_giants_are_not_the_main_sequence() {
        assert_ne!(class_light("MS"), class_light("M"));
        assert_ne!(class_light("S"), class_light("M"));
        assert_eq!(class_light("MS"), class_light("S"));
    }

    /// T Tauri is caught whole, not read as a T dwarf, which would be far
    /// cooler and far fainter.
    #[test]
    fn t_tauri_is_not_a_brown_dwarf() {
        assert_ne!(class_light("TTS"), class_light("T"));
        assert!(class_light("TTS").temperature > class_light("T").temperature);
    }

    /// A white dwarf's subtype rides the same family figures as the bare `D`.
    #[test]
    fn white_dwarf_subtypes_share_the_family() {
        for sub in ["D", "DA", "DB", "DC", "DQ", "DAV", "DX"] {
            assert_eq!(class_light(sub), class_light("D"));
        }
    }

    /// A black hole gives off no light, so its flux is nothing the sky ever
    /// draws — a magnitude far below any limit.
    #[test]
    fn a_black_hole_is_dark() {
        let h = class_light("H");
        assert!(apparent_magnitude(h.absolute_magnitude, 10.0) > EYE_LIMIT);
        assert!(flux(h.absolute_magnitude) < 1e-10);
        assert_eq!(class_light("SupermassiveBlackHole"), h);
    }

    /// An unrecognized class falls to the default rather than to nothing.
    #[test]
    fn an_unknown_class_defaults() {
        assert_eq!(class_light("ZZZ"), DEFAULT_CLASS_LIGHT);
        assert_eq!(class_light(""), DEFAULT_CLASS_LIGHT);
    }

    /// The Sun loses nothing crossing into the visible: the V band is where its
    /// light already is, so its visual magnitude is its bolometric one.
    #[test]
    fn the_sun_keeps_its_magnitude_in_the_visible() {
        assert!(close(
            visual_magnitude(SOLAR_ABSOLUTE_MAGNITUDE, SOLAR_TEMPERATURE),
            SOLAR_ABSOLUTE_MAGNITUDE,
        ));
    }

    /// A blackbody hotter or cooler than the Sun spills more of its light out
    /// of the visible band, so it is fainter there than its total output says,
    /// and the further from the Sun's heat the fainter it grows.
    #[test]
    fn the_visible_band_dims_both_extremes() {
        let sun = visual_magnitude(0.0, SOLAR_TEMPERATURE);
        let hot = visual_magnitude(0.0, 40_000.0);
        let hotter = visual_magnitude(0.0, 100_000.0);
        let cool = visual_magnitude(0.0, 3000.0);
        let cooler = visual_magnitude(0.0, 1500.0);
        assert!(hot > sun && hotter > hot, "a hotter star dims further");
        assert!(cool > sun && cooler > cool, "a cooler star dims further");
    }

    /// A neutron star's heat is almost all in X-rays, so next to none of it
    /// reaches the visible: a scan near naked-eye brightness corrects to tens
    /// of magnitudes below any eye.
    #[test]
    fn a_neutron_star_all_but_vanishes_in_the_visible() {
        let corrected = visual_magnitude(5.0, 5_950_000.0);
        assert!(
            corrected > EYE_LIMIT + 10.0,
            "a neutron star is no naked-eye star: {corrected}"
        );
    }

    /// A black hole has no temperature and no light: whatever placeholder
    /// magnitude a scan carries, it comes to the dark figure and no flux.
    #[test]
    fn a_black_hole_has_no_visible_light() {
        assert_eq!(visual_magnitude(20.0, 0.0), DARK_MAGNITUDE);
        assert!(flux(visual_magnitude(20.0, 0.0)) < 1e-10);
    }

    /// A white dwarf keeps the brightness its scan gives, shifted only a little
    /// by its heat, so one white dwarf still differs from another — the
    /// variation a flat class figure would erase.
    #[test]
    fn a_white_dwarf_keeps_its_scanned_variation() {
        // Two white dwarfs the game scans at one bolometric magnitude but
        // different heat stay distinct, the hotter the fainter in the visible.
        let cool = visual_magnitude(12.0, 7000.0);
        let hot = visual_magnitude(12.0, 16_000.0);
        assert!(hot > cool, "the hotter white dwarf is fainter in V");
        // Neither is thrown far from its scanned figure.
        assert!((cool - 12.0).abs() < 0.5);
        assert!((hot - 12.0).abs() < 1.5);
    }
    /// The Sun's colour index reads its temperature back, which is the one
    /// anchor for this fit a reader can check by eye.
    #[test]
    fn the_suns_colour_index_gives_the_suns_temperature() {
        let t = color_index_to_temperature(0.656);
        assert!((t - SOLAR_TEMPERATURE).abs() < 100.0, "{t}");
    }

    /// Bluer is hotter, the whole content of a colour index, and it holds
    /// across the range the fit is defined on.
    #[test]
    fn a_bluer_colour_index_is_hotter() {
        let seq = [-0.3, 0.0, 0.3, 0.656, 1.0, 1.5, 2.0];
        for pair in seq.windows(2) {
            assert!(
                color_index_to_temperature(pair[0])
                    > color_index_to_temperature(pair[1]),
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
            let t = color_index_to_temperature(bv);
            assert!(t.is_finite() && t > 0.0, "{bv} gave {t}");
        }
    }

    /// A colour index and a class agree on a Sun-like star, which is the check
    /// that the two independent routes into a temperature meet.
    #[test]
    fn the_two_routes_to_a_temperature_agree_on_a_g_star() {
        let from_colour = color_index_to_temperature(0.656);
        let from_class = class_light("G").temperature;
        assert!((from_colour - from_class).abs() < 500.0);
    }

    /// A star at the zero point is one unit of exposure, and the scale stays
    /// the magnitude scale either side of it.
    #[test]
    fn the_zero_point_is_one_unit_of_exposure() {
        assert!(close(relative_exposure(1.0, 1.0), 1.0));
        assert!(close(relative_exposure(6.0, 1.0), 0.01));
        assert!(close(relative_exposure(-4.0, 1.0), 100.0));
    }

    /// Turning the exposure up by a magnitude brightens everything by the same
    /// factor, which is what makes it one dial rather than a per-star one.
    #[test]
    fn exposure_scales_every_star_alike() {
        let ms = [-1.4, 0.03, 4.83, 8.0];
        let ratios: Vec<f64> = ms
            .iter()
            .map(|&m| relative_exposure(m, 2.0) / relative_exposure(m, 1.0))
            .collect();
        for r in &ratios {
            assert!(close(*r, ratios[0]));
        }
    }
}
