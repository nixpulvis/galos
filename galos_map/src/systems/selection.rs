//! The systems the user has picked out
//!
//! Pointing at a system says what is under the pointer, for as long as it is
//! there. Selecting one says what the user came for, and holds while they
//! pan, orbit and zoom around it. The two are drawn the same way in
//! different colours, so a selection reads as the lasting form of a point.
//!
//! Several are held at once, in the order they were picked. A plain click
//! picks one out in place of the rest and ctrl-click gathers them up, so a
//! handful can be marked out together and handed to a filter, which is what
//! makes the set worth holding rather than only the last one clicked.
//!
//! What is selected is held as values rather than as entities. A system
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
use crate::systems::filter::{self, Filtered};
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

/// The systems the user has picked out
///
/// Kept as the map's own [`System`]s rather than as entities, so that a
/// system named by search can be described before it has been fetched.
///
/// In the order they were picked. A set with an order is what lets the bar
/// draw a row per system that holds still: ordering them by anything measured
/// from the camera would have the rows swap places as the user flies, and a
/// close mark that moves out from under the pointer is a close mark that
/// lets go of the wrong system.
#[derive(Resource, Default)]
pub struct Selection(Vec<System>);

impl Selection {
    /// Pick `system` out, alongside the rest or in place of them
    ///
    /// What a click means, wherever the click landed. A star in the sky and
    /// the line naming it in the search results are the same system offered
    /// twice, so they are picked out by the one gesture and through the one
    /// call: `gathering` is whether the user held the modifier that means as
    /// well as rather than instead.
    pub fn pick(&mut self, system: System, gathering: bool) {
        if gathering {
            self.toggle(system);
        } else {
            self.set(system);
        }
    }

    /// Pick out `system` alone, in place of whatever was picked out before
    pub fn set(&mut self, system: System) {
        self.0.clear();
        self.0.push(system);
    }

    /// Pick `system` out alongside the rest, or let go of it if it is already
    ///
    /// One gesture that builds a set and takes it apart again, so that a
    /// system added by mistake is undone by doing the same thing twice rather
    /// than by starting over.
    pub fn toggle(&mut self, system: System) {
        match self.0.iter().position(|held| held.address == system.address) {
            Some(at) => {
                self.0.remove(at);
            }
            None => self.0.push(system),
        }
    }

    /// Pick out nothing
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Let go of the system in the `index`th place, and hold the rest
    pub fn remove(&mut self, index: usize) {
        if index < self.0.len() {
            self.0.remove(index);
        }
    }

    /// The system in the `index`th place
    ///
    /// A whole row, for whoever can read one. A [`System`]'s fields are
    /// private to [`super`], so the bar reaches what it draws through
    /// [`Self::name`] and [`Self::position`] and uses this only to hand the
    /// row on to a panel.
    pub fn system(&self, index: usize) -> Option<&System> {
        self.0.get(index)
    }

    /// What the system in the `index`th place is called
    pub fn name(&self, index: usize) -> Option<&str> {
        self.0.get(index).map(|system| system.name.as_str())
    }

    /// Which system stands in the `index`th place
    ///
    /// What the bar keys a row on. A row keyed on where it sits would hand
    /// its place, and whatever egui remembers against it, to whichever row
    /// moved up when one above it was let go of.
    pub fn address(&self, index: usize) -> Option<i64> {
        self.0.get(index).map(|system| system.address)
    }

    /// How many are picked out
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing at all is picked out
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every system picked out, by address, in the order it was picked
    ///
    /// What a filter over the selection is built from. Addresses rather than
    /// rows, since that is all a filter tests against, and because a filter
    /// holding its own copy is what lets the selection be let go of while the
    /// filter stands.
    pub fn addresses(&self) -> Vec<i64> {
        self.0.iter().map(|system| system.address).collect()
    }

    /// Where the system in the `index`th place is
    ///
    /// A [`System`]'s fields are private to [`super`], and the control that
    /// sends the camera to a selected system is drawn with the rest of the
    /// UI, which is not. So this is what the rest of the crate can ask.
    pub fn position(&self, index: usize) -> Option<DVec3> {
        self.0.get(index).map(|system| DVec3::from(system.position))
    }
}

/// A selected system, once it is on the map
///
/// What everything drawn for a selection asks, so that none of them has to
/// search the sky for the systems named in [`Selection`].
#[derive(Component)]
pub struct Selected;

/// Keep a mark on every selected system, and the selection on whatever the
/// map last heard about each of them
///
/// A selection outlives the entity it names: a searched system is picked out
/// before it is fetched, and one flown away from is despawned while still
/// selected. So the marks are placed by matching addresses, and the sky is
/// only swept for addresses that have none.
///
/// Where a mark and a row do meet, the map's row is the fresher of the two.
/// The selection was taken when the system was picked out, from a search that
/// answered before anything was fetched or from a click on a row that may
/// have been fetched some time ago, and a later fetch replaces the row
/// without the selection hearing of it. So a row that has changed is copied
/// back, and what is picked out is the row the map holds rather than the one
/// it held when the user pointed at it.
///
/// Nothing drawn is placed from the selection's own position: the marks go by
/// address, and each ring is drawn where its star's transform puts it. The
/// rows are kept whole all the same, since a selection describing a system in
/// part would have to be asked which part.
fn follow_selection(
    mut selection: ResMut<Selection>,
    marked: Query<(Entity, Ref<System>), With<Selected>>,
    systems: Query<(Entity, &System)>,
    mut commands: Commands,
) {
    // Which of the selected systems already wear a mark, so that the sweep
    // below looks only for the ones that do not.
    let mut settled = vec![false; selection.0.len()];

    for (entity, system) in &marked {
        let held = selection
            .0
            .iter()
            .position(|selected| selected.address == system.address);
        match held {
            Some(at) => {
                settled[at] = true;
                if system.is_changed() {
                    selection.0[at] = (*system).clone();
                }
            }
            None => {
                commands.entity(entity).remove::<Selected>();
            }
        }
    }

    if settled.iter().all(|found| *found) {
        return;
    }

    // One sweep for however many are still missing, rather than one each.
    // The sky runs to thousands of systems and the selection to a handful.
    for (entity, system) in &systems {
        let held = selection
            .0
            .iter()
            .position(|selected| selected.address == system.address);
        let Some(at) = held else { continue };
        if settled[at] {
            continue;
        }
        settled[at] = true;
        commands.entity(entity).insert(Selected);
        // The row that has just arrived, rather than the one the search was
        // answered with, which is what the mark being placed here means in
        // the first place.
        selection.0[at] = system.clone();
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
/// though it landed on the map. Shutting the bar's form is done by pressing
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
    if !pointed_at.is_empty() || selection.is_empty() {
        return;
    }

    // All of them. The gesture means let go, and letting go of some of what
    // is held is not something a click on empty sky could say which of.
    selection.clear();
}

/// Ring the selected system
///
/// Drawn from the same target [`super::pointing::ring`] measures, so the two
/// rings are the same size and a selection sits exactly where a point did.
///
/// A system the spyglass has hidden is skipped, since a ring around a star
/// that is not drawn is a ring around nothing.
///
/// A ring dims with the star it is drawn around. A selection the filters
/// exclude stays selected, and a full strength ring around a faint star would
/// read as the filter having let go of it.
fn ring(
    mut gizmos: Gizmos,
    camera: Query<&OrbitCamera>,
    selected: Query<
        (&GlobalTransform, &Visibility, &Children, Has<Filtered>),
        (With<System>, With<Selected>),
    >,
    targets: Query<&GlobalTransform, With<PointerTarget>>,
) {
    let Ok(camera) = camera.single() else { return };

    for (system, visibility, children, filtered) in &selected {
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
            filter::dim(SELECTION, filtered),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::tests::system;

    /// A world with nothing in it but the selection and the mark
    fn map() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Selection>();
        app.add_systems(Update, follow_selection);
        app
    }

    /// A click reads as replace or as gather, by the modifier alone
    ///
    /// The one call a click goes through, whether it landed on a star or on
    /// the line naming that star in the search results. Both used to work the
    /// modifier out for themselves, and two readings of one gesture is one
    /// more than there is a gesture for.
    #[test]
    fn a_pick_replaces_or_gathers_by_the_modifier() {
        let mut selection = Selection::default();

        selection.pick(system(1), false);
        selection.pick(system(2), true);
        assert_eq!(selection.addresses(), vec![1, 2]);

        selection.pick(system(2), true);
        assert_eq!(selection.addresses(), vec![1]);

        selection.pick(system(3), false);
        assert_eq!(selection.addresses(), vec![3]);
    }

    /// Picking one out plainly lets go of whatever was held
    #[test]
    fn picking_one_out_replaces_the_rest() {
        let mut selection = Selection::default();
        selection.set(system(1));
        selection.toggle(system(2));

        selection.set(system(3));

        assert_eq!(selection.addresses(), vec![3]);
    }

    /// Gathering keeps what is held, in the order it was picked
    #[test]
    fn gathering_holds_them_in_the_order_they_were_picked() {
        let mut selection = Selection::default();
        selection.set(system(2));
        selection.toggle(system(1));
        selection.toggle(system(3));

        assert_eq!(selection.addresses(), vec![2, 1, 3]);
    }

    /// And gathering one already held lets go of it
    ///
    /// One gesture that builds a set and takes it apart, so a system added by
    /// mistake is undone by doing the same thing again.
    #[test]
    fn gathering_one_already_held_lets_go_of_it() {
        let mut selection = Selection::default();
        selection.set(system(1));
        selection.toggle(system(2));

        selection.toggle(system(1));

        assert_eq!(selection.addresses(), vec![2]);
    }

    /// A row's close mark lets go of that one and holds the rest
    #[test]
    fn letting_go_of_one_holds_the_rest() {
        let mut selection = Selection::default();
        selection.set(system(1));
        selection.toggle(system(2));
        selection.toggle(system(3));

        selection.remove(1);

        assert_eq!(selection.addresses(), vec![1, 3]);
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

    /// Picking a second system out plainly takes the mark off the first
    ///
    /// A plain pick replaces what is held, so the mark left on the system let
    /// go of would ring a star nothing names.
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

    /// Every system in the set wears a mark
    ///
    /// The mark is how everything drawn finds them, so one gathered and left
    /// unmarked would be a system in the bar with no ring on the map.
    #[test]
    fn every_gathered_system_is_marked() {
        let mut app = map();
        let one = app.world_mut().spawn(system(1)).id();
        let two = app.world_mut().spawn(system(2)).id();
        let three = app.world_mut().spawn(system(3)).id();

        let mut selection = app.world_mut().resource_mut::<Selection>();
        selection.set(system(1));
        selection.toggle(system(3));
        app.update();

        assert!(app.world().entity(one).contains::<Selected>());
        assert!(!app.world().entity(two).contains::<Selected>());
        assert!(app.world().entity(three).contains::<Selected>());
    }

    /// One let go of loses its mark, and the rest keep theirs
    #[test]
    fn letting_go_of_one_takes_only_its_mark() {
        let mut app = map();
        let one = app.world_mut().spawn(system(1)).id();
        let two = app.world_mut().spawn(system(2)).id();

        let mut selection = app.world_mut().resource_mut::<Selection>();
        selection.set(system(1));
        selection.toggle(system(2));
        app.update();

        app.world_mut().resource_mut::<Selection>().remove(0);
        app.update();

        assert!(!app.world().entity(one).contains::<Selected>());
        assert!(app.world().entity(two).contains::<Selected>());
    }

    /// A row that changes reaches the right one of several
    ///
    /// The copy-back has to find its own place in the set. Writing the fresher
    /// row into the wrong one would have the bar name a system twice and lose
    /// the other.
    #[test]
    fn a_changed_row_reaches_its_own_place_in_the_set() {
        let mut app = map();
        app.world_mut().spawn(system(1));
        let two = app.world_mut().spawn(system(2)).id();

        let mut selection = app.world_mut().resource_mut::<Selection>();
        selection.set(system(1));
        selection.toggle(system(2));
        app.update();

        let mut fresher = system(2);
        fresher.population = 900;
        app.world_mut().entity_mut(two).insert(fresher);
        app.update();

        let selection = app.world().resource::<Selection>();
        assert_eq!(selection.addresses(), vec![1, 2]);
        assert_eq!(selection.system(0).unwrap().population, 0);
        assert_eq!(selection.system(1).unwrap().population, 900);
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
        app.world().resource::<Selection>().system(0).unwrap().population
    }

    /// The selection says where what is picked out is
    ///
    /// Which is all the control that centres the camera on it can ask: a
    /// `System`'s fields are private to this module and its neighbours, and
    /// that control is drawn with the rest of the UI.
    #[test]
    fn the_selection_says_where_it_is() {
        let mut selection = Selection::default();
        assert_eq!(selection.position(0), None);

        let mut sol = system(1);
        sol.position = [1., -2., 3.];
        selection.set(sol);

        assert_eq!(selection.position(0), Some(DVec3::new(1., -2., 3.)));
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
