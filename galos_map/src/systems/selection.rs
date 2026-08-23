//! What the user has picked out
//!
//! Pointing at something says what is under the pointer, for as long as it is
//! there. Selecting it says what the user came for, and holds while they pan,
//! orbit and zoom around it. The two are drawn the same way in different
//! colors, so a selection reads as the lasting form of a point.
//!
//! A system out in the sky and a body inside one are both picked out, by the
//! same gesture and into the same list. Which of the two something is decides
//! how it is drawn and what its row says, and decides nothing about how it is
//! held or how it is picked: a plain click holds one thing in place of the
//! rest, and the modifier gathers.
//!
//! Several are held at once, in the order they were picked. A handful can be
//! marked out together and handed to a filter, which is what makes the set
//! worth holding rather than only the last one clicked.
//!
//! What is selected is held as values rather than as entities. A system
//! reached by name is answered by the database before the map fetches
//! anything, so there is nothing on the map to mark until the camera arrives,
//! and the name is worth coloring from the moment it resolves. A body is held
//! the same way for the sake of one list rather than out of need, and
//! [`follow_selection`] is the one place either is matched to what is drawn.
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
use crate::systems::bodies::spawn::{Body, HeldSystem, Strength};
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::pointing::{
    DRAG_THRESHOLD, DragDistance, Indicator, PointedAt, RING_POINTS,
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
        (clear_when_nothing_is_clicked, clear_not_drawn, follow_selection)
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

/// One thing the user has picked out
///
/// A system out in the sky, or a body inside one. The gesture that picks them
/// is the same gesture and the list holds them together in the order they were
/// picked, so nothing has to ask which kind it has except where the two are
/// drawn differently.
#[derive(Clone)]
pub enum Picked {
    System(System),
    Body(PickedBody),
}

/// A body picked out, as the map knew it at the moment it was picked
///
/// Everything a row needs and nothing that has to be asked of the map again.
/// A body does not move once it is placed, and where a row is drawn from has
/// nothing to do with where the camera is, so its place is taken once here
/// rather than worked out afresh every frame.
#[derive(Clone)]
pub struct PickedBody {
    /// Which system it is in, and which of that system's numbering it is
    ///
    /// What tells one body from another. The id alone is not enough: only one
    /// system's contents are drawn at a time, but a selection outlives the
    /// camera leaving one, and body 3 of the next system along is a different
    /// body.
    address: i64,
    id: i16,
    name: String,
    /// Where it stands in the galaxy, in light years
    ///
    /// What its row measures from the focus, as a system's row measures its
    /// own position. Everything else about a body is asked of the panel
    /// describing it rather than carried here.
    at: DVec3,
}

impl PickedBody {
    /// A body of `name`, the `id`th of the system at `address`, standing `at`
    pub fn new(address: i64, id: i16, name: &str, at: DVec3) -> Self {
        PickedBody { address, id, name: name.to_owned(), at }
    }

    /// Which of its system's numbering it is
    ///
    /// What the map's own rows for it are found by, which is where everything
    /// about a body beyond its name is written down.
    pub fn id(&self) -> i16 {
        self.id
    }
}

impl Picked {
    /// What it is called
    pub fn name(&self) -> &str {
        match self {
            Picked::System(system) => &system.name,
            Picked::Body(body) => &body.name,
        }
    }

    /// Where it stands in the galaxy, in light years
    pub fn position(&self) -> DVec3 {
        match self {
            Picked::System(system) => DVec3::from(system.position),
            Picked::Body(body) => body.at,
        }
    }

    /// Which system it is, or is inside
    ///
    /// What the bar keys a row on. A row keyed on where it sits would hand its
    /// place, and whatever egui remembers against it, to whichever row moved
    /// up when one above it was let go of.
    pub fn address(&self) -> i64 {
        match self {
            Picked::System(system) => system.address,
            Picked::Body(body) => body.address,
        }
    }

    /// Which of a system's numbering it is, where it is a body
    ///
    /// Nothing for a system, which is the place rather than a thing in it.
    /// Two picked things are the same thing where both of these agree.
    pub fn id(&self) -> Option<i16> {
        match self {
            Picked::System(_) => None,
            Picked::Body(body) => Some(body.id),
        }
    }

    /// Whether the two name the same thing
    fn same(&self, other: &Picked) -> bool {
        self.address() == other.address() && self.id() == other.id()
    }
}

/// What the user has picked out
///
/// Kept as values rather than as entities, so that a system named by a search
/// can be described before it has been fetched, and so that both kinds of
/// thing are held the same way. What is on the map wears [`Selected`], which
/// [`follow_selection`] puts there by matching one against the other.
///
/// In the order they were picked. A set with an order is what lets the bar
/// draw a row apiece that holds still: ordering them by anything measured
/// from the camera would have the rows swap places as the user flies, and a
/// close mark that moves out from under the pointer is a close mark that
/// lets go of the wrong thing.
#[derive(Resource, Default)]
pub struct Selection(Vec<Picked>);

impl Selection {
    /// Pick `picked` out, alongside the rest or in place of them
    ///
    /// What a click means, wherever the click landed and whatever it landed
    /// on. A star in the sky, a planet inside one and the line naming a system
    /// in what a search found are all picked out by the one gesture and
    /// through the one call: `gathering` is whether the user held the key
    /// that means as well as rather than instead.
    pub fn pick(&mut self, picked: Picked, gathering: bool) {
        if gathering {
            self.toggle(picked);
        } else {
            self.set(picked);
        }
    }

    /// Pick out `picked` alone, in place of whatever was picked out before
    pub fn set(&mut self, picked: Picked) {
        self.0.clear();
        self.0.push(picked);
    }

    /// Pick `picked` out alongside the rest, or let go of it if it is already
    ///
    /// One gesture that builds a set and takes it apart again, so that
    /// something added by mistake is undone by doing the same thing twice
    /// rather than by starting over.
    pub fn toggle(&mut self, picked: Picked) {
        match self.0.iter().position(|one| one.same(&picked)) {
            Some(at) => {
                self.0.remove(at);
            }
            None => self.0.push(picked),
        }
    }

    /// Pick out nothing
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Let go of the system at `address`, holding everything else
    ///
    /// What descending into a system does to the selection that brought the
    /// camera there. Once it is standing inside, the ring is a ring around the
    /// view and the row names where the user already is, so the system lets go
    /// of itself. Bodies picked out inside it are left alone: they are what
    /// there is to look at now.
    pub fn deselect_system(&mut self, address: i64) {
        self.0.retain(|picked| {
            !matches!(picked, Picked::System(system) if system.address == address)
        });
    }

    /// Let go of the thing in the `index`th place, and hold the rest
    pub fn remove(&mut self, index: usize) {
        if index < self.0.len() {
            self.0.remove(index);
        }
    }

    /// The thing in the `index`th place
    pub fn get(&self, index: usize) -> Option<&Picked> {
        self.0.get(index)
    }

    /// The system in the `index`th place, where that is what stands there
    ///
    /// A whole row, for whoever can read one. A [`System`]'s fields are
    /// private to [`super`], so the bar reaches what it draws through
    /// [`Picked`] and uses this only to hand the row on to a panel.
    pub fn system(&self, index: usize) -> Option<&System> {
        match self.0.get(index)? {
            Picked::System(system) => Some(system),
            Picked::Body(_) => None,
        }
    }

    /// What the thing in the `index`th place is called
    pub fn name(&self, index: usize) -> Option<&str> {
        self.0.get(index).map(Picked::name)
    }

    /// How many are picked out
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing at all is picked out
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every system picked out, in the order it was picked
    ///
    /// The bodies are left out rather than answered for by the system holding
    /// them. What asks this is asking about places: a filter admits systems, a
    /// route runs between two of them, and a body is neither.
    pub fn systems(&self) -> impl Iterator<Item = &System> {
        self.0.iter().filter_map(|one| match one {
            Picked::System(system) => Some(system),
            Picked::Body(_) => None,
        })
    }

    /// Every system picked out, by address
    ///
    /// What a filter over the selection is built from. Addresses rather than
    /// rows, since that is all a filter tests against, and because a filter
    /// holding its own copy is what lets the selection be let go of while the
    /// filter stands.
    pub fn addresses(&self) -> Vec<i64> {
        self.systems().map(|system| system.address).collect()
    }

    /// Where the thing in the `index`th place is
    ///
    /// A [`System`]'s fields are private to [`super`], and the control that
    /// sends the camera to what is picked out is drawn with the rest of the
    /// UI, which is not. So this is what the rest of the crate can ask.
    pub fn position(&self, index: usize) -> Option<DVec3> {
        self.0.get(index).map(Picked::position)
    }
}

/// A picked out thing, once it is on the map
///
/// What everything drawn for a selection asks, so that none of them has to
/// search the map for what [`Selection`] names.
#[derive(Component)]
pub struct Selected;

/// Keep a mark on everything picked out, and the selection on whatever the
/// map last heard about each of them
///
/// The one place values and marks are reconciled, for a system and for a body
/// alike. Everything else asks [`Selected`] and never has to search the map
/// for what [`Selection`] names.
///
/// A selection outlives the entity it names: a searched system is picked out
/// before it is fetched, and one flown away from is despawned while still
/// selected. So the marks are placed by matching what a thing is called by —
/// which system, and which of its numbering — and the map is only swept for
/// the ones that have none.
///
/// Where a mark and a row do meet, the map's row is the fresher of the two.
/// The selection was taken when the system was picked out, from a search that
/// answered before anything was fetched or from a click on a row that may
/// have been fetched some time ago, and a later fetch replaces the row
/// without the selection hearing of it. So a row that has changed is copied
/// back, and what is picked out is the row the map holds rather than the one
/// it held when the user pointed at it. Nothing does this for a body: what a
/// body is was settled when it was drawn, and it does not change under one.
///
/// A body with no entity is let go of, where a system with none is kept. That
/// is the one place the two part company, and the reason is what can name
/// them: a search names a system the map has never fetched, and nothing at all
/// names a body that is not drawn, so a body still picked out after the camera
/// has left its system is a row about nowhere.
///
/// Nothing drawn is placed from the selection's own position: the marks say
/// which entity, and each ring is drawn where that entity's transform puts it.
fn follow_selection(
    mut selection: ResMut<Selection>,
    marked: Query<(Entity, Ref<System>), With<Selected>>,
    marked_bodies: Query<(Entity, &Body), With<Selected>>,
    systems: Query<(Entity, &System)>,
    bodies: Query<(Entity, &Body)>,
    mut commands: Commands,
) {
    // Which of the picked out things already wear a mark, so that the sweep
    // below looks only for the ones that do not.
    let mut settled = vec![false; selection.0.len()];

    for (entity, system) in &marked {
        let at = selection.0.iter().position(|one| {
            one.address() == system.address && one.id().is_none()
        });
        match at {
            Some(at) => {
                settled[at] = true;
                if system.is_changed() {
                    selection.0[at] = Picked::System((*system).clone());
                }
            }
            None => {
                commands.entity(entity).remove::<Selected>();
            }
        }
    }

    for (entity, body) in &marked_bodies {
        let at = selection.0.iter().position(|one| {
            one.address() == body.address && one.id() == Some(body.id)
        });
        match at {
            Some(at) => settled[at] = true,
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
        let found = selection.0.iter().position(|one| {
            one.address() == system.address && one.id().is_none()
        });
        let Some(at) = found else { continue };
        if settled[at] {
            continue;
        }
        settled[at] = true;
        commands.entity(entity).insert(Selected);
        // The row that has just arrived, rather than the one the search was
        // answered with, which is what the mark being placed here means in
        // the first place.
        selection.0[at] = Picked::System(system.clone());
    }

    for (entity, body) in &bodies {
        let found = selection.0.iter().position(|one| {
            one.address() == body.address && one.id() == Some(body.id)
        });
        let Some(at) = found else { continue };
        if settled[at] {
            continue;
        }
        settled[at] = true;
        commands.entity(entity).insert(Selected);
    }

    // Whatever is left unsettled is either a system the map has yet to fetch,
    // which is kept, or a body whose system the camera has left, which is
    // gone. Written only where there is something to let go of, so that a
    // settled selection is not marked as changed every frame.
    let dropping = settled
        .iter()
        .enumerate()
        .any(|(at, found)| !found && selection.0[at].id().is_some());
    if dropping {
        let mut at = 0;
        selection.0.retain(|one| {
            let keep = settled[at] || one.id().is_none();
            at += 1;
            keep
        });
    }
}

/// Let go of a system the filters have taken off the map
///
/// A filter excluding a system draws it faintly, and at a [`DimTo`] of zero
/// does not draw it at all. The spyglass is then free to despawn it, so what
/// is picked out is a system with nothing on the map and nothing coming: a
/// ring around empty sky, and a row naming somewhere the user can no longer
/// see or point at.
///
/// Read off the mark rather than asked of the filters again. A span's near
/// edge moves with the clock, so asking here would answer a moment later than
/// [`crate::systems::filter::mark`] last cut, and a system would be let go of
/// seconds before the star it named stopped being drawn. One decision, made
/// where the mark is made, and both the sky and the selection follow it.
///
/// Only at zero. Above it the system is still drawn, faintly, and a selection
/// on one the filters exclude is a selection the user can see and let go of
/// for themselves.
///
/// Systems alone. A body is let go of by [`follow_selection`] when it stops
/// being drawn, which is what leaving its system does.
fn clear_not_drawn(
    mut selection: ResMut<Selection>,
    dim: Res<DimTo>,
    excluded: Query<&System, (With<Selected>, With<Filtered>)>,
) {
    if dim.0 != 0. {
        return;
    }

    // Asked of what is both picked out and excluded, which is a handful of a
    // handful, and written only where there is something to let go of: a
    // settled selection marked as changed every frame is a query on the wire
    // every frame.
    let dropping: Vec<i64> =
        excluded.iter().map(|system| system.address).collect();
    if dropping.is_empty() {
        return;
    }

    selection.0.retain(|one| match one {
        Picked::System(system) => !dropping.contains(&system.address),
        Picked::Body(_) => true,
    });
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

    // All of it, bodies as well as systems. The gesture means let go, and
    // letting go of some of what is held is not something a click on empty
    // sky could say which of.
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
///
/// A stop a route reaches is left to [`super::pointing::ring`] while the map
/// is holding a system, that being where every mark for a stop is drawn then
/// and the only place they all land together.
///
/// And it goes out with the shell, as the camera comes inside the system it
/// stands around. Everything the map draws for a system as a whole is a mark
/// standing in for something too small to see, and a ring left around a system
/// the camera is standing inside is a ring around the view.
fn ring(
    mut gizmos: Gizmos,
    camera: Query<(&OrbitCamera, &Camera)>,
    spyglass: Res<Spyglass>,
    holding: Res<HeldSystem>,
    selected: Query<
        (
            &System,
            &Strength,
            &GlobalTransform,
            &Indicator,
            Has<Filtered>,
            Has<crate::systems::route::Hop>,
        ),
        With<Selected>,
    >,
    // Whatever inside a system is picked out, which carries neither a filter
    // nor a galactic position of its own.
    inside: Query<(&GlobalTransform, &Indicator), (With<Body>, With<Selected>)>,
    eye_at: Query<&GlobalTransform, With<OrbitCamera>>,
    dim: Res<DimTo>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    if let Ok(eye) = eye_at.single() {
        for (at, indicator) in &inside {
            let offset = (at.translation() - eye.translation()).as_dvec3();
            let radius = super::pointing::drawn_radius_of(
                orbit,
                cot_half_fov,
                viewport,
                offset,
                indicator.0,
            );

            gizmos
                .circle(
                    Isometry3d::new(at.translation(), orbit.rotation),
                    radius,
                    SELECTION,
                )
                .resolution(RING_POINTS);
        }
    }

    for (system, mark, at, indicator, filtered, hop) in &selected {
        // A stop a route reaches is ringed by [`super::pointing::ring`],
        // in this same color, while the map is holding a system. Everything
        // drawn for a stop then is drawn where the camera can see it rather
        // than where the stop is, and a ring drawn here would be out at the
        // stop's true distance with the rest of the mark a jump nearer.
        if hop && holding.of().is_some() {
            continue;
        }

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
        let standing = mark.0;
        if standing <= 0. {
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

        gizmos
            .circle(
                Isometry3d::new(at.translation(), orbit.rotation),
                radius,
                going(ringed(&dim, filtered), standing),
            )
            .resolution(RING_POINTS);
    }
}

/// `color` with `standing` of it left
///
/// The alpha carries it, as it carries a name being dimmed: a mark faded by
/// darkening goes black against the sky and reads as a hole rather than as
/// something on its way out.
pub(super) fn going(color: Srgba, standing: f32) -> Srgba {
    Srgba { alpha: color.alpha * standing, ..color }
}

/// What color a selected system's ring is drawn in
///
/// It dims with the star it is drawn around, so that a selection the filters
/// exclude does not read as one they have let go of. Drawn at nothing where
/// the star is, which is a ring around a system that is not there: at that
/// point the selection itself goes, by [`clear_not_drawn`].
fn ringed(dim: &DimTo, filtered: bool) -> Srgba {
    dim.as_drawn(SELECTION, filtered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::pointing::PRIMARY;
    use crate::systems::tests::system;
    use crate::ui::PressOwner;

    /// A system picked out, which is what most of these are about
    fn picked(address: i64) -> Picked {
        Picked::System(system(address))
    }

    /// A world with nothing in it but the selection and the mark
    fn map() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Selection>();
        app.add_systems(Update, follow_selection);
        app
    }

    /// A body of the system at `address`, the `id`th of its numbering
    fn body(address: i64, id: i16) -> Body {
        Body {
            address,
            name: format!("Test {address} {id}"),
            id,
            class: String::new(),
            radius: 1e6,
            ancestors: 0,
            primary: false,
            star: false,
        }
    }

    /// That body, picked out
    fn picked_body(address: i64, id: i16) -> Picked {
        Picked::Body(PickedBody::new(address, id, "", DVec3::ZERO))
    }

    /// The mark goes to a body as it goes to a system
    ///
    /// One bridge for both, so that everything drawn for a selection asks
    /// [`Selected`] and never has to know which kind it found.
    #[test]
    fn the_mark_lands_on_a_picked_body() {
        let mut app = map();
        let three = app.world_mut().spawn(body(1, 3)).id();
        let four = app.world_mut().spawn(body(1, 4)).id();

        app.world_mut().resource_mut::<Selection>().set(picked_body(1, 3));
        app.update();

        assert!(app.world().entity(three).contains::<Selected>());
        assert!(!app.world().entity(four).contains::<Selected>());
    }

    /// Body three of one system is not body three of the next
    ///
    /// Which is why a picked body carries the system it is in. One system's
    /// contents are drawn at a time, so an id on its own is unique among what
    /// is on the map and names something else the moment the camera moves on.
    #[test]
    fn a_body_of_another_system_is_another_body() {
        let mut app = map();
        let elsewhere = app.world_mut().spawn(body(2, 3)).id();

        app.world_mut().resource_mut::<Selection>().set(picked_body(1, 3));
        app.update();

        assert!(!app.world().entity(elsewhere).contains::<Selected>());
    }

    /// A body is let go of once it is no longer drawn
    ///
    /// Where a system is kept: a search names a system the map has never
    /// fetched, and nothing at all names a body that is not drawn. A body
    /// still picked out after the camera has left its system is a row about
    /// nowhere, and one that would come back the next time some system
    /// numbered a body the same way.
    #[test]
    fn a_body_that_is_no_longer_drawn_is_let_go_of() {
        let mut app = map();
        let three = app.world_mut().spawn(body(1, 3)).id();

        let mut selection = app.world_mut().resource_mut::<Selection>();
        selection.set(picked(1));
        selection.toggle(picked_body(1, 3));
        app.update();
        assert_eq!(app.world().resource::<Selection>().len(), 2);

        app.world_mut().entity_mut(three).despawn();
        app.update();

        let selection = app.world().resource::<Selection>();
        assert_eq!(selection.len(), 1, "the body outlived what it named");
        assert_eq!(selection.name(0), Some("Test 1"));
    }

    /// And a system with nothing on the map is kept
    #[test]
    fn a_system_not_on_the_map_is_still_picked_out() {
        let mut app = map();

        app.world_mut().resource_mut::<Selection>().set(picked(1));
        app.update();
        app.update();

        assert_eq!(app.world().resource::<Selection>().len(), 1);
    }

    /// Descending into a system lets go of that system
    ///
    /// The selection that flew the camera in circled a star out in the sky.
    /// Standing inside it, that ring would circle the whole view, so the
    /// system lets go of itself as the camera arrives.
    #[test]
    fn descending_into_a_system_lets_go_of_it() {
        let mut selection = Selection::default();
        selection.set(picked(1));

        selection.deselect_system(1);

        assert!(selection.is_empty());
    }

    /// And leaves the rest of the selection picked out
    ///
    /// Only the system the camera descended into is let go of. Another
    /// system gathered alongside it, and any body picked out inside the one
    /// being entered, are what there is left to work with.
    #[test]
    fn descending_holds_the_rest_of_the_selection() {
        let mut selection = Selection::default();
        selection.set(picked(1));
        selection.toggle(picked(2));
        selection.toggle(picked_body(1, 3));

        selection.deselect_system(1);

        assert_eq!(selection.len(), 2);
        assert!(selection.systems().any(|system| system.address == 2));
        assert_eq!(selection.get(1).map(Picked::id), Some(Some(3)));
    }

    /// A world holding a selection and the click that may let go of it
    ///
    /// Nothing is pointed at and no pointer has travelled, so what the click
    /// lands on is empty sky.
    fn clicked_on() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<PressOwner>();

        let mut selection = Selection::default();
        selection.set(picked(1));
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
        world.resource_mut::<PressOwner>().settle(&buttons, wanted);
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

        selection.pick(picked(1), false);
        selection.pick(picked(2), true);
        assert_eq!(selection.addresses(), vec![1, 2]);

        selection.pick(picked(2), true);
        assert_eq!(selection.addresses(), vec![1]);

        selection.pick(picked(3), false);
        assert_eq!(selection.addresses(), vec![3]);
    }

    /// Picking one out plainly lets go of whatever was held
    #[test]
    fn picking_one_out_replaces_the_rest() {
        let mut selection = Selection::default();
        selection.set(picked(1));
        selection.toggle(picked(2));

        selection.set(picked(3));

        assert_eq!(selection.addresses(), vec![3]);
    }

    /// Gathering keeps what is held, in the order it was picked
    #[test]
    fn gathering_holds_them_in_the_order_they_were_picked() {
        let mut selection = Selection::default();
        selection.set(picked(2));
        selection.toggle(picked(1));
        selection.toggle(picked(3));

        assert_eq!(selection.addresses(), vec![2, 1, 3]);
    }

    /// And gathering one already held lets go of it
    ///
    /// One gesture that builds a set and takes it apart, so a system added by
    /// mistake is undone by doing the same thing again.
    #[test]
    fn gathering_one_already_held_lets_go_of_it() {
        let mut selection = Selection::default();
        selection.set(picked(1));
        selection.toggle(picked(2));

        selection.toggle(picked(1));

        assert_eq!(selection.addresses(), vec![2]);
    }

    /// A row's close mark lets go of that one and holds the rest
    #[test]
    fn letting_go_of_one_holds_the_rest() {
        let mut selection = Selection::default();
        selection.set(picked(1));
        selection.toggle(picked(2));
        selection.toggle(picked(3));

        selection.remove(1);

        assert_eq!(selection.addresses(), vec![1, 3]);
    }

    /// The mark goes to the system the selection names
    #[test]
    fn the_mark_lands_on_what_is_selected() {
        let mut app = map();
        let one = app.world_mut().spawn(system(1)).id();
        let two = app.world_mut().spawn(system(2)).id();

        app.world_mut().resource_mut::<Selection>().set(picked(2));
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

        app.world_mut().resource_mut::<Selection>().set(picked(1));
        app.update();
        app.world_mut().resource_mut::<Selection>().set(picked(2));
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
        selection.set(picked(1));
        selection.toggle(picked(3));
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
        selection.set(picked(1));
        selection.toggle(picked(2));
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
        selection.set(picked(1));
        selection.toggle(picked(2));
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

        app.world_mut().resource_mut::<Selection>().set(picked(1));
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

    /// And goes out with it
    ///
    /// At an opacity of nothing the star is not drawn and the map is free to
    /// despawn it, so a ring left standing would be a mark around empty sky.
    /// The selection goes at that point, by [`clear_not_drawn`],
    /// and this is what is drawn in the frame before it does.
    #[test]
    fn a_ring_goes_out_with_its_star() {
        assert_eq!(ringed(&DimTo(0.), true).alpha, 0.);
    }

    /// A world holding the selection and how faintly to draw what is excluded
    fn filtered_map(dim: f32) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Selection>();
        app.insert_resource(DimTo(dim));
        app.add_systems(Update, clear_not_drawn);
        app
    }

    /// A system picked out and marked as excluded, as the map would have it
    fn excluded(app: &mut App, address: i64) {
        app.world_mut().spawn((system(address), Selected, Filtered));
        app.world_mut().resource_mut::<Selection>().set(picked(address));
    }

    /// A system the filters exclude is let go of once it stops being drawn
    ///
    /// At an opacity of nothing it is not on the map and the spyglass may
    /// despawn it, so what is picked out is somewhere the user can no longer
    /// see, point at, or fly to.
    #[test]
    fn a_system_the_filters_exclude_is_let_go_of_at_nothing() {
        let mut app = filtered_map(0.);
        excluded(&mut app, 1);

        app.update();

        assert!(
            app.world().resource::<Selection>().is_empty(),
            "a ring was left around a system that is not drawn"
        );
    }

    /// And is kept while it is still drawn, however faintly
    ///
    /// Above nothing the user can see what they picked out and let go of it
    /// themselves, which is theirs to decide rather than the filters'.
    #[test]
    fn a_system_the_filters_exclude_is_kept_while_it_is_drawn() {
        let mut app = filtered_map(0.15);
        excluded(&mut app, 1);

        app.update();

        assert_eq!(app.world().resource::<Selection>().len(), 1);
    }

    /// A system the filters admit is kept at any opacity
    #[test]
    fn a_system_the_filters_admit_is_kept_at_nothing() {
        let mut app = filtered_map(0.);
        app.world_mut().spawn((system(1), Selected));
        app.world_mut().resource_mut::<Selection>().set(picked(1));

        app.update();

        assert_eq!(app.world().resource::<Selection>().len(), 1);
    }

    /// A selection with nothing to let go of is not written to
    ///
    /// The same thing [`a_settled_selection_holds_still`] holds for the mark,
    /// and it matters more here: this is asked every frame the filters are
    /// drawn at nothing, which is for as long as the user leaves them there.
    #[test]
    fn a_selection_with_nothing_to_drop_is_left_alone() {
        let mut app = filtered_map(0.);
        app.world_mut().spawn((system(1), Selected));
        app.world_mut().resource_mut::<Selection>().set(picked(1));
        app.update();
        app.update();

        let changed_at =
            app.world().resource_ref::<Selection>().last_changed().get();
        app.update();
        app.update();

        assert_eq!(
            app.world().resource_ref::<Selection>().last_changed().get(),
            changed_at,
            "the selection was written to with nothing to let go of"
        );
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

        app.world_mut().resource_mut::<Selection>().set(picked(1));
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

        app.world_mut().resource_mut::<Selection>().set(picked(1));
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

        app.world_mut().resource_mut::<Selection>().set(picked(1));
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
        selection.set(Picked::System(sol));

        assert_eq!(selection.position(0), Some(DVec3::new(1., -2., 3.)));
    }

    /// Clearing the selection takes the mark with it
    #[test]
    fn the_mark_goes_when_the_selection_does() {
        let mut app = map();
        let one = app.world_mut().spawn(system(1)).id();

        app.world_mut().resource_mut::<Selection>().set(picked(1));
        app.update();
        app.world_mut().resource_mut::<Selection>().clear();
        app.update();

        assert!(!app.world().entity(one).contains::<Selected>());
    }
}
