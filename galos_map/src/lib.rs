//! A 3D Galaxy Map for `galos`
//!
//! ![](https://github.com/nixpulvis/galos/blob/master/galos_map/demo.gif?raw=true)
//!
//! Requires (read-only) access to [`galos_db`].
use bevy::prelude::*;
use galos_db::Database;

pub mod camera;
pub mod schedule;
pub mod search;
pub mod space;
pub mod systems;
pub mod ui;

#[derive(Resource)]
pub struct Db(pub Database);

#[cfg(test)]
pub(crate) mod tests {
    use bevy_egui::egui;

    /// Draw `contents` into a bare context and tessellate what it made
    ///
    /// Laying a widget out is not the half of it that goes wrong. Egui defers
    /// a colour the caller did not give as a placeholder for the painter to
    /// answer, and one answered by another placeholder is caught nowhere until
    /// epaint meets it and panics. So the shapes are turned into triangles
    /// here, which is the step that looks.
    ///
    /// Shared by the chrome and the panels, which paint their rows the same
    /// way and can go wrong in it the same way.
    pub(crate) fn painted(mut contents: impl FnMut(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| contents(ui));
        ctx.tessellate(output.shapes, output.pixels_per_point);
    }

    /// What egui complained about in the margins while `contents` was drawn
    ///
    /// Egui reports two widgets sharing an id by painting the offending
    /// rectangle in its error colour and writing what happened beside it. It
    /// says so nowhere else, so this reads it back off the shapes.
    pub(crate) fn complaints(
        mut contents: impl FnMut(&mut egui::Ui),
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| contents(ui));

        fn text_of(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => into.push(text.galley.text().into()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text_of(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut said = Vec::new();
        for shape in &output.shapes {
            text_of(&shape.shape, &mut said);
        }
        said.retain(|line| {
            line.contains("Double use") || line.contains("use of")
        });
        said
    }
}
