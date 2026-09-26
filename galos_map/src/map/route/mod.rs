use crate::map::bodies::spawn::{HeldSystem, Strength};
use crate::map::camera::{FRAMING_MARGIN, MoveCamera, OrbitCamera};
use crate::map::filter::{Filter, Filters, Plotted};
use crate::map::galaxy::Spyglass;
use crate::map::galaxy::System;
use crate::map::index::Names;
use crate::map::schedule::MapSet;
use crate::map::search::Search;
use bevy::asset::RenderAssetUsages;
use bevy::math::DVec3;
use bevy::mesh::PrimitiveTopology;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use elite_journal::Boxel;
use galos_route::Boosts;
use galos_route::graph;
use galos_route::graph::{Drive, Jumps, Routing, Tuning};
use std::sync::Arc;
/// How a route is written: the two systems it runs between, in order
pub(crate) const ARROW: &str = " -> ";

/// How the next route is to be asked for: the three settings the form edits
///
/// One resource rather than three, since nothing reads one of them without
/// the others: a route is asked with all three, and a row carries all three
/// to be asked again with. See [`galos_route::graph`] for what each means.
#[derive(Resource, Default, Clone, Copy, PartialEq, Debug)]
pub(crate) struct RouteSettings {
    /// How far over the fewest jumps a route may settle, and what it weighs
    pub(crate) how: Routing,
    /// What the ship can supercharge on
    pub(crate) drive: Drive,
    /// How a long supercharged route is planned
    pub(crate) tune: Tuning,
}

/// What routes are searched over: the jump graph and the supercharge table
///
/// Held together because the graph is built weighed by the table, and a
/// refresh that replaces the table has to let go of the graph built on the
/// old one. An absent table is what the map says where it cannot say where
/// a jet cone is, which is exactly the right answer before the index is
/// read — see [`Boosts::absent`].
#[derive(Resource, Default, Clone)]
pub(crate) struct Router {
    /// The graph, once a route has asked for one, and the galaxy it reads
    pub(crate) jumps: Jumps,
    /// Which systems can supercharge a drive, as published
    pub(crate) boosts: Boosts,
}

impl Router {
    /// The graph, opening it if this is the first route of the session
    pub(crate) fn built(&mut self) -> Option<Arc<graph::JumpGraph>> {
        self.jumps.built(&self.boosts)
    }
}

pub fn plugin(app: &mut App) {
    app.add_message::<PlottedRoute>();
    app.add_message::<UnflownLeg>();
    app.init_resource::<SelectedFilter>();
    app.init_resource::<RouteSettings>();
    app.init_resource::<Router>();
    // After the fetch it answers has been drawn, and before the camera is
    // pointed, since where it asks the camera to go is what `move_camera`
    // then works out.
    app.add_systems(
        Update,
        (plotted, follow_filters)
            .chain()
            .in_set(MapSet::Populate)
            .after(crate::map::galaxy::spawn::spawn),
    );
    // Where the trip is asked for rather than where its legs land, which is
    // the whole point of it: see `frame_trip`.
    app.add_systems(Update, frame_trip.in_set(MapSet::Fetch));
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
        trim.in_set(MapSet::Present).after(crate::map::galaxy::visibility),
    );
    // Which of a route's systems are worth a mark, which is a question about
    // the screen. Before the field is built from them, that being what reads
    // the answer.
    app.add_systems(
        Update,
        thin.in_set(MapSet::Present)
            .before(crate::map::paint::field::build_field),
    );
    // What the searches under way have reached, drawn while they run and
    // taken down as each finishes. In `Populate`, with the rest of what the
    // map builds, and before the lines are cut back: a frontier is not a
    // route and `trim` has nothing to say about it.
    app.init_resource::<frontier::Frontiers>();
    app.add_systems(Update, frontier::draw.in_set(MapSet::Populate));
}

/// The stops a route's line runs through, and which of them were drawn
///
/// The addresses as well as the places, since which systems are on the map
/// decides what of the line is drawn and the places alone cannot say. Kept on
/// the line because a system the route runs through may not be spawned at all:
/// there is then no entity to read a position off, and the line still has to
/// know where the leg was going.
#[derive(Component)]
pub(crate) struct Path {
    /// Each stop, by address, and where it sits in the line's own space
    stops: Vec<(i64, Vec3)>,
    /// Which of them were on the map when the line was last cut
    shown: Vec<bool>,
    /// Which jumps were flown on a jet cone, one flag a jump
    ///
    /// Settled when the line is spawned and never again: it is a fact about
    /// the route, where [`Self::shown`] is a fact about the camera. See
    /// [`spawn::charged`].
    charged: Vec<bool>,
    /// How long the dashes it was last cut with are, in metres
    ///
    /// A fact about the camera, like [`Self::shown`]: a dash is a share of
    /// the view, so zooming asks for another one. Nothing where the line has
    /// never been cut for a view, which reads as a dash of no length and is
    /// why a line is dashed on the first frame it is trimmed.
    dash: f32,
}

impl Path {
    /// A path through `stops`, with nothing yet known about what is drawn
    ///
    /// Nor about the view: a line is spawned with every stop taken as drawn,
    /// and a whole leg is drawn whole whatever a dash would be. [`trim`]
    /// settles both against the camera before anything is seen of it.
    pub(crate) fn new(stops: Vec<(i64, Vec3)>, charged: Vec<bool>) -> Path {
        let shown = vec![true; stops.len()];
        Path { stops, shown, charged, dash: 0. }
    }

    /// The line as it stands, whole
    pub(crate) fn whole(&self) -> Vec<Vec3> {
        self.stops.iter().map(|(_, at)| *at).collect()
    }
}

/// Cut each route's line back to what is on the map, and to the zoom
///
/// Runs over the lines rather than over the systems, and rebuilds one only
/// where the answer moved. The mesh is rebuilt in place, under the handle the
/// line already holds, so nothing downstream has to be told.
///
/// Two things move it. Which stops are on the map decides what of the line is
/// drawn at all, and how much sky the camera takes in decides how long the
/// dashes running off the map are: a dash is a share of the view, so zooming
/// asks for a new one — past [`REDASHED_AT`], which keeps a four hundred jump
/// route off the rebuild queue for every click of the wheel.
fn trim(
    camera: Query<(&OrbitCamera, Option<&Projection>)>,
    systems: Query<(&System, &Visibility)>,
    mut lines: Query<(&mut Path, &Mesh3d)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if lines.is_empty() {
        return;
    }
    let Ok((orbit, lens)) = camera.single() else { return };

    let shown: HashSet<i64> = systems
        .iter()
        .filter(|(_, visibility)| **visibility != Visibility::Hidden)
        .map(|(system, _)| system.address)
        .collect();

    // How long a dash wants to be for the view as it now stands. One answer
    // for every line, the view being one view.
    let dash = dash_of(
        2. * crate::map::camera::framed(orbit.radius, lens) as f64
            * crate::map::space::LIGHT_YEAR,
    );

    for (mut path, mesh) in &mut lines {
        // A system the map never spawned is not on it, which is the same
        // answer as one the spyglass or the filters put away.
        let wanted: Vec<bool> = path
            .stops
            .iter()
            .map(|(address, _)| shown.contains(address))
            .collect();
        // Cut again where either answer moved, and only then: a line holding
        // this view's dashes over this frame's systems is the line already
        // drawn.
        let held = 1. / REDASHED_AT..=REDASHED_AT;
        let drifted = !held.contains(&(dash / path.dash));
        if path.shown == wanted && !drifted {
            continue;
        }

        let mut line = legs(&path.whole(), &wanted, &path.charged, dash);
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
        if line.points.is_empty() {
            line = LineList {
                points: vec![Vec3::ZERO, Vec3::ZERO],
                colors: vec![spawn::jump_color(false); 2],
            };
        }

        // Nothing to write to where the mesh has already gone. What was drawn
        // is left unrecorded with it, so the cut is tried again rather than
        // taken as done.
        if meshes.insert(&mesh.0, line.into()).is_err() {
            continue;
        }
        path.shown = wanted;
        path.dash = dash;
    }
}

/// What the ring around a stop is drawn in
///
/// The white a route's line is drawn in, and at full strength where the line
/// is faint: the line crosses systems that are meant to go on being seen, and
/// this is a mark around one of them.
pub(crate) const HOP: Srgba = Srgba::new(1., 1., 1., 0.9);

/// How much of a route system's mark is left, where a route asked for less
///
/// One per system on a route, and only there: the marks are the star field's
/// and it paints them at what the filters and the descent leave: see
/// [`crate::map::paint::field`]. This is a route's own say over the systems it
/// runs through, which it has for one reason — a route of a hundred jumps seen
/// from far enough away is a hundred marks a pixel apart, and a pixel apart
/// they are not systems on a route any more, they are a stipple over the line
/// that says where the route goes.
///
/// Whole where a hop stands clear of the one before it, nothing where they
/// have closed up, and part of the way between: see [`thin`].
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub(crate) struct Thinned(pub(crate) f32);

/// Under how many pixels apart two systems on a route stop telling apart
///
/// A mark is a few pixels across, so hops closer together than this overlap
/// outright: what is drawn is not two systems but a smear where two were.
const MERGED: f32 = 2.5;

/// And past how many they read as separate systems
///
/// Between the two a hop is faded rather than dropped, so nothing pops as the
/// camera pulls back. Roomy rather than tight: marks stop reading as separate
/// places well before they touch.
const APART: f32 = 8.;

/// Thin a route's hops down to the ones that can be told apart
///
/// Which systems on a route are worth a mark is a question about the screen
/// and not about the route: zoomed in they are the nodes the line joins and
/// the whole of what a route is made of, and zoomed out they are a hundred
/// marks over a line a hundred pixels long. So this walks each route in the
/// order it is flown and keeps whichever hops stand clear of the last one it
/// kept — the pixels between them measured where they actually fall, so the
/// answer follows the camera without anything having to be told.
///
/// A stop is never thinned. It is what the user asked for rather than where
/// the ship refuels, and it is the one thing on a route that is worth a mark
/// at any distance; see [`Filter::stops`].
///
/// The suppressed hops do not move the reckoning on. Measuring from the last
/// system *seen* rather than the last one *kept* would thin every hop after
/// the first crowded one, and a route through a crowded region into a bare one
/// would come out with a gap in it.
///
/// A system on two routes keeps the most either asks for: two routes are two
/// answers and neither is entitled to rub out the other's node.
fn thin(
    filters: Res<Filters>,
    camera: Query<(&OrbitCamera, &Camera)>,
    systems: Query<(Entity, &System, Option<&Thinned>)>,
    mut commands: Commands,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    // Which systems any route runs through, before the sky is walked: the map
    // holds a hundred thousand systems and a route holds hundreds, so the
    // question asked of each system is a lookup in the small set rather than
    // the routes being searched for each of them.
    let mut on_routes: HashSet<i64> = HashSet::default();
    for route in shown(&filters) {
        let Filter::Route { systems: hops, .. } = route else { continue };
        on_routes.extend(hops.iter().copied());
    }

    // One pass over the sky, which is the only one: where each of those
    // systems falls on screen, and what it is standing at now. A mark that has
    // gone off every route is put back to whole here, that being the pass that
    // can see it has.
    let mut on_screen: HashMap<i64, (Entity, Option<Vec2>, Option<f32>)> =
        HashMap::default();
    for (entity, system, thinned) in &systems {
        if !on_routes.contains(&system.address) {
            if thinned.is_some() {
                commands.entity(entity).remove::<Thinned>();
            }
            continue;
        }
        let at = crate::map::screen::screen_position(
            orbit,
            cot_half_fov,
            viewport,
            DVec3::from(system.position),
        );
        on_screen
            .insert(system.address, (entity, at, thinned.map(|left| left.0)));
    }

    // Every route on the map is walked, so what a system is left at is the
    // most any of them wants of it.
    let mut wanted: HashMap<i64, f32> = HashMap::default();
    for route in shown(&filters) {
        let Filter::Route { systems: hops, .. } = route else { continue };
        let along = walk(hops, &route.stops(), |address| {
            on_screen.get(&address).and_then(|(_, at, _)| *at)
        });
        for (address, left) in along {
            keep(&mut wanted, address, left);
        }
    }

    // Written only where it moved: this runs every frame, and a route nobody
    // is zooming past is a walk that writes nothing.
    for (address, (entity, _, standing)) in &on_screen {
        let left = wanted.get(address).copied().unwrap_or(1.);
        if *standing != Some(left) {
            commands.entity(*entity).insert(Thinned(left));
        }
    }
}

/// What each of `hops` is left at, walking the route in the order it is flown
///
/// The rule itself, apart from the sky it is asked about: `at` says where a
/// system falls on screen, and [`None`] is a system off the frame or not on
/// the map — nothing to measure and nothing drawn either way, so it is left
/// whole and left out of the reckoning.
fn walk(
    hops: &[i64],
    stops: &[i64],
    at: impl Fn(i64) -> Option<Vec2>,
) -> Vec<(i64, f32)> {
    let mut along = Vec::with_capacity(hops.len());
    let mut last: Option<Vec2> = None;
    for address in hops {
        let Some(here) = at(*address) else {
            along.push((*address, 1.));
            continue;
        };
        if stops.contains(address) {
            along.push((*address, 1.));
            last = Some(here);
            continue;
        }
        let left = match last {
            None => 1.,
            Some(last) => {
                let pixels = here.distance(last);
                ((pixels - MERGED) / (APART - MERGED)).clamp(0., 1.)
            }
        };
        along.push((*address, left));
        // Only a hop that is drawn stands as the one the next is measured
        // against. See [`thin`].
        if left > 0. {
            last = Some(here);
        }
    }
    along
}

/// Hold `address` at the most anything has asked for it.
fn keep(wanted: &mut HashMap<i64, f32>, address: i64, left: f32) {
    let held = wanted.entry(address).or_insert(left);
    *held = held.max(left);
}

/// A stop a route reaches from the system the camera is standing in
///
/// The one behind and the one ahead, of every route running through that
/// system. A route is drawn as a line between systems, and that line is gone
/// by the time the camera is inside one of them, so what is left to say where
/// the route goes is the systems it goes to and from.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Hop {
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
    selected: Res<SelectedFilter>,
    contents: Res<crate::map::bodies::Contents>,
    holding: Res<HeldSystem>,
    marks: Query<&Strength>,
    systems: Query<(Entity, &System, Option<&Hop>)>,
    mut commands: Commands,
) {
    // The routes in front asked first, so that where two of them reach the
    // same system in opposite directions the one being worked with says which
    // way it lies.
    let front = active(&filters, &selected.0);
    let routes = front
        .iter()
        .copied()
        .chain(shown(&filters).filter(|route| !front.contains(route)));
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
pub(crate) struct Route(pub(crate) Filter);

/// A route that has landed and been drawn
///
/// Written where the fetch is collected, since that is the one place the
/// systems it runs through are in hand, and answered here so that what a
/// route does to the map is in one place rather than threaded through the
/// system that draws stars.
#[derive(Message, Debug)]
pub(crate) struct PlottedRoute {
    /// Its two ends, as the database spells them
    pub(crate) label: String,
    /// Every system it runs through, by address, in the order travelled
    pub(crate) systems: Vec<i64>,
    /// The trip this leg is one of, where it is one of several
    pub(crate) trip: Option<String>,
    /// How far the ship it was plotted for reaches in one jump, in light years
    ///
    /// Carried along rather than worked out from the legs. The longest jump a
    /// route happens to take is not what was asked for: a route plotted for a
    /// ship reaching 20 may never need more than 12, and it is what the user
    /// asked that tells two plots between the same ends apart.
    pub(crate) range: String,
    /// Which drive it was plotted for, carried along for the same reason the
    /// range is: it is part of what tells two plots between the same ends
    /// apart. See [`galos_route::graph::Drive`].
    pub(crate) drive: Drive,
    /// How hard the search worked at it, carried along for the same reason.
    /// See [`galos_route::graph::Routing`].
    pub(crate) how: Routing,
    /// How the plan over the boost stars was worked out, carried along for
    /// the same reason the rest are: it is part of what was asked, and part
    /// of what tells two plots between the same ends apart. See
    /// [`galos_route::graph::Tuning`].
    pub(crate) tune: Tuning,
    /// How long it took, from the click to the answer landing
    ///
    /// Wall time and not the search's own: what it measures is the wait,
    /// which includes the leg sitting in the pool's queue and the jump graph
    /// being opened for the first route of a session. Kept beside the row
    /// rather than in the filter; see [`crate::map::filter::Entry::took`].
    pub(crate) took: std::time::Duration,
}

impl PlottedRoute {
    /// The filter this route asks for, and the line's own name for itself
    ///
    /// Built in one place and read in two: the row in the bar is this filter,
    /// and so is the mark the drawn line carries. They have to be the same
    /// value or closing the row would leave a line nothing can find.
    pub(crate) fn filter(&self) -> Filter {
        Filter::Route {
            label: self.label.clone(),
            systems: self.systems.clone(),
            range: self.range.clone(),
            trip: self.trip.clone(),
            drive: self.drive,
            how: self.how,
            tune: self.tune,
        }
    }
}

/// Look at the whole trip, and reach far enough to hold it
///
/// Written where the trip is asked for rather than where its legs land. A
/// trip is several routes and they land one at a time, so a camera pointed by
/// each of them in turn is a camera flung from leg to leg and left framing
/// whichever answered last. The trip is one thing to look at, and this is the
/// one place that knows the whole of it.
///
/// It needs no route to say so. Where the stops stand is already on record,
/// so the trip can be framed the moment it is asked for rather than when the
/// last leg comes back — which is also the better moment, the camera moving
/// as the button is pressed rather than seconds later.
///
/// The spyglass is set rather than left alone because a trip is usually
/// longer than whatever the user was looking at when they asked for it, and a
/// route drawn as a line running out through the edge of an unchanged
/// spyglass is a route with no systems on it.
fn frame_trip(
    mut asked: MessageReader<Search>,
    names: Res<Names>,
    mut camera: MessageWriter<MoveCamera>,
    mut spyglass: ResMut<Spyglass>,
) {
    for ask in asked.read() {
        let Search::Route { stops, .. } = ask else { continue };
        // Whatever is on record. A stop the names table does not know is a
        // leg that will come back with nothing, and the form is already
        // saying so; the trip is still framed over the stops that are real.
        //
        // The middle of the boxel each address names rather than the
        // system's own place: this frames a camera over a trip thousands
        // of light years long, and a boxel is ten of them at the class
        // most systems are. Free, too — arithmetic on the address, where
        // the exact place would be a sphere query a stop.
        let places: Vec<DVec3> = stops
            .iter()
            .filter_map(|stop| names.address(stop))
            .map(|address| DVec3::from(Boxel::of(address).place().0))
            .collect();
        let Some((middle, extent)) = crate::map::route::spawn::framing(&places)
        else {
            continue;
        };

        camera.write(MoveCamera {
            position: Some(middle),
            framing: Some(extent),
        });

        // Measured from the middle, which is where the camera is going, so
        // what the spyglass holds is what the camera is about to see. The
        // same room around it that the camera is stood back to leave, since a
        // reach set to the trip's own extent puts the far stops exactly on
        // the rim of it: the extent is the distance to the furthest of them,
        // and whether that counts as reaching them comes down to which way an
        // `f32` rounded.
        //
        // Held inside what the map will reach unasked. Everything the
        // spyglass takes in is fetched and spawned, and a trip long enough
        // would otherwise set a reach nobody asked the size of.
        spyglass.radius = (extent * FRAMING_MARGIN)
            .clamp(Spyglass::OPENING, Spyglass::UNASKED);
    }
}

/// A leg that came back with no route
///
/// Written where the fetch is collected, as [`PlottedRoute`] is: a leg
/// answering with fewer than two systems is the router saying it could not
/// get from one end to the other at the range asked, and the row standing for
/// that leg has to say so rather than go on saying it is being searched.
///
/// By the key it was asked under, that being what the collector has in hand —
/// the two ends as the user typed them. Which row asked is a question about
/// addresses, so [`plotted`] resolves the pair against the names table, which
/// is the table the row's own ends were resolved through.
#[derive(Message, Debug)]
pub(crate) struct UnflownLeg(pub(crate) crate::map::galaxy::fetch::FetchIndex);

impl UnflownLeg {
    /// The ask this leg was, as the row standing for it holds it
    ///
    /// [`None`] where either end is no longer a name the table knows, there
    /// being no row it could have gone up as either.
    fn ask(&self, names: &Names) -> Option<Filter> {
        let crate::map::galaxy::fetch::FetchIndex::Route(
            start,
            end,
            range,
            trip,
            drive,
            how,
            tune,
        ) = &self.0
        else {
            return None;
        };
        let ends = (names.address(start)?, names.address(end)?);
        Some(Filter::Route {
            label: format!(
                "{}{}{}",
                said(names, ends.0),
                crate::map::route::ARROW,
                said(names, ends.1),
            ),
            systems: vec![ends.0, ends.1],
            range: range.clone(),
            trip: trip.clone(),
            drive: *drive,
            how: *how,
            tune: *tune,
        })
    }
}

/// What the names table spells the system at `address`
///
/// The map's own spelling rather than whatever the user typed, which is how
/// [`crate::map::galaxy::spawn::build_system`] names a stop and so how a landed
/// route's label is built: a leg's row is put up before its answer and has to
/// read the same after, being the one row.
pub(crate) fn said(names: &Names, address: i64) -> String {
    names
        .get(address)
        .map(|entry| entry.name.to_string())
        .unwrap_or_else(|| address.to_string())
}

/// Hand a leg's row what became of it
///
/// **The row is already there.** It went up when the leg was asked for
/// ([`fetch::fetch_route`]), carrying the two ends and no route between them,
/// so a leg landing hands that row its answer and a leg that could not be
/// flown tells it to stop saying it is searching. One row and one line per
/// leg, so a trip through five systems leaves four of each: each leg is a
/// route the user can close, turn off, or pick out on its own. Where the
/// camera goes is the trip's business rather than any one leg's; see
/// [`frame_trip`].
fn plotted(
    mut plotted: MessageReader<PlottedRoute>,
    mut unflown: MessageReader<UnflownLeg>,
    names: Res<Names>,
    mut filters: ResMut<Filters>,
) {
    for route in plotted.read() {
        filters.landed(route.filter(), route.took);
    }
    for leg in unflown.read() {
        if let Some(ask) = leg.ask(&names) {
            filters.gave_up(&ask, Plotted::Unreachable);
        }
    }
}

/// Where a leg's row belongs among the rows already there
///
/// After every leg of the same trip that is flown before it, and before every
/// one flown after. Anything else held -- a faction, a hand-picked set, a
/// route from some earlier trip -- is left where it is: the trip orders its
/// own legs and says nothing about anyone else's.
///
/// The same trip plotted again for another ship is another trip, and its legs
/// are nobody else's to order: a leg is one of these where the stops and the
/// ship both agree. Weighing the stops alone laid the second plot's legs in
/// among the first's, and the bar drew the two as one trip.
///
/// The end of the list for a route belonging to no trip, which is what
/// [`Filters::add`] would have done with it: a route asked for on its own has
/// nothing to be in order with.
///
/// Read off the route itself. A leg carries the trip it belongs to, and a
/// trip is named for its stops, so the stops and which leg this is are both
/// in hand without anything else being asked.
pub(crate) fn placed_at(leg: &Filter, filters: &Filters) -> usize {
    let last = filters.iter().count();
    let Some((trip, at)) = leg_of(leg) else { return last };

    filters
        .iter()
        .position(|entry| {
            entry.filter.ship() == leg.ship()
                && leg_of(&entry.filter)
                    .is_some_and(|(held, after)| held == trip && after > at)
        })
        .unwrap_or(last)
}

/// Which trip a route is a leg of, and which leg of it it is
///
/// A trip is named for its stops joined by [`crate::map::route::ARROW`] and a leg for
/// its two ends the same way, so which leg it is, is where its own pair falls
/// among them. Spelled either way: the trip holds what the user typed and a
/// line is named as the rows name it.
///
/// Nothing for a route belonging to no trip, and nothing for one whose ends
/// are not a pair of its trip's stops.
fn leg_of(leg: &Filter) -> Option<(&str, usize)> {
    let trip = leg.trip()?;
    let (start, end) = leg.name().split_once(crate::map::route::ARROW)?;
    let stops: Vec<&str> = trip.split(crate::map::route::ARROW).collect();
    let at = stops.windows(2).position(|pair| {
        start.eq_ignore_ascii_case(pair[0]) && end.eq_ignore_ascii_case(pair[1])
    })?;

    Some((trip, at))
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

/// The routes the user picked out of the ones on screen, if they picked any
///
/// Written when a route's row or panel is pressed, which is how the user says
/// which of several they are working with. Cleared by plotting, a route just
/// asked for being the one they are looking at.
///
/// Several rather than one, because a trip is several routes and is picked
/// out as one thing: a press on its row means the whole of it, and a trip
/// with one leg in front and the rest held back would be the map drawing a
/// trip nobody asked for over the trip they did.
///
/// An override rather than the answer itself. What it stands in front of is
/// the last route plotted, and [`active`] puts the two together.
#[derive(Resource, Default)]
pub(crate) struct SelectedFilter(pub(crate) Vec<Filter>);

/// Which routes are the ones being worked with
///
/// The ones last picked out, and failing those the last one plotted, which is
/// the last route filter held: they are added in the order they land, so the
/// end of the list is the newest.
///
/// `selected` is weighed against the filters rather than trusted. A route
/// picked out and then closed would otherwise go on being active with nothing
/// on screen standing for it, and nothing left to hand the emphasis back to.
/// A trip whose legs are picked out and one of them closed keeps the rest:
/// what is left of it is still what the user is working with.
///
/// Only among the routes being shown. A row turned off takes its line off the
/// map, and a route nobody can see cannot be the one in front: the rest would
/// be held back for it and the map would have every route drawn faintly and
/// none of them picked out.
///
/// Empty where no route is being shown at all, there being nothing to be
/// active.
fn active<'a>(filters: &'a Filters, selected: &'a [Filter]) -> Vec<&'a Filter> {
    let picked: Vec<&Filter> = selected
        .iter()
        .filter(|picked| shown(filters).any(|filter| filter == *picked))
        .collect();
    if !picked.is_empty() {
        return picked;
    }

    // The last route held, which is the last one plotted: they are added in
    // the order they land.
    shown(filters).last().into_iter().collect()
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
pub(crate) fn strength(is_active: bool) -> f32 {
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
    selected: Res<SelectedFilter>,
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
            spawn::line_color(strength(active.contains(&&line.0)) * standing);
        // Written only where it changed. Touching a material marks the asset
        // changed, which re-uploads it, and this runs every frame.
        if material.base_color != wanted {
            material.base_color = wanted;
        }
    }
}

pub(crate) mod fetch;
pub(crate) mod frontier;
pub(crate) mod spawn;

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
///
/// Each point carries a colour, because a route is not one colour: a jump
/// flown on a jet cone is drawn blue and an ordinary jump white, and both
/// are jumps of the same route. Per vertex rather than per entity so it
/// stays one mesh and one material — the material's own colour is what the
/// fade writes, and the two multiply.
#[derive(Debug, Clone, Default)]
pub(crate) struct LineList {
    pub(crate) points: Vec<Vec3>,
    pub(crate) colors: Vec<[f32; 4]>,
}

impl LineList {
    /// A line of one colour, which is most of them: an orbit, a crosshair,
    /// a layer of a search. The colour is the material's alone, and no
    /// attribute is written — a vertex colour per point is four floats a
    /// vertex to say the same thing at every one of them.
    pub(crate) fn plain(points: Vec<Vec3>) -> LineList {
        LineList { points, colors: Vec::new() }
    }
}

impl From<LineList> for Mesh {
    fn from(line: LineList) -> Self {
        let mesh = Mesh::new(
            PrimitiveTopology::LineList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, line.points);
        match line.colors.is_empty() {
            true => mesh,
            false => {
                mesh.with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, line.colors)
            }
        }
    }
}

/// How much of a leg into a system that is not drawn is drawn solid
///
/// About a third. Enough that the leg reads as a leg where it leaves the
/// system that is drawn, and no more than that: the rest is given over to the
/// dashes, which are what says the route goes on past the edge of what is
/// being shown.
const SOLID: f32 = 0.35;

/// How many dashes, each with the gap after it, cross the view
///
/// A count against the screen rather than a length in the world, which is how
/// the rings inside a system are dashed as well — see `Spacing`'s `DASHES` in
/// `galos_index::system::orbit`. **A dash held at a fixed distance cannot be read at
/// more than one zoom**: half a light year is a clear mark with one stop in
/// view and a hundredth of a pixel with the galaxy in view, so a leg trailing
/// off the map read as a faint solid line exactly where the dashes were the
/// thing saying the route goes on past what is drawn. Reported that way.
///
/// A share of the view instead, so a dash is the same size on screen at every
/// zoom: the dashes stay dashes on the way out and do not swallow the leg on
/// the way in. Twenty of them and their gaps across the sky the camera takes
/// in, which is a dash of some tens of pixels — read as a dashed line rather
/// than as a chain of ticks, and the same reading the rings give.
const DASHES: f64 = 20.;

/// How long a dash is, and the gap after it, for a camera taking in `across`
/// metres of sky
///
/// In metres, a line's vertices being measured in them. What the camera takes
/// in is the whole of the view top to bottom, and a dash and its gap are one
/// [`DASHES`]th of that between them.
pub(crate) fn dash_of(across: f64) -> f32 {
    (across / (2. * DASHES)) as f32
}

/// The most dashes one leg running off the map is drawn with
///
/// The bound that keeps a dash a share of the *view* from becoming a mesh
/// the size of memory. A dash is [`DASHES`]th of the height of what the
/// camera takes in, which inside a system is thousandths of a light year,
/// while the leg it is dashing stays tens of light years long — so the
/// number of them is one divided by the other and grows without limit as
/// the camera descends. Measured on a fifty light year leg with its far
/// end off the map: 1,300 points with a light year in view, 1.28 million
/// with a thousandth of one, **15.5 million with a ten-thousandth**, at 28
/// bytes a point and rebuilt on every [`REDASHED_AT`] step of the zoom.
///
/// **A backstop and not a style.** What a dash should *look* like is the
/// view's business and [`dash_of`] answers it; this only says how many of
/// them may be built, so the zooms where the view's own answer is sane —
/// which is every zoom that can see the leg — are left exactly as they
/// were. Four thousand and ninety-six is past any of them and comes to
/// 229 KB a leg at the worst, where the view's own answer came to 414 MB
/// and climbing.
const DASH_CAP: f32 = 4096.;

/// How far off its dashes may drift before a line is cut again
///
/// As a ratio of the dash the view asks for. Zooming is continuous and a mesh
/// is not: rebuilt on every scroll click a route of four hundred jumps would
/// be rebuilt while the wheel is still turning, and rebuilt never it would be
/// drawn for a zoom the camera has left. A third is under what the eye reads
/// as a change of spacing, which is the same line the rings are redrawn on
/// (`bodies::spawn::RELAID_AT`).
const REDASHED_AT: f32 = 1.33;

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
/// `charged` is one flag a jump, saying that jump was flown on a jet cone;
/// see [`spawn::charged`]. Every point of a jump takes that jump's colour,
/// so what the line says about a stretch is what the ship did on it.
///
/// `dash` is how long a dash and the gap after it are, in metres, which is
/// the camera's business rather than the route's — see [`dash_of`]. Nothing
/// is dashed at nothing, which is what a line spawned before the camera has
/// been asked is drawn with: the solid stretch alone, until [`trim`] cuts it
/// again with a dash the view has asked for.
pub(crate) fn legs(
    points: &[Vec3],
    shown: &[bool],
    charged: &[bool],
    dash: f32,
) -> LineList {
    if points.len() != shown.len() || charged.len() + 1 != points.len() {
        return LineList::default();
    }

    let mut drawn = LineList::default();
    for (leg, ends) in points.windows(2).enumerate() {
        let color = spawn::jump_color(charged[leg]);
        let draw = |from: Vec3, to: Vec3, drawn: &mut LineList| {
            drawn.points.push(from);
            drawn.points.push(to);
            drawn.colors.push(color);
            drawn.colors.push(color);
        };
        match (shown[leg], shown[leg + 1]) {
            (true, true) => draw(ends[0], ends[1], &mut drawn),
            (false, false) => {}
            // From whichever end is on the map, towards the one that is not.
            (here, _) => {
                let (from, to) =
                    if here { (ends[0], ends[1]) } else { (ends[1], ends[0]) };
                let leg = (to - from).length();
                let Some(along) = (to - from).try_normalize() else { continue };

                draw(from, from + along * leg * SOLID, &mut drawn);

                // Short enough that a couple of them fit the stretch given
                // over to them, wherever the view asks for more than that.
                // The view's dash is the one to draw where there is room for
                // it, being the one that reads at this zoom; a leg too short
                // to hold one would otherwise come back as the solid stub
                // alone and say nothing about going on past the map.
                let tail = leg * (1. - SOLID);
                let dash = dash.min(tail / 5.);
                // **And long enough that there is a bounded number of
                // them.** A dash is a share of the view ([`dash_of`]), and
                // the view goes all the way down to the inside of a system
                // — thousandths of a light year, and light seconds past
                // that — while a leg stays tens of light years long. The
                // count is the one over the other, so it grows without
                // bound as the camera descends: measured, one fifty light
                // year leg with its far end off the map came to 1.28
                // million points at a thousandth of a light year across
                // and **15.5 million at a ten-thousandth, which is 414
                // MB**. Zooming rebuilds it every [`REDASHED_AT`] step, so
                // what that ate was the whole of memory. Reported exactly
                // that way.
                //
                // [`DASH_CAP`] of them is the bound, and it lengthens the
                // dash rather than stopping the run part way: a line that
                // gave up half way along its leg would read as the route
                // ending there, which is the one thing the dashes are
                // drawn to deny.
                let dash = dash.max(tail / (2. * DASH_CAP));
                if dash <= 0. {
                    continue;
                }

                // A dash and the gap after it are the same length, so the run
                // reads as a dashed line rather than as marks left by one. It
                // starts a gap clear of the solid stretch and stops a gap
                // short of the system that is not drawn, so however many fit
                // is however many the leg has room for.
                let mut at = leg * SOLID + dash;
                while at + dash <= leg - dash {
                    draw(
                        from + along * at,
                        from + along * (at + dash),
                        &mut drawn,
                    );
                    at += dash + dash;
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
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
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
    const B: Vec3 =
        Vec3::new(20. * crate::map::space::LIGHT_YEAR as f32, 0., 0.);

    /// A dash for a camera taking in four hundred light years of sky
    ///
    /// Which is a view a plotted route is read at, and leaves a dash of ten
    /// light years: half a jump of `A` to `B`, so the legs below are long
    /// enough to hold a couple of them and short enough for the count to be
    /// read off by hand.
    fn dash() -> f32 {
        dash_of(400. * crate::map::space::LIGHT_YEAR)
    }

    /// One jump, unaided, for the tests that are about the geometry.
    fn plain(points: &[Vec3], shown: &[bool]) -> Vec<Vec3> {
        legs(points, shown, &vec![false; points.len() - 1], dash()).points
    }

    /// A leg running off the map is bounded however far the camera descends
    ///
    /// **The reported trouble: zooming into a system a route runs through
    /// ate all of memory.** A dash is a share of the view ([`dash_of`]) and
    /// the view runs down to the inside of a system — thousandths of a
    /// light year, and light seconds past that — while the leg it dashes
    /// stays tens of light years long. So the count was one over the other
    /// and unbounded. Measured on one fifty light year leg with its far end
    /// off the map, at 28 bytes a point:
    ///
    /// | in view | points | |
    /// |---|---|---|
    /// | 1 ly | 1,300 | 0.03 MB |
    /// | 0.001 ly | 1,278,380 | 34 MB |
    /// | 0.0001 ly | 15,517,262 | **414 MB** |
    ///
    /// and rebuilt on every [`REDASHED_AT`] step of the wheel.
    ///
    /// What is asserted is both halves: bounded where the view asks for the
    /// absurd, and *untouched* where it does not — the cap is a backstop
    /// and the view's own dash is still what a reader sees at any zoom that
    /// can make out the leg at all.
    #[test]
    fn a_leg_off_the_map_is_dashed_within_a_bound() {
        use crate::map::space::LIGHT_YEAR;
        let ly = LIGHT_YEAR as f32;
        let points = vec![Vec3::ZERO, Vec3::new(50. * ly, 0., 0.)];
        let shown = vec![true, false];
        let charged = vec![false];
        let drawn = |across: f64| {
            legs(&points, &shown, &charged, dash_of(across * LIGHT_YEAR))
        };

        // Two points a dash, and the solid stub on the front of it.
        let ceiling = (2. * DASH_CAP) as usize + 2;
        for across in [0.1, 0.01, 0.001, 0.0001, 0.000_001] {
            let line = drawn(across);
            assert!(
                line.points.len() <= ceiling,
                "{across} ly in view drew {} points",
                line.points.len(),
            );
            assert_eq!(
                line.colors.len(),
                line.points.len(),
                "a point uncoloured"
            );
        }

        // And a zoom that can see the leg is left alone: the view's own
        // dash, well under the cap, which is what keeps this a backstop
        // rather than a rule about how a route looks.
        let line = drawn(1.);
        assert!(
            line.points.len() < ceiling,
            "an ordinary zoom hit the cap: {} points",
            line.points.len(),
        );
        assert!(line.points.len() > 64, "the dashes went missing");

        // The run still reaches the far end whatever the cap did, a line
        // stopping half way along its leg reading as the route ending
        // there — which is the one thing the dashes are drawn to deny.
        let far = drawn(0.000_001);
        let end = far.points.iter().map(|at| at.x).fold(0.0f32, f32::max);
        assert!(
            end > 40. * ly,
            "the dashes stopped at {} of fifty light years",
            end / ly,
        );
    }

    /// A leg between two systems on the map is drawn whole
    #[test]
    fn a_leg_between_two_drawn_systems_is_one_line() {
        assert_eq!(plain(&[A, B], &[true, true]), vec![A, B]);
    }

    /// And each of its points carries its own jump's colour
    ///
    /// Per jump rather than per route, so a route reads as what the ship did
    /// on each stretch of it: the two points of a charged jump are blue and
    /// the two of an ordinary one white. Every vertex the leg draws takes
    /// that colour, dashes and all, or a leg trailing off the map would
    /// change colour halfway.
    #[test]
    fn each_jump_carries_its_own_colour() {
        let middle = Vec3::new(B.x / 2., 0., 0.);

        let line =
            legs(&[A, middle, B], &[true, true, true], &[true, false], dash());

        assert_eq!(line.points.len(), 4, "two jumps are four points");
        assert_eq!(line.colors.len(), line.points.len(), "a point uncoloured");
        assert_eq!(
            line.colors,
            vec![
                spawn::jump_color(true),
                spawn::jump_color(true),
                spawn::jump_color(false),
                spawn::jump_color(false),
            ],
        );

        // And a leg that trails off the map keeps one colour over its
        // solid stretch and every dash of it.
        let trailing = legs(&[A, B], &[true, false], &[true], dash());
        assert!(trailing.points.len() > 4, "the leg was not dashed");
        assert!(
            trailing.colors.iter().all(|&at| at == spawn::jump_color(true)),
            "a dash of a charged jump came out another colour",
        );
    }

    /// A leg between two systems that are not on the map is not drawn
    ///
    /// Neither end is there to be joined to anything, so a line between them
    /// says nothing about what the viewer can see.
    #[test]
    fn a_leg_between_two_undrawn_systems_is_nothing() {
        assert!(plain(&[A, B], &[false, false]).is_empty());
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
            let drawn = plain(&[A, B], &shown);

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

    /// A leg away from `A`, `far` light years off
    fn away(far: f32) -> Vec3 {
        Vec3::new(far * crate::map::space::LIGHT_YEAR as f32, 0., 0.)
    }

    /// The first dash of a leg out of the map, which is the pair drawn
    /// after the solid stretch
    fn first_dash(to: Vec3, across: f32) -> f32 {
        let drawn =
            legs(&[A, to], &[true, false], &[false], dash_of(across as f64))
                .points;
        assert!(drawn.len() > 4, "the leg came back undashed: {drawn:?}");
        (drawn[3] - drawn[2]).length()
    }

    /// A dash is the same length on any leg with the room for one
    ///
    /// It is a share of the view, so two legs read at one zoom are dashed
    /// alike however far each of them runs. Both of these are long enough
    /// to hold the dash this view asks for.
    #[test]
    fn a_dash_is_the_same_length_whatever_the_leg() {
        let view = 400. * crate::map::space::LIGHT_YEAR as f32;

        let long = first_dash(away(400.), view);
        let longer = first_dash(away(1000.), view);

        assert!(
            (long - longer).abs() < crate::map::space::LIGHT_YEAR as f32,
            "a dash drew {long} on one leg and {longer} on another",
        );
    }

    /// And it grows with the view, so zooming out leaves it still dashed
    ///
    /// The reported trouble, and the whole reason for measuring a dash
    /// against the view: held at half a light year, a dash was a clear mark
    /// with one stop in view and a hundredth of a pixel with the galaxy in
    /// view, so the stretch that says a route goes on past the map read as a
    /// faint solid line at the zoom a whole route is looked at from.
    ///
    /// Asked of the same leg at three zooms: the dash the wider view draws
    /// is the longer one, until the leg itself is what is left to share out.
    #[test]
    fn a_dash_grows_with_the_view() {
        let leg = away(50.);
        let close = first_dash(leg, 20. * crate::map::space::LIGHT_YEAR as f32);
        let out = first_dash(leg, 400. * crate::map::space::LIGHT_YEAR as f32);
        let galaxy =
            first_dash(leg, 20_000. * crate::map::space::LIGHT_YEAR as f32);

        assert!(out > close * 2., "{out} is not clear of {close}");

        // And where the view asks for more than the leg can hold, the leg
        // decides: a dash of a fifth of what trails off the map, which is
        // two dashes and their gaps. Still a share of what is drawn rather
        // than a length that vanished with the zoom -- the old half light
        // year was a fortieth of this leg and this is better than a tenth
        // of it.
        assert!(galaxy >= out, "{galaxy} fell under {out}");
        assert!(
            galaxy > leg.length() / 10.,
            "a dash of {galaxy} on a leg of {} is too small to read",
            leg.length(),
        );
    }

    /// A line is cut only where its stops and what is drawn agree in length
    #[test]
    fn a_line_nothing_is_known_about_is_not_drawn() {
        assert!(legs(&[A, B], &[true], &[false], dash()).points.is_empty());
        // And where the flags and the stops disagree in length.
        assert!(legs(&[A, B], &[true, true], &[], dash()).points.is_empty());
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
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
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

    /// The spyglass a trip through `places` leaves, and where it looked
    ///
    /// Driven the way the map drives it: the stops go on record, the trip is
    /// asked for by name, and [`frame_trip`] answers. Nothing is routed --
    /// where the stops stand is all the framing needs.
    fn framed(places: &[DVec3]) -> (Spyglass, Vec<Option<f32>>) {
        use galos_index::records::NameEntry;

        // **The stops are minted as addresses, not as places.** The names
        // table holds no position since it stopped holding the names an
        // address spells, and what it answers with is the middle of the
        // boxel the address names — so a fixture that wants a system ten
        // light years along asks for the boxel ten light years along. A
        // class `A` boxel is exactly 10 ly, so the geometry these tests
        // assert is the geometry they get.
        let entries: Vec<NameEntry> = places
            .iter()
            .enumerate()
            .map(|(at, place)| NameEntry {
                address: crate::testing::boxel_at([place.x, place.y, place.z]),
                name: format!("S{at}").into(),
                position: [place.x as f32, place.y as f32, place.z as f32],
            })
            .collect();
        let stops: Vec<String> =
            (0..places.len()).map(|at| format!("S{at}")).collect();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<Search>();
        app.add_message::<MoveCamera>();
        app.insert_resource(Names::reaching(entries, Vec::new()));
        app.insert_resource(Spyglass {
            radius: Spyglass::OPENING,
            clear: true,
            lock_camera: false,
            follow_camera: false,
        });
        app.add_systems(Update, frame_trip);

        app.world_mut().write_message(Search::Route {
            stops,
            range: "10".to_owned(),
            drive: Drive::Unaided,
            how: Routing::default(),
        });
        app.update();

        let held = app.world().resource::<Spyglass>();
        let spyglass = Spyglass {
            radius: held.radius,
            clear: held.clear,
            lock_camera: held.lock_camera,
            follow_camera: held.follow_camera,
        };
        let mut moves = app.world_mut().resource_mut::<Messages<MoveCamera>>();
        let framings = moves.drain().map(|asked| asked.framing).collect();
        (spyglass, framings)
    }

    /// The spyglass a trip through `places` leaves
    fn spyglass_for(places: &[DVec3]) -> Spyglass {
        framed(places).0
    }

    /// What reach a trip of `extent` light years pulls the spyglass out to
    ///
    /// Two stops either side of the middle, so the extent is the distance to
    /// each of them.
    fn reach_for(extent: f32) -> f32 {
        spyglass_for(&[
            DVec3::new(-(extent as f64), 0., 0.),
            DVec3::new(extent as f64, 0., 0.),
        ])
        .radius
    }

    /// A leg asked for and then answered, as a plot does it
    ///
    /// The row goes up first, carrying the two ends it was asked between —
    /// which is [`fetch::fetch_route`]'s, off the names table — and the
    /// answer lands in that row afterwards. So what a test about the order of
    /// the rows exercises is [`placed_at`], wherever the ask reaches it.
    fn asked_and_landed(app: &mut App, route: PlottedRoute) {
        let ask = Filter::Route {
            label: route.label.clone(),
            systems: route.filter().stops(),
            range: route.range.clone(),
            trip: route.trip.clone(),
            drive: route.drive,
            how: route.how,
            tune: route.tune,
        };
        let mut filters = app.world_mut().resource_mut::<Filters>();
        let at = placed_at(&ask, &filters);
        filters.searching(at, ask);
        app.world_mut().write_message(route);
        app.update();
    }

    /// A trip's legs read in the trip's order, whatever order they land in
    ///
    /// The reported trouble. The legs are walked at once and each row is
    /// written when its own walk finishes, so the rows came out in the order
    /// the router happened to answer -- an order nobody chose. A trip picked
    /// out as SOL, LAVE, DISO is those two legs in that order, or it is a set
    /// again.
    #[test]
    fn a_trips_legs_read_in_the_order_they_are_flown() {
        let stops = ["SOL", "LAVE", "DISO", "REORTE"];
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<PlottedRoute>();
        app.add_message::<UnflownLeg>();
        app.insert_resource(Names::reaching(Vec::new(), Vec::new()));
        app.init_resource::<Filters>();
        app.init_resource::<SelectedFilter>();
        app.add_systems(Update, plotted);
        let trip = stops.join(crate::map::route::ARROW);

        // Backwards, which is as good an order as any other: what decides it
        // is which walk finished first.
        for leg in [2, 0, 1] {
            asked_and_landed(
                &mut app,
                PlottedRoute {
                    label: format!(
                        "{}{}{}",
                        stops[leg],
                        crate::map::route::ARROW,
                        stops[leg + 1]
                    ),
                    systems: vec![leg as i64],
                    range: "10".to_owned(),
                    trip: Some(trip.clone()),
                    drive: Drive::Unaided,
                    how: Routing::default(),
                    took: std::time::Duration::ZERO,
                    tune: Tuning::default(),
                },
            );
        }

        let rows: Vec<String> = app
            .world()
            .resource::<Filters>()
            .iter()
            .map(|entry| entry.filter.name().to_owned())
            .collect();
        assert_eq!(rows, vec!["SOL -> LAVE", "LAVE -> DISO", "DISO -> REORTE"]);
    }

    /// And a second plot of the same trip keeps its legs to itself
    ///
    /// The reported trouble. Two trips asked for back to back through the
    /// same stops, the second for a ship reaching further. A trip is named
    /// for its stops and nothing else, so the second answered to the first's
    /// name and its legs were laid in among the first's rows: two plots read
    /// as one trip of four legs, in an order neither of them is flown in.
    #[test]
    fn a_second_plot_of_a_trip_keeps_its_legs_to_itself() {
        let stops = ["SOL", "LAVE", "DISO"];
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<PlottedRoute>();
        app.add_message::<UnflownLeg>();
        app.insert_resource(Names::reaching(Vec::new(), Vec::new()));
        app.init_resource::<Filters>();
        app.init_resource::<SelectedFilter>();
        app.add_systems(Update, plotted);
        let trip = stops.join(crate::map::route::ARROW);

        for range in ["10", "20"] {
            // Backwards again: what decides the order they land in is which
            // walk finished first, and neither plot is asked in order.
            for leg in [1, 0] {
                asked_and_landed(
                    &mut app,
                    PlottedRoute {
                        label: format!(
                            "{}{}{}",
                            stops[leg],
                            crate::map::route::ARROW,
                            stops[leg + 1]
                        ),
                        systems: vec![leg as i64],
                        range: range.to_owned(),
                        trip: Some(trip.clone()),
                        drive: Drive::Unaided,
                        how: Routing::default(),
                        took: std::time::Duration::ZERO,
                        tune: Tuning::default(),
                    },
                );
            }
        }

        let rows: Vec<(&str, Option<&str>)> = app
            .world()
            .resource::<Filters>()
            .iter()
            .map(|entry| (entry.filter.name(), entry.filter.range()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("SOL -> LAVE", Some("10")),
                ("LAVE -> DISO", Some("10")),
                ("SOL -> LAVE", Some("20")),
                ("LAVE -> DISO", Some("20")),
            ]
        );
    }

    /// And a route that is no leg of it is left at the end
    ///
    /// A faction, a hand-picked set, a route from some earlier trip: the trip
    /// orders its own legs and says nothing about anyone else's.
    #[test]
    fn a_route_that_is_no_leg_of_the_trip_goes_last() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<PlottedRoute>();
        app.add_message::<UnflownLeg>();
        app.insert_resource(Names::reaching(Vec::new(), Vec::new()));
        app.init_resource::<Filters>();
        app.init_resource::<SelectedFilter>();
        app.add_systems(Update, plotted);

        // The first is a leg of a trip and the second is a route of its own,
        // which is what a trip has nothing to say about.
        for (label, trip) in [
            ("SOL -> LAVE", Some("SOL -> LAVE -> DISO".to_owned())),
            ("WOLF 359 -> SIRIUS", None),
        ] {
            asked_and_landed(
                &mut app,
                PlottedRoute {
                    label: label.to_owned(),
                    systems: vec![1],
                    range: "10".to_owned(),
                    trip,
                    drive: Drive::Unaided,
                    how: Routing::default(),
                    took: std::time::Duration::ZERO,
                    tune: Tuning::default(),
                },
            );
        }

        let rows: Vec<String> = app
            .world()
            .resource::<Filters>()
            .iter()
            .map(|entry| entry.filter.name().to_owned())
            .collect();
        assert_eq!(rows, vec!["SOL -> LAVE", "WOLF 359 -> SIRIUS"]);
    }

    /// A trip is looked at whole, once, however many legs it has
    ///
    /// The legs land one at a time, so a camera pointed by each of them would
    /// be flung from leg to leg and left framing whichever answered last.
    #[test]
    fn a_trip_is_framed_once_over_the_whole_of_it() {
        let places =
            [DVec3::ZERO, DVec3::new(10., 0., 0.), DVec3::new(20., 0., 0.)];

        let (_, framings) = framed(&places);

        // The far stops stand ten light years off the middle one, which is
        // where the camera goes.
        assert_eq!(framings, vec![Some(10.)]);
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

        let spyglass = spyglass_for(&places);

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
        assert!(active(&Filters::default(), &[]).is_empty());
    }

    /// The last route plotted is the active one
    ///
    /// They are added in the order they land, so the end of the list is the
    /// newest, and a route just asked for is the one being looked at.
    #[test]
    fn the_last_route_plotted_is_the_active_one() {
        let (first, second) = (asking(&[1, 2]), asking(&[8, 9]));
        let filters = holding(&[first, second.clone()]);

        assert_eq!(active(&filters, &[]), vec![&second]);
    }

    /// Picking one out puts it in front of the last plotted
    ///
    /// Which is what pressing a route's panel says: this is the one I am
    /// working with, whichever landed most recently.
    #[test]
    fn a_route_picked_out_stands_in_front_of_the_last() {
        let (first, second) = (asking(&[1, 2]), asking(&[8, 9]));
        let filters = holding(&[first.clone(), second]);

        assert_eq!(
            active(&filters, std::slice::from_ref(&first)),
            vec![&first]
        );
    }

    /// And every route picked out stands in front, not one of them
    ///
    /// A trip is picked out as the one thing it was plotted as and is several
    /// routes: one leg in front with the rest held back would be the map
    /// drawing part of a trip nobody asked for.
    #[test]
    fn every_route_picked_out_stands_in_front() {
        let (first, second, other) =
            (asking(&[1, 2]), asking(&[2, 3]), asking(&[8, 9]));
        let filters = holding(&[first.clone(), second.clone(), other]);

        assert_eq!(
            active(&filters, &[first.clone(), second.clone()]),
            vec![&first, &second]
        );
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

        assert_eq!(active(&filters, &[closed]), vec![&held]);
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

        assert_eq!(active(&filters, &[]), vec![&older]);
    }

    /// Nor when it was the one picked out
    #[test]
    fn a_route_picked_out_and_turned_off_hands_it_back() {
        let (held, hidden) = (asking(&[1, 2]), asking(&[8, 9]));
        let mut filters = holding(&[held.clone(), hidden.clone()]);
        filters.toggle(1);

        assert_eq!(active(&filters, &[hidden]), vec![&held]);
    }

    /// With every route turned off there is none in front
    #[test]
    fn every_route_turned_off_leaves_none_active() {
        let mut filters = holding(&[asking(&[1, 2]), asking(&[8, 9])]);
        filters.toggle_all(&[0, 1]);

        assert!(active(&filters, &[]).is_empty());
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

        assert_eq!(active(&filters, &[]), vec![&asking(&[1, 2])]);
    }

    /// The active route is drawn at full strength and the rest behind it
    #[test]
    fn what_is_not_active_stands_behind_what_is() {
        assert_eq!(strength(true), 1.);
        assert!(strength(false) < strength(true));
        assert!(strength(false) > 0., "a route faded to nothing is no route");
    }

    /// A place `along` pixels down the screen's x axis
    fn px(along: f32) -> Option<Vec2> {
        Some(Vec2::new(along, 0.))
    }

    /// Systems on a route keep their marks while they can be told apart
    ///
    /// Zoomed in, a route is nodes with edges between them and every one of
    /// them is worth a mark: this is the case that must not be lost, whatever
    /// is done about the other one.
    #[test]
    fn a_route_whose_systems_stand_apart_keeps_every_mark() {
        let hops = [1, 2, 3, 4];
        let along =
            walk(&hops, &[1, 4], |address| px((address - 1) as f32 * 40.));

        for (address, left) in along {
            assert_eq!(left, 1., "system {address} lost its mark");
        }
    }

    /// And lose them where they have closed up into a stipple
    ///
    /// The reported trouble: pulled far enough back, a hundred jumps are a
    /// hundred marks a pixel apart, which is not a route with systems on it
    /// but a smear over the line that says where the route goes. Half a pixel
    /// to the jump is that, and nothing of it is left but the stops — which
    /// are what the user asked for and are worth a mark at any distance.
    #[test]
    fn a_route_whose_systems_merge_keeps_its_stops_alone() {
        let hops = [1, 2, 3, 4, 5];
        let along =
            walk(&hops, &[1, 5], |address| px((address - 1) as f32 * 0.5));

        for (address, left) in along {
            match address {
                1 | 5 => assert_eq!(left, 1., "stop {address} was thinned"),
                _ => assert_eq!(left, 0., "hop {address} was left drawn"),
            }
        }
    }

    /// Whatever the zoom, no two marks it leaves drawn are on top of each other
    ///
    /// The whole of what thinning is for, and the invariant rather than the
    /// arithmetic: a hop is drawn only where it stands clear of the last hop
    /// that was, so what survives is spread however tightly the route is
    /// packed. Walked at a spacing that leaves some drawn and some not, which
    /// is where an off-by-one in what the reckoning moves to would show.
    #[test]
    fn the_marks_left_drawn_are_never_on_top_of_one_another() {
        let hops: Vec<i64> = (1..=40).collect();
        // A pixel to the jump: forty systems over forty pixels.
        let along = walk(&hops, &[], |address| px((address - 1) as f32));

        let drawn: Vec<f32> = along
            .iter()
            .filter(|(_, left)| *left > 0.)
            .map(|(address, _)| (*address - 1) as f32)
            .collect();
        assert!(drawn.len() > 1, "nothing was left drawn at all");
        for two in drawn.windows(2) {
            assert!(
                two[1] - two[0] >= MERGED,
                "marks {} and {} are {} pixels apart",
                two[0],
                two[1],
                two[1] - two[0]
            );
        }
    }

    /// A hop faded to nothing does not move the reckoning on
    ///
    /// Measuring from the last system seen rather than the last one drawn
    /// would thin every hop after the first crowded pair, so a route through a
    /// crowded region into a bare one would come out of the crowd and never
    /// come back: here the hop two pixels along is dropped, and the one past
    /// it is measured from the mark that is actually on screen.
    #[test]
    fn a_dropped_hop_is_not_what_the_next_is_measured_from() {
        let along = walk(&[1, 2, 3], &[], |address| {
            px(match address {
                1 => 0.,
                2 => 2.,
                _ => 4.,
            })
        });

        assert_eq!(along[1].1, 0., "two pixels along is not a second mark");
        assert!(
            along[2].1 > 0.,
            "measured from the dropped hop, not from the drawn one: {}",
            along[2].1
        );
    }

    /// A system the map is not drawing is left whole and left out
    ///
    /// Off the frame there is nothing to measure and nothing drawn either
    /// way. Thinning it would be a mark suppressed for the moment it comes
    /// back on screen, and counting it would measure the next hop from
    /// somewhere the user cannot see.
    #[test]
    fn a_system_off_the_frame_is_not_measured_against() {
        let along = walk(&[1, 2, 3], &[], |address| match address {
            2 => None,
            1 => px(0.),
            _ => px(1.),
        });

        assert_eq!(along[1].1, 1., "an unseen system was thinned");
        assert_eq!(along[2].1, 0., "it was measured against all the same");
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
