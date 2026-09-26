//! The face everything is lettered in, the chrome and the map's own names
//! alike.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

/// Set the lettering once, before anything is drawn in it.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        bevy_egui::EguiPrimaryContextPass,
        lettering.in_set(crate::map::schedule::PaintSet::Style),
    );
}

/// Set every style the chrome is drawn in
///
/// Once. A style set on the context is the style it keeps, and a font asked
/// for every frame is a font asked for sixty times a second to no end.
pub(crate) fn lettering(
    mut contexts: EguiContexts,
    mut set: Local<bool>,
) -> Result {
    if *set {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    ctx.all_styles_mut(styled);
    *set = true;

    Ok(())
}

/// Set the chrome's lettering and the marks that stand in it
///
/// The map is read in names and numbers standing in columns: how far off each
/// system is, how long each jump of a route is, how much of the sky is getting
/// through the filters. Set proportionally those columns are ragged, digits
/// being narrower than the letters beside them.
///
/// And what a route is called is one system, an arrow, and another. The hyphen
/// and the angle of an ASCII arrow are drawn to a width apiece in a monospaced
/// face and meet as an arrow; set proportionally the hyphen is short and low
/// and the two read as punctuation that happened to land side by side.
///
/// A point smaller than egui letters them, each of them, so that what stands
/// over what is unchanged. A monospaced face is wider than the proportional
/// one it stands in for, and the chrome is read at a glance off the top of a
/// map rather than paragraph by paragraph.
///
/// The marks egui draws for itself are sized here as well: the fold arrow on a
/// panel's title bar, the mark that shuts it, and the boxes in the settings
/// pane. They are set for lettering a size larger than this, and a mark drawn
/// to one scale beside words drawn to another reads as two pieces of chrome
/// that came from different maps.
pub(crate) fn styled(style: &mut egui::Style) {
    use egui::FontFamily::Monospace;
    use egui::{FontId, TextStyle};

    style.text_styles = [
        (TextStyle::Small, FontId::new(8., Monospace)),
        (TextStyle::Body, FontId::new(11.5, Monospace)),
        (TextStyle::Button, FontId::new(11.5, Monospace)),
        (TextStyle::Monospace, FontId::new(11., Monospace)),
        (TextStyle::Heading, FontId::new(17., Monospace)),
    ]
    .into();

    style.spacing.icon_width = 12.;
    style.spacing.icon_width_inner = 7.;
}

/// An sRGB color as egui knows it
///
/// [`Srgba`] channels are already gamma-encoded, the space [`egui::Color32`]
/// holds, so they cross straight over; the alpha is not premultiplied on
/// either side.
pub(crate) fn color32(color: Srgba) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (color.red * 255.) as u8,
        (color.green * 255.) as u8,
        (color.blue * 255.) as u8,
        (color.alpha * 255.) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every style the chrome is drawn in is lettered the same
    ///
    /// Egui keeps a font per text style, and one left proportional is one
    /// heading or one button standing among columns that no longer line up
    /// with it.
    #[test]
    fn the_chrome_is_lettered_in_one_width() {
        let mut style = egui::Style::default();
        styled(&mut style);

        for (kind, font) in &style.text_styles {
            assert_eq!(font.family, egui::FontFamily::Monospace, "{kind:?}");
        }
    }
}
