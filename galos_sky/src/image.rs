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
        let mut out = Vec::with_capacity(self.pixels.len() * 3);
        for pixel in &self.pixels {
            for &channel in pixel {
                let mapped = 1.0 - (-channel.max(0.0)).exp();
                out.push((srgb_encode(mapped) * 255.0).round() as u8);
            }
        }
        out
    }

    /// Write the tone-mapped image to a PNG.
    pub fn write_png(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let file = BufWriter::new(File::create(path)?);
        let mut encoder = png::Encoder::new(file, self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&self.to_srgb8())?;
        Ok(())
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
}
