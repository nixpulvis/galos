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
//! arrives, and the name is worth colouring from the moment it resolves.
//!
//! A click on empty sky lets go of a selection, unless the UI has already
//! answered that press. A search leaves the camera where it is, so what is
//! picked out is what the user is working with rather than where they happen
//! to be looking, and the press that shuts a form is no reason to throw a
//! typed name away.
//!
//! What the map knows about the selected system beyond its name is written
//! out by [`super::info`], which the user asks for separately.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::pointing::{
    DRAG_THRESHOLD, DragDistance, PRIMARY, PointedAt, PointerTarget,
};
use crate::ui::{PointerOverUi, PressAnswered};
use bevy::math::DVec3;
use bevy::prelude::*;

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

    /// What is picked out is called, if anything is
    ///
    /// Alongside [`Self::position`] and there for the same reason: the
    /// search bar says what it is holding, and a [`System`]'s fields are
    /// private to [`super`].
    pub fn name(&self) -> Option<&str> {
        self.0.as_ref().map(|system| system.name.as_str())
    }

    /// Where what is picked out is, if anything is
    ///
    /// A [`System`]'s fields are private to [`super`], and the control that
    /// sends the camera to the selection is drawn with the rest of the UI,
    /// which is not. So this is the one thing about the selected system the
    /// rest of the crate can ask for.
    pub fn position(&self) -> Option<DVec3> {
        self.0.as_ref().map(|system| DVec3::from(system.position))
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
/// back, and what is picked out is the row the map holds rather than the one
/// it held when the user pointed at it.
///
/// Nothing drawn is placed from the selection's own position: the mark goes
/// by address, and the ring is drawn where the star's transform puts it. The
/// row is kept whole all the same, since a selection describing a system in
/// part would have to be asked which part.
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
///
/// A press the UI has already spent is the UI's own business too, even
/// though it landed on the map. Shutting the search form is done by pressing
/// off it, and that press closing a form and letting go of a selection would
/// be one gesture doing two things.
fn clear_when_nothing_is_clicked(
    buttons: Res<ButtonInput<MouseButton>>,
    over_ui: Res<PointerOverUi>,
    mut press: ResMut<PressAnswered>,
    dragged: Query<&DragDistance>,
    pointed_at: Query<(), With<PointedAt>>,
    mut selection: ResMut<Selection>,
) {
    if !buttons.just_released(PRIMARY) {
        return;
    }
    // Spent either way, since whatever the press was for is over.
    let answered = std::mem::take(&mut press.0);
    if over_ui.0 || answered {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

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

    /// What the selection holds for the population
    fn population_shown(app: &App) -> u64 {
        app.world().resource::<Selection>().system().unwrap().population
    }

    /// The selection says where what is picked out is
    ///
    /// Which is all the control that centres the camera on it can ask: a
    /// `System`'s fields are private to this module and its neighbours, and
    /// that control is drawn with the rest of the UI.
    #[test]
    fn the_selection_says_where_it_is() {
        let mut selection = Selection::default();
        assert_eq!(selection.position(), None);

        let mut sol = system(1);
        sol.position = [1., -2., 3.];
        selection.set(sol);

        assert_eq!(selection.position(), Some(DVec3::new(1., -2., 3.)));
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
}
