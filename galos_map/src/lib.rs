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

    /// Every piece of text `contents` painted, in the order it was painted
    ///
    /// What a widget draws is the whole of what the user is told, and a row
    /// that lays its text out itself has no label to be asked what it says.
    /// So this reads it back off the shapes.
    pub(crate) fn words(
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
        said
    }

    /// What egui said about `first` being redrawn as `second`
    ///
    /// Egui checks between one pass and the next whether a rectangle kept its
    /// place while everything in it changed identity, which is how a widget
    /// taking another's state shows up. It says so through `log` and nowhere
    /// else, so this listens for that rather than reading the painted output
    /// the way [`complaints`] does.
    ///
    /// Two passes over one context, since a warning about what changed
    /// between them cannot be had from either alone.
    pub(crate) fn between_passes(
        first: impl FnMut(&mut egui::Ui),
        second: impl FnMut(&mut egui::Ui),
    ) -> Vec<String> {
        use std::sync::{Mutex, OnceLock};

        /// What the logger has heard, and the lock that keeps two tests from
        /// hearing each other. Tests share a process, and a logger may be
        /// installed once in one.
        static HEARD: Mutex<Vec<String>> = Mutex::new(Vec::new());
        static LOCK: Mutex<()> = Mutex::new(());
        static LOGGER: OnceLock<()> = OnceLock::new();

        struct Listener;
        impl log::Log for Listener {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                if record.level() <= log::Level::Warn {
                    HEARD.lock().unwrap().push(record.args().to_string());
                }
            }
            fn flush(&self) {}
        }

        LOGGER.get_or_init(|| {
            let _ = log::set_boxed_logger(Box::new(Listener));
            log::set_max_level(log::LevelFilter::Warn);
        });

        let _held = LOCK.lock().unwrap_or_else(|held| held.into_inner());
        HEARD.lock().unwrap().clear();

        let ctx = egui::Context::default();
        for pass in
            [Box::new(first) as Box<dyn FnMut(&mut egui::Ui)>, Box::new(second)]
        {
            let mut pass = pass;
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| pass(ui));
        }

        let heard = HEARD.lock().unwrap().clone();
        heard
    }

    /// What egui complained about in the margins while `contents` was drawn
    ///
    /// Egui reports two widgets sharing an id by painting the offending
    /// rectangle in its error colour and writing what happened beside it. It
    /// says so nowhere else, so this picks it out of what was painted.
    pub(crate) fn complaints(
        contents: impl FnMut(&mut egui::Ui),
    ) -> Vec<String> {
        let mut said = words(contents);
        said.retain(|line| {
            line.contains("Double use") || line.contains("use of")
        });
        said
    }
}
