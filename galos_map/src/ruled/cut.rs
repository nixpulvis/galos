//! Cutting a face into the strip of glyphs a plane paints its numbers from
//!
//! A plane numbers itself by reading characters out of one strip of equal
//! cells. Which face those are cut from is the caller's to say, and everything
//! else about the strip is here.
use super::{LETTERS, Lettering};
use ab_glyph::{Font, FontRef, PxScale};
use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::prelude::*;
use bevy_rich_text3d::LoadFonts;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat,
};

/// The face a plane's numbers are painted in
///
/// Handed to [`super::RuledPlugin`] and passed from there to everything that
/// sets a character, rather than left in the world for them to reach for.
/// Monospaced; see [`cut_lettering`].
#[derive(Clone)]
pub struct Face {
    /// The face itself, in any format `ab_glyph` reads
    ///
    /// What the strip painted onto a plane is cut from.
    pub bytes: &'static [u8],
    /// And what the same face is called, for the text meshes standing over it
    ///
    /// A number on a plane and a number over it are then the one typeface.
    pub family: &'static str,
}

/// Ask for `face` to be loaded, without taking the list from whoever else asked
///
/// The text stack keeps one list of faces to load. Set outright rather than
/// added to, whichever plugin was added last would decide whether anybody
/// else's face is there at all, and a face that was never loaded draws as
/// nothing with nothing said about it.
pub(super) fn wanted(app: &mut App, face: &'static [u8]) {
    app.world_mut()
        .get_resource_or_insert_with(LoadFonts::default)
        .font_embedded
        .push(face);
}

/// How wide and tall a glyph's cell in the lettering strip is, in pixels
///
/// Three by five, the shape the plane lays a character out in, times sixteen.
/// Enough that a number drawn larger than this reads as a letter rather than
/// as a mosaic, and small enough that the whole strip is a few tens of
/// kilobytes.
const CELL_WIDE: u32 = 48;
const CELL_TALL: u32 = 80;

/// How much of a cell's height a digit fills
///
/// Short of the whole, so that a comma has somewhere below the line to hang
/// and the card reading one glyph cannot pick up the one above.
const FILLS: f32 = 0.78;

/// And how much of the rest sits above it rather than below
const AIR: f32 = 0.06;

/// Cut the strip of glyphs a plane's numbers are painted from
///
/// One cell per [`LETTERS`], in that order, each glyph drawn inside its own
/// cell with room around it so that reading one does not pick up its
/// neighbour. Single channel; only coverage is wanted.
///
/// The face is the caller's, in whatever format `ab_glyph` reads. Monospaced,
/// which is what makes a strip of equal cells the right shape for it: the
/// plane counts characters rather than measuring them, so a proportional face
/// comes out with its letters adrift in their cells.
pub(super) fn cut_lettering(
    face: Face,
) -> impl FnMut(Commands, ResMut<Assets<Image>>) {
    move |mut commands, mut images| {
        let Ok(face) = FontRef::try_from_slice(face.bytes) else {
            return;
        };
        let wide = CELL_WIDE as usize * LETTERS.len();
        let mut strip = vec![0u8; wide * CELL_TALL as usize];

        // How tall a digit comes out at a trial size, so that the real one can be
        // chosen to fill the cell rather than guessed at from the face's ascent.
        // A face's ascent leaves room for accents no digit has, and a glyph cut to
        // it fills half the cell and is read as nothing at all.
        let measure = |scale: PxScale| {
            face.outline_glyph(face.glyph_id('0').with_scale(scale))
                .map(|it| it.px_bounds())
        };
        let Some(trial) = measure(PxScale::from(CELL_TALL as f32)) else { return };
        let scale = PxScale::from(
            CELL_TALL as f32 * CELL_TALL as f32 * FILLS / trial.height(),
        );
        let Some(digit) = measure(scale) else { return };
        // Where the line the letters stand on falls in the cell: a little air
        // above the digits, and what a comma needs under them.
        let base = CELL_TALL as f32 * AIR - digit.min.y;

        for (nth, letter) in LETTERS.iter().enumerate() {
            let glyph = face.glyph_id(*letter).with_scale(scale);
            let Some(cut) = face.outline_glyph(glyph) else { continue };
            let bounds = cut.px_bounds();
            // Middle of its own cell across, on the line down.
            let left = nth as f32 * CELL_WIDE as f32
                + (CELL_WIDE as f32 - bounds.width()) / 2.;
            cut.draw(|x, y, covered| {
                let at = (
                    (left + x as f32).round() as i32,
                    (base + bounds.min.y + y as f32).round() as i32,
                );
                if at.0 < 0 || at.1 < 0 || at.0 as usize >= wide {
                    return;
                }
                if at.1 as u32 >= CELL_TALL {
                    return;
                }
                let ink = &mut strip[at.1 as usize * wide + at.0 as usize];
                *ink = (*ink).max((covered * 255.) as u8);
            });
        }

        let mut image = Image::new(
            Extent3d {
                width: wide as u32,
                height: CELL_TALL,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            strip,
            TextureFormat::R8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );
        // Read smoothly, and never off the end of the strip.
        image.sampler = ImageSampler::linear();

        commands.insert_resource(Lettering(images.add(image)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two askers both get their face loaded
    ///
    /// A caller that draws its own text asks for the same list, and the ruled
    /// plane's numbers have to be set in something whichever of the two was
    /// added first.
    #[test]
    fn two_askers_both_get_their_face_loaded() {
        let mut app = App::new();

        wanted(&mut app, &b"one"[..]);
        wanted(&mut app, &b"two"[..]);

        let fonts = app.world().resource::<LoadFonts>();
        assert_eq!(fonts.font_embedded, vec![&b"one"[..], &b"two"[..]]);
    }
}
