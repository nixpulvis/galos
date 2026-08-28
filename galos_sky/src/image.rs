//! The buffer a sky is drawn into, and how it becomes a picture.
//!
//! Two representations, and keeping them apart is the point. The buffer holds
//! **linear energy** — what actually landed on each pixel, in the same units
//! the exposure law hands out, unbounded above. A picture holds **display
//! values**, eight bits a channel, compressed and gamma-encoded so a screen
//! can show it.
//!
//! Photometry is done on the first and looking is done on the second. A
//! comparison between two renderers has to be made on linear energy, because
//! the tone curve is not invertible where it saturates and two pictures that
//! agree after it may disagree by a factor of ten before it. So
//! [`Image::total_energy`] and [`Image::pixels`] read the honest buffer, and
//! [`Image::to_srgb8`] is the lossy step taken last.

use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;

/// A rendered sky: linear RGB energy per pixel, row-major from the top left.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    width: u32,
    height: u32,
    pixels: Vec<[f32; 3]>,
}

impl Image {
    /// A black image of the given size.
    pub fn new(width: u32, height: u32) -> Image {
        Image {
            width,
            height,
            pixels: vec![[0.0; 3]; (width as usize) * (height as usize)],
        }
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The linear energy buffer, row-major from the top left.
    pub fn pixels(&self) -> &[[f32; 3]] {
        &self.pixels
    }

    /// Add energy to one pixel, ignoring anything off the edge.
    ///
    /// Light adds — two stars overlapping are the sum of both, never the
    /// brighter of the two — which is the same reason the index's aggregates
    /// carry summed flux rather than a representative colour.
    pub fn add(&mut self, x: i64, y: i64, energy: [f32; 3]) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let i = (y as usize) * (self.width as usize) + (x as usize);
        for c in 0..3 {
            self.pixels[i][c] += energy[c];
        }
    }

    /// Every channel of every pixel summed: how much light is in this picture.
    ///
    /// The figure two renderings of one sky are compared by, and the reason
    /// the PSF conserves energy. It is meaningful only before tone mapping.
    pub fn total_energy(&self) -> f64 {
        self.pixels
            .iter()
            .flat_map(|p| p.iter())
            .map(|&c| c as f64)
            .sum()
    }

    /// The brightest single channel in the image.
    pub fn peak(&self) -> f32 {
        self.pixels.iter().flat_map(|p| p.iter()).copied().fold(0.0, f32::max)
    }

    /// The image as 8-bit sRGB, tone-mapped and gamma-encoded.
    ///
    /// The curve is `1 - exp(-x)`, a film response: linear in the dark where
    /// faint stars live, asymptotic to white so no amount of light overflows.
    /// A sky spans magnitudes from Sirius at −1.4 to the eye's limit at 8,
    /// which is four decades of flux, and nothing linear shows both ends at
    /// once.
    pub fn to_srgb8(&self) -> Vec<u8> {
        self.to_srgb8_with(&[])
    }

    /// The tone-mapped image with rings drawn over it.
    ///
    /// The marks are written into the eight-bit output after the tone curve,
    /// so nothing about them reaches the linear buffer and
    /// [`total_energy`](Self::total_energy) is unchanged. An overlay is for
    /// finding things in a picture; it is not part of the picture's photometry.
    pub fn to_srgb8_with(&self, marks: &[Mark]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 3);
        for pixel in &self.pixels {
            for &channel in pixel {
                let mapped = 1.0 - (-channel.max(0.0)).exp();
                out.push((srgb_encode(mapped) * 255.0).round() as u8);
            }
        }
        for mark in marks {
            self.stroke(&mut out, mark);
        }
        out
    }

    /// Draw one ring into an eight-bit buffer.
    ///
    /// A one-pixel outline: every pixel whose distance from the centre is
    /// within half a pixel of the radius. Written rather than blended, since
    /// chrome that dimmed over a bright star would be least visible exactly
    /// where it is most wanted.
    fn stroke(&self, out: &mut [u8], mark: &Mark) {
        let reach = mark.radius + 1.0;
        let (x0, x1) = ((mark.x - reach) as i64, (mark.x + reach) as i64);
        let (y0, y1) = ((mark.y - reach) as i64, (mark.y + reach) as i64);
        for y in y0..=y1 {
            for x in x0..=x1 {
                if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64
                {
                    continue;
                }
                let dx = x as f64 + 0.5 - mark.x;
                let dy = y as f64 + 0.5 - mark.y;
                if ((dx * dx + dy * dy).sqrt() - mark.radius).abs() > 0.5 {
                    continue;
                }
                let i = ((y as usize) * (self.width as usize) + x as usize) * 3;
                for c in 0..3 {
                    out[i + c] =
                        (mark.color[c].clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
    }

    /// Write the tone-mapped image to a PNG.
    pub fn write_png(&self, path: impl AsRef<Path>) -> io::Result<()> {
        self.write_png_with(path, &[])
    }

    /// Write the tone-mapped image to a PNG with rings drawn over it.
    pub fn write_png_with(
        &self,
        path: impl AsRef<Path>,
        marks: &[Mark],
    ) -> io::Result<()> {
        let file = BufWriter::new(File::create(path)?);
        let mut encoder = png::Encoder::new(file, self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&self.to_srgb8_with(marks))?;
        Ok(())
    }
}

/// A ring drawn around something, in display space.
///
/// Annotation rather than light, and the distinction is load-bearing: marks are
/// rasterized into the eight-bit output and never into the linear buffer, so
/// [`Image::total_energy`] is the same whether a picture is annotated or not.
/// If they added energy, ringing a star would change the quantity two renderers
/// are compared by, and the overlay meant to help the comparison would be the
/// thing breaking it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Mark {
    /// Centre, in pixels, from the top left.
    pub x: f64,
    /// Centre, in pixels, from the top left.
    pub y: f64,
    /// The ring's radius, pixels.
    pub radius: f64,
    /// The ring's colour, as displayed. Not tone-mapped — it is chrome, not
    /// light, and is written straight into the output.
    pub color: [f32; 3],
}

/// The default ring colour: green.
///
/// Chosen because **a blackbody is never green**. Across the whole Planckian
/// locus the dominant channel is red below about 6500 K and blue above it, and
/// the green channel leads at no temperature at all, so a green ring cannot be
/// mistaken for a star however faint or saturated it is drawn. Any other hue
/// would collide with something real.
pub const MARK_COLOR: [f32; 3] = [0.0, 1.0, 0.35];

/// The colour for a ring around something that is *not* drawn: magenta.
///
/// Safe on a weaker property than [`MARK_COLOR`]'s, and it is worth being
/// exact about which. Green being the *minimum* channel is not impossible for a
/// blackbody — between about 6250 and 7250 K, where red and blue cross over and
/// the star is very nearly white, green dips a few per cent under both. What is
/// true is that a blackbody's green never falls below about 0.176 of its peak,
/// the floor it reaches at the cold end of the locus and holds down to zero.
///
/// Magenta puts green at nothing. That is far outside anything on the locus, so
/// no star approaches it, but the guarantee is a margin rather than the flat
/// impossibility green enjoys.
pub const HOLE_COLOR: [f32; 3] = [1.0, 0.0, 0.85];

/// The lowest a blackbody's green channel goes, as a fraction of its peak.
///
/// Reached below about 1500 K, where the fit clamps, and rising from there. It
/// is the margin [`HOLE_COLOR`] leans on.
pub const MIN_BLACKBODY_GREEN: f32 = 0.17;

impl Mark {
    /// A ring of the default colour.
    pub fn new(x: f64, y: f64, radius: f64) -> Mark {
        Mark { x, y, radius, color: MARK_COLOR }
    }

    /// The same ring in another colour.
    pub fn colored(mut self, color: [f32; 3]) -> Mark {
        self.color = color;
        self
    }
}

/// One linear channel in `0..=1`, gamma-encoded for a display.
///
/// The sRGB transfer function: a short linear toe near black, then a power
/// curve. Writing linear values straight into a PNG makes a sky far too dark,
/// since a display undoes a gamma nobody applied.
fn srgb_encode(linear: f32) -> f32 {
    let l = linear.clamp(0.0, 1.0);
    if l <= 0.003_130_8 {
        12.92 * l
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Light adds where it overlaps rather than replacing.
    #[test]
    fn overlapping_light_adds() {
        let mut image = Image::new(4, 4);
        image.add(1, 1, [0.25, 0.0, 0.0]);
        image.add(1, 1, [0.25, 0.0, 0.0]);
        assert_eq!(image.pixels()[5][0], 0.5);
    }

    /// Energy landing off the edge is lost, not wrapped onto the far side.
    #[test]
    fn energy_off_the_edge_is_dropped() {
        let mut image = Image::new(4, 4);
        image.add(-1, 2, [1.0, 1.0, 1.0]);
        image.add(9, 2, [1.0, 1.0, 1.0]);
        image.add(2, -3, [1.0, 1.0, 1.0]);
        assert_eq!(image.total_energy(), 0.0);
    }

    /// The tone curve never overflows: however much light lands, the picture
    /// saturates at white instead of wrapping.
    #[test]
    fn the_tone_curve_saturates_rather_than_overflowing() {
        let mut image = Image::new(1, 1);
        image.add(0, 0, [1e12, 1e12, 1e12]);
        assert_eq!(image.to_srgb8(), vec![255, 255, 255]);
    }

    /// Black stays black and the curve rises monotonically out of it, so a
    /// brighter star is never drawn darker.
    #[test]
    fn the_tone_curve_rises_from_black() {
        let mut last = 0u8;
        for step in 0..64 {
            let mut image = Image::new(1, 1);
            image.add(0, 0, [step as f32 * 0.05, 0.0, 0.0]);
            let value = image.to_srgb8()[0];
            assert!(value >= last, "step {step} went backwards");
            last = value;
        }
        assert_eq!(Image::new(1, 1).to_srgb8(), vec![0, 0, 0]);
    }

    /// A mid-grey linear value gamma-encodes to about the mid-grey a display
    /// expects, which is the check that the encoding is applied at all.
    #[test]
    fn linear_half_encodes_to_display_mid_grey() {
        assert!((srgb_encode(0.5) - 0.7354).abs() < 0.001);
    }

    /// **An overlay is not light.** Ringing every star in a picture leaves its
    /// energy exactly as it was, which is what keeps `total_energy` the
    /// quantity two renderers can be compared by whether or not either is
    /// annotated.
    #[test]
    fn marks_do_not_change_the_photometry() {
        let mut image = Image::new(32, 32);
        image.add(16, 16, [1.0, 1.0, 1.0]);
        let before = image.total_energy();
        let marks = [Mark::new(16.5, 16.5, 6.0), Mark::new(4.0, 4.0, 3.0)];
        let annotated = image.to_srgb8_with(&marks);
        assert_eq!(image.total_energy(), before);
        assert_ne!(annotated, image.to_srgb8(), "the ring should be visible");
    }

    /// A ring is a ring: it lands at its radius and leaves the centre alone,
    /// so the thing being pointed at is not painted over.
    #[test]
    fn a_ring_surrounds_rather_than_covers() {
        let image = Image::new(41, 41);
        let out = image.to_srgb8_with(&[Mark::new(20.5, 20.5, 8.0)]);
        let at = |x: usize, y: usize| {
            let i = (y * 41 + x) * 3;
            [out[i], out[i + 1], out[i + 2]]
        };
        assert_eq!(at(20, 20), [0, 0, 0], "the centre should be untouched");
        assert_ne!(at(28, 20), [0, 0, 0], "the ring should be at the radius");
        assert_eq!(at(35, 20), [0, 0, 0], "and nothing beyond it");
    }

    /// A ring near the edge is clipped, not wrapped, and does not panic.
    #[test]
    fn a_ring_off_the_edge_is_clipped() {
        let image = Image::new(16, 16);
        let out = image.to_srgb8_with(&[
            Mark::new(0.0, 0.0, 5.0),
            Mark::new(15.0, 15.0, 9.0),
            Mark::new(-40.0, 8.0, 3.0),
        ]);
        assert_eq!(out.len(), 16 * 16 * 3);
    }

    /// The default mark colour is one no star can wear. A blackbody's dominant
    /// channel is red below about 6500 K and blue above, and green leads at no
    /// temperature at all, so a green ring is never mistaken for light.
    #[test]
    fn no_star_is_ever_the_colour_of_a_mark() {
        for t in (500..60000).step_by(25) {
            let c = galos_photometry::blackbody_color(t as f64);
            assert!(
                c[1] < c[0].max(c[2]),
                "a blackbody at {t} K leads with green: {c:?}"
            );
        }
        assert!(MARK_COLOR[1] > MARK_COLOR[0].max(MARK_COLOR[2]));
    }

    /// The hole colour rests on a different and weaker property: not that
    /// green can never be a blackbody's smallest channel — between about 6250
    /// and 7250 K it is, by a few per cent — but that it never goes anywhere
    /// near zero. Magenta's green is nothing, which is far under the floor.
    #[test]
    fn no_blackbody_comes_near_the_hole_colour() {
        for t in (500..60000).step_by(25) {
            let c = galos_photometry::blackbody_color(t as f64);
            assert!(
                c[1] >= MIN_BLACKBODY_GREEN,
                "at {t} K green fell to {}, under the floor the hole colour \
                 leans on",
                c[1]
            );
        }
        assert!(HOLE_COLOR[1] < MIN_BLACKBODY_GREEN);
    }

}
