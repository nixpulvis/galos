//! What the map knows about a system, written out
//!
//! Pointing at a system and picking one out are both answered on the map
//! itself, by a ring and a name. This is the long form of the same answer,
//! and the user asks for it deliberately, from the mark beside the name of
//! whatever is picked out. A panel is kept until they shut it, and as many
//! of them stand open at once as they care to open, so two systems can be
//! read side by side.
//!
//! A panel holds a [`System`] value rather than an entity, for the reason a
//! selection does: a system flown away from is despawned, and a panel opened
//! for it has no reason to go with it.

use crate::camera::MoveCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::ui::MARGIN;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui::Ui;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use std::fmt::Display;

pub fn plugin(app: &mut App) {
    app.init_resource::<Panels>();
    app.add_systems(Update, refresh.in_set(MapSet::Present));
    // `ui::chrome` concludes at its end whether the pointer is busy with the
    // UI, from every window drawn in the pass so far. Drawn before it, these
    // are counted in the same frame they are shown rather than the next.
    app.add_systems(EguiPrimaryContextPass, panels.before(crate::ui::chrome));
}

/// How wide a panel stands
///
/// Wide enough for a position and for the longest of the names a field is
/// answered with, so that the two columns do not shift from one system to
/// the next.
const WIDTH: f32 = 230.;

/// The systems the user has a panel open for
///
/// A list rather than a map, since there are only ever a handful of them and
/// what matters about the order is where the next one lands.
#[derive(Resource, Default)]
pub struct Panels {
    open: Vec<Panel>,
    /// How many panels have been opened, ever
    ///
    /// Which place in the tiling each one takes. Counting openings rather
    /// than what is open now means a place is never handed on: a panel put
    /// away and opened again comes back to the place it had, which egui has
    /// been remembering for it all along.
    opened: usize,
    /// How tall a panel came out, from the last one drawn
    ///
    /// Every panel says the same eight things, so one measurement stands for
    /// all of them. Measured rather than guessed because it decides where
    /// the next one opens, and a guess one line short would have them
    /// overlap, which is the whole of what the tiling is for.
    ///
    /// Nothing until one has been drawn, which leaves the first panel of a
    /// session laid out as taking no room. It still opens where it asked
    /// to, since egui only moves a window to bring it back inside the
    /// viewport.
    height: f32,
}

/// One open panel
struct Panel {
    /// The row it is drawn from
    system: System,
    /// Which place in the tiling it opened at
    slot: usize,
}

impl Panels {
    /// Open a panel for `system`
    ///
    /// A system already being read about is left where it is rather than
    /// opened a second time, since two windows describing one system are two
    /// copies of one answer.
    pub fn open(&mut self, system: System) {
        if self.showing(system.address) {
            return;
        }
        self.open.push(Panel { system, slot: self.opened });
        self.opened += 1;
    }

    /// Whether a panel is open for the system at `address`
    fn showing(&self, address: i64) -> bool {
        self.open.iter().any(|panel| panel.system.address == address)
    }
}

/// Which place down and across the tiling the panel at `slot` stands in
///
/// Down the right hand edge until another would not fit above the bottom of
/// the viewport, then across into a fresh column to its left, and back to the
/// corner once the viewport is full. Answers in places rather than in pixels,
/// so how large a panel is stays the caller's business.
///
/// Filling the last place is the one time two panels are left on top of each
/// other. There is nowhere else for the next one to go, and shrinking every
/// panel to make room would be a poor trade for the one the user is reading.
fn tile(slot: usize, down: usize, across: usize) -> (usize, usize) {
    let down = down.max(1);
    let slot = slot % (down * across.max(1));
    (slot % down, slot / down)
}

/// Keep each panel on whatever the map last heard about its system
///
/// A panel is drawn from the row it was opened with, and a fetch replaces
/// the row of a system already on the map without the panel hearing of it.
/// So a row that has changed is copied across, and a panel says what the map
/// holds rather than what it held.
fn refresh(
    mut panels: ResMut<Panels>,
    changed: Query<&System, Changed<System>>,
) {
    // Every star arrives changed, so without this the whole of a fetch is
    // walked for the sake of the panels nobody has open.
    if panels.open.is_empty() {
        return;
    }
    for system in &changed {
        for panel in &mut panels.open {
            if panel.system.address == system.address {
                panel.system = system.clone();
            }
        }
    }
}

/// Tell the user what is known about the systems they have opened
///
/// Written here rather than alongside the rest of the UI because a
/// [`System`]'s fields are the business of this module and its neighbours,
/// and this is the one place they are read out rather than drawn with.
fn panels(
    mut contexts: EguiContexts,
    mut panels: ResMut<Panels>,
    mut camera: MessageWriter<MoveCamera>,
) -> Result {
    if panels.open.is_empty() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    // The top right corner, clear of the settings pane and the search bar,
    // which stand against the left edge and the top middle. Measured from
    // the window's full width rather than the width of what is written in
    // it, so that its right edge stands off the viewport by the margin
    // instead of its text doing so.
    //
    // Only where a panel opens: the windows are movable, so where they end
    // up is the user's business.
    let room = ctx.content_rect();
    let width =
        WIDTH + egui::Frame::window(&ctx.global_style()).total_margin().sum().x;
    let corner = room.right_top() + egui::vec2(-width - MARGIN, MARGIN);

    // A panel and the gap under it, and how many of those the viewport holds
    // each way. Worked out afresh every frame, since the window it is all
    // measured against is the user's to resize.
    let step = egui::vec2(-(width + MARGIN), panels.height + MARGIN);
    let down = ((room.height() - MARGIN) / step.y).floor().max(1.) as usize;
    let across = ((room.width() - MARGIN) / -step.x).floor().max(1.) as usize;

    let mut shut = Vec::new();
    let mut tallest: f32 = 0.;
    for panel in &panels.open {
        let system = &panel.system;
        let mut showing = true;
        let mut centred = None;
        let (row, column) = tile(panel.slot, down, across);
        // Named for the system but identified by its address, which does not
        // change with the row it was fetched in, so a window keeps the place
        // the user dragged it to across a refresh.
        let window = egui::Window::new(system.name.as_str())
            .id(egui::Id::new(("system-panel", system.address)))
            .open(&mut showing)
            .resizable(false)
            // Left unsaid this is `Style::default_area_size`, 600 wide,
            // which will not fit where a panel is asked to be placed.
            .default_size(egui::vec2(WIDTH, panels.height))
            .default_pos(
                corner
                    + egui::vec2(step.x * column as f32, step.y * row as f32),
            )
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                egui::Grid::new(("system-fields", system.address))
                    .num_columns(2)
                    .show(ui, |ui| {
                        let [x, y, z] = system.position;
                        field(
                            ui,
                            "Position",
                            format!("{x:.2}, {y:.2}, {z:.2}"),
                        );
                        field(ui, "Population", thousands(system.population));
                        field(ui, "Allegiance", named(&system.allegiance));
                        field(ui, "Government", named(&system.government));
                        field(ui, "Security", named(&system.security));
                        field(ui, "Economy", named(&system.primary_economy));
                        field(
                            ui,
                            "Secondary",
                            named(&system.secondary_economy),
                        );
                        field(
                            ui,
                            "Updated",
                            system
                                .updated_at
                                .format("%Y-%m-%d %H:%M UTC")
                                .to_string(),
                        );
                    });

                // Its own system rather than whatever is selected, since
                // several panels stand open at once and each one is about
                // the system named in its title bar.
                ui.add_space(MARGIN);
                if ui.button("Center Camera").clicked() {
                    centred = Some(DVec3::from(system.position));
                }
            });

        // Only a panel that drew what it holds. A window rolled up into its
        // title bar stands a line high, which is no height to place the next
        // panel by.
        if let Some(window) = window
            && window.inner.is_some()
        {
            tallest = tallest.max(window.response.rect.height());
        }
        if let Some(position) = centred {
            camera.write(MoveCamera { position: Some(position) });
        }
        if !showing {
            shut.push(system.address);
        }
    }

    // Nothing while every panel is rolled up into its title bar, which is
    // not a height to place the next one by.
    if tallest > 0. {
        panels.height = tallest;
    }
    if !shut.is_empty() {
        panels.open.retain(|panel| !shut.contains(&panel.system.address));
    }

    Ok(())
}

/// One named thing the database knows about a system
fn field(ui: &mut Ui, name: &str, value: String) {
    ui.label(name);
    ui.label(value);
    ui.end_row();
}

/// What the database says, or that it says nothing
///
/// Most of what is recorded about a system is optional, and a blank row
/// reads as a bug rather than as an answer.
fn named<T: Display>(value: &Option<T>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "Unknown".into(),
    }
}

/// A count with its digits grouped in threes
///
/// Populations run to eleven digits, which is a length rather than a number
/// until it is broken up.
fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (place, digit) in digits.char_indices() {
        if place > 0 && (digits.len() - place).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use elite_journal::Allegiance;

    /// A system with nothing on record but the address that names it
    fn system(address: i64) -> System {
        System {
            address,
            name: format!("Test {address}"),
            position: [0., 0., 0.],
            population: 0,
            allegiance: None,
            government: None,
            security: None,
            primary_economy: None,
            secondary_economy: None,
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    /// Each panel takes the place after the last one opened
    #[test]
    fn panels_take_one_place_after_another() {
        let mut panels = Panels::default();
        panels.open(system(1));
        panels.open(system(2));

        let slots: Vec<_> = panels.open.iter().map(|p| p.slot).collect();
        assert_eq!(slots, [0, 1]);
    }

    /// Shutting a panel does not hand its place to the next one
    ///
    /// Egui goes on remembering where a window was, so a panel opened again
    /// comes back to the place it had. Handing that place to some other
    /// panel in the meantime is how the two would end up on top of each
    /// other.
    #[test]
    fn a_shut_panel_keeps_its_place() {
        let mut panels = Panels::default();
        panels.open(system(1));
        panels.open(system(2));
        panels.open.retain(|panel| panel.system.address != 2);
        panels.open(system(3));

        let slots: Vec<_> = panels.open.iter().map(|p| p.slot).collect();
        assert_eq!(slots, [0, 2]);
    }

    /// The first panel opens in the corner
    #[test]
    fn the_first_panel_takes_the_corner() {
        assert_eq!(tile(0, 3, 2), (0, 0));
    }

    /// The next opens below it rather than on it
    #[test]
    fn panels_tile_down_the_edge() {
        assert_eq!(tile(1, 3, 2), (1, 0));
        assert_eq!(tile(2, 3, 2), (2, 0));
    }

    /// A full column starts a fresh one to its left
    #[test]
    fn a_full_column_moves_across() {
        assert_eq!(tile(3, 3, 2), (0, 1));
        assert_eq!(tile(5, 3, 2), (2, 1));
    }

    /// A full viewport starts over in the corner
    ///
    /// The one place two panels are left on top of each other. There is
    /// nowhere else for the next one to go.
    #[test]
    fn a_full_viewport_starts_over() {
        assert_eq!(tile(6, 3, 2), (0, 0));
    }

    /// A viewport with room for nothing still answers
    ///
    /// The tiling is measured against a window the user can drag as small as
    /// they like, and dividing by what is left is how that would come back
    /// as a crash rather than as a cramped panel.
    #[test]
    fn no_room_is_still_a_place() {
        assert_eq!(tile(0, 0, 0), (0, 0));
        assert_eq!(tile(4, 0, 0), (0, 0));
    }

    /// A system already being read about is not opened twice
    #[test]
    fn one_system_gets_one_panel() {
        let mut panels = Panels::default();
        panels.open(system(1));
        panels.open(system(1));

        assert_eq!(panels.open.len(), 1);
    }

    /// A number short enough to read is left as it is
    ///
    /// Including the empty systems, of which there are far more than
    /// inhabited ones, so this is the common answer rather than an edge.
    #[test]
    fn small_populations_are_left_alone() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(999), "999");
    }

    /// Longer ones are broken into threes from the right
    ///
    /// From the right, so that the leading group is whatever is left over
    /// rather than the number being padded to fit.
    #[test]
    fn long_populations_are_grouped_from_the_right() {
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(22_780), "22,780");
        assert_eq!(thousands(999_999), "999,999");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }

    /// The largest populations on record still read
    ///
    /// The most populous systems run to eleven digits, which is the length
    /// this is here for.
    #[test]
    fn the_largest_populations_are_grouped() {
        assert_eq!(thousands(22_780_919_531), "22,780,919,531");
    }

    /// A separator never leads or trails
    ///
    /// The grouping is decided per digit from how many follow it, so a count
    /// whose length is a multiple of three is where a stray leading comma
    /// would show up.
    #[test]
    fn grouping_never_leads_or_trails() {
        for count in [1u64, 100, 1_000, 100_000, 1_000_000] {
            let grouped = thousands(count);
            assert!(!grouped.starts_with(','), "{grouped} leads with one");
            assert!(!grouped.ends_with(','), "{grouped} trails one");
        }
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
