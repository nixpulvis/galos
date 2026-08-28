//! Where the eye stands, which way it looks, and what it makes of the light.
//!
//! A [`Camera`] is a place, a direction, a field of view, and two dials that
//! decide what a magnitude comes to on a screen. Everything else in this crate
//! serves it.
//!
//! # Placing and pointing
//!
//! Position is light years in whatever frame the stars are in — a catalog's
//! equatorial axes, or galactic ones if they have been turned by
//! [`galos_catalog::frame`]. The camera does not care which, only that it and
//! the stars agree.
//!
//! Pointing is [`Camera::looking_from`], which takes where to stand and what
//! to look at, because that is how the question is actually asked: *stand on
//! Sol and look at Sirius*, not *stand on Sol with this quaternion*. Roll is
//! chosen for you from an up vector, and the default up is the frame's `+z`,
//! so a rendered sky comes out the same way twice.
//!
//! # The two dials
//!
//! **Exposure** is a magnitude: the one that fills a pixel. It reads as a
//! magnitude because that is the unit the decision is made in — a picture is
//! set up by saying how faint a star should still register, not by picking a
//! multiplier.
//!
//! The default is 6.0, near the naked-eye limit, which puts the brightest few
//! dozen stars well into saturation and leaves a field of fainter ones behind
//! them. Turning it down to 1.0 does not give a naked-eye sky but a nearly
//! empty one: at that setting even Sirius peaks at about three quarters of
//! full scale and a sixth-magnitude star lands three parts in 255, which is
//! not black but is not a star anybody sees either.
//!
//! **Seeing** is the width of the point-spread function in pixels, which sets
//! how large a star of a given brightness draws. Together they are the whole
//! of the response, and the magnitude half of it lives in
//! [`galos_photometry::relative_exposure`] rather than here, so that a GPU
//! renderer adopting the law is running the same physics rather than a second
//! copy of it.

use crate::image::{Image, Mark};
use galos_catalog::Star;
use galos_photometry::psf::Gaussian;
use galos_photometry::{apparent_magnitude_ly, blackbody_color, relative_exposure};

/// The energy per pixel below which this renderer shows nothing.
///
/// A pixel holding less than this tone-maps to under one part in 255 through
/// [`Image::to_srgb8`]'s curve and cannot be seen, so a star's disc is cut
/// where it falls below. It belongs to this renderer rather than to
/// [`galos_photometry::psf`] precisely because it follows from that curve: a
/// GPU pipeline with its own tonemapper has a different one, and passes its
/// own.
///
/// Small enough that the truncated tail is a fraction of a percent of a star's
/// light, which is what keeps the sum over an image close to the sum over the
/// stars in it.
pub const FLOOR: f64 = 1e-4;

/// A place to stand, a way to look, and a response to light.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    /// Where the eye is, light years, in the stars' own frame.
    pub position: [f64; 3],
    /// The unit direction looked along.
    pub forward: [f64; 3],
    /// The unit up direction, which fixes the roll.
    pub up: [f64; 3],
    /// Vertical field of view, radians.
    pub fov_y: f64,
    /// Image width, pixels.
    pub width: u32,
    /// Image height, pixels.
    pub height: u32,
    /// The apparent magnitude that fills a pixel.
    pub exposure: f64,
    /// The point-spread function's width, pixels.
    pub seeing: f64,
}

impl Camera {
    /// A camera of the given size, at the origin, looking down `+x`.
    ///
    /// The defaults are a sixty-degree field — a normal lens — an exposure at
    /// magnitude 6, near the naked-eye limit, and seeing of 1.8 pixels. That
    /// combination is the one the golden image is drawn at, and it is what
    /// gives a sky in which the bright stars dominate a field of fainter ones
    /// rather than sitting among them.
    pub fn new(width: u32, height: u32) -> Camera {
        Camera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov_y: 60f64.to_radians(),
            width: width.max(1),
            height: height.max(1),
            exposure: 6.0,
            seeing: 1.8,
        }
    }

    /// Stand at `from` and look at `at`.
    ///
    /// The way the question is asked. Where the two coincide, or the look
    /// direction is straight up the current up vector, the camera is left
    /// pointing somewhere valid rather than filled with NaN — a degenerate
    /// aim should give a boring picture, not a blank one.
    pub fn looking_from(mut self, from: [f64; 3], at: [f64; 3]) -> Camera {
        self.position = from;
        let direction = sub(at, from);
        self.forward = normalize(direction).unwrap_or([1.0, 0.0, 0.0]);
        // Any up not parallel to the aim will do; the frame's `+z` unless the
        // camera is looking along it, in which case `+y`.
        let up = [0.0, 0.0, 1.0];
        self.up = if cross(self.forward, up).iter().all(|c| c.abs() < 1e-9) {
            [0.0, 1.0, 0.0]
        } else {
            up
        };
        self
    }

    /// Point the camera along a direction rather than at a place.
    pub fn looking_along(self, direction: [f64; 3]) -> Camera {
        let from = self.position;
        let at = add(from, direction);
        self.looking_from(from, at)
    }

    /// Set the vertical field of view, in degrees.
    pub fn with_fov_degrees(mut self, degrees: f64) -> Camera {
        self.fov_y = degrees.clamp(0.001, 179.0).to_radians();
        self
    }

    /// Set the magnitude that fills a pixel.
    pub fn with_exposure(mut self, magnitude: f64) -> Camera {
        self.exposure = magnitude;
        self
    }

    /// Set the point-spread function's width, in pixels.
    pub fn with_seeing(mut self, pixels: f64) -> Camera {
        self.seeing = pixels.max(1e-3);
        self
    }

    /// Set the roll explicitly, by an up direction.
    pub fn with_up(mut self, up: [f64; 3]) -> Camera {
        if let Some(up) = normalize(up) {
            self.up = up;
        }
        self
    }

    /// Where a point in space lands on the image, and how far away it is.
    ///
    /// [`None`] when it falls behind the camera. A point outside the frame
    /// still answers, with coordinates off the edge, because a star just past
    /// the border still spills light into the picture through its disc and
    /// dropping it here would cut discs off at the frame edge.
    pub fn project(&self, point: [f64; 3]) -> Option<(f64, f64, f64)> {
        let relative = sub(point, self.position);
        let depth = dot(relative, self.forward);
        if depth <= 0.0 {
            return None;
        }

        let right = normalize(cross(self.forward, self.up))?;
        let up = cross(right, self.forward);

        let tan_half_y = (self.fov_y * 0.5).tan();
        let tan_half_x = tan_half_y * self.width as f64 / self.height as f64;

        let ndc_x = dot(relative, right) / (depth * tan_half_x);
        let ndc_y = dot(relative, up) / (depth * tan_half_y);

        let x = (ndc_x + 1.0) * 0.5 * self.width as f64;
        let y = (1.0 - ndc_y) * 0.5 * self.height as f64;
        let distance = dot(relative, relative).sqrt();
        Some((x, y, distance))
    }

    /// Where to ring a point, if it is in front of the camera.
    ///
    /// The overlay's other half: [`project`](Self::project) says where
    /// something lands and this turns that into a [`Mark`] the image can draw.
    /// Split from the drawing so the same answer serves an annotation, a
    /// caption, or a list of screen positions handed to another renderer to
    /// compare against — which is what this is really for. A star ringed here
    /// and the same star drawn by `galos_map` under the same camera should
    /// coincide, and a ring is how a person checks that in one glance.
    ///
    /// The radius is in pixels and is the ring's, not the star's: a mark is
    /// chrome, so it is a fixed size rather than one that grows with the light.
    pub fn mark(&self, point: [f64; 3], radius: f64) -> Option<Mark> {
        let (x, y, _) = self.project(point)?;
        Some(Mark::new(x, y, radius))
    }

    /// Rings for every one of a set of points that is in frame.
    pub fn marks(
        &self,
        points: impl IntoIterator<Item = [f64; 3]>,
        radius: f64,
    ) -> Vec<Mark> {
        points.into_iter().filter_map(|p| self.mark(p, radius)).collect()
    }

    /// How bright a star looks from here, and what colour, and how much energy
    /// that comes to at this exposure.
    ///
    /// The whole photometric path in one place: the distance modulus for the
    /// magnitude, the blackbody for the tint, the exposure law for the energy.
    /// Every one of the three is [`galos_photometry`]'s, which is what makes
    /// this renderer a check on that crate rather than a second opinion.
    fn light(&self, star: &Star, distance: f64) -> (f64, [f32; 3]) {
        let apparent =
            apparent_magnitude_ly(star.absolute_magnitude, distance);
        let energy = relative_exposure(apparent, self.exposure);
        (energy, blackbody_color(star.temperature()))
    }

    /// Draw every star that lands in frame.
    ///
    /// A loop over the whole catalog, deliberately. Nothing is pruned by an
    /// index, because a renderer that pruned the way the map prunes could not
    /// be used to check the map's pruning.
    pub fn render(&self, stars: &[Star]) -> Image {
        let mut image = Image::new(self.width, self.height);
        let psf = Gaussian::new(self.seeing);

        for star in stars {
            let Some((cx, cy, distance)) = self.project(star.position) else {
                continue;
            };
            if distance <= 0.0 {
                continue;
            }
            let (energy, tint) = self.light(star, distance);
            let Some(radius) = psf.radius(energy, FLOOR) else { continue };

            // Only the pixels the disc actually reaches, which is why a
            // faint star costs a handful of them and Sirius costs a few
            // thousand.
            let n = radius.ceil() as i64;
            let (px, py) = (cx.floor() as i64, cy.floor() as i64);
            if px + n < 0
                || py + n < 0
                || px - n >= self.width as i64
                || py - n >= self.height as i64
            {
                continue;
            }
            for y in (py - n)..=(py + n) {
                for x in (px - n)..=(px + n) {
                    // Sampled at the pixel's centre, so a star between two
                    // pixels lights both rather than snapping to one.
                    let dx = x as f64 + 0.5 - cx;
                    let dy = y as f64 + 0.5 - cy;
                    let d = (dx * dx + dy * dy).sqrt();
                    if d > radius {
                        continue;
                    }
                    let value = psf.at(energy, d) as f32;
                    image.add(
                        x,
                        y,
                        [value * tint[0], value * tint[1], value * tint[2]],
                    );
                }
            }
        }

        image
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f64; 3]) -> Option<[f64; 3]> {
    let length = dot(v, v).sqrt();
    (length > 1e-12).then(|| [v[0] / length, v[1] / length, v[2] / length])
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_catalog::hyg;

    fn bright() -> Vec<Star> {
        hyg::read(include_str!("../../galos_catalog/data/bright.csv").as_bytes())
            .expect("the fixture is a HYG catalog")
            .0
    }

    fn named(stars: &[Star], name: &str) -> Star {
        stars
            .iter()
            .find(|s| s.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{name} should be in the fixture"))
            .clone()
    }

    /// What is aimed at lands in the middle of the frame. The one property
    /// "place it and point it" has to have.
    #[test]
    fn what_is_aimed_at_lands_in_the_centre() {
        let stars = bright();
        let sirius = named(&stars, "Sirius");
        let camera =
            Camera::new(800, 600).looking_from([0.0; 3], sirius.position);
        let (x, y, distance) =
            camera.project(sirius.position).expect("in front of the camera");
        assert!((x - 400.0).abs() < 0.01, "{x}");
        assert!((y - 300.0).abs() < 0.01, "{y}");
        // The catalog rounds its coordinates and its distance column
        // separately, so the two agree to catalog precision rather than
        // exactly.
        assert!((distance - sirius.distance).abs() < 0.01, "{distance}");
    }

    /// A star behind the eye is not drawn in front of it.
    #[test]
    fn what_is_behind_is_not_projected() {
        let camera = Camera::new(64, 64).looking_along([1.0, 0.0, 0.0]);
        assert_eq!(camera.project([-10.0, 0.0, 0.0]), None);
    }

    /// A narrower field magnifies: the same star moves further from the centre
    /// as the field closes, which is what a field of view means.
    #[test]
    fn a_narrower_field_pushes_stars_outward() {
        let camera = Camera::new(800, 800).looking_along([1.0, 0.0, 0.0]);
        let off_axis = [10.0, 1.0, 0.0];
        let wide = camera.clone().with_fov_degrees(90.0);
        let narrow = camera.with_fov_degrees(30.0);
        let (wx, _, _) = wide.project(off_axis).unwrap();
        let (nx, _, _) = narrow.project(off_axis).unwrap();
        assert!(
            (nx - 400.0).abs() > (wx - 400.0).abs(),
            "narrow {nx}, wide {wx}"
        );
    }

    /// Aiming a camera at where it already stands leaves it pointing
    /// somewhere, rather than filling the picture with NaN.
    #[test]
    fn a_degenerate_aim_is_survivable() {
        let camera = Camera::new(32, 32).looking_from([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]);
        assert!(camera.forward.iter().all(|c| c.is_finite()));
        let image = camera.render(&bright());
        assert!(image.pixels().iter().flatten().all(|c| c.is_finite()));
    }

    /// Looking straight up the default up vector still gives a valid frame.
    #[test]
    fn looking_along_the_up_axis_still_has_a_roll() {
        let camera = Camera::new(32, 32).looking_along([0.0, 0.0, 1.0]);
        let star = [0.0, 0.1, 10.0];
        let projected = camera.project(star);
        assert!(projected.is_some_and(|(x, y, _)| x.is_finite() && y.is_finite()));
    }

    /// **Sirius is the brightest thing in the sky.** Point a camera at each of
    /// the brightest stars in turn and the picture of Sirius holds more light
    /// than any other — which it should, because that is what "brightest star
    /// in the sky" means, and it is a fact about the world rather than about
    /// this code.
    #[test]
    fn sirius_makes_the_brightest_picture() {
        let stars = bright();
        let mut ranked: Vec<(String, f64)> = ["Sirius", "Canopus", "Vega", "Betelgeuse", "Procyon"]
            .iter()
            .map(|name| {
                let star = named(&stars, name);
                let camera = Camera::new(200, 200)
                    .looking_from([0.0; 3], star.position)
                    .with_fov_degrees(5.0);
                (name.to_string(), camera.render(&[star]).total_energy())
            })
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        assert_eq!(ranked[0].0, "Sirius", "{ranked:?}");
    }

    /// A star's colour is its temperature's: Betelgeuse draws red and Rigel
    /// blue, in the picture and not merely in the table.
    #[test]
    fn the_picture_carries_the_stars_colour() {
        let stars = bright();
        for (name, red_leads) in [("Betelgeuse", true), ("Rigel", false)] {
            let star = named(&stars, name);
            let camera = Camera::new(64, 64)
                .looking_from([0.0; 3], star.position)
                .with_fov_degrees(5.0);
            let image = camera.render(&[star]);
            let centre = image.pixels()[64 * 32 + 32];
            assert_eq!(
                centre[0] > centre[2],
                red_leads,
                "{name} came out {centre:?}"
            );
        }
    }

    /// Turning the exposure up by a magnitude puts about two and a half times
    /// the light in the picture, because that is what a magnitude is. The
    /// check that the response law reaches the pixels rather than stopping at
    /// the function.
    #[test]
    fn a_magnitude_of_exposure_is_two_and_a_half_times_the_light() {
        let stars = bright();
        let vega = named(&stars, "Vega");
        let camera = Camera::new(400, 400)
            .looking_from([0.0; 3], vega.position)
            .with_fov_degrees(5.0);
        let dim = camera.clone().with_exposure(6.0).render(&[vega.clone()]);
        let bright = camera.with_exposure(7.0).render(&[vega]);
        let ratio = bright.total_energy() / dim.total_energy();
        assert!((ratio - 10f64.powf(0.4)).abs() < 0.05, "{ratio}");
    }

    /// The light in the picture is the light of the stars in it: the PSF
    /// spends a star's energy across pixels without inventing or losing any.
    #[test]
    fn the_picture_holds_the_light_that_went_into_it() {
        let stars = bright();
        let vega = named(&stars, "Vega");
        let camera = Camera::new(600, 600)
            .looking_from([0.0; 3], vega.position)
            .with_fov_degrees(5.0)
            .with_exposure(4.0);
        let image = camera.render(&[vega.clone()]);

        let apparent =
            apparent_magnitude_ly(vega.absolute_magnitude, vega.distance);
        let expected = relative_exposure(apparent, 4.0);

        // The tint has to be divided back out, and *that it does* is a
        // finding rather than an incidental. `blackbody_color` normalizes so
        // its brightest channel is one, which makes the three channels sum to
        // between 1.6 and 2.8 depending on temperature — see
        // `a_stars_colour_should_not_change_how_bright_it_is` below. Until
        // that is settled this test can only check that the PSF conserves
        // whatever energy it was handed, which is what it is for.
        let tint = blackbody_color(vega.temperature());
        let scale: f64 = tint.iter().map(|&c| c as f64).sum();
        let ratio = image.total_energy() / (expected * scale);
        assert!((ratio - 1.0).abs() < 0.02, "{ratio}");
    }

    /// A whole sky renders, and it is neither black nor blown out — the
    /// end-to-end check that eighty measured stars come through the whole path
    /// and land somewhere a picture can show them.
    #[test]
    fn the_whole_fixture_renders_to_a_visible_sky() {
        let camera = Camera::new(320, 240)
            .looking_from([0.0; 3], [0.0, 1.0, 0.0])
            .with_fov_degrees(120.0)
            .with_exposure(3.0);
        let image = camera.render(&bright());
        assert!(image.total_energy() > 0.0, "the sky came out black");
        assert!(image.peak().is_finite());
        let lit = image.pixels().iter().filter(|p| p[0] + p[1] + p[2] > 1e-3).count();
        assert!(lit > 10, "only {lit} pixels lit");
    }

    /// **A star's colour should not change how bright it is drawn, and it
    /// does.**
    ///
    /// `blackbody_color` normalizes a tint so its brightest channel is one,
    /// which carries hue correctly and luminance not at all: a white tint
    /// spreads across three channels and a saturated one concentrates in a
    /// single one, so multiplying flux by the tint makes a Sun-like star draw
    /// brighter than an equally luminous red giant or hot blue star.
    ///
    /// The size of it, in relative luminance of the tint alone:
    ///
    /// | temperature | luminance | penalty vs a G star |
    /// |---|---|---|
    /// | 3000 K | 0.566 | 0.50 mag |
    /// | 5772 K | 0.900 | — |
    /// | 30000 K | 0.519 | 0.60 mag |
    ///
    /// That is a factor of 1.7, or about 0.6 magnitudes, applied purely for
    /// being red or blue rather than white, and it is not monotonic: both ends
    /// lose and the middle wins. It reaches `galos_map` too, whose cell tint is
    /// a flux-weighted `blackbody_color`, so a cell of hot stars is drawn
    /// dimmer than a white cell of the same luminosity.
    ///
    /// It is a computed defect rather than one any single frame shows. Two
    /// stars far apart in temperature are usually also far apart in magnitude,
    /// and the penalty is flat across the middle of the range — Betelgeuse at
    /// 3794 K and Rigel at 10516 K both sit at 0.680, so it cannot be what
    /// separates them in a picture of Orion.
    ///
    /// The fix is to normalize the tint to unit *luminance* rather than unit
    /// peak, but that is a change to a contract two renderers share, so this
    /// test records the defect rather than asserting the fix. It fails the day
    /// somebody makes it right, which is when to delete it.
    #[test]
    fn a_stars_colour_should_not_change_how_bright_it_is() {
        let luminance = |t: f64| {
            let c = blackbody_color(t);
            0.2126 * c[0] as f64 + 0.7152 * c[1] as f64 + 0.0722 * c[2] as f64
        };
        let white = luminance(5772.0);
        let red = luminance(3000.0);
        let blue = luminance(30000.0);

        // What it should be: colour carries hue, flux carries brightness, so
        // every tint has the same luminance and this passes.
        let flat = (white - red).abs() < 0.05 && (white - blue).abs() < 0.05;
        assert!(!flat, "the tint is luminance-normalized now; delete this test");

        // What it is: white is drawn over half a magnitude brighter than
        // either end for no physical reason.
        let penalty = -2.5 * (red / white).log10();
        assert!(penalty > 0.4, "red is penalized by {penalty:.2} magnitudes");
    }


    /// A ring lands on what it rings, which is the only thing an overlay has
    /// to get right.
    #[test]
    fn a_mark_lands_on_its_star() {
        let stars = bright();
        let vega = named(&stars, "Vega");
        let camera = Camera::new(300, 300).looking_from([0.0; 3], vega.position);
        let mark = camera.mark(vega.position, 9.0).expect("in front");
        assert!((mark.x - 150.0).abs() < 0.01 && (mark.y - 150.0).abs() < 0.01);
        assert_eq!(mark.radius, 9.0);
    }

    /// Nothing behind the camera is ringed, and a set of points comes back as
    /// only those in front of it.
    #[test]
    fn marks_skip_what_is_behind() {
        let camera = Camera::new(64, 64).looking_along([1.0, 0.0, 0.0]);
        assert_eq!(camera.mark([-5.0, 0.0, 0.0], 4.0), None);
        let marks =
            camera.marks([[10.0, 0.0, 0.0], [-10.0, 0.0, 0.0]], 4.0);
        assert_eq!(marks.len(), 1);
    }

}
