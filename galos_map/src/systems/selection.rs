//! The systems the user has picked out
//!
//! Pointing at a system says what is under the pointer, for as long as it is
//! there. Selecting one says what the user came for, and holds while they
//! pan, orbit and zoom around it. The two are drawn the same way in
//! different colors, so a selection reads as the lasting form of a point.
//!
//! Several are held at once, in the order they were picked. A plain click
//! picks one out in place of the rest and ctrl-click gathers them up, so a
//! handful can be marked out together and handed to a filter, which is what
//! makes the set worth holding rather than only the last one clicked.
//!
//! What is selected is held as values rather than as entities. A system
//! reached by name is answered by the database before the map fetches
//! anything, so there is nothing on the map to mark until the camera
//! arrives, and the name is worth coloring from the moment it resolves.
//!
//! A click on empty sky lets go of a selection, so long as the click was the
//! map's rather than the UI's. A search leaves the camera where it is, so what
//! is picked out is what the user is working with rather than where they
//! happen to be looking, and the press that shuts a form is no reason to throw
//! a typed name away.
//!
//! What the map knows about the selected system beyond its name is written
//! out by [`super::info`], which the user asks for separately.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::pointing::{
    DRAG_THRESHOLD, DragDistance, Indicator, PointedAt,
};
use crate::systems::{Spyglass, System};
use crate::ui::Gesture;
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

/// The color everything about the selection is drawn in
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
    /// the line naming it in what a search found are the same system in two
    /// places, so they are picked out by the one gesture and through the one
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
/// The same two questions a click on a system is weighed by, so that the two
/// cannot both answer one press: a click the map owns, and travel short
/// enough to be a click rather than a drag of the map. What is left is a
/// click on empty sky.
///
/// Whose the click is covers what the pointer was over and what the UI spent
/// it on both. Shutting the bar's form is done by pressing off it, and that
/// press closing a form and letting go of a selection would be one gesture
/// doing two things.
fn clear_when_nothing_is_clicked(
    gesture: Gesture,
    dragged: Query<&DragDistance>,
    pointed_at: Query<(), With<PointedAt>>,
    mut selection: ResMut<Selection>,
) {
    if !gesture.on_map() {
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
    camera: Query<(&OrbitCamera, &Camera)>,
    spyglass: Res<Spyglass>,
    selected: Query<
        (&System, &GlobalTransform, &Indicator, Has<Filtered>),
        With<Selected>,
    >,
    dim: Res<DimTo>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (system, at, indicator, filtered) in &selected {
        // Reach rather than whether the star is drawn. The two part company
        // where the filters draw what they exclude at nothing, and this ring
        // answers the wrong one of them: the spyglass says where the user is
        // looking, and a ring outside it is a ring off the edge of that. What
        // the filters say is about the sky rather than about the handful of
        // systems the user picked out by hand.
        let position = DVec3::from(system.position);
        if !spyglass.reaches(orbit.center, position) {
            continue;
        }

        // The mark is held in pixels, and a gizmo is drawn in the world, so
        // this is where the two meet. Through the same conversion the
        // pointing ring uses, so the two circles are the same circle.
        let radius = super::pointing::drawn_radius(
            orbit,
            cot_half_fov,
            viewport,
            position,
            indicator.0,
        );

        gizmos.circle(
            Isometry3d::new(at.translation(), orbit.rotation),
            radius,
            ringed(&dim, filtered),
        );
    }
}

/// What color a selected system's ring is drawn in
///
/// It dims with the star it is drawn around, so that a selection the filters
/// exclude does not read as one they have let go of.
///
/// Where that star is not drawn at all there is nothing to dim with, and a
/// mark faded to nothing is no mark. So the ring stands at full strength and
/// says where the system is with nothing drawn inside it, which is the whole
/// of what is left to say: the filters have taken the sky away and the user
/// has picked this one out of it regardless.
fn ringed(dim: &DimTo, filtered: bool) -> Srgba {
    if filtered && dim.0 == 0. {
        SELECTION
    } else {
        dim.against(SELECTION, filtered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::pointing::PRIMARY;
    use crate::systems::tests::system;
    use crate::ui::Grasp;

    /// A world with nothing in it but the selection and the mark
    fn map() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Selection>();
        app.add_systems(Update, follow_selection);
        app
    }

    /// A world holding a selection and the click that may let go of it
    ///
    /// Nothing is pointed at and no pointer has travelled, so what the click
    /// lands on is empty sky.
    fn clicked_on() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<Grasp>();

        let mut selection = Selection::default();
        selection.set(system(1));
        app.insert_resource(selection);

        app.add_systems(Update, clear_when_nothing_is_clicked);
        app
    }

    /// Take a frame, with the button doing `act` at the start of it and the
    /// UI settling whose the press was at the end
    ///
    /// The order egui runs in: it draws from `PostUpdate`, after everything
    /// that answers a click, so a press is settled at the close of the frame
    /// it landed in. `wanted` is whether the UI took it.
    fn frame(
        app: &mut App,
        wanted: bool,
        act: impl FnOnce(&mut ButtonInput<MouseButton>),
    ) {
        let mut buttons =
            app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        buttons.clear();
        act(&mut buttons);

        app.update();

        let world = app.world_mut();
        let buttons = world.resource::<ButtonInput<MouseButton>>().clone();
        world.resource_mut::<Grasp>().settle(&buttons, wanted);
    }

    /// Whether anything is still held
    fn holding(app: &App) -> bool {
        !app.world().resource::<Selection>().is_empty()
    }

    /// A click on empty sky lets go of the selection
    #[test]
    fn a_click_on_nothing_lets_go() {
        let mut app = clicked_on();

        frame(&mut app, false, |buttons| buttons.press(PRIMARY));
        assert!(holding(&app), "let go before the button came up");
        frame(&mut app, false, |buttons| buttons.release(PRIMARY));

        assert!(!holding(&app));
    }

    /// A press the UI took is not the map's to answer
    ///
    /// Whose it was is settled at the press and stands for the whole of it,
    /// so the release finds the answer already there.
    #[test]
    fn a_press_the_ui_took_does_not_let_go() {
        let mut app = clicked_on();

        frame(&mut app, true, |buttons| buttons.press(PRIMARY));
        frame(&mut app, false, |buttons| buttons.release(PRIMARY));

        assert!(holding(&app));
    }

    /// Nor when the whole click falls inside one frame
    ///
    /// A frame slow enough to hold a press and its release together puts the
    /// map's reading of the click before the UI's, egui drawing from
    /// `PostUpdate`. The click that shut the form would otherwise let go of
    /// the selection as well, which is one gesture doing two things.
    #[test]
    fn a_whole_click_in_one_slow_frame_does_not_let_go() {
        let mut app = clicked_on();

        frame(&mut app, true, |buttons| {
            buttons.press(PRIMARY);
            buttons.release(PRIMARY);
        });
        frame(&mut app, false, |_| {});

        assert!(holding(&app));
    }

    /// And the click after it is still the map's to answer
    ///
    /// Whose a press was is spent on that press. Left standing, it would be
    /// taken out of the next click instead, and the selection would outlast
    /// the gesture that let go of it.
    #[test]
    fn the_click_after_a_slow_one_still_lets_go() {
        let mut app = clicked_on();

        frame(&mut app, true, |buttons| {
            buttons.press(PRIMARY);
            buttons.release(PRIMARY);
        });
        frame(&mut app, false, |_| {});
        assert!(holding(&app), "let go of it on the UI's own click");

        frame(&mut app, false, |buttons| buttons.press(PRIMARY));
        frame(&mut app, false, |buttons| buttons.release(PRIMARY));

        assert!(!holding(&app));
    }

    /// A whole click in one frame still lets go where the UI wanted none of it
    ///
    /// Held over a frame rather than thrown away. A slow map is one a click
    /// still has to work on.
    #[test]
    fn a_whole_click_in_one_frame_lets_go_of_its_own() {
        let mut app = clicked_on();

        frame(&mut app, false, |buttons| {
            buttons.press(PRIMARY);
            buttons.release(PRIMARY);
        });
        frame(&mut app, false, |_| {});

        assert!(!holding(&app));
    }

    /// A press that began on the map is the map's wherever it ends
    ///
    /// The UI wanting the pointer by the time the button comes up says
    /// nothing about the gesture: a drag off the sky that happens to finish
    /// over a panel is still a drag of the sky.
    #[test]
    fn a_press_that_began_on_the_map_stays_the_map_s() {
        let mut app = clicked_on();

        frame(&mut app, false, |buttons| buttons.press(PRIMARY));
        frame(&mut app, true, |buttons| buttons.release(PRIMARY));

        assert!(!holding(&app));
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

    /// A ring dims with the star it is drawn around
    ///
    /// So that a selection the filters exclude does not read as one they have
    /// let go of.
    #[test]
    fn a_ring_dims_with_its_star() {
        let faint = ringed(&DimTo(0.15), true);

        assert!(faint.alpha < SELECTION.alpha);
        assert_eq!(ringed(&DimTo(0.15), false), SELECTION);
    }

    /// And stands at full strength where there is no star to dim with
    ///
    /// At an opacity of nothing the star is not drawn, so a ring dimmed to
    /// match it would be no ring, and the one thing left to say is where the
    /// system the user picked out is.
    #[test]
    fn a_ring_with_no_star_stands_at_full_strength() {
        assert_eq!(ringed(&DimTo(0.), true), SELECTION);
    }

    /// What is not excluded is never dimmed, whatever the opacity
    #[test]
    fn a_ring_around_an_admitted_system_is_never_dimmed() {
        assert_eq!(ringed(&DimTo(0.), false), SELECTION);
        assert_eq!(ringed(&DimTo(1.), false), SELECTION);
    }

    /// A settled selection is not written to as the frames go by
    ///
    /// What [`super::fetch::fetch_selected`] leans on. It asks the database
    /// for whatever is picked out and has no star, and it asks only when the
    /// selection changes, so a selection that read as changed every frame
    /// would put a query on the wire every frame for as long as one system
    /// stayed picked out.
    #[test]
    fn a_settled_selection_holds_still() {
        let mut app = map();

        app.world_mut().resource_mut::<Selection>().set(system(1));
        app.world_mut().spawn(system(1));
        // One to place the mark, and one for the write that placing it
        // brought with it to have been and gone.
        app.update();
        app.update();

        let changed_at =
            app.world().resource_ref::<Selection>().last_changed().get();
        app.update();
        app.update();

        assert_eq!(
            app.world().resource_ref::<Selection>().last_changed().get(),
            changed_at,
            "the selection was written to with nothing about it changing"
        );
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
    /// Which is all the control that centers the camera on it can ask: a
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
