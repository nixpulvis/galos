//! The screen the window stands on while the index is still being read
//!
//! What is being read and how far it has got is the map's to say — see
//! [`crate::map::index::load`] — and this is where a reader is told it.

use crate::map::index::IndexDir;
use crate::map::index::load::{Opening, Reading, Step};
use crate::map::schedule::PaintSet;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        EguiPrimaryContextPass,
        screen.run_if(in_state(Opening::Reading)).in_set(PaintSet::Ui),
    );
}

/// Say that the map is coming, and what it is waiting on
fn screen(
    mut contexts: EguiContexts,
    reading: Option<Res<Reading>>,
    dir: Res<IndexDir>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let step = reading.as_ref().map_or(Step::Stamps, |it| it.step());
    let failed = reading.as_ref().and_then(|it| it.failed().map(str::to_owned));

    egui::Area::new(egui::Id::new("loading"))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| waiting(ui, &dir.0, step, failed.as_deref()));

    Ok(())
}

/// The words on the loading screen
///
/// The directory first, since which index is being read is the thing a reader
/// with two of them wants to know, and it is the answer to the commonest way
/// of getting this wrong. Then what is happening: the part being read while
/// the read is going, and what went wrong where it did not.
fn waiting(ui: &mut egui::Ui, dir: &str, step: Step, failed: Option<&str>) {
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(dir).weak());
        match failed {
            Some(said) => {
                ui.label(egui::RichText::new(said).color(egui::Color32::RED));
            }
            None => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(format!("Reading {}", step.said()));
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::words;

    /// The screen says which index it is reading and what it has reached
    ///
    /// A spinner alone says the map is busy, which the black window said
    /// already. What a reader wants is which of their directories is being
    /// read and which part of it is being read now, the parts taking long
    /// enough apart that a stuck one is worth naming.
    #[test]
    fn the_screen_says_what_it_is_reading() {
        let said = words(|ui| waiting(ui, ".galos_index", Step::Names, None));

        assert!(said.contains(&".galos_index".to_owned()), "{said:?}");
        assert!(said.contains(&"Reading the names".to_owned()), "{said:?}");
    }

    /// And says what went wrong rather than spinning at nothing
    ///
    /// A directory that is not there is the commonest thing to get wrong about
    /// running the map. Read on a task pool thread, the panic that used to say
    /// so would be a backtrace with the path buried in it.
    #[test]
    fn the_screen_says_what_went_wrong() {
        let said = words(|ui| {
            waiting(ui, "/nowhere", Step::Cells, Some("no such directory"))
        });

        assert!(said.contains(&"no such directory".to_owned()), "{said:?}");
        assert!(
            !said.iter().any(|line| line.starts_with("Reading")),
            "a failed read said it was still going: {said:?}"
        );
    }
}
