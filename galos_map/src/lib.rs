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
}
