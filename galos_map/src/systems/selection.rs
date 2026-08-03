//! The system the user has picked out
//!
//! Pointing at a system says what is under the pointer, for as long as it is
//! there. Selecting one says what the user came for, and holds while they
//! pan, orbit and zoom around it. The two are drawn the same way in
//! different colours, so a selection reads as the lasting form of a point.
//!
//! What is selected is held as a value rather than as an entity. A system
//! reached by name is answered by the database before the map fetches
//! anything, so there is nothing on the map to mark until the camera
//! arrives, and the panel has something to say from the moment the name
//! resolves.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::pointing::{
    DRAG_THRESHOLD, DragDistance, PRIMARY, PointedAt, PointerTarget,
};
use crate::ui::PointerOverUi;
use bevy::prelude::*;
use bevy_egui::egui::Ui;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use std::fmt::Display;

pub fn plugin(app: &mut App) {
    app.init_resource::<Selection>();
    // Both answer to what is pointed at this frame, which `point_at`
    // decides. Clearing before following keeps the mark from outliving the
    // selection by a frame.
    app.add_systems(
        Update,
        (clear_when_nothing_is_clicked, follow_selection)
            .chain()
            .in_set(MapSet::Present)
            .after(super::pointing::point_at),
    );
    // Reads where a star ended up rather than deciding it, so it waits for
    // the transforms to be worked out, as `pointing::ring` does.
    app.add_systems(PostUpdate, ring.after(TransformSystems::Propagate));
    // `ui::panels` concludes at its end whether the pointer is busy with the
    // UI, from every window drawn in the pass so far. Drawn before it, this
    // window is counted in the same frame it is shown rather than the next.
    app.add_systems(EguiPrimaryContextPass, panel.before(crate::ui::panels));
}

/// The colour everything about the selection is drawn in
///
/// Answers [`super::pointing::INDICATOR`], and has to be told apart from it
/// at a glance, since hovering one system while another is selected shows
/// both rings at once.
pub const SELECTION: Srgba = Srgba::new(0.35, 0.7, 1., 1.);

/// The system the user has picked out, if any
///
/// Kept as the map's own [`System`] rather than as an entity, so that a
/// system named by search can be described before it has been fetched.
#[derive(Resource, Default)]
pub struct Selection(Option<System>);

impl Selection {
    /// Pick out `system`, in place of whatever was picked out before
    pub fn set(&mut self, system: System) {
        self.0 = Some(system);
    }

    /// Pick out nothing
    pub fn clear(&mut self) {
        self.0 = None;
    }

    /// What is picked out
    pub fn system(&self) -> Option<&System> {
        self.0.as_ref()
    }
}

/// The selected system, once it is on the map
///
/// What everything drawn for a selection asks, so that none of them has to
/// search the sky for the system named in [`Selection`].
#[derive(Component)]
pub struct Selected;

/// Keep the mark on whichever system is selected, and the selection on
/// whatever the map last heard about it
///
/// A selection outlives the entity it names: a searched system is picked out
/// before it is fetched, and one flown away from is despawned while still
/// selected. So the mark is placed by matching addresses, and the sky is
/// only swept while the two disagree.
///
/// While the two do agree, the map's row is the fresher of the two. The
/// selection was taken when the system was picked out, from a search that
/// answered before anything was fetched or from a click on a row that may
/// have been fetched some time ago, and a later fetch replaces the row
/// without the selection hearing of it. So a row that has changed is copied
/// back, and the panel says what the map holds rather than what it held.
///
/// The whole row, position included. Nothing reads the position out of the
/// selection but the panel: the mark is placed by address, and the ring is
/// drawn where the star's own transform puts it. So a system that moves
/// simply reads as having moved.
fn follow_selection(
    mut selection: ResMut<Selection>,
    marked: Query<(Entity, Ref<System>), With<Selected>>,
    systems: Query<(Entity, &System)>,
    mut commands: Commands,
) {
    let wanted = selection.0.as_ref().map(|system| system.address);

    let mut settled = false;
    for (entity, system) in &marked {
        if wanted == Some(system.address) {
            settled = true;
            if system.is_changed() {
                selection.set((*system).clone());
            }
        } else {
            commands.entity(entity).remove::<Selected>();
        }
    }

    let Some(address) = wanted else { return };
    if settled {
        return;
    }

    for (entity, system) in &systems {
        if system.address == address {
            commands.entity(entity).insert(Selected);
            // The row that has just arrived, rather than the one the search
            // was answered with, which is what the mark being placed here
            // means in the first place.
            selection.set(system.clone());
            break;
        }
    }
}

/// Let go of the selection when a click lands on nothing
///
/// The same three questions a click on a system is weighed by, so that the
/// two cannot both answer one press: the primary button, travel short enough
/// to be a click rather than a drag of the map, and the pointer's own
/// business rather than the UI's. What is left is a click on empty sky.
fn clear_when_nothing_is_clicked(
    buttons: Res<ButtonInput<MouseButton>>,
    over_ui: Res<PointerOverUi>,
    dragged: Query<&DragDistance>,
    pointed_at: Query<(), With<PointedAt>>,
    mut selection: ResMut<Selection>,
) {
    if !buttons.just_released(PRIMARY) || over_ui.0 {
        return;
    }
    if dragged.iter().any(|travelled| travelled.0 > DRAG_THRESHOLD) {
        return;
    }
    if !pointed_at.is_empty() || selection.0.is_none() {
        return;
    }

    selection.clear();
}

/// Ring the selected system
///
/// Drawn from the same target [`super::pointing::ring`] measures, so the two
/// rings are the same size and a selection sits exactly where a point did.
///
/// A system the spyglass has hidden is skipped, since a ring around a star
/// that is not drawn is a ring around nothing.
fn ring(
    mut gizmos: Gizmos,
    camera: Query<&OrbitCamera>,
    selected: Query<
        (&GlobalTransform, &Visibility, &Children),
        (With<System>, With<Selected>),
    >,
    targets: Query<&GlobalTransform, With<PointerTarget>>,
) {
    let Ok(camera) = camera.single() else { return };

    for (system, visibility, children) in &selected {
        if *visibility == Visibility::Hidden {
            continue;
        }

        let Some(radius) = children
            .iter()
            .filter_map(|child| targets.get(child).ok())
            .map(|target| target.scale().x)
            .next()
        else {
            continue;
        };

        gizmos.circle(
            Isometry3d::new(system.translation(), camera.rotation),
            radius,
            SELECTION,
        );
    }
}

/// Tell the user what is known about the system they picked out
///
/// Written here rather than alongside the rest of the UI because a
/// [`System`]'s fields are the business of this module and its neighbours,
/// and this is the one place they are read out rather than drawn with.
fn panel(
    mut contexts: EguiContexts,
    mut selection: ResMut<Selection>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let Some(system) = selection.0.as_ref() else { return Ok(()) };

    // Closing the window is what clears the selection, so the panel is shut
    // the way any window is rather than by a control of its own.
    let mut open = true;
    // Named for the system but identified by something that does not change
    // with it, so that the window stays where the user put it as they go
    // from one system to the next.
    egui::Window::new(system.name.as_str())
        .id(egui::Id::new("selection"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.set_width(230.);
            egui::Grid::new("selection-fields").num_columns(2).show(ui, |ui| {
                let [x, y, z] = system.position;
                field(ui, "Position", format!("{x:.2}, {y:.2}, {z:.2}"));
                field(ui, "Population", thousands(system.population));
                field(ui, "Allegiance", named(&system.allegiance));
                field(ui, "Government", named(&system.government));
                field(ui, "Security", named(&system.security));
                field(ui, "Economy", named(&system.primary_economy));
                field(ui, "Secondary", named(&system.secondary_economy));
                field(
                    ui,
                    "Updated",
                    system.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                );
            });
        });

    if !open {
        selection.clear();
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

    /// A world with nothing in it but the selection and the mark
    fn map() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Selection>();
        app.add_systems(Update, follow_selection);
        app
    }

    /// The mark goes to the system the selection names
    #[test]
    fn the_mark_lands_on_what_is_selected() {
        let mut app = map();
        let one = app.world_mut().spawn(system(1)).id();
        let two = app.world_mut().spawn(system(2)).id();

        app.world_mut().resource_mut::<Selection>().set(system(2));
        app.update();

        assert!(!app.world().entity(one).contains::<Selected>());
        assert!(app.world().entity(two).contains::<Selected>());
    }

    /// Selecting a second system takes the mark off the first
    ///
    /// Only one system is selected at a time, and the mark is how everything
    /// drawn finds it, so two of them would draw two selections.
    #[test]
    fn the_mark_follows_a_change_of_selection() {
        let mut app = map();
        let one = app.world_mut().spawn(system(1)).id();
        let two = app.world_mut().spawn(system(2)).id();

        app.world_mut().resource_mut::<Selection>().set(system(1));
        app.update();
        app.world_mut().resource_mut::<Selection>().set(system(2));
        app.update();

        assert!(!app.world().entity(one).contains::<Selected>());
        assert!(app.world().entity(two).contains::<Selected>());
    }

    /// A system selected before it is on the map is marked when it arrives
    ///
    /// Which is what a search does: the name resolves against the database
    /// while the map has yet to fetch anything around it.
    #[test]
    fn the_mark_waits_for_a_system_to_arrive() {
        let mut app = map();

        app.world_mut().resource_mut::<Selection>().set(system(1));
        app.update();

        let one = app.world_mut().spawn(system(1)).id();
        app.update();

        assert!(app.world().entity(one).contains::<Selected>());
    }

    /// A system arriving on the map answers for itself
    ///
    /// Which is the other half of what a search leaves behind: the selection
    /// was built from the row the name resolved against, and the fetch that
    /// follows the camera is the map's own answer about the same system.
    #[test]
    fn a_system_arriving_brings_its_own_row() {
        let mut app = map();

        app.world_mut().resource_mut::<Selection>().set(system(1));
        app.update();

        let mut fetched = system(1);
        fetched.population = 42;
        app.world_mut().spawn(fetched);
        app.update();

        assert_eq!(population_shown(&app), 42);
    }

    /// A row that changes under the selection is carried into it
    ///
    /// A fetch replaces the row of a system already on the map, and the
    /// panel is drawn from the selection rather than from the entity, so
    /// without this it would go on saying what was true when the system was
    /// picked out.
    #[test]
    fn a_changed_row_reaches_the_selection() {
        let mut app = map();
        let one = app.world_mut().spawn(system(1)).id();

        app.world_mut().resource_mut::<Selection>().set(system(1));
        app.update();

        let mut fresher = system(1);
        fresher.population = 1_000;
        app.world_mut().entity_mut(one).insert(fresher);
        app.update();

        assert_eq!(population_shown(&app), 1_000);
    }

    /// What the panel would draw for the population
    fn population_shown(app: &App) -> u64 {
        app.world().resource::<Selection>().system().unwrap().population
    }

    /// Clearing the selection takes the mark with it
    #[test]
    fn the_mark_goes_when_the_selection_does() {
        let mut app = map();
        let one = app.world_mut().spawn(system(1)).id();

        app.world_mut().resource_mut::<Selection>().set(system(1));
        app.update();
        app.world_mut().resource_mut::<Selection>().clear();
        app.update();

        assert!(!app.world().entity(one).contains::<Selected>());
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
