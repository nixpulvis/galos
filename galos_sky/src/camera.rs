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
//! **Seeing** is the width of the point-spread function, and it is an *angle*
//! — arcminutes — not a pixel count, because a star's blur on the sky is the
//! same blur whatever lens frames it. The renderer turns it into pixels at the
//! plate scale the field and image size imply, so one setting draws the same
//! sky whether it is a wide field or a tight crop. Below about a pixel a star
//! falls between samples and cannot be drawn smaller, so a coarse or very wide
//! frame is held at [`MIN_SEEING_PX`] — sampling-limited rather than
//! seeing-limited, the standard "seeing or pixel scale, whichever is coarser".
//! Its default is measured; see [`DEFAULT_SEEING_ARCMIN`].
//!
//! Together the two dials are the whole of the response, and the magnitude half
//! of it lives in [`galos_photometry::relative_exposure`] rather than here, so
//! that a GPU renderer adopting the law is running the same physics rather than
//! a second copy of it.

use crate::image::{Image, Mark, Segment};
use galos_catalog::Star;
use galos_catalog::asterism::Figures;
use galos_photometry::psf::{
    AUREOLE_BETA, AUREOLE_WEIGHT, AUREOLE_WIDTH, Kernel, Profile, Psf,
};
use galos_photometry::{
    apparent_magnitude_ly, blackbody_color, relative_exposure,
};
use std::collections::HashMap;

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

/// How far a figure line stops short of the star it joins, sized to the bright
/// disc it must clear.
///
/// The line ends where the star stops being solid: [`GAP_FLOOR`] is the profile
/// value that counts as the bright disc's edge — well above [`FLOOR`], the faint
/// edge — and the line stops a small [`GAP_MARGIN`] beyond the radius the star
/// clears it at. Bounded so a faint star still parts from its lines and a
/// brilliant one does not push them out of the figure.
const GAP_FLOOR: f64 = 0.5;
const GAP_MARGIN: f64 = 2.0;
const GAP_MIN: f64 = 3.0;
const GAP_MAX: f64 = 32.0;

/// A star position as an exact hash key: a figure resolves to the very
/// positions its stars carry, so the bit patterns match and no tolerance is
/// wanted.
fn position_key(position: [f64; 3]) -> [u64; 3] {
    [position[0].to_bits(), position[1].to_bits(), position[2].to_bits()]
}

/// The default stellar core width, in arcminutes.
///
/// Tuned against the reference photo `AstroHub-OrionConstellation-02` so the
/// default render draws the crisp, roughly two-pixel faint stars it shows. It
/// is finer than the photo's own ~1.9′ seeing disc because the default's deep
/// exposure and the tone curve widen a core's apparent size, so a tight one is
/// what keeps the faint field sharp. At a wide field this is sub-pixel and held
/// at [`MIN_SEEING_PX`]; it only bites once the field is narrow enough to
/// resolve it.
pub const DEFAULT_SEEING_ARCMIN: f64 = 0.5;

/// The narrowest a star's core is ever drawn, in pixels.
///
/// Seeing is an angle, so a wide enough field or a small enough image asks for
/// a core finer than a pixel. The render can draw one — [`Camera::render`]
/// supersamples a sharp core so it stays the right brightness rather than
/// spiking a single pixel — but not one finer than the subsample grid resolves,
/// and below that a "star" is only aliasing between pixels. So this is the true
/// sampling limit rather than a matter of taste, well under a pixel, and a
/// [`seeing`](Camera::seeing_arcmin) that asks for less is held here.
pub const MIN_SEEING_PX: f64 = 0.3;

/// A halo laid behind the seeing core: one broad, faint layer of the point
/// spread.
///
/// A bright star is not a disc but a disc with a glow around it — light the air
/// and the optics scatter into a wide aureole that falls off far more slowly
/// than the seeing core. This is one such layer; a [`Camera`] carries a list of
/// them and lays each behind the core, so a halo can be tuned or stacked
/// without touching the core that sets the faint stars. See
/// [`galos_photometry::psf::Psf`] for how the layers combine.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aureole {
    /// The share of a star's light in this halo rather than the core, `0..1`.
    pub weight: f64,
    /// How much broader than the seeing core the halo is, as a multiple of the
    /// core width.
    pub width: f64,
    /// The halo's Moffat wing index: smaller is heavier and reaches further.
    pub beta: f64,
}

impl Aureole {
    /// The halo measured from the reference photograph, and the one a camera
    /// wears unless told otherwise.
    pub const DEFAULT: Aureole = Aureole {
        weight: AUREOLE_WEIGHT,
        width: AUREOLE_WIDTH,
        beta: AUREOLE_BETA,
    };
}

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
    /// The stellar core width, in arcminutes; see [`DEFAULT_SEEING_ARCMIN`].
    /// An angle rather than a pixel count — [`seeing_pixels`](Self::seeing_pixels)
    /// turns it into the width the point-spread function draws with.
    pub seeing_arcmin: f64,
    /// Which point-spread profile the core lands as; see [`Profile`].
    pub profile: Profile,
    /// The halos laid behind the core, base first; see [`Aureole`]. Empty is a
    /// plain seeing disc with no glow.
    pub aureoles: Vec<Aureole>,
}

impl Camera {
    /// A camera of the given size, at the origin, looking down `+x`.
    ///
    /// The defaults are a sixty-degree field — a normal lens — an exposure at
    /// magnitude 7.5, deep enough to saturate the bright stars so their halos
    /// bloom, the tight [`DEFAULT_SEEING_ARCMIN`] core, and the reference
    /// [`Aureole::DEFAULT`] glow behind it. That combination gives a sky of
    /// crisp faint stars under a handful of haloed bright ones.
    pub fn new(width: u32, height: u32) -> Camera {
        Camera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov_y: 60f64.to_radians(),
            width: width.max(1),
            height: height.max(1),
            exposure: 6.0,
            seeing_arcmin: DEFAULT_SEEING_ARCMIN,
            profile: Profile::default(),
            aureoles: vec![Aureole::DEFAULT],
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

    /// Set the stellar core width, in arcminutes.
    pub fn with_seeing(mut self, arcminutes: f64) -> Camera {
        self.seeing_arcmin = arcminutes.max(0.0);
        self
    }

    /// The core width the point-spread function is drawn with, in pixels.
    ///
    /// [`seeing_arcmin`](Self::seeing_arcmin) turned into pixels at this
    /// camera's plate scale — the vertical field spread over the image
    /// height — then held no finer than [`MIN_SEEING_PX`], since a core below a
    /// pixel cannot be sampled. This is the width the render loop hands the
    /// [`Psf`], and where seeing stops being an angle and becomes pixels.
    pub fn seeing_pixels(&self) -> f64 {
        let pixels_per_arcminute =
            self.height as f64 / (self.fov_y.to_degrees() * 60.0);
        (self.seeing_arcmin * pixels_per_arcminute).max(MIN_SEEING_PX)
    }

    /// Set the point-spread profile: a Moffat with its wings, or a Gaussian.
    pub fn with_profile(mut self, profile: Profile) -> Camera {
        self.profile = profile;
        self
    }

    /// Lay a halo behind the seeing core — one more layer of the point spread.
    pub fn with_aureole(mut self, aureole: Aureole) -> Camera {
        self.aureoles.push(aureole);
        self
    }

    /// Drop every halo, leaving a plain seeing disc.
    pub fn without_aureoles(mut self) -> Camera {
        self.aureoles.clear();
        self
    }

    /// The point spread this camera draws with: the seeing core, with every
    /// [`Aureole`] laid behind it.
    ///
    /// Each halo's [`weight`](Aureole::weight) is its share of the whole star's
    /// light; the core takes the rest. The share is turned into the relative
    /// weight a [`Psf`] layer wants — `share / (1 − total halo share)` against a
    /// base of one — so however many halos are stacked, each ends up with the
    /// fraction it asked for and the core with what is left.
    pub fn psf(&self) -> Psf {
        let core = self.seeing_pixels();
        let halo_share: f64 =
            self.aureoles.iter().map(|a| a.weight).sum::<f64>().min(0.95);
        let mut psf = Psf::new(self.profile, core);
        for aureole in &self.aureoles {
            let relative = aureole.weight / (1.0 - halo_share);
            psf = psf.with_layer(
                Kernel::moffat(core * aureole.width, aureole.beta),
                relative,
            );
        }
        psf
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

    /// Where to ring a *bearing* rather than a place.
    ///
    /// For the stars the catalog locates on the sky but not in space — see
    /// [`galos_catalog::hyg::Unplaced`]. They have a measured direction and no
    /// distance, so there is exactly one viewpoint from which the bearing says
    /// where the star is: the one it was measured from, which for every survey
    /// in this crate's reach is Sol.
    ///
    /// So this answers [`None`] unless the camera stands at the frame's origin.
    /// That is not Sol being privileged as a place to stand — the renderer does
    /// not care where it is — it is the measurement being what it is. From ten
    /// light years away the star is somewhere along that line and the picture
    /// cannot say where, and a ring drawn anyway would be pointing at a guess.
    pub fn mark_bearing(&self, bearing: [f64; 3], radius: f64) -> Option<Mark> {
        if dot(self.position, self.position) > 1e-12 {
            return None;
        }
        // A bearing is a point on a unit sphere about the eye. Any positive
        // multiple projects the same, since the projection divides by depth.
        self.mark(bearing, radius)
    }

    /// Whether a ring would show: any of it inside the picture.
    ///
    /// [`project`](Self::project) deliberately answers for points outside the
    /// frame, because a star just past the border still spills light in through
    /// its disc. A *mark* has no such spill — it is either visible or it is
    /// not — so anything counting or reporting marks should count these rather
    /// than the ones merely in front of the camera. The two differ by a lot:
    /// over a sixty-degree field most of the sky is in front of the eye and
    /// off the picture.
    pub fn frames(&self, mark: &Mark) -> bool {
        mark.x + mark.radius >= 0.0
            && mark.y + mark.radius >= 0.0
            && mark.x - mark.radius < self.width as f64
            && mark.y - mark.radius < self.height as f64
    }

    /// Rings for every one of a set of points that is in frame.
    pub fn marks(
        &self,
        points: impl IntoIterator<Item = [f64; 3]>,
        radius: f64,
    ) -> Vec<Mark> {
        points.into_iter().filter_map(|p| self.mark(p, radius)).collect()
    }

    /// The line between two points, if both are in front of the camera.
    ///
    /// [`None`] when either endpoint is behind the eye, since a line to a star
    /// the camera cannot see is not a line it can draw — one whose endpoint sat
    /// behind would wrap across the frame. Either endpoint may be off the edge,
    /// as [`mark`](Self::mark) allows, because a figure line into a star just
    /// past the border still crosses the picture.
    pub fn segment(&self, from: [f64; 3], to: [f64; 3]) -> Option<Segment> {
        let (x0, y0, _) = self.project(from)?;
        let (x1, y1, _) = self.project(to)?;
        Some(Segment::new(x0, y0, x1, y1))
    }

    /// The figure lines a set of [`Figures`] draws over these stars.
    ///
    /// The seam that keeps this renderer clear of where a figure came from: it
    /// takes any [`Figures`] provider — a parsed Stellarium file, another
    /// format, figures keyed by name rather than number — asks it for the
    /// segments those stars resolve to, and projects each into the frame. A
    /// segment with an endpoint behind the camera is dropped by
    /// [`segment`](Self::segment).
    pub fn figure_lines(
        &self,
        stars: &[Star],
        figures: &impl Figures,
    ) -> Vec<Segment> {
        let psf = self.psf();
        // Each star's position back to itself, so a resolved endpoint can be
        // sized: the gap a line stops short by is that star's own bright disc.
        let by_position: HashMap<[u64; 3], &Star> =
            stars.iter().map(|s| (position_key(s.position), s)).collect();
        let gap = |at: [f64; 3]| -> f64 {
            let Some(star) = by_position.get(&position_key(at)) else {
                return GAP_MIN;
            };
            let energy = relative_exposure(
                apparent_magnitude_ly(star.absolute_magnitude, star.distance),
                self.exposure,
            );
            psf.radius(energy, GAP_FLOOR)
                .map_or(GAP_MIN, |r| (r + GAP_MARGIN).clamp(GAP_MIN, GAP_MAX))
        };
        figures
            .segments(stars)
            .into_iter()
            .filter_map(|[from, to]| {
                Some(self.segment(from, to)?.with_gaps(gap(from), gap(to)))
            })
            .collect()
    }

    /// How bright a star looks from here, and what colour, and how much energy
    /// that comes to at this exposure.
    ///
    /// The whole photometric path in one place: the distance modulus for the
    /// magnitude, the blackbody for the tint, the exposure law for the energy.
    /// Every one of the three is [`galos_photometry`]'s, which is what makes
    /// this renderer a check on that crate rather than a second opinion.
    fn light(&self, star: &Star, distance: f64) -> (f64, [f32; 3]) {
        let apparent = apparent_magnitude_ly(star.absolute_magnitude, distance);
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
        let psf = self.psf();

        // A core finer than a pixel varies too fast across one to read at its
        // centre: point-sampled, it spikes the middle pixel and the star comes
        // out too bright. So a sharp core is supersampled — a pixel's value is
        // the profile averaged over a grid within it, its share of the light
        // rather than its centre's height. An ordinary-width core reads fine at
        // its centre and takes a single sample; only a sharp one pays for the
        // grid, and only near the middle where the profile actually bends.
        let core = self.seeing_pixels();
        let grid = if core >= 1.5 {
            1
        } else {
            (2.0 / core).ceil().clamp(1.0, 4.0) as i64
        };
        let supersampled = (3.0 * core).max(2.0);

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
                    let value = if grid > 1 && d < supersampled {
                        let mut acc = 0.0;
                        for sy in 0..grid {
                            for sx in 0..grid {
                                let ox =
                                    x as f64 + (sx as f64 + 0.5) / grid as f64;
                                let oy =
                                    y as f64 + (sy as f64 + 0.5) / grid as f64;
                                let (ex, ey) = (ox - cx, oy - cy);
                                acc +=
                                    psf.at(energy, (ex * ex + ey * ey).sqrt());
                            }
                        }
                        (acc / (grid * grid) as f64) as f32
                    } else {
                        psf.at(energy, d) as f32
                    };
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
        hyg::read(
            include_str!("../../galos_catalog/data/bright.csv").as_bytes(),
        )
        .expect("the fixture is a HYG catalog")
        .stars
    }

    fn named(stars: &[Star], name: &str) -> Star {
        stars
            .iter()
            .find(|s| s.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{name} should be in the fixture"))
            .clone()
    }

    /// Seeing is an angle: the pixels a core spans are its arcminutes scaled by
    /// the plate — the vertical field over the image height — so halving the
    /// field over the same image doubles them. Set explicitly here so the check
    /// is on the scaling, not on whatever the default happens to be.
    #[test]
    fn seeing_is_an_angle_scaled_to_the_plate() {
        let wide =
            Camera::new(1330, 560).with_fov_degrees(10.0).with_seeing(2.0);
        let expected = 2.0 * 560.0 / (10.0 * 60.0); // arcmin → px at this plate
        assert!((wide.seeing_pixels() - expected).abs() < 1e-9);
        // Same sky, half the field: twice the pixels for the same angle.
        let tight =
            Camera::new(1330, 560).with_fov_degrees(5.0).with_seeing(2.0);
        assert!(
            (tight.seeing_pixels() - 2.0 * wide.seeing_pixels()).abs() < 1e-9
        );
    }

    /// A core finer than the sampling limit is held at the floor rather than
    /// drawn as something the grid cannot resolve: below it, lowering seeing
    /// stops changing anything, and on purpose.
    #[test]
    fn a_core_below_the_sampling_limit_is_held() {
        // A seeing so fine no plate scale keeps it above the floor.
        let sharp = Camera::new(1600, 900).with_seeing(0.001);
        assert!(
            sharp.seeing_arcmin * (900.0 / (60.0 * 60.0)) < MIN_SEEING_PX,
            "the request is below the limit"
        );
        assert_eq!(sharp.seeing_pixels(), MIN_SEEING_PX);
        // And a still-lower request draws the same: the floor is where it stops.
        let sharper = Camera::new(1600, 900).with_seeing(0.0001);
        assert_eq!(sharper.seeing_pixels(), sharp.seeing_pixels());
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
        let camera =
            Camera::new(32, 32).looking_from([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]);
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
        assert!(
            projected.is_some_and(|(x, y, _)| x.is_finite() && y.is_finite())
        );
    }

    /// **Sirius is the brightest thing in the sky.** Point a camera at each of
    /// the brightest stars in turn and the picture of Sirius holds more light
    /// than any other — which it should, because that is what "brightest star
    /// in the sky" means, and it is a fact about the world rather than about
    /// this code.
    #[test]
    fn sirius_makes_the_brightest_picture() {
        let stars = bright();
        let mut ranked: Vec<(String, f64)> =
            ["Sirius", "Canopus", "Vega", "Betelgeuse", "Procyon"]
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
            .with_exposure(4.0)
            // The core alone: the aureole deliberately spreads a few percent of
            // a star's light out below the display floor, so a picture with one
            // holds a little less than went in. That the *core* conserves what
            // it is handed is the invariant here; the stack's own conservation
            // is `psf::tests::a_layered_psf_conserves_energy`.
            .without_aureoles();
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
        let lit =
            image.pixels().iter().filter(|p| p[0] + p[1] + p[2] > 1e-3).count();
        assert!(lit > 10, "only {lit} pixels lit");
    }

    /// A star's colour does not change how bright it is drawn.
    ///
    /// `blackbody_color` is normalized to unit luminance, so the tint carries
    /// hue and nothing else and the exposure's energy reaches the picture whole
    /// whatever the hue. Weighted the way the eye weights colour, what a star
    /// deposits is its energy — so a red giant, a blue supergiant and a white
    /// star of the same energy come out equally bright. This was once a
    /// documented defect: peak normalization drew the saturated ends of the
    /// sequence half a magnitude under the white middle for no physical reason.
    ///
    /// End to end in this renderer, and across hues far apart: the luminance in
    /// the picture matches the energy the exposure gave, to within the Moffat's
    /// truncated tail.
    #[test]
    fn a_stars_colour_does_not_change_how_bright_it_is() {
        let stars = bright();
        for name in ["Betelgeuse", "Rigel", "Sirius"] {
            let star = named(&stars, name);
            let camera = Camera::new(400, 400)
                .looking_from([0.0; 3], star.position)
                .with_fov_degrees(5.0)
                .with_exposure(4.0)
                // Colour is a core property; the aureole's sub-floor tail would
                // only add noise to a luminance-conservation check.
                .without_aureoles();
            let image = camera.render(&[star.clone()]);

            let apparent =
                apparent_magnitude_ly(star.absolute_magnitude, star.distance);
            let energy = relative_exposure(apparent, 4.0);
            let luminance: f64 = image
                .pixels()
                .iter()
                .map(|p| {
                    0.2126 * p[0] as f64
                        + 0.7152 * p[1] as f64
                        + 0.0722 * p[2] as f64
                })
                .sum();
            let ratio = luminance / energy;
            assert!(
                (ratio - 1.0).abs() < 0.03,
                "{name} ({:.0} K) drew {ratio} of its energy as luminance",
                star.temperature(),
            );
        }
    }

    /// A bright star wears a halo the bare core does not: with the default
    /// aureole it reaches further and lights more pixels than without, and the
    /// extra is a broad faint glow rather than a wider core.
    #[test]
    fn an_aureole_gives_a_bright_star_a_halo() {
        let stars = bright();
        let rigel = named(&stars, "Rigel");
        let base = Camera::new(400, 400)
            .looking_from([0.0; 3], rigel.position)
            .with_fov_degrees(3.0)
            .with_exposure(4.0);
        let bare = base.clone().without_aureoles().render(&[rigel.clone()]);
        let haloed = base.render(&[rigel.clone()]);

        let lit = |img: &Image| {
            img.pixels().iter().filter(|p| p[0] + p[1] + p[2] > 1e-3).count()
        };
        assert!(
            lit(&haloed) > lit(&bare),
            "the halo lights more pixels: {} vs {}",
            lit(&haloed),
            lit(&bare),
        );
        // The halo draws light out of the core rather than into it, so the
        // centre is no brighter than the bare core — dimmer, by the share moved
        // to the glow.
        assert!(
            haloed.peak() < bare.peak(),
            "the halo should not brighten the core: {} vs {}",
            haloed.peak(),
            bare.peak(),
        );
    }

    /// A ring lands on what it rings, which is the only thing an overlay has
    /// to get right.
    #[test]
    fn a_mark_lands_on_its_star() {
        let stars = bright();
        let vega = named(&stars, "Vega");
        let camera =
            Camera::new(300, 300).looking_from([0.0; 3], vega.position);
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
        let marks = camera.marks([[10.0, 0.0, 0.0], [-10.0, 0.0, 0.0]], 4.0);
        assert_eq!(marks.len(), 1);
    }

    /// A bearing rings where the star is, seen from where the bearing was
    /// measured.
    #[test]
    fn a_bearing_rings_from_the_origin() {
        let stars = bright();
        let vega = named(&stars, "Vega");
        let bearing = [
            vega.position[0] / vega.distance,
            vega.position[1] / vega.distance,
            vega.position[2] / vega.distance,
        ];
        let camera =
            Camera::new(300, 300).looking_from([0.0; 3], vega.position);
        let mark = camera.mark_bearing(bearing, 9.0).expect("in front");
        assert!((mark.x - 150.0).abs() < 0.01 && (mark.y - 150.0).abs() < 0.01);
    }

    /// **And nowhere else.** A bearing carries no distance, so from any other
    /// viewpoint there is no answer, and the honest reply is silence rather
    /// than a ring around a guess.
    #[test]
    fn a_bearing_means_nothing_from_anywhere_else() {
        let camera = Camera::new(300, 300)
            .looking_from([4.0, 0.0, 0.0], [100.0, 0.0, 0.0]);
        assert_eq!(camera.mark_bearing([1.0, 0.0, 0.0], 9.0), None);
    }

    /// A mark is visible or it is not, and "in front of the camera" is not the
    /// same question. Most of the sky is in front of an eye and off its
    /// picture, so anything reporting marks has to ask this one.
    #[test]
    fn framing_is_a_narrower_question_than_being_in_front() {
        let camera = Camera::new(100, 100)
            .looking_along([1.0, 0.0, 0.0])
            .with_fov_degrees(60.0);
        // Far off to the side but still ahead: projects, does not frame.
        let aside = camera.mark([1.0, 8.0, 0.0], 5.0).expect("in front");
        assert!(!camera.frames(&aside), "at {:.0},{:.0}", aside.x, aside.y);
        // Dead ahead: both.
        let ahead = camera.mark([10.0, 0.0, 0.0], 5.0).expect("in front");
        assert!(camera.frames(&ahead));
        // Just past the border, close enough that its ring still shows.
        let edge = Mark::new(-2.0, 50.0, 5.0);
        assert!(camera.frames(&edge));
    }

    /// The profile reaches the picture: the same star drawn as a Moffat and as
    /// a Gaussian is not the same picture. Both conserve the light, so the total
    /// is near enough alike; what differs is where it sits — the Gaussian holds
    /// more of it in a tighter core, the Moffat spreads it into wings — so the
    /// peak pixel differs. It is the check that `with_profile` is honoured.
    #[test]
    fn the_profile_reaches_the_picture() {
        let stars = bright();
        let sirius = named(&stars, "Sirius");
        let look = |profile| {
            Camera::new(200, 200)
                .looking_from([0.0; 3], sirius.position)
                .with_fov_degrees(5.0)
                .with_exposure(4.0)
                .with_profile(profile)
                .render(&[sirius.clone()])
        };
        let moffat = look(Profile::Moffat);
        let gaussian = look(Profile::Gaussian);
        assert!(moffat != gaussian, "the profile did not reach the render");
        assert!(
            (moffat.peak() - gaussian.peak()).abs() > 1e-6,
            "the two profiles drew the same peak"
        );
    }

    /// A figure line needs both ends in front of the eye: a line to a star
    /// behind the camera is not one it can draw.
    #[test]
    fn a_figure_line_needs_both_ends_in_front() {
        let camera = Camera::new(100, 100)
            .looking_along([1.0, 0.0, 0.0])
            .with_fov_degrees(60.0);
        assert!(
            camera.segment([10.0, 0.0, 0.0], [10.0, 1.0, 0.0]).is_some(),
            "both ahead"
        );
        assert!(
            camera.segment([10.0, 0.0, 0.0], [-10.0, 0.0, 0.0]).is_none(),
            "one behind"
        );
    }

    /// The whole seam: a Stellarium-format figure, parsed, joined to the stars
    /// by Hipparcos number and projected into lines. Two Orion stars, one line
    /// between them, drawn from Sol where both are in front.
    #[test]
    fn a_figure_is_joined_to_the_stars_and_projected() {
        let stars = bright();
        let betelgeuse = named(&stars, "Betelgeuse");
        // Betelgeuse is HIP 27989, Bellatrix HIP 25336.
        let figures =
            galos_catalog::asterism::parse("Ori 1 27989 25336".as_bytes())
                .expect("a figure file");
        let camera = Camera::new(400, 400)
            .looking_from([0.0; 3], betelgeuse.position)
            .with_fov_degrees(40.0);
        let lines = camera.figure_lines(&stars, &figures);
        assert_eq!(lines.len(), 1, "the one line resolves and projects");
    }
}
