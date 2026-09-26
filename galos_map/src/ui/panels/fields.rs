//! One named thing to a line, and how a value is said in it

use crate::map::bodies::DAY;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::Ui;
use chrono::{DateTime, Utc};
use std::fmt::Display;

/// Days in an Earth year, for reading a span too long to say in days
const YEAR: f64 = 365.25;

/// How long something takes, in the largest unit it fills
///
/// Days for anything that turns slowly, which is most of what is scanned, and
/// hours for the rest. A period in seconds is eight digits nobody reads.
///
/// Earth's, and said so for the two units a body has of its own. A day is a
/// body's turn about itself and a year is its turn about its star -- both of
/// them things the panel is otherwise reporting, so `1.2 days` beside a
/// rotation period is a real question about whose day is meant. An hour is
/// nobody's, so it goes unremarked, and neither is anything under one.
///
/// Down to seconds, which no orbit on record is but a span the map has been
/// run on by certainly can be: the status slider's near end is minutes, and a
/// span of them read as `0.1 hours` is a number in the wrong unit.
pub(crate) fn lasting(seconds: f32) -> String {
    if seconds <= 0. {
        return UNKNOWN.into();
    }
    let days = seconds as f64 / DAY;
    // Years for the long end, which a system's outermost bodies live at: the
    // slowest body of a system takes a median eighteen years to come round, and
    // six thousand days is a number nobody reads either.
    if days >= YEAR {
        format!("{:.1} Earth years", days / YEAR)
    } else if days >= 1. {
        format!("{days:.1} Earth days")
    } else if days * 24. >= 1. {
        format!("{:.1} hours", days * 24.)
    } else if days * 24. * 60. >= 1. {
        format!("{:.1} minutes", days * 24. * 60.)
    } else {
        format!("{:.0} seconds", seconds)
    }
}

/// How far something reaches, in the larger unit it fills
///
/// Light seconds where a light second is not most of the answer, and
/// kilometres where it is. A moon a few thousand kilometres out is a
/// hundredth of a light second, and a planet is millions of kilometres.
pub(super) fn spanning(metres: f32) -> String {
    let metres = metres as f64;
    if metres >= crate::map::space::LIGHT_SECOND {
        format!("{:.2} Ls", metres / crate::map::space::LIGHT_SECOND)
    } else {
        format!("{} km", crate::ui::text::thousands((metres / 1e3) as u64))
    }
}

/// What the database says of a yes or no question
pub(super) fn yes_no(answer: bool) -> String {
    if answer { "Yes".into() } else { "No".into() }
}

/// When something happened, where anything has said it did
///
/// Unknown for a discovery whose time nobody reported, which is most of them:
/// a scan finding a body already charted says somebody had been there without
/// saying when, and only a scan that found it unclaimed dates the finding.
pub(super) fn dated(at: Option<DateTime<Utc>>) -> String {
    match at {
        Some(at) => at.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => UNKNOWN.into(),
    }
}

/// One named thing the database knows about a system
///
/// The name is written as brightly as a header, and the answer beside it in
/// the ordinary text of the panel. A panel is read down its left hand column
/// until the line wanted is found, and it is the names that column is made
/// of.
pub(super) fn field(ui: &mut Ui, name: &str, value: String) {
    ui.label(egui::RichText::new(name).strong());
    ui.label(value);
    ui.end_row();
}

/// One named thing worth taking away, copied by clicking on it
///
/// A position is typed into other tools to the last decimal, and one read off
/// a panel and retyped is a digit out somewhere. The value is the control
/// rather than a button beside it, so a panel of fields reads as it did and
/// the one row that answers a click says so when the pointer rests on it.
pub(super) fn copied(ui: &mut Ui, name: &str, value: String) {
    ui.label(egui::RichText::new(name).strong());
    let shown = ui
        .add(egui::Label::new(value.clone()).sense(egui::Sense::click()))
        .on_hover_text("Click to copy")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if shown.clicked() {
        ui.ctx().copy_text(value);
    }
    ui.end_row();
}

/// How far a field written under a header sits in from the ones above it
pub(super) const NEST: f32 = 12.;

/// One named thing, written under the header it belongs to
pub(super) fn under(ui: &mut Ui, name: &str, value: String) {
    ui.horizontal(|ui| {
        ui.add_space(NEST);
        ui.label(egui::RichText::new(name).strong());
    });
    ui.label(value);
    ui.end_row();
}

/// What the database has yet to say about a system
pub(super) const UNKNOWN: &str = "Unknown";

/// What the database says, or that it says nothing
///
/// Most of what is recorded about a system is optional, and a blank row
/// reads as a bug rather than as an answer.
pub(super) fn named<T: Display>(value: &Option<T>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => UNKNOWN.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elite_journal::Allegiance;

    /// How far something reaches is read in whichever unit it fills
    #[test]
    fn a_reach_is_read_in_the_unit_it_fills() {
        assert_eq!(spanning(1.0023064e11), "334.33 Ls");
        assert_eq!(spanning(5.4946205e6), "5,494 km");
    }

    /// And how long it takes, likewise
    #[test]
    fn a_span_of_time_is_read_in_the_unit_it_fills() {
        assert_eq!(lasting(3.6254802e6), "42.0 Earth days");
        assert_eq!(lasting(3600.), "1.0 hours");
        // The short end, which only a span the map has been run on reaches.
        assert_eq!(lasting(225.), "3.8 minutes");
        assert_eq!(lasting(45.), "45 seconds");
        assert_eq!(lasting(0.), UNKNOWN);
        // The long end, which a system's outermost bodies live at. Six
        // thousand days is as unreadable as eight digits of seconds.
        assert_eq!(
            lasting((18.5 * 365.25 * DAY as f64) as f32),
            "18.5 Earth years",
        );
    }

    /// What the database does not say is said to be unknown
    ///
    /// Most of what is recorded about a system is optional, and a blank row
    /// reads as the panel having failed rather than as an answer.
    #[test]
    fn what_is_not_recorded_says_so() {
        assert_eq!(named(&Some(Allegiance::Empire)), "Empire");
        assert_eq!(named::<Allegiance>(&None), "Unknown");
    }
}
