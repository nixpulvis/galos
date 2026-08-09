//! A 3D Galaxy Map for `galos`
//!
//! ![](https://github.com/nixpulvis/galos/blob/master/galos_map/demo.gif?raw=true)
//!
//! Requires (read-only) access to [`galos_db`].
use bevy::prelude::*;
use galos_db::Database;

pub mod camera;
pub mod grid;
pub mod ruled;
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

    /// A context lettered as the map letters its own
    ///
    /// What is drawn here is measured, and how wide a word comes out is the
    /// font's answer. A test weighing a line against the room there is for it
    /// in a face the map does not use is a test about some other map.
    pub(crate) fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::ui::styled);
        ctx
    }

    /// Draw `contents` into a bare context and tessellate what it made
    ///
    /// Laying a widget out is not the half of it that goes wrong. Egui defers
    /// a color the caller did not give as a placeholder for the painter to
    /// answer, and one answered by another placeholder is caught nowhere until
    /// epaint meets it and panics. So the shapes are turned into triangles
    /// here, which is the step that looks.
    ///
    /// Shared by the chrome and the panels, which paint their rows the same
    /// way and can go wrong in it the same way.
    pub(crate) fn painted(mut contents: impl FnMut(&mut egui::Ui)) {
        let ctx = context();
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
        let ctx = context();
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
    ///
    /// What is heard is kept per thread, and a test hears its own thread and
    /// no other. A logger is installed once for the whole process and the
    /// tests it hears run at the same time as the rest, so a warning from
    /// somewhere else would otherwise be read as this pass having complained.
    /// Egui logs from whichever thread called it, which is this one.
    pub(crate) fn between_passes(
        first: impl FnMut(&mut egui::Ui),
        second: impl FnMut(&mut egui::Ui),
    ) -> Vec<String> {
        use std::collections::HashMap;
        use std::sync::{Mutex, OnceLock};
        use std::thread::{self, ThreadId};

        /// What the logger has heard, by the thread that said it. Tests share
        /// a process, and a logger may be installed once in one.
        static HEARD: Mutex<Option<HashMap<ThreadId, Vec<String>>>> =
            Mutex::new(None);
        static LOGGER: OnceLock<()> = OnceLock::new();

        /// What `HEARD` has for `thread`, made if it has none
        fn heard_by<R>(
            thread: ThreadId,
            act: impl FnOnce(&mut Vec<String>) -> R,
        ) -> R {
            // Poisoning says some other test panicked mid-pass, which is that
            // test's news to break rather than this one's.
            let mut heard =
                HEARD.lock().unwrap_or_else(|held| held.into_inner());
            act(heard
                .get_or_insert_with(HashMap::new)
                .entry(thread)
                .or_default())
        }

        struct Listener;
        impl log::Log for Listener {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                if record.level() <= log::Level::Warn {
                    let said = record.args().to_string();
                    heard_by(thread::current().id(), |heard| heard.push(said));
                }
            }
            fn flush(&self) {}
        }

        LOGGER.get_or_init(|| {
            let _ = log::set_boxed_logger(Box::new(Listener));
            log::set_max_level(log::LevelFilter::Warn);
        });

        let mine = thread::current().id();
        heard_by(mine, Vec::clear);

        let ctx = context();
        for pass in
            [Box::new(first) as Box<dyn FnMut(&mut egui::Ui)>, Box::new(second)]
        {
            let mut pass = pass;
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| pass(ui));
        }

        heard_by(mine, std::mem::take)
    }

    /// What egui complained about in the margins while `contents` was drawn
    ///
    /// Egui reports two widgets sharing an id by painting the offending
    /// rectangle in its error color and writing what happened beside it. It
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
