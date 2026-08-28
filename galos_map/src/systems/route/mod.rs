use crate::camera::{FRAMING_MARGIN, MoveCamera};
use crate::schedule::MapSet;
use crate::systems::Spyglass;
use crate::systems::System;
use crate::systems::bodies::spawn::{HeldSystem, Strength};
use crate::systems::filter::{Filter, Filters};
use bevy::asset::RenderAssetUsages;
use bevy::math::DVec3;
use bevy::mesh::PrimitiveTopology;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_message::<PlottedRoute>();
    app.init_resource::<SelectedRoute>();
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
    // After what is drawn has been settled, that being what the line is cut
    // back to.
    app.add_systems(
        Update,
        trim.in_set(MapSet::Present).after(crate::systems::visibility),
    );
}

/// The stops a route's line runs through, and which of them were drawn
///
/// The addresses as well as the places, since which systems are on the map
/// decides what of the line is drawn and the places alone cannot say. Kept on
/// the line because a system the route runs through may not be spawned at all:
/// there is then no entity to read a position off, and the line still has to
/// know where the leg was going.
#[derive(Component)]
pub struct Path {
    /// Each stop, by address, and where it sits in the line's own space
    stops: Vec<(i64, Vec3)>,
    /// Which of them were on the map when the line was last cut
    shown: Vec<bool>,
}

impl Path {
    /// A path through `stops`, with nothing yet known about what is drawn
    pub fn new(stops: Vec<(i64, Vec3)>) -> Path {
        let shown = vec![true; stops.len()];
        Path { stops, shown }
    }

    /// The line as it stands, whole
    pub(super) fn whole(&self) -> Vec<Vec3> {
        self.stops.iter().map(|(_, at)| *at).collect()
    }
}

/// Cut each route's line back to what is on the map
///
/// Runs over the lines rather than over the systems, and rebuilds one only
/// where the answer moved. The mesh is rebuilt in place, under the handle the
/// line already holds, so nothing downstream has to be told.
fn trim(
    systems: Query<(&System, &Visibility)>,
    mut lines: Query<(&mut Path, &Mesh3d)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if lines.is_empty() {
        return;
    }

    let shown: HashSet<i64> = systems
        .iter()
        .filter(|(_, visibility)| **visibility != Visibility::Hidden)
        .map(|(system, _)| system.address)
        .collect();

    for (mut path, mesh) in &mut lines {
        // A system the map never spawned is not on it, which is the same
        // answer as one the spyglass or the filters put away.
        let wanted: Vec<bool> = path
            .stops
            .iter()
            .map(|(address, _)| shown.contains(address))
            .collect();
        if path.shown == wanted {
            continue;
        }

        let mut points = legs(&path.whole(), &wanted);
        // A cut that leaves nothing is a route with none of its systems on the
        // map, which the spyglass or the filters can do at any moment. Handing
        // the renderer a mesh of no vertices leaves its slab allocator holding
        // a key that was never allocated, and it says so, every frame:
        //
        //     ERROR bevy_render::slab_allocator: Use-after-free: attempted to
        //     copy element data for an unallocated key
        //
        // A line of no length is a mesh all the same and draws nothing. The
        // line's own `Visibility` is not free to say this instead: it carries
        // whether the route's row is turned on, and `follow_filters` writes it
        // every frame from that.
        if points.is_empty() {
            points = vec![Vec3::ZERO, Vec3::ZERO];
        }

        // Nothing to write to where the mesh has already gone. What was drawn
        // is left unrecorded with it, so the cut is tried again rather than
        // taken as done.
        if meshes.insert(&mesh.0, LineList { points }.into()).is_err() {
            continue;
        }
        path.shown = wanted;
    }
}

/// What the ring around a stop is drawn in
///
/// The white a route's line is drawn in, and at full strength where the line
/// is faint: the line crosses systems that are meant to go on being seen, and
/// this is a mark around one of them.
pub const HOP: Srgba = Srgba::new(1., 1., 1., 0.9);

/// A stop a route reaches from the system the camera is standing in
///
/// The one behind and the one ahead, of every route running through that
/// system. A route is drawn as a line between systems, and that line is gone
/// by the time the camera is inside one of them, so what is left to say where
/// the route goes is the systems it goes to and from.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hop {
    /// Where the route came from
    Last,
    /// Where it goes next
    Next,
}

/// How strongly the mark for the system the map is holding is drawn
///
/// Whole where the map holds nothing, which is the camera out among the stars.
/// Read off the system rather than worked out here, so that a route fading in
/// as the camera descends and the mark it is fading in behind are the one
/// figure and go together.
fn standing(holding: &HeldSystem, marks: &Query<&Strength>) -> f32 {
    holding
        .of()
        .and_then(|system| marks.get(system).ok())
        .map_or(1., |strength| strength.0)
}

/// Which systems `routes` reach from `here`, and which way each of them lies
///
/// Every route the map is showing, in whatever order they are handed over, so
/// that standing on one of several is standing on a route: the one being
/// worked with is drawn in front out among the stars, and in here what matters
/// is which routes come through the system the camera is in.
///
/// Nothing for either end of a route, which reaches only one way, and nothing
/// from a system a route does not run through: a route passes near far more
/// systems than it stops at, and being beside one is not being on it.
///
/// A system two routes both reach, one going on and the other coming back, is
/// marked the way the first of them reaches it. There is one mark to be drawn
/// and it points one way, so the routes are asked in the order they are given
/// and the first answer stands.
///
/// And nothing at all until the camera is inside, which `standing` says: it is
/// how much of the mark for the system being held is left, and it reaches
/// nothing only once the camera is in there. The rows come in from far
/// further out than that, so the system being held is not the system being
/// stood in, and reading the rows alone put the mark up while the camera was
/// still out among the stars. Out there the line answers this question
/// already, and answers it better; in here the line is gone.
fn reaching<'a>(
    routes: impl IntoIterator<Item = &'a Filter>,
    here: Option<i64>,
    standing: f32,
) -> Vec<(i64, Hop)> {
    if standing > 0. {
        return Vec::new();
    }
    let Some(here) = here else { return Vec::new() };

    let mut reached: Vec<(i64, Hop)> = Vec::new();
    for route in routes {
        let Filter::Route { systems, .. } = route else { continue };
        let Some(at) = systems.iter().position(|address| *address == here)
        else {
            continue;
        };

        let behind = at.checked_sub(1).and_then(|before| systems.get(before));
        let ahead = systems.get(at + 1);
        for (address, way) in [(behind, Hop::Last), (ahead, Hop::Next)] {
            let Some(address) = address else { continue };
            if reached.iter().any(|(held, _)| held == address) {
                continue;
            }
            reached.push((*address, way));
        }
    }

    reached
}

/// Keep the mark on whichever systems the routes reach from here
///
/// Written only where it changed. This runs over every star every frame, and
/// inserting a component marks the star changed whether or not the value
/// moved, which drags its name and its material along behind it.
fn hops(
    filters: Res<Filters>,
    selected: Res<SelectedRoute>,
    contents: Res<crate::systems::bodies::Contents>,
    holding: Res<HeldSystem>,
    marks: Query<&Strength>,
    systems: Query<(Entity, &System, Option<&Hop>)>,
    mut commands: Commands,
) {
    // The route in front asked first, so that where two of them reach the same
    // system in opposite directions the one being worked with says which way
    // it lies.
    let front = active(&filters, &selected.0);
    let routes = front
        .into_iter()
        .chain(shown(&filters).filter(|route| Some(*route) != front));
    let reached = reaching(routes, contents.of(), standing(&holding, &marks));

    for (entity, system, held) in &systems {
        let wanted = reached
            .iter()
            .find(|(address, _)| *address == system.address)
            .map(|(_, way)| *way);

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
pub struct PlottedRoute {
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

impl PlottedRoute {
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
    mut plotted: MessageReader<PlottedRoute>,
    mut camera: MessageWriter<MoveCamera>,
    mut spyglass: ResMut<Spyglass>,
    mut filters: ResMut<Filters>,
    mut selected: ResMut<SelectedRoute>,
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
pub struct SelectedRoute(pub Option<Filter>);

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
    selected
        .as_ref()
        .filter(|picked| shown(filters).any(|filter| filter == *picked))
        // The last route held, which is the last one plotted: they are added
        // in the order they land.
        .or_else(|| shown(filters).last())
}

/// Every route the map is showing, in the order they were plotted
///
/// A row turned off is not among them, its line being off the map: what is
/// asked of the routes is asked about what the user can see.
fn shown(filters: &Filters) -> impl Iterator<Item = &Filter> {
    filters
        .iter()
        .filter(|active| active.enabled)
        .map(|active| &active.filter)
        .filter(|filter| matches!(filter, Filter::Route { .. }))
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
    selected: Res<SelectedRoute>,
    holding: Res<HeldSystem>,
    marks: Query<&Strength>,
    lines: Query<(&Route, &MeshMaterial3d<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let active = active(&filters, &selected.0);
    let standing = standing(&holding, &marks);

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
pub mod graph;
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

/// Points taken two at a time, each pair a line of its own
///
/// A strip joins everything handed to it, which a route cannot use: it has to
/// leave gaps, between the dashes running out to a system that is not drawn
/// and across the legs that are not drawn at all.
#[derive(Debug, Clone)]
pub(crate) struct LineList {
    pub(crate) points: Vec<Vec3>,
}

impl From<LineList> for Mesh {
    fn from(line: LineList) -> Self {
        Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, line.points)
    }
}

/// How much of a leg into a system that is not drawn is drawn solid
///
/// About a third. Enough that the leg reads as a leg where it leaves the
/// system that is drawn, and no more than that: the rest is given over to the
/// dashes, which are what says the route goes on past the edge of what is
/// being shown.
const SOLID: f32 = 0.35;

/// How long a dash is, and the gap after it, in metres
///
/// A length in the world rather than a share of the leg. A share cannot be
/// read at more than one zoom: the legs of a route differ by tens of times
/// over, so the same share draws a dash of one size out at one stop and
/// another size at the next, and flying in leaves it a few pixels long against
/// a leg that now runs off both edges of the screen. Held at a distance
/// instead, a dash is the same thing everywhere on the route and grows on
/// screen as the camera comes in, which is what everything else drawn in the
/// world does.
///
/// Half a light year, which is a few pixels with a whole route in view and a
/// clear mark by the time one stop is.
const DASH: f32 = (0.5 * crate::space::LIGHT_YEAR) as f32;

/// The line to draw for a route, as pairs of points
///
/// A leg between two systems that are both drawn is drawn whole. One between
/// two that are not is not drawn at all: neither end is on the map, so a line
/// joining them says nothing about anything the viewer can see, and a route
/// crossing an unfetched stretch would otherwise be one long line over
/// nothing.
///
/// A leg with one end on the map is the interesting one. It is drawn from the
/// end that is there, solid most of the way and then in dashes, and stops
/// short of the end that is not. What that says is that the route goes on past
/// what is being shown, which is true and is the one thing the viewer cannot
/// otherwise tell: a leg simply cut at the edge of the reach reads as a route
/// that ends there.
pub(super) fn legs(points: &[Vec3], shown: &[bool]) -> Vec<Vec3> {
    if points.len() != shown.len() {
        return Vec::new();
    }

    let mut drawn = Vec::new();
    for (leg, ends) in points.windows(2).enumerate() {
        match (shown[leg], shown[leg + 1]) {
            (true, true) => drawn.extend_from_slice(ends),
            (false, false) => {}
            // From whichever end is on the map, towards the one that is not.
            (here, _) => {
                let (from, to) =
                    if here { (ends[0], ends[1]) } else { (ends[1], ends[0]) };
                let leg = (to - from).length();
                let Some(along) = (to - from).try_normalize() else { continue };

                drawn.push(from);
                drawn.push(from + along * leg * SOLID);

                // A dash and the gap after it are the same length, so the run
                // reads as a dashed line rather than as marks left by one. It
                // starts a gap clear of the solid stretch and stops a gap
                // short of the system that is not drawn, so however many fit
                // is however many the leg has room for.
                let mut at = leg * SOLID + DASH;
                while at + DASH <= leg - DASH {
                    drawn.push(from + along * at);
                    drawn.push(from + along * (at + DASH));
                    at += DASH + DASH;
                }
            }
        }
    }

    drawn
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

        assert_eq!(
            reaching([&route], Some(2), 0.),
            vec![(1, Hop::Last), (3, Hop::Next)]
        );
    }

    /// Either end of a route reaches only the one way
    #[test]
    fn the_ends_of_a_route_reach_one_way() {
        let route = route(vec![1, 2, 3]);

        assert_eq!(reaching([&route], Some(1), 0.), vec![(2, Hop::Next)]);
        assert_eq!(reaching([&route], Some(3), 0.), vec![(2, Hop::Last)]);
    }

    /// A route reaches from where it stands whether or not it is the one in
    /// front
    ///
    /// Several routes stand at once, and the camera is inside one system at a
    /// time. Which route the user was last working with says nothing about
    /// which of them runs through the system they are standing in.
    #[test]
    fn every_route_shown_reaches_from_where_it_stands() {
        let front = route(vec![1, 2, 3]);
        let behind = route(vec![7, 8, 9]);

        assert_eq!(
            reaching([&front, &behind], Some(8), 0.),
            vec![(7, Hop::Last), (9, Hop::Next)]
        );
    }

    /// Two routes through one system reach both ways along each of them
    #[test]
    fn two_routes_through_a_system_both_reach() {
        let one = route(vec![1, 2, 3]);
        let other = route(vec![4, 2, 5]);

        assert_eq!(
            reaching([&one, &other], Some(2), 0.),
            vec![
                (1, Hop::Last),
                (3, Hop::Next),
                (4, Hop::Last),
                (5, Hop::Next)
            ]
        );
    }

    /// A stop two routes disagree about is marked the way the first says
    ///
    /// One mark is drawn for it and it points one way. The routes are handed
    /// over with the one being worked with at the head, so that is the one
    /// answering.
    #[test]
    fn a_stop_reached_both_ways_takes_the_first_answer() {
        let there = route(vec![1, 2, 3]);
        let back = route(vec![3, 2, 1]);

        assert_eq!(
            reaching([&there, &back], Some(2), 0.),
            vec![(1, Hop::Last), (3, Hop::Next)]
        );
        assert_eq!(
            reaching([&back, &there], Some(2), 0.),
            vec![(3, Hop::Last), (1, Hop::Next)]
        );
    }

    /// Two points, so a leg is one pair of them
    ///
    /// Twenty light years apart, which is a jump a route is plotted in.
    const A: Vec3 = Vec3::ZERO;
    const B: Vec3 = Vec3::new(20. * DASH / 0.5, 0., 0.);

    /// A leg between two systems on the map is drawn whole
    #[test]
    fn a_leg_between_two_drawn_systems_is_one_line() {
        assert_eq!(legs(&[A, B], &[true, true]), vec![A, B]);
    }

    /// A leg between two systems that are not on the map is not drawn
    ///
    /// Neither end is there to be joined to anything, so a line between them
    /// says nothing about what the viewer can see.
    #[test]
    fn a_leg_between_two_undrawn_systems_is_nothing() {
        assert!(legs(&[A, B], &[false, false]).is_empty());
    }

    /// A leg with one end on the map runs out from that end and stops short
    ///
    /// Whichever end it is. The solid stretch begins at the system that is
    /// drawn, and nothing reaches the one that is not: a line touching it
    /// would say it is there.
    #[test]
    fn a_leg_out_of_the_map_trails_off_before_it_arrives() {
        for (shown, near, far) in [([true, false], A, B), ([false, true], B, A)]
        {
            let drawn = legs(&[A, B], &shown);

            assert_eq!(drawn.first(), Some(&near), "did not start where drawn");
            assert!(
                drawn.iter().all(|at| at.distance(far) > 0.1),
                "a dash reached the system that is not drawn"
            );
            assert!(
                drawn.len() > 4,
                "the leg came back as {} points, too few to be dashed",
                drawn.len()
            );
        }
    }

    /// A dash is the same length on a short leg as on a long one
    ///
    /// The whole reason for measuring it in the world. A share of the leg
    /// draws one size out at one stop and another at the next, and a route's
    /// legs differ by tens of times over.
    #[test]
    fn a_dash_is_the_same_length_whatever_the_leg() {
        let short = Vec3::new(B.x / 3., 0., 0.);

        let long = legs(&[A, B], &[true, false]);
        let brief = legs(&[A, short], &[true, false]);

        // The first dash of each, which is the pair after the solid run.
        assert!(
            ((long[3] - long[2]).length() - (brief[3] - brief[2]).length())
                .abs()
                < 1.,
            "a dash drew {} on one leg and {} on another",
            (long[3] - long[2]).length(),
            (brief[3] - brief[2]).length()
        );
    }

    /// A line is cut only where its stops and what is drawn agree in length
    #[test]
    fn a_line_nothing_is_known_about_is_not_drawn() {
        assert!(legs(&[A, B], &[true]).is_empty());
    }

    /// Nothing is reached from outside the system, whole mark or fading one
    ///
    /// The rows for a system come in from far further out than the camera ever
    /// goes, so holding them is not standing in it. Out there the line says
    /// where the route runs, and says it better than two rings could.
    #[test]
    fn a_route_reaches_nowhere_from_outside_the_system() {
        let route = route(vec![1, 2, 3]);

        assert!(reaching([&route], Some(2), 1.).is_empty());
        assert!(reaching([&route], Some(2), 0.5).is_empty());
    }

    /// Standing beside a route is not standing on it
    ///
    /// A route passes near far more systems than it stops at, and a mark
    /// saying where to go next means nothing from a system it never visits.
    #[test]
    fn a_system_the_route_misses_reaches_nowhere() {
        let route = route(vec![1, 2, 3]);

        assert!(reaching([&route], Some(9), 0.).is_empty());
        assert!(reaching([&route], None, 0.).is_empty());
        assert!(reaching([], Some(2), 0.).is_empty());
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
        app.add_message::<PlottedRoute>();
        app.add_message::<MoveCamera>();
        app.insert_resource(Spyglass {
            fetch: true,
            radius: Spyglass::OPENING,
            clear: true,
            lock_camera: false,
            follow_camera: false,
        });
        app.init_resource::<Filters>();
        app.init_resource::<SelectedRoute>();
        app.add_systems(Update, plotted);

        app.world_mut().write_message(PlottedRoute {
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
        app.add_message::<PlottedRoute>();
        app.add_message::<MoveCamera>();
        app.init_resource::<Filters>();
        app.insert_resource(Spyglass {
            fetch: true,
            radius: Spyglass::OPENING,
            clear: true,
            lock_camera: false,
            follow_camera: false,
        });
        app.insert_resource(SelectedRoute(Some(asking(&[1, 2]))));
        app.add_systems(Update, plotted);

        app.world_mut().write_message(PlottedRoute {
            label: "C -> D".to_owned(),
            systems: vec![8, 9],
            middle: DVec3::ZERO,
            extent: 10.,
            range: "10".to_owned(),
        });
        app.update();

        assert!(app.world().resource::<SelectedRoute>().0.is_none());
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
