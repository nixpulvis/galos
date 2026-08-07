use crate::camera::{FRAMING_MARGIN, MoveCamera};
use crate::schedule::MapSet;
use crate::systems::Spyglass;
use crate::systems::System;
use crate::systems::filter::{Filter, Filters};
use bevy::asset::RenderAssetUsages;
use bevy::math::DVec3;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;

use super::system_to_vec;

pub fn plugin(app: &mut App) {
    app.add_message::<Plotted>();
    app.init_resource::<Selected>();
    // After the fetch it answers has been drawn, and before the camera is
    // pointed, since where it asks the camera to go is what `move_camera`
    // then works out.
    app.add_systems(
        Update,
        (plotted, follow_filters)
            .chain()
            .in_set(MapSet::Populate)
            .after(super::spawn::spawn),
    );
    // Once the lines and the filters have settled, so what is drawn faintly
    // this frame answers what is being asked this frame.
    app.add_systems(
        Update,
        emphasise.in_set(MapSet::Present).after(follow_filters),
    );
    // Marked while the stars are being populated, so that what reads the mark
    // in `Present` -- what is drawn, and what is ringed -- reads this frame's
    // answer rather than last frame's.
    app.add_systems(
        Update,
        hops.in_set(MapSet::Populate).after(follow_filters),
    );
}

/// What the ring around a stop is drawn in
///
/// The white a route's line is drawn in, and at full strength where the line
/// is faint: the line crosses systems that are meant to go on being seen, and
/// this is a mark around one of them.
pub const HOP: Srgba = Srgba::new(1., 1., 1., 0.9);

/// A stop the route reaches from the system the camera is standing in
///
/// The one behind and the one ahead. A route is drawn as a line between
/// systems, and that line is gone by the time the camera is inside one of
/// them, so what is left to say where the route goes is the two systems it
/// goes to and from.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hop {
    /// Where the route came from
    Last,
    /// Where it goes next
    Next,
}

/// Which systems a route reaches from `here`, behind and ahead
///
/// Nothing for either end of the route, which reaches only one way, and
/// nothing at all where the camera is not standing in a system the route runs
/// through: a route passes near far more systems than it stops at, and being
/// beside one is not being on it.
fn reaching(
    route: Option<&Filter>,
    here: Option<i64>,
) -> (Option<i64>, Option<i64>) {
    let (Some(Filter::Route { systems, .. }), Some(here)) = (route, here)
    else {
        return (None, None);
    };
    let Some(at) = systems.iter().position(|address| *address == here) else {
        return (None, None);
    };

    (
        at.checked_sub(1).and_then(|before| systems.get(before)).copied(),
        systems.get(at + 1).copied(),
    )
}

/// Keep the mark on whichever two systems the route reaches from here
///
/// Written only where it changed. This runs over every star every frame, and
/// inserting a component marks the star changed whether or not the value
/// moved, which drags its name and its material along behind it.
fn hops(
    filters: Res<Filters>,
    selected: Res<Selected>,
    contents: Res<crate::systems::bodies::Contents>,
    systems: Query<(Entity, &System, Option<&Hop>)>,
    mut commands: Commands,
) {
    let (last, next) = reaching(active(&filters, &selected.0), contents.of());

    for (entity, system, held) in &systems {
        let wanted = if last.is_some() && last == Some(system.address) {
            Some(Hop::Last)
        } else if next.is_some() && next == Some(system.address) {
            Some(Hop::Next)
        } else {
            None
        };

        match (held, wanted) {
            (Some(held), Some(wanted)) if *held == wanted => {}
            (None, None) => {}
            (_, Some(wanted)) => {
                commands.entity(entity).insert(wanted);
            }
            (Some(_), None) => {
                commands.entity(entity).remove::<Hop>();
            }
        }
    }
}

/// A drawn route, and which route it is
///
/// Several stand at once, so a line has to say which of them it is: the row in
/// the bar is what lets go of it, and a line that could not be told from the
/// next would leave the wrong one drawn.
///
/// The filter itself rather than a name of its own. It is what the row holds
/// and what the panel is keyed on, so a line, a row and a window about one
/// route are one value in three places rather than three things to keep in
/// step.
#[derive(Component)]
pub struct Route(pub Filter);

/// A route that has landed and been drawn
///
/// Written where the fetch is collected, since that is the one place the
/// systems it runs through are in hand, and answered here so that what a
/// route does to the map is in one place rather than threaded through the
/// system that draws stars.
#[derive(Message, Debug)]
pub struct Plotted {
    /// The two ends, as the database spells them
    pub label: String,
    /// Every system it runs through, by address, in the order travelled
    pub systems: Vec<i64>,
    /// The middle of what it spans
    pub middle: DVec3,
    /// How far it reaches from there, in light years
    pub extent: f32,
    /// How far the ship it was plotted for reaches in one jump, in light years
    ///
    /// Carried along rather than worked out from the legs. The longest jump a
    /// route happens to take is not what was asked for: a route plotted for a
    /// ship reaching 20 may never need more than 12, and it is what the user
    /// asked that tells two plots between the same ends apart.
    pub range: String,
}

impl Plotted {
    /// The filter this route asks for, and the line's own name for itself
    ///
    /// Built in one place and read in two: the row in the bar is this filter,
    /// and so is the mark the drawn line carries. They have to be the same
    /// value or closing the row would leave a line nothing can find.
    pub fn filter(&self) -> Filter {
        Filter::Route {
            label: self.label.clone(),
            systems: self.systems.clone(),
            range: self.range.clone(),
        }
    }
}

/// Show a route that has just been plotted
///
/// Three things at once, all of them the same thought: look at the whole of
/// it, reach far enough to hold the whole of it, and pick the whole of it
/// out from everything else.
///
/// The spyglass is set rather than left alone because a route is usually
/// longer than whatever the user was looking at when they asked for it, and a
/// route drawn as a line running out through the edge of an unchanged
/// spyglass is a route with no systems on it.
fn plotted(
    mut plotted: MessageReader<Plotted>,
    mut camera: MessageWriter<MoveCamera>,
    mut spyglass: ResMut<Spyglass>,
    mut filters: ResMut<Filters>,
    mut selected: ResMut<Selected>,
) {
    for route in plotted.read() {
        // A route just asked for is the one being looked at, so whichever was
        // picked out before it stands down. Cleared rather than set to this
        // one, the last route held being what [`active`] falls back to.
        if selected.0.is_some() {
            selected.0 = None;
        }

        camera.write(MoveCamera {
            position: Some(route.middle),
            framing: Some(route.extent),
        });

        // Measured from the middle, which is where the camera is going, so
        // what the spyglass holds is what the camera is about to see. The
        // same room around it that the camera is stood back to leave, since a
        // reach set to the route's own extent puts the two ends exactly on
        // the rim of it: the extent is the distance to the furthest of them,
        // and whether that counts as reaching them comes down to which way an
        // `f32` rounded.
        //
        // Held inside what the map will reach unasked. Everything the
        // spyglass takes in is fetched and spawned, and a route long enough
        // would otherwise set a reach nobody asked the size of.
        spyglass.radius = (route.extent * FRAMING_MARGIN)
            .clamp(Spyglass::OPENING, Spyglass::UNASKED);

        // Beside whatever is already plotted rather than in place of it. Each
        // route keeps its own line and its own row, so plotting a second is
        // asking to see both. The same route asked for twice is deduped by
        // `add`, there being nothing to see twice.
        filters.add(route.filter());
    }
}

/// Keep each line answering to the row that names it
///
/// The line and the filter row are two halves of one answer: the row says
/// which route is being shown and the line shows it. So the row's two gestures
/// reach the line, and each means what it means everywhere else in the bar.
///
/// Closing the row takes the route away for good, and the line goes with it. A
/// line left drawn afterwards is an answer to a question nobody is asking, and
/// one with nothing left on screen to say what it is.
///
/// Turning the row off hides the line and keeps it. That is what a filter
/// turned off is: something to come back to. The route it names is still the
/// route they plotted, so the line waits rather than being worked out again,
/// and the row is still there to turn back on.
///
/// Each line is weighed against the filters by which route it is, rather than
/// every line answering to whether any route at all is held. Several stand at
/// once and they are turned off one at a time.
fn follow_filters(
    filters: Res<Filters>,
    mut lines: Query<(Entity, &Route, &mut Visibility)>,
    mut commands: Commands,
) {
    for (entity, line, mut visibility) in &mut lines {
        match asked(&filters, &line.0) {
            None => commands.entity(entity).despawn(),
            Some(enabled) => {
                visibility.set_if_neq(if enabled {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                });
            }
        }
    }
}

/// Whether `route` is among the filters, and whether it is turned on
///
/// Nothing at all where no row names it, which is a route that was closed.
/// That is the difference the line answers with two different things: a row
/// turned off still names its route.
fn asked(filters: &Filters, route: &Filter) -> Option<bool> {
    filters
        .iter()
        .find(|active| active.filter == *route)
        .map(|active| active.enabled)
}

/// The route the user picked out of the ones on screen, if they picked one
///
/// Written when a route's panel is pressed, which is how the user says which
/// of several they are working with. Cleared by plotting, a route just asked
/// for being the one they are looking at.
///
/// An override rather than the answer itself. What it stands in front of is
/// the last route plotted, and [`active`] puts the two together.
#[derive(Resource, Default)]
pub struct Selected(pub Option<Filter>);

/// Which route is the one being worked with
///
/// The one whose panel was last pressed, and failing that the last one
/// plotted, which is the last route filter held: they are added in the order
/// they land, so the end of the list is the newest.
///
/// `selected` is weighed against the filters rather than trusted. A route
/// picked out and then closed would otherwise go on being the active one with
/// nothing on screen standing for it, and nothing left to hand the emphasis
/// back to.
///
/// Only among the routes being shown. A row turned off takes its line off the
/// map, and a route nobody can see cannot be the one in front: the rest would
/// be held back for it and the map would have every route drawn faintly and
/// none of them picked out.
///
/// Nothing where no route is being shown at all, there being nothing to be
/// active.
fn active<'a>(
    filters: &'a Filters,
    selected: &'a Option<Filter>,
) -> Option<&'a Filter> {
    let held = || {
        filters
            .iter()
            .filter(|active| active.enabled)
            .map(|active| &active.filter)
            .filter(|filter| matches!(filter, Filter::Route { .. }))
    };

    selected
        .as_ref()
        .filter(|picked| held().any(|filter| filter == *picked))
        // The last route held, which is the last one plotted: they are added
        // in the order they land.
        .or_else(|| held().last())
}

/// How faint a route that is not the active one is drawn
///
/// A fraction of what the active one is drawn at. Faint enough that the one
/// being worked with reads as the one in front, and not so faint that the
/// others stop being routes on the map: they are there to be compared with,
/// which is the whole reason for holding more than one.
const BEHIND: f32 = 0.4;

/// What a route line is drawn at, given whether it is the active one
pub fn strength(is_active: bool) -> f32 {
    if is_active { 1. } else { BEHIND }
}

/// Draw the active route at full strength and hold the rest behind it
///
/// The color is left alone and the alpha carries it, as it does for a system
/// the filters exclude, so a route standing back reads as further off rather
/// than as something else.
///
/// Each line was spawned with a material of its own, so this writes to one
/// route's color without touching another's.
///
/// The lines go with the marks. A route is drawn between systems at the scale
/// the sky is read at, and once the camera has descended into one of them
/// there is no sky left for it to be read against: what is drawn there is one
/// system at its own size, and a line laid over it is a light year wide and
/// runs out through the walls. So it fades on exactly the band the mark
/// standing for that system fades on, and the two go together.
fn emphasise(
    filters: Res<Filters>,
    selected: Res<Selected>,
    seen_as: Res<crate::systems::bodies::spawn::Apparent>,
    lines: Query<(&Route, &MeshMaterial3d<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let active = active(&filters, &selected.0);
    let standing = seen_as.held();

    for (line, material) in &lines {
        let Some(mut material) = materials.get_mut(&material.0) else {
            continue;
        };
        let wanted =
            spawn::line_color(strength(Some(&line.0) == active) * standing);
        // Written only where it changed. Touching a material marks the asset
        // changed, which re-uploads it, and this runs every frame.
        if material.base_color != wanted {
            material.base_color = wanted;
        }
    }
}

pub mod fetch;
pub mod spawn;

/// A list of points that will have a line drawn between each consecutive points
#[derive(Debug, Clone)]
pub(crate) struct LineStrip {
    pub(crate) points: Vec<Vec3>,
}

impl From<LineStrip> for Mesh {
    fn from(line: LineStrip) -> Self {
        Mesh::new(
            // This tells wgpu that the positions are a list of points
            // where a line will be drawn between each consecutive point
            PrimitiveTopology::LineStrip,
            RenderAssetUsages::RENDER_WORLD,
        )
        // Add the point positions as an attribute
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, line.points)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A route running through three systems, in the order travelled
    fn route(systems: Vec<i64>) -> Filter {
        Filter::Route {
            label: "a to c".to_owned(),
            systems,
            range: "20".to_owned(),
        }
    }

    /// The stop behind and the stop ahead, from the middle of a route
    #[test]
    fn a_route_reaches_both_ways_from_where_it_stands() {
        let route = route(vec![1, 2, 3]);

        assert_eq!(reaching(Some(&route), Some(2)), (Some(1), Some(3)));
    }

    /// Either end of a route reaches only the one way
    #[test]
    fn the_ends_of_a_route_reach_one_way() {
        let route = route(vec![1, 2, 3]);

        assert_eq!(reaching(Some(&route), Some(1)), (None, Some(2)));
        assert_eq!(reaching(Some(&route), Some(3)), (Some(2), None));
    }

    /// Standing beside a route is not standing on it
    ///
    /// A route passes near far more systems than it stops at, and a mark
    /// saying where to go next means nothing from a system it never visits.
    #[test]
    fn a_system_the_route_misses_reaches_nowhere() {
        let route = route(vec![1, 2, 3]);

        assert_eq!(reaching(Some(&route), Some(9)), (None, None));
        assert_eq!(reaching(Some(&route), None), (None, None));
        assert_eq!(reaching(None, Some(2)), (None, None));
    }

    /// A route filter over the systems at `addresses`
    fn asking(addresses: &[i64]) -> Filter {
        Filter::Route {
            label: "A -> B".to_owned(),
            systems: addresses.to_vec(),
            range: "10".to_owned(),
        }
    }

    /// A world holding nothing but the filters and a line for `drawing`
    fn map(filters: Filters, drawing: Filter) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(filters);
        app.add_systems(Update, follow_filters);
        let line = line_for(&mut app, drawing);
        (app, line)
    }

    /// A drawn line for `route`, as `spawn_route` leaves one
    fn line_for(app: &mut App, route: Filter) -> Entity {
        app.world_mut().spawn((Route(route), Visibility::default())).id()
    }

    /// Whether the line is still held, shown or hidden
    fn drawn(app: &App, line: Entity) -> bool {
        app.world().get_entity(line).is_ok()
    }

    /// Whether the line is on screen
    fn shown(app: &App, line: Entity) -> bool {
        app.world().get::<Visibility>(line) == Some(&Visibility::Visible)
    }

    /// The spyglass a route centered on `middle` and reaching `extent` leaves
    fn spyglass_for(middle: DVec3, extent: f32) -> Spyglass {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<Plotted>();
        app.add_message::<MoveCamera>();
        app.insert_resource(Spyglass {
            fetch: true,
            radius: Spyglass::OPENING,
            clear: true,
            lock_camera: false,
            follow_camera: false,
        });
        app.init_resource::<Filters>();
        app.init_resource::<Selected>();
        app.add_systems(Update, plotted);

        app.world_mut().write_message(Plotted {
            label: "A -> B".to_owned(),
            systems: vec![1, 2],
            middle,
            extent,
            range: "10".to_owned(),
        });
        app.update();

        let held = app.world().resource::<Spyglass>();
        Spyglass {
            fetch: held.fetch,
            radius: held.radius,
            clear: held.clear,
            lock_camera: held.lock_camera,
            follow_camera: held.follow_camera,
        }
    }

    /// What reach a route of `extent` light years pulls the spyglass out to
    fn reach_for(extent: f32) -> f32 {
        spyglass_for(DVec3::ZERO, extent).radius
    }

    /// A route reaches past its own ends rather than up to them
    #[test]
    fn a_route_reaches_past_its_own_ends() {
        assert!(reach_for(60.) > 60., "{}", reach_for(60.));
    }

    /// So the systems at those ends are drawn
    ///
    /// The extent is the distance from the middle to the furthest of them,
    /// worked out in `f64` and kept as an `f32`. A reach set to exactly that
    /// puts those systems on its rim, where whether they are drawn comes down
    /// to which way the one cast rounded. These coordinates are a case where
    /// it rounds down, so the ends fall outside a reach of their own length.
    #[test]
    fn the_systems_at_a_route_s_ends_are_within_reach() {
        let places = [
            DVec3::ZERO,
            DVec3::new(26.03, 8.676666666666668, 3.718571428571429),
        ];
        let (middle, extent) = spawn::framing(&places).unwrap();
        assert!(
            middle.distance(places[1]) > extent as f64,
            "these coordinates no longer round the way the test is about",
        );

        let spyglass = spyglass_for(middle, extent);

        for place in places {
            assert!(spyglass.reaches(middle, place), "{place} is out of reach");
        }
    }

    /// A short one still leaves the map something to see around it
    #[test]
    fn a_short_route_leaves_room_around_it() {
        assert_eq!(reach_for(2.), Spyglass::OPENING);
    }

    /// A long one is held to what the map will reach unasked
    ///
    /// Everything the spyglass takes in is fetched and spawned, so a reach
    /// set from the length of whatever was plotted is a query nobody asked
    /// the size of.
    #[test]
    fn a_long_route_does_not_reach_as_far_as_it_likes() {
        assert_eq!(reach_for(5000.), Spyglass::UNASKED);
    }

    /// The line stays while a route filter names it
    #[test]
    fn a_route_keeps_its_line() {
        let mut filters = Filters::default();
        filters.add(asking(&[1, 2]));
        let (mut app, line) = map(filters, asking(&[1, 2]));

        app.update();

        assert!(drawn(&app, line));
    }

    /// And goes when that filter is dropped
    #[test]
    fn dropping_a_route_takes_its_line() {
        let mut filters = Filters::default();
        filters.add(asking(&[1, 2]));
        let (mut app, line) = map(filters, asking(&[1, 2]));
        app.update();

        app.world_mut().resource_mut::<Filters>().remove(0);
        app.update();

        assert!(!drawn(&app, line));
    }

    /// Turning a route's row off takes its line off the map
    ///
    /// The row is the control that says whether that route is being shown, as
    /// it is for every other filter, so it reaches the line the row is about.
    #[test]
    fn a_route_turned_off_is_taken_off_the_map() {
        let mut filters = Filters::default();
        filters.add(asking(&[1, 2]));
        filters.toggle(0);
        let (mut app, line) = map(filters, asking(&[1, 2]));

        app.update();

        assert!(!shown(&app, line));
    }

    /// And keeps it, so turning the row back on draws it again
    ///
    /// A filter turned off is one the user means to come back to. The route
    /// it names is still the route they plotted, so the line waits rather than
    /// having to be worked out a second time.
    #[test]
    fn a_route_turned_off_and_back_on_is_drawn_again() {
        let mut filters = Filters::default();
        filters.add(asking(&[1, 2]));
        filters.toggle(0);
        let (mut app, line) = map(filters, asking(&[1, 2]));
        app.update();
        assert!(drawn(&app, line), "the line was not kept to come back to");

        app.world_mut().resource_mut::<Filters>().toggle(0);
        app.update();

        assert!(shown(&app, line));
    }

    /// Filters of another kind say nothing about a route's line
    #[test]
    fn a_faction_does_not_keep_a_line() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 7, name: "Some Lot".to_owned() });
        let (mut app, line) = map(filters, asking(&[1, 2]));

        app.update();

        assert!(!drawn(&app, line));
    }

    /// The filters holding each of `routes`, in that order
    fn holding(routes: &[Filter]) -> Filters {
        let mut filters = Filters::default();
        for route in routes {
            filters.add(route.clone());
        }
        filters
    }

    /// With nothing plotted there is no active route
    #[test]
    fn nothing_plotted_is_nothing_to_put_forward() {
        assert_eq!(active(&Filters::default(), &None), None);
    }

    /// The last route plotted is the active one
    ///
    /// They are added in the order they land, so the end of the list is the
    /// newest, and a route just asked for is the one being looked at.
    #[test]
    fn the_last_route_plotted_is_the_active_one() {
        let (first, second) = (asking(&[1, 2]), asking(&[8, 9]));
        let filters = holding(&[first, second.clone()]);

        assert_eq!(active(&filters, &None), Some(&second));
    }

    /// Picking one out puts it in front of the last plotted
    ///
    /// Which is what pressing a route's panel says: this is the one I am
    /// working with, whichever landed most recently.
    #[test]
    fn a_route_picked_out_stands_in_front_of_the_last() {
        let (first, second) = (asking(&[1, 2]), asking(&[8, 9]));
        let filters = holding(&[first.clone(), second]);

        assert_eq!(active(&filters, &Some(first.clone())), Some(&first));
    }

    /// One picked out and then closed hands the emphasis back
    ///
    /// Weighed against the filters rather than trusted, or a route let go of
    /// would go on being the active one with nothing on screen standing for
    /// it and no line drawn in front.
    #[test]
    fn a_route_picked_out_and_closed_falls_back_to_the_last() {
        let (closed, held) = (asking(&[1, 2]), asking(&[8, 9]));
        let filters = holding(std::slice::from_ref(&held));

        assert_eq!(active(&filters, &Some(closed)), Some(&held));
    }

    /// A route turned off is not the one put in front
    ///
    /// Its line is off the map, so holding the rest back for it would leave
    /// every route drawn faintly and none of them picked out.
    #[test]
    fn a_route_turned_off_is_not_the_active_one() {
        let (older, newest) = (asking(&[1, 2]), asking(&[8, 9]));
        let mut filters = holding(&[older.clone(), newest]);
        // The last plotted, which is the one it would otherwise fall to.
        filters.toggle(1);

        assert_eq!(active(&filters, &None), Some(&older));
    }

    /// Nor when it was the one picked out
    #[test]
    fn a_route_picked_out_and_turned_off_hands_it_back() {
        let (held, hidden) = (asking(&[1, 2]), asking(&[8, 9]));
        let mut filters = holding(&[held.clone(), hidden.clone()]);
        filters.toggle(1);

        assert_eq!(active(&filters, &Some(hidden)), Some(&held));
    }

    /// With every route turned off there is none in front
    #[test]
    fn every_route_turned_off_leaves_none_active() {
        let mut filters = holding(&[asking(&[1, 2]), asking(&[8, 9])]);
        filters.toggle_all(&[0, 1]);

        assert_eq!(active(&filters, &None), None);
    }

    /// A faction is never the active route
    ///
    /// The filters hold every kind together, and only a route has a line to
    /// put in front of the others.
    #[test]
    fn only_a_route_is_ever_active() {
        let mut filters = Filters::default();
        filters.add(asking(&[1, 2]));
        filters.add(Filter::Faction { id: 7, name: "Some Lot".to_owned() });

        assert_eq!(active(&filters, &None), Some(&asking(&[1, 2])));
    }

    /// Plotting takes back whatever was picked out
    ///
    /// A route just asked for is the one the user is looking at, so the one
    /// they had picked out stands down and the fall back does the rest.
    #[test]
    fn plotting_takes_back_what_was_picked_out() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<Plotted>();
        app.add_message::<MoveCamera>();
        app.init_resource::<Filters>();
        app.insert_resource(Spyglass {
            fetch: true,
            radius: Spyglass::OPENING,
            clear: true,
            lock_camera: false,
            follow_camera: false,
        });
        app.insert_resource(Selected(Some(asking(&[1, 2]))));
        app.add_systems(Update, plotted);

        app.world_mut().write_message(Plotted {
            label: "C -> D".to_owned(),
            systems: vec![8, 9],
            middle: DVec3::ZERO,
            extent: 10.,
            range: "10".to_owned(),
        });
        app.update();

        assert!(app.world().resource::<Selected>().0.is_none());
    }

    /// The active route is drawn at full strength and the rest behind it
    #[test]
    fn what_is_not_active_stands_behind_what_is() {
        assert_eq!(strength(true), 1.);
        assert!(strength(false) < strength(true));
        assert!(strength(false) > 0., "a route faded to nothing is no route");
    }

    /// Closing one route's row leaves the other route drawn
    ///
    /// Which is the whole of why a line says which route it is. Several stand
    /// at once and they are closed one at a time, so a line that could not be
    /// told from the next would go with it.
    #[test]
    fn closing_one_route_leaves_the_others_drawn() {
        let (kept, closed) = (asking(&[1, 2]), asking(&[8, 9]));
        let mut filters = Filters::default();
        filters.add(kept.clone());
        filters.add(closed.clone());

        let (mut app, first) = map(filters, kept);
        let second = line_for(&mut app, closed);
        app.update();
        assert!(drawn(&app, first) && drawn(&app, second));

        // The second row, which is the second route added.
        app.world_mut().resource_mut::<Filters>().remove(1);
        app.update();

        assert!(drawn(&app, first), "the route that was kept was rubbed out");
        assert!(!drawn(&app, second), "the route let go of is still drawn");
    }
}
