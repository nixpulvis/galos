use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use bevy::math::DVec3;
use bevy::prelude::*;
use chrono::{DateTime, Utc};
use elite_journal::{
    // TODO: Fix these imports, they should all be in system.
    Allegiance,
    Government,
    system::Security,
};
use galos_index::meta::{Economies, NameEntry};
use std::collections::HashSet;

pub fn plugin(app: &mut App) {
    app.insert_resource(Spyglass {
        radius: Spyglass::OPENING,
        fetch: true,
        clear: true,
        lock_camera: false,
        follow_camera: true,
    });

    app.add_plugins(fetch::plugin);
    app.add_plugins(roundness::plugin);
    app.add_plugins(bodies::plugin);
    app.add_plugins(spawn::plugin);
    app.add_plugins(despawn::plugin);
    app.add_plugins(scale::plugin);
    app.add_plugins(labels::plugin);
    app.add_plugins(pointing::plugin);
    app.add_plugins(route::plugin);
    app.add_plugins(selection::plugin);
    app.add_plugins(filter::plugin);
    app.add_plugins(info::plugin);

    app.init_resource::<InReach>();
    app.init_resource::<Evictions>();
    app.init_resource::<PendingEvictions>();

    // Both ask the camera for something, and `orbit_camera` then works out
    // where it lands, so both have to have spoken by the time it runs.
    //
    // The two spyglass systems are the same link read in either direction, so
    // they are ordered rather than left to whichever Bevy picked: only one of
    // them is reachable at a time through the settings, and an order spelled
    // out is what keeps that from being the only thing holding them apart.
    app.add_systems(
        Update,
        (zoom_with_spyglass, reach_with_camera)
            .chain()
            .in_set(MapSet::Camera)
            .after(crate::camera::move_camera)
            .before(crate::camera::orbit_camera),
    );
    app.add_systems(Update, visibility.in_set(MapSet::Present));
    // After [`visibility`], which has already read the reach this frame, and in
    // the same set so the drop and the hide are decided together.
    app.add_systems(Update, evict.in_set(MapSet::Present).after(visibility));
    // After [`evict`], which marks what to drop; this drops a budgeted number
    // so the despawn churn a frame does stays bounded.
    app.add_systems(
        Update,
        drain_evictions.in_set(MapSet::Present).after(evict),
    );
}

/// Clones because a selection holds one, and a system may be selected before
/// the map has fetched it or after it has been despawned.
#[derive(Component, Clone)]
#[require(bodies::spawn::Strength)]
pub struct System {
    address: i64,
    name: String,
    /// Absolute galactic position, in light years
    ///
    /// The grid this is drawn in splits a position into a cell and an offset
    /// within it, which is what the renderer needs but an awkward thing to
    /// measure distances between. The database's own answer is kept here,
    /// undiminished, and everything that wants to know how far apart two
    /// systems are asks this instead of unpicking the split.
    position: [f64; 3],
    population: u64,
    allegiance: Option<Allegiance>,
    government: Option<Government>,
    security: Option<Security>,
    economies: Option<Economies>,
    /// The factions present in the system, by id
    ///
    /// What [`filter`] asks a system about, so ids rather than names: the
    /// question is put to every system drawn, every frame, and an integer
    /// compare is what that wants. A filter naming a faction has resolved it
    /// to an id already, since it was picked from a list.
    factions: Vec<i32>,
    /// How many bodies the system holds, and how many belts and rings
    ///
    /// What the system is made of rather than what the map has fetched of it,
    /// so a system says how much of itself is still unvisited. Either may
    /// stand [`None`]: the bodies come from the all-found tally, a nav beacon
    /// or the honk, and the belts and rings from the honk alone.
    body_count: Option<i32>,
    non_body_count: Option<i32>,
    /// How far the system reaches from its arrival star, in metres
    ///
    /// What the shell standing for the system is drawn at. Carried on every
    /// system rather than asked about the one the camera is nearest, so that
    /// two systems side by side are drawn the same size whichever of them the
    /// map happens to be looking into.
    ///
    /// [`None`] where the database has nothing on record and where nothing has
    /// asked. Both are the map unable to say how far the system reaches, and
    /// both are drawn at [`bodies::STAND_IN`].
    reach: Option<f32>,
    updated_at: DateTime<Utc>,
}

impl System {
    /// Where the system is, in light years from the galactic center
    ///
    /// A [`System`]'s fields are private to this module, and the camera is
    /// not in it. It has to measure from a system to descend into one.
    pub fn position(&self) -> DVec3 {
        DVec3::from(self.position)
    }

    /// What the system is called
    ///
    /// A [`System`]'s fields are private to this module, and a route names
    /// both of its ends in the bar, which is not.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How far the system reaches from its arrival star, in metres
    ///
    /// Never under [`bodies::STAND_IN`], which stands in for a system the map
    /// cannot say the size of and is a floor under one that says it is smaller
    /// than a mark. A star with nothing on record around it reaches a
    /// twenty-five thousandth of that, and a shell drawn there is a skin on
    /// the star rather than a mark around the system.
    pub fn reach(&self) -> f32 {
        self.reach.unwrap_or_default().max(bodies::STAND_IN)
    }
}

pub mod bodies;
pub mod despawn;
pub mod fetch;
pub mod filter;
pub mod info;
pub mod labels;
pub mod pointing;
pub mod roundness;
pub mod route;
pub mod scale;
pub mod selection;
pub mod spawn;

/// A global setting which controls the spyglass around the camera
#[derive(Resource)]
pub struct Spyglass {
    /// Ask the database for what is within the reach
    ///
    /// The two halves of what a spyglass does, this and [`Spyglass::clear`],
    /// and each is worth having without the other. Off, the map draws what it
    /// has and asks for nothing more, which is how to look at a sky that
    /// stops changing under you.
    pub fetch: bool,
    pub radius: f32,
    /// Clear away what the reach does not hold
    ///
    /// On to begin with, that being what looking through a spyglass is. Off,
    /// everything loaded is drawn however far off it lies, which is
    /// everywhere the camera has been rather than anywhere it is looking.
    pub clear: bool,
    /// Zoom the camera to whatever the reach is set to
    ///
    /// Only meaningful while [`Spyglass::follow_camera`] is off. The two are
    /// the same link read in opposite directions, and the camera cannot both
    /// be told where to stand and be asked where it is standing.
    pub lock_camera: bool,
    /// Reach as far as the camera can see, rather than as far as it is told
    ///
    /// On to begin with. What the camera is looking at is what the user is
    /// asking about, so a reach taken from it is the map fetching and drawing
    /// the view rather than a circle set beside it and kept in step by hand.
    ///
    /// It runs the whole way to [`Spyglass::CEILING`], which is the galaxy.
    /// Scrolling out that far asks for every system on record, and asks for it
    /// in one gesture, but a map that fetches less than it is showing is a map
    /// with a circle of stars in the middle of an empty window, and a reach
    /// that follows the view has said it will not do that. The fetch is
    /// throttled and the wheel is where the user says when to stop.
    pub follow_camera: bool,
}

impl Spyglass {
    /// How far the map reaches when it opens
    ///
    /// Also the least anything sets it to without being asked. A route
    /// between two neighbours spans a few light years, and a reach drawn in
    /// that far shows the two ends and nothing around them.
    pub const OPENING: f32 = 10.;

    /// The shortest reach worth offering, in light years
    ///
    /// Stars stand far enough apart that a shorter one shows the system at
    /// the middle of it and nothing else, so every setting under it draws the
    /// same picture.
    ///
    /// Measured over a sample of inhabited systems, counting what stands
    /// within reach of each: at 1, 2, 3 and 5 light years the middling answer
    /// is one system, which is the one being stood on. It first rises at 8,
    /// reaches 4 by 10, and 19 by 20.
    pub const FLOOR: f32 = 5.;

    /// The longest, in light years
    ///
    /// The galaxy is 105,700 across, so this reaches the whole of it. Only
    /// ever asked for by hand: everything it takes in is fetched and drawn,
    /// and at this reach that is every system on record.
    pub const CEILING: f32 = 1.1e5;

    /// The furthest the map reaches without being asked to
    ///
    /// What a route may pull it out to. Everything the spyglass takes in is
    /// fetched and spawned, so a reach set from the length of whatever was
    /// plotted is a query nobody asked the size of: measured against the
    /// systems on record, 200 light years takes in about fourteen thousand,
    /// 500 about twenty five, and 1000 about thirty two.
    ///
    /// Past this a route is drawn as a line with stars about the middle of it
    /// and none out at the ends, which is a poorer picture than the one asked
    /// for and a far better one than a map that has stopped answering.
    pub const UNASKED: f32 = 200.;
}

impl Spyglass {
    /// Whether the spyglass reaches `position`, from a camera centered on
    /// `center`
    ///
    /// Everything loaded while it is not clearing, there being no reach to be
    /// outside of then.
    ///
    /// Asked by whatever has to tell the two reasons a system is not drawn
    /// apart. Out of reach is the map saying it is not looking there;
    /// excluded by a filter is the user saying they are not interested, and
    /// what is drawn for a system they picked out by hand answers only the
    /// first.
    pub fn reaches(&self, center: DVec3, position: DVec3) -> bool {
        !self.clear || center.distance(position) <= self.radius as f64
    }

    /// Whether the camera stands wherever the reach puts it
    ///
    /// Locking holds the camera only while the camera is not itself what sets
    /// the reach, the two being the same link read in opposite directions.
    ///
    /// Asked by both ends of that: by what writes the camera's distance from
    /// the reach, and by what would otherwise let a scroll write it too. A
    /// zoom that is going to be written back over on the next frame is a
    /// camera that lurches and returns, so while this holds there is no zoom
    /// at all.
    pub fn locks_camera(&self) -> bool {
        self.lock_camera && !self.follow_camera
    }
}

/// How much of the sky is in reach, and how much of that the filters admit
///
/// Tallied by [`visibility`], which has already settled both for every system
/// on the map. Counted there rather than by whoever draws the number, so what
/// is said is the answer the map acted on rather than a second count taken
/// from the side. Both halves want the same two questions asked of the same
/// system, and that is the one place both are in hand.
///
/// In reach rather than loaded. What the spyglass has dragged in from wherever
/// the camera has been is not what the user is looking at.
#[derive(Resource, Default, PartialEq, Eq)]
pub struct InReach {
    /// Systems the spyglass reaches
    ///
    /// The sky only where the excluded systems are being fetched to be dimmed.
    /// At [`filter::DimTo`] zero the region is asked for what the filters
    /// admit and nothing else, so what is in reach and excluded is whatever
    /// was brought in before the filter was asked for and never despawned.
    /// That is a number about where the camera has been rather than about the
    /// sky, and [`crate::ui`] draws this only where it is the sky.
    pub total: usize,
    /// How many of those the filters admit
    ///
    /// Whole however the region was asked for. Narrowing the query asks for
    /// exactly the systems a filter admits, so what is left out of it is what
    /// this was never counting.
    pub admitted: usize,
}

/// Decide which systems are drawn
///
/// Two questions, and a system has to pass both: the spyglass has to reach it,
/// and the filters have to admit it. One answer written in one place, since
/// two systems each writing a `Visibility` would take turns undoing each
/// other.
///
/// Unless it is a stop one of the routes reaches from where the camera stands,
/// which is drawn whatever either of them says. What that mark is for is
/// finding the way on from here, and a spyglass narrower than the jump ahead,
/// or a filter that admits this system and not the next, would take away the
/// one thing that answers it.
///
/// A filtered system is only hidden where it is being dimmed to nothing.
/// Anywhere above that it is drawn faintly, which is the other half of what
/// [`filter`] is for: a faction read against the space around it.
///
/// Runs over every star every frame, so it writes only where the answer
/// actually changed. Assigning regardless would mark the whole sky as
/// changed each frame, and each star drags its name along with it.
pub fn visibility(
    camera: Query<&OrbitCamera>,
    mut systems: Query<(
        &System,
        &mut Visibility,
        Has<filter::Filtered>,
        Has<route::Hop>,
    )>,
    spyglass: Res<Spyglass>,
    dim: Res<filter::DimTo>,
    mut in_reach: ResMut<InReach>,
) {
    let Ok(camera) = camera.single() else { return };
    let excluded_are_drawn = dim.0 > 0.;

    let mut tally = InReach::default();
    for (system, mut visibility, filtered, hop) in &mut systems {
        let within =
            spyglass.reaches(camera.center, DVec3::from(system.position));
        if within {
            tally.total += 1;
            if !filtered {
                tally.admitted += 1;
            }
        }

        visibility.set_if_neq(
            if hop || (within && (!filtered || excluded_are_drawn)) {
                Visibility::Visible
            } else {
                Visibility::Hidden
            },
        );
    }

    // Only where it moved, so that a count nobody is watching does not mark
    // itself changed every frame.
    if *in_reach != tally {
        *in_reach = tally;
    }
}

/// How far past the reach a system is kept before it is dropped
///
/// Wider than the spyglass so a camera resting on the boundary does not spawn
/// and drop the same systems every frame. What falls beyond this is far enough
/// behind the camera that reading it again on return costs less than walking
/// its transform every frame it is gone.
pub(crate) const EVICT_MARGIN: f64 = 1.5;

/// Mark the systems the camera has left behind for dropping
///
/// [`visibility`] hides what the spyglass does not reach; this decides what to
/// take off the map altogether, so the resident set is what the camera is
/// looking at rather than everywhere it has ever looked. Without it a session's
/// cost climbs with every region a zoom-out pulled in and never falls, since
/// [`big_space`](crate::space) recomputes a transform for every resident system
/// each frame the camera moves, drawn or not.
///
/// The dropping itself is [`drain_evictions`]'s, under a per-frame budget:
/// despawning mutates the world and cannot leave the main thread, so a wide
/// region left behind all at once is spread over frames rather than stalling
/// one. This recomputes the marked set each frame it runs, so a system come
/// back into reach falls out of it before it is ever dropped.
///
/// Two grounds mark a system: the spyglass no longer reaches it (only while it
/// clears), or a filter excludes it and the dim is zero — which says draw
/// nothing for what is excluded rather than draw it faintly, so it is taken off
/// the map exactly as the out-of-reach ones are. A route's stops, a picked-out
/// system, and the system the camera is standing in are kept on either ground:
/// the first is how the way on is found, the second the user is holding onto by
/// hand, and the last carries the floating origin while the camera is inside
/// it. Everything else the galaxy holds — the route lines among them — is kept
/// by never being marked.
///
/// Marking a system forgets the surveys that vouched for its region, so a
/// camera coming back asks for it again rather than finding the region marked
/// held and empty.
pub fn evict(
    camera: Query<&OrbitCamera>,
    systems: Query<(Entity, &System, Has<route::Hop>)>,
    spyglass: Res<Spyglass>,
    selection: Res<selection::Selection>,
    holding: Res<bodies::spawn::HeldSystem>,
    filters: Res<filter::Filters>,
    dim: Res<filter::DimTo>,
    mut tasks: ResMut<fetch::FetchTasks>,
    mut pending: ResMut<PendingEvictions>,
) {
    let Ok(camera) = camera.single() else { return };

    let clears = spyglass.clear;
    let drops_filtered = dim.0 == 0.;
    if !clears && !drops_filtered {
        pending.0.clear();
        return;
    }

    let keep = spyglass.radius as f64 * EVICT_MARGIN;
    let now = Utc::now();
    let held: HashSet<i64> = selection.addresses().into_iter().collect();
    let inside = holding.of();

    let evicted: HashSet<Entity> = systems
        .iter()
        .filter(|(entity, system, hop)| {
            // A route's stops, a picked-out system, and the one the camera is
            // standing in are kept whatever the reach or the filters — the last
            // because the floating origin hangs off it while zoomed in, so
            // dropping it would take the camera down with it and leave the map
            // with no origin to draw from.
            if *hop || held.contains(&system.address) || Some(*entity) == inside
            {
                return false;
            }
            let out_of_reach = clears
                && camera.center.distance(DVec3::from(system.position)) > keep;
            let excluded = drops_filtered && !filters.admit(system, now);
            out_of_reach || excluded
        })
        .map(|(entity, _, _)| entity)
        .collect();
    if evicted.is_empty() {
        pending.0.clear();
        return;
    }

    // Shrink each survey to what the drop has left rather than forgetting it
    // whole. A survey the reach has eaten into is clamped to the kept sphere,
    // so the region still in view stays surveyed — forgetting it would have
    // the map re-fetch and re-spawn what it already holds, every frame a zoom
    // drops the systems it left behind — while a return to what was dropped
    // still asks again. Only while clearing: a filter drop leaves every region
    // as resident as it was, and the filters forget their own surveys.
    if clears {
        tasks.surveyed.retain_mut(|survey| {
            match survey.asked.clamp_to(camera.center, keep) {
                Some(asked) => {
                    survey.asked = asked;
                    true
                }
                None => false,
            }
        });
    }
    pending.0 = evicted;
}

/// How many systems the evictor may despawn in one frame
///
/// The companion to [`super::spawn::SPAWN_BUDGET`]. A big eviction — a zoom-out
/// pulled a wide region in and the camera has since left it — is spread over
/// frames so the structural churn a frame does stays bounded.
const EVICT_BUDGET: usize = 4096;

/// Despawn a budgeted number of the systems [`evict`] has marked
///
/// The batch is detached from the galaxy in one pass and then despawned, which
/// is what keeps eviction off the quadratic a naive despawn falls into: a child
/// leaving its parent one at a time rescans and reshifts the parent's whole
/// child list each time (see [`super::despawn`]), so dropping thousands would
/// cost millions. Replacing the child list with the keepers empties the batch's
/// links first, so each drop is O(1); a detached system then despawns with no
/// parent left to unlink from, and its shell and labels go with it.
fn drain_evictions(
    galaxy: Res<crate::space::Galaxy>,
    children: Query<&Children>,
    mut pending: ResMut<PendingEvictions>,
    mut evictions: ResMut<Evictions>,
    mut commands: Commands,
) {
    evictions.last = 0;
    if pending.0.is_empty() {
        return;
    }
    let Ok(children) = children.get(galaxy.0) else {
        return;
    };

    let batch: HashSet<Entity> =
        pending.0.iter().copied().take(EVICT_BUDGET).collect();
    for entity in &batch {
        pending.0.remove(entity);
    }

    let keepers: Vec<Entity> =
        children.iter().filter(|entity| !batch.contains(entity)).collect();
    commands.entity(galaxy.0).replace_children(&keepers);
    for entity in &batch {
        commands.entity(*entity).despawn();
    }

    evictions.last = batch.len();
    evictions.total += batch.len() as u64;
}

/// The systems [`evict`] has marked to drop, waiting on the budget.
#[derive(Resource, Default)]
pub struct PendingEvictions(HashSet<Entity>);

impl PendingEvictions {
    /// How many systems are waiting to be dropped, for the diagnostics panel.
    pub fn queued(&self) -> usize {
        self.0.len()
    }
}

/// What the evictor has dropped, for the diagnostics panel to read.
#[derive(Resource, Default)]
pub struct Evictions {
    /// How many systems the last pass dropped.
    pub last: usize,
    /// How many have been dropped since the map opened.
    pub total: u64,
}

pub fn zoom_with_spyglass(
    spyglass: Res<Spyglass>,
    mut camera: Query<&mut OrbitCamera>,
) {
    if spyglass.locks_camera() {
        if let Ok(mut camera) = camera.single_mut() {
            camera.target_radius = spyglass.radius * 3.;
        }
    }
}

/// How much of the view is left empty around the reach, in hundredths
///
/// The reach is a sphere about what the camera looks at, and stars stop at its
/// surface. Reaching exactly as far as the camera sees stands that surface on
/// the edge of the screen, so the last stars sit hard against the frame with
/// the sky ending along it. Holding it inside the view leaves them short of
/// the edge with empty space beyond, which is what the edge of a reach looks
/// like rather than what the edge of a window looks like.
///
/// Unsigned, and taken off a hundred, so this can only ever bring the reach in
/// from the edge of the view. A margin that would push it past the edge is a
/// margin that does not compile.
///
/// Under thirteen, and not by accident. A camera stood back over a route takes
/// in [`crate::camera::FRAMING_MARGIN`] more than the route, which is a
/// thirteenth of the view left over, so a margin larger than that eats through
/// the room the framing left and puts the ends of every plotted route outside
/// the reach that was taken from the camera framing it.
const FOLLOW_MARGIN: u32 = 10;

/// Reach not quite as far as the camera can see
///
/// [`zoom_with_spyglass`] read the other way. That one stands the camera back
/// to take in the reach; this one takes the reach from where the camera is
/// already standing, so scrolling out fetches and draws more of the sky and
/// scrolling in narrows to what is being looked at.
///
/// Short of what the camera sees by [`FOLLOW_MARGIN`], so that the sky stops
/// inside the window rather than along the edge of it.
///
/// The target rather than the radius the camera has reached, so that the reach
/// is settled the moment a scroll or a move asks for it and the systems are on
/// their way while the camera is still travelling. Reading the radius instead
/// would move the reach a little every frame of a zoom, and every step of it
/// is a region to be fetched.
pub fn reach_with_camera(
    mut spyglass: ResMut<Spyglass>,
    camera: Query<&OrbitCamera>,
    lens: Query<&Projection>,
) {
    if !spyglass.follow_camera {
        return;
    }
    let Ok(camera) = camera.single() else { return };

    // The margin here rather than inside `framed`, which answers what the
    // camera takes in and would be answering something else with room left
    // over folded into it. How far short of that to stop is the spyglass's to
    // say.
    let seen = crate::camera::framed(camera.target_radius, lens.single().ok());
    let inside = seen * (100 - FOLLOW_MARGIN) as f32 / 100.;
    let reach = inside.clamp(Spyglass::FLOOR, Spyglass::CEILING);

    // Only where it moved. Nothing watches this resource for changes today,
    // and writing the same number every frame is how that stops being true
    // without anyone meaning it to.
    if spyglass.radius != reach {
        spyglass.radius = reach;
    }
}

/// Where a system named in the resident table sits, in light years
///
/// The names table holds only placed systems, so this is always an answer;
/// kept as an [`Option`] for the callers that still ask it as a question.
pub fn system_to_vec(entry: &NameEntry) -> Option<DVec3> {
    Some(DVec3::new(
        entry.position[0] as f64,
        entry.position[1] as f64,
        entry.position[2] as f64,
    ))
}

impl From<&NameEntry> for System {
    /// A system as the names table alone gives it: named and placed, with no
    /// political columns. Those come from the populated table once a fetch
    /// draws it, so a system picked out of a search is this until then.
    fn from(entry: &NameEntry) -> System {
        System {
            address: entry.address,
            name: entry.name.clone(),
            position: [
                entry.position[0] as f64,
                entry.position[1] as f64,
                entry.position[2] as f64,
            ],
            population: 0,
            allegiance: None,
            government: None,
            security: None,
            economies: None,
            factions: Vec::new(),
            body_count: None,
            non_body_count: None,
            reach: None,
            updated_at: Utc::now(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::DateTime;

    /// A system with nothing on record but the address that names it
    ///
    /// Shared by the modules under [`super`], each of which tests something
    /// that keys off a system without caring what is in it. Fields are set
    /// on the way out by whichever test cares about one.
    pub(crate) fn system(address: i64) -> System {
        System {
            address,
            name: format!("Test {address}"),
            position: [0., 0., 0.],
            population: 0,
            allegiance: None,
            government: None,
            security: None,
            economies: None,
            factions: vec![],
            body_count: None,
            non_body_count: None,
            reach: None,
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    /// A system called `name`
    ///
    /// For whoever is testing what gets said about a system rather than what
    /// is done with it. A [`System`]'s fields are private to this module, so
    /// the name has to be set from in here.
    pub(crate) fn named(address: i64, name: &str) -> System {
        let mut system = system(address);
        system.name = name.to_owned();
        system
    }

    /// A system placed at `at`, in light years, for the evictor's reach tests.
    fn placed(address: i64, at: DVec3) -> System {
        let mut system = system(address);
        system.position = [at.x, at.y, at.z];
        system
    }

    /// The evictor drops what the reach and its margin no longer hold, keeps a
    /// route's stops and a picked-out system whatever the reach, and clamps the
    /// surveys it reached into to what it still holds — keeping the region in
    /// view surveyed so it is not needlessly re-fetched, while a return to what
    /// was dropped asks again.
    #[test]
    fn the_evictor_drops_the_far_and_keeps_the_held() {
        use crate::camera::OrbitCamera;
        use crate::systems::fetch::{
            FetchIndex, FetchTasks, tests::surveyed_at,
        };
        use crate::systems::route::Hop;
        use crate::systems::selection::{Picked, Selection};

        let mut app = App::new();
        app.insert_resource(Spyglass {
            radius: 10.,
            fetch: true,
            clear: true,
            lock_camera: false,
            follow_camera: true,
        });
        app.init_resource::<FetchTasks>();
        app.init_resource::<Evictions>();
        app.init_resource::<filter::Filters>();
        app.init_resource::<filter::DimTo>();
        app.init_resource::<bodies::spawn::HeldSystem>();
        app.init_resource::<PendingEvictions>();
        app.add_systems(Update, (evict, drain_evictions).chain());

        // Two regions the map thinks it holds. The wide one reaches past the 15
        // ly kept, so the drop eats into it and it is clamped in to what is
        // still held; the near one is wholly resident and kept unchanged, so a
        // return to it is not asked again. Wide added first: a narrow survey
        // added after a wider one at the same place is absorbed by it, so the
        // order keeps both on record.
        let (wide_survey, wide_at) = surveyed_at(0, 100, 0);
        let (near_survey, near_at) = surveyed_at(0, 10, 0);
        {
            let mut tasks = app.world_mut().resource_mut::<FetchTasks>();
            tasks.surveyed(wide_survey.clone(), wide_at);
            tasks.surveyed(near_survey.clone(), near_at);
        }

        // Address 4 is picked out by hand; it must survive being far off.
        let mut selection = Selection::default();
        selection
            .pick(Picked::System(placed(4, DVec3::new(60., 0., 0.))), false);
        app.insert_resource(selection);

        let galaxy = app.world_mut().spawn_empty().id();
        app.insert_resource(crate::space::Galaxy(galaxy));

        app.world_mut().spawn(OrbitCamera { center: DVec3::ZERO, ..default() });

        // A non-system child of the galaxy — stand-in for a route line — must
        // survive: the evictor touches far systems, not all the galaxy holds.
        let line = app.world_mut().spawn(ChildOf(galaxy)).id();

        // radius 10 * margin 1.5 = kept within 15 ly.
        let spawn = |app: &mut App, system: System| {
            app.world_mut().spawn((system, ChildOf(galaxy))).id()
        };
        let near = spawn(&mut app, placed(1, DVec3::new(5., 0., 0.)));
        let band = spawn(&mut app, placed(2, DVec3::new(12., 0., 0.)));
        let far = spawn(&mut app, placed(3, DVec3::new(50., 0., 0.)));
        let held = spawn(&mut app, placed(4, DVec3::new(60., 0., 0.)));
        let hop = app
            .world_mut()
            .spawn((
                placed(5, DVec3::new(70., 0., 0.)),
                Hop::Next,
                ChildOf(galaxy),
            ))
            .id();

        app.update();

        let alive = |e| app.world().get_entity(e).is_ok();
        assert!(alive(near), "dropped a system inside the reach");
        assert!(alive(band), "dropped a system inside the margin");
        assert!(alive(held), "dropped a picked-out system");
        assert!(alive(hop), "dropped a route's stop");
        assert!(alive(line), "dropped a non-system the galaxy held");
        assert!(!alive(far), "kept a system past the margin");
        let surveys = &app.world().resource::<FetchTasks>().surveyed;
        let radii: Vec<i32> = surveys
            .iter()
            .filter_map(|survey| match survey.asked {
                FetchIndex::Region(_, radius, ..) => Some(radius),
                _ => None,
            })
            .collect();
        // The near region is wholly held, so its survey stays as it was.
        assert!(radii.contains(&10), "forgot the region still held: {radii:?}");
        // The wide one is clamped to the kept sphere, not forgotten: 10 * 1.5.
        assert!(
            radii.contains(&15),
            "did not clamp the eaten survey: {radii:?}"
        );
        // And nothing still claims a reach past what the drop left.
        assert!(
            radii.iter().all(|radius| *radius <= 15),
            "kept a survey reaching past the drop: {radii:?}"
        );
    }

    /// At zero dim a filter drops what it excludes off the map, exactly as the
    /// spyglass drops what it does not reach, rather than leaving it dimmed to
    /// nothing. The reach plays no part here — the spyglass does not clear — so
    /// the filter alone decides.
    #[test]
    fn a_filter_at_zero_dim_evicts_what_it_excludes() {
        use crate::camera::OrbitCamera;
        use crate::systems::fetch::FetchTasks;
        use crate::systems::filter::{DimTo, Filter, Filters};
        use crate::systems::selection::Selection;

        let mut app = App::new();
        app.insert_resource(Spyglass {
            radius: 10.,
            fetch: true,
            clear: false,
            lock_camera: false,
            follow_camera: true,
        });
        app.init_resource::<FetchTasks>();
        app.init_resource::<Evictions>();
        app.init_resource::<Selection>();
        app.init_resource::<bodies::spawn::HeldSystem>();

        // Admit only system 1; the dim is zero, so 2 is dropped, not dimmed.
        let mut filters = Filters::default();
        filters
            .add(Filter::Systems { label: "picked".into(), systems: vec![1] });
        app.insert_resource(filters);
        app.insert_resource(DimTo(0.));
        app.init_resource::<PendingEvictions>();
        app.add_systems(Update, (evict, drain_evictions).chain());

        let galaxy = app.world_mut().spawn_empty().id();
        app.insert_resource(crate::space::Galaxy(galaxy));
        app.world_mut().spawn(OrbitCamera { center: DVec3::ZERO, ..default() });

        let here = DVec3::new(1., 0., 0.);
        let admitted =
            app.world_mut().spawn((placed(1, here), ChildOf(galaxy))).id();
        let excluded =
            app.world_mut().spawn((placed(2, here), ChildOf(galaxy))).id();

        app.update();

        assert!(
            app.world().get_entity(admitted).is_ok(),
            "dropped a system the filter admits"
        );
        assert!(
            app.world().get_entity(excluded).is_err(),
            "kept a system no filter admits at zero dim"
        );
    }

    /// The system the camera is standing in is never evicted, even far off and
    /// excluded by a filter at zero dim, because the floating origin hangs off
    /// it while zoomed in — dropping it would leave the map with no origin.
    #[test]
    fn the_system_the_camera_stands_in_survives_eviction() {
        use crate::camera::OrbitCamera;
        use crate::systems::bodies::spawn::HeldSystem;
        use crate::systems::fetch::FetchTasks;
        use crate::systems::filter::{DimTo, Filter, Filters};
        use crate::systems::selection::Selection;

        let mut app = App::new();
        app.insert_resource(Spyglass {
            radius: 10.,
            fetch: true,
            clear: true,
            lock_camera: false,
            follow_camera: true,
        });
        app.init_resource::<FetchTasks>();
        app.init_resource::<Evictions>();
        app.init_resource::<Selection>();

        // A filter admitting only something not here, at zero dim: the system
        // below is both far past the margin and excluded, so it would be
        // dropped on either ground were it not the one held.
        let mut filters = Filters::default();
        filters.add(Filter::Systems {
            label: "elsewhere".into(),
            systems: vec![9],
        });
        app.insert_resource(filters);
        app.insert_resource(DimTo(0.));

        let galaxy = app.world_mut().spawn_empty().id();
        app.insert_resource(crate::space::Galaxy(galaxy));
        app.world_mut().spawn(OrbitCamera { center: DVec3::ZERO, ..default() });

        let inside = app
            .world_mut()
            .spawn((placed(1, DVec3::new(500., 0., 0.)), ChildOf(galaxy)))
            .id();
        app.insert_resource(HeldSystem::holding(inside));
        app.init_resource::<PendingEvictions>();
        app.add_systems(Update, (evict, drain_evictions).chain());

        app.update();

        assert!(
            app.world().get_entity(inside).is_ok(),
            "evicted the system the camera is standing in"
        );
    }

    /// A system tallied as holding `bodies` bodies and `non_bodies` belts
    ///
    /// Shared for the same reason [`named`] is. Either count is passed as it
    /// stands, since a system may be tallied for one and not the other.
    pub(crate) fn tallied(
        address: i64,
        bodies: Option<i32>,
        non_bodies: Option<i32>,
    ) -> System {
        let mut system = system(address);
        system.body_count = bodies;
        system.non_body_count = non_bodies;
        system
    }

    /// A system last heard from `secs` after the epoch
    ///
    /// Shared for the same reason [`named`] is. A moment is set from in here
    /// or not at all, and what the map does with one is tested from wherever
    /// it reads it.
    pub(crate) fn heard(address: i64, secs: i64) -> System {
        let mut system = system(address);
        system.updated_at =
            DateTime::from_timestamp(secs, 0).expect("a moment");
        system
    }

    /// A system at `away` light years from the origin, on the x axis
    ///
    /// Shared for the same reason [`named`] is: a position is set from in
    /// here or not at all, and what is drawn about a system is tested from
    /// wherever it is drawn.
    pub(crate) fn at(address: i64, away: f64) -> System {
        let mut system = system(address);
        system.position = [away, 0., 0.];
        system
    }

    /// A system reaching `metres` from its arrival star
    ///
    /// Shared for the same reason [`named`] is. Systems run from a light
    /// second across to a fifth of a light year, so anything drawing one whole
    /// is tested over that range.
    pub(crate) fn reaching(address: i64, away: f64, metres: f32) -> System {
        let mut system = at(address, away);
        system.reach = Some(metres);
        system
    }

    /// A camera that can say how large its view is and how wide it opens
    ///
    /// Both are the render target's to answer, and a test brings no render
    /// target up, so they are written in by hand. Without them a camera
    /// answers nothing for its viewport and everything sized against one
    /// stands down.
    ///
    /// Shared because everything the camera decides the size of is tested the
    /// same way: hand it a view, step the world, and read what came out.
    pub(crate) fn seeing() -> Camera {
        let lens = PerspectiveProjection::default();
        Camera {
            computed: bevy::camera::ComputedCameraValues {
                target_info: Some(bevy::camera::RenderTargetInfo {
                    physical_size: UVec2::new(800, 600),
                    scale_factor: 1.,
                }),
                clip_from_view: Projection::Perspective(lens)
                    .get_clip_from_view(),
                ..default()
            },
            ..default()
        }
    }

    /// A world holding one camera at the origin, and nothing else
    fn map(radius: f32, clear: bool, dim: f32) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(Spyglass {
            radius,
            clear,
            fetch: false,
            lock_camera: false,
            follow_camera: false,
        });
        app.insert_resource(filter::DimTo(dim));
        app.init_resource::<InReach>();
        app.world_mut().spawn(OrbitCamera::default());
        app.add_systems(Update, visibility);
        app
    }

    /// Whether `entity` came out drawn
    fn drawn(app: &App, entity: Entity) -> bool {
        *app.world().entity(entity).get::<Visibility>().unwrap()
            != Visibility::Hidden
    }

    /// The spyglass reaches what is inside its radius and no further
    #[test]
    fn the_spyglass_reaches_what_is_within_it() {
        let spyglass = Spyglass {
            radius: 10.,
            fetch: false,
            clear: true,
            lock_camera: false,
            follow_camera: false,
        };
        let center = DVec3::ZERO;

        assert!(spyglass.reaches(center, DVec3::new(5., 0., 0.)));
        assert!(spyglass.reaches(center, DVec3::new(10., 0., 0.)));
        assert!(!spyglass.reaches(center, DVec3::new(11., 0., 0.)));
    }

    /// Not clearing, it reaches everything loaded however far off
    #[test]
    fn a_spyglass_that_does_not_clear_reaches_whatever_is_loaded() {
        let spyglass = Spyglass {
            radius: 10.,
            fetch: false,
            clear: false,
            lock_camera: false,
            follow_camera: false,
        };

        assert!(spyglass.reaches(DVec3::ZERO, DVec3::new(5e4, 0., 0.)));
    }

    /// The spyglass draws what it reaches and hides the rest
    #[test]
    fn the_spyglass_decides_what_is_drawn() {
        let mut app = map(10., true, 0.15);
        let near =
            app.world_mut().spawn((at(1, 5.), Visibility::default())).id();
        let far =
            app.world_mut().spawn((at(2, 50.), Visibility::default())).id();

        app.update();

        assert!(drawn(&app, near));
        assert!(!drawn(&app, far));
    }

    /// A spyglass that does not clear draws everything loaded, however far off
    #[test]
    fn a_spyglass_that_does_not_clear_draws_everything() {
        let mut app = map(10., false, 0.15);
        let far =
            app.world_mut().spawn((at(1, 5e4), Visibility::default())).id();

        app.update();

        assert!(drawn(&app, far));
    }

    /// A filtered system is still drawn, so long as it is being dimmed
    ///
    /// Which is the whole of why filters dim rather than hide: a faction is
    /// read against the space around it.
    #[test]
    fn a_filtered_system_is_drawn_to_be_dimmed() {
        let mut app = map(10., true, 0.15);
        let excluded = app
            .world_mut()
            .spawn((at(1, 5.), Visibility::default(), filter::Filtered))
            .id();

        app.update();

        assert!(drawn(&app, excluded));
    }

    /// Dimming to nothing takes the excluded systems off the map
    ///
    /// A star faded to nothing is still a star being drawn, and still one the
    /// pointer can land on, so this is hidden rather than merely invisible.
    #[test]
    fn dimming_to_nothing_hides_what_is_filtered() {
        let mut app = map(10., true, 0.);
        let excluded = app
            .world_mut()
            .spawn((at(1, 5.), Visibility::default(), filter::Filtered))
            .id();
        let included =
            app.world_mut().spawn((at(2, 5.), Visibility::default())).id();

        app.update();

        assert!(!drawn(&app, excluded));
        assert!(drawn(&app, included));
    }

    /// What the tally came to
    fn counted(app: &App) -> (usize, usize) {
        let in_reach = app.world().resource::<InReach>();
        (in_reach.admitted, in_reach.total)
    }

    /// The tally counts what is in reach rather than what has been loaded
    ///
    /// The spyglass drags systems in from everywhere the camera has been,
    /// and those are not what the user is looking at.
    #[test]
    fn the_tally_counts_what_is_in_reach() {
        let mut app = map(10., true, 0.15);
        app.world_mut().spawn((at(1, 5.), Visibility::default()));
        app.world_mut().spawn((at(2, 7.), Visibility::default()));
        app.world_mut().spawn((at(3, 5e3), Visibility::default()));

        app.update();

        assert_eq!(counted(&app), (2, 2));
    }

    /// A filter takes systems out of the count without taking them out of
    /// reach
    #[test]
    fn the_tally_says_how_many_a_filter_admits() {
        let mut app = map(10., true, 0.15);
        app.world_mut().spawn((at(1, 5.), Visibility::default()));
        app.world_mut().spawn((
            at(2, 5.),
            Visibility::default(),
            filter::Filtered,
        ));

        app.update();

        assert_eq!(counted(&app), (1, 2));
    }

    /// The tally counts the excluded, which are drawn along with the rest
    ///
    /// A count over what is drawn would come to the same number twice and say
    /// nothing at all, since what a filter excludes is dimmed rather than
    /// taken away. What is asked of it is how much of the sky is getting
    /// through, which only the marks answer.
    #[test]
    fn the_tally_counts_the_excluded_among_the_drawn() {
        let mut app = map(10., true, 0.15);
        app.world_mut().spawn((at(1, 5.), Visibility::default()));
        app.world_mut().spawn((
            at(2, 5.),
            Visibility::default(),
            filter::Filtered,
        ));
        app.world_mut().spawn((
            at(3, 5.),
            Visibility::default(),
            filter::Filtered,
        ));

        app.update();

        assert_eq!(counted(&app), (1, 3));
    }

    /// Dimming to nothing still counts what it took off the map
    ///
    /// Being hidden is not being gone. A system the spyglass reaches is in
    /// reach whether or not a filter is letting it be drawn, so the two
    /// numbers go on being two numbers here rather than collapsing into one
    /// the moment the sky is put out.
    ///
    /// Which is what the tally is for and not what it is read for: the map
    /// stops fetching the excluded at this opacity, so the systems counted
    /// here are the ones that happened to be loaded already, and the bar says
    /// only how many are getting through. See [`InReach::total`].
    #[test]
    fn the_tally_holds_up_when_nothing_excluded_is_drawn() {
        let mut app = map(10., true, 0.);
        app.world_mut().spawn((at(1, 5.), Visibility::default()));
        app.world_mut().spawn((
            at(2, 5.),
            Visibility::default(),
            filter::Filtered,
        ));
        app.world_mut().spawn((
            at(3, 5.),
            Visibility::default(),
            filter::Filtered,
        ));

        app.update();

        assert_eq!(counted(&app), (1, 3));
    }

    /// The spyglass still hides what it cannot reach, filter or no filter
    #[test]
    fn a_filtered_system_out_of_reach_stays_hidden() {
        let mut app = map(10., true, 0.15);
        let far = app
            .world_mut()
            .spawn((at(1, 50.), Visibility::default(), filter::Filtered))
            .id();

        app.update();

        assert!(!drawn(&app, far));
    }

    /// A spyglass that does not clear leaves the filters clearing
    ///
    /// The two answer different questions, and the one that draws everything
    /// loaded has nothing to say about what was asked for.
    #[test]
    fn a_spyglass_that_does_not_clear_still_honours_a_filter() {
        let mut app = map(10., false, 0.);
        let excluded = app
            .world_mut()
            .spawn((at(1, 5e4), Visibility::default(), filter::Filtered))
            .id();

        app.update();

        assert!(!drawn(&app, excluded));
    }

    /// A world holding a camera `back` light years out and the two systems
    /// that read it
    ///
    /// No lens is spawned, so the default half angle answers, which is what a
    /// window as wide as it is tall would give anyway.
    fn linked(back: f32, lock_camera: bool, follow_camera: bool) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(Spyglass {
            radius: Spyglass::OPENING,
            fetch: false,
            clear: true,
            lock_camera,
            follow_camera,
        });
        app.world_mut().spawn(OrbitCamera {
            radius: back,
            target_radius: back,
            ..default()
        });
        app.add_systems(
            Update,
            (zoom_with_spyglass, reach_with_camera).chain(),
        );
        app
    }

    /// How far the spyglass reaches, and how far back the camera stands
    fn linkage(app: &mut App) -> (f32, f32) {
        let reach = app.world().resource::<Spyglass>().radius;
        let back = app
            .world_mut()
            .query::<&OrbitCamera>()
            .single(app.world())
            .unwrap()
            .target_radius;
        (reach, back)
    }

    /// What the reach comes to at a camera `back` light years out
    fn following(back: f32) -> f32 {
        crate::camera::framed(back, None) * (100 - FOLLOW_MARGIN) as f32 / 100.
    }

    /// Following the camera, the reach is what the camera can see
    #[test]
    fn the_reach_follows_what_the_camera_sees() {
        let mut app = linked(100., false, true);

        app.update();

        let (reach, _) = linkage(&mut app);
        assert!((reach - following(100.)).abs() < 1e-3, "reached {reach}");
        assert!(reach != Spyglass::OPENING, "left where it opened");
    }

    /// The reach stops inside the view rather than along the edge of it
    ///
    /// Stars stop at the surface of the reach, and a surface standing on the
    /// edge of the screen is a sky that ends where the window does.
    #[test]
    fn the_reach_stops_short_of_what_the_camera_sees() {
        let mut app = linked(100., false, true);

        app.update();

        let (reach, _) = linkage(&mut app);
        let seen = crate::camera::framed(100., None);
        assert!(reach < seen, "reached {reach} of the {seen} seen");
    }

    /// A route framed by the camera is still held whole by the reach
    ///
    /// Plotting a route stands the camera back over it, and with the reach
    /// following the camera that framing is what sets the reach. The room the
    /// framing leaves has to outlast the room [`FOLLOW_MARGIN`] takes, or the
    /// ends of every route plotted would fall outside the reach that was taken
    /// from the camera framing it.
    #[test]
    fn a_framed_route_is_held_by_the_reach_taken_from_it() {
        for extent in [10., 50., 150.] {
            let back = crate::camera::stand_back(extent, None);
            let mut app = linked(back, false, true);

            app.update();

            let (reach, _) = linkage(&mut app);
            assert!(
                reach > extent,
                "a route reaching {extent} left a reach of {reach}"
            );
        }
    }

    /// Scrolling out reaches further and scrolling in reaches less
    #[test]
    fn the_reach_moves_with_the_camera() {
        let mut app = linked(100., false, true);
        app.update();
        let (near, _) = linkage(&mut app);

        app.world_mut()
            .query::<&mut OrbitCamera>()
            .single_mut(app.world_mut())
            .unwrap()
            .target_radius = 400.;
        app.update();
        let (far, _) = linkage(&mut app);

        assert!(far > near, "reached {far} from further out than {near}");
    }

    /// Pulled back far enough, the reach takes in the whole galaxy
    ///
    /// Nothing holds it in at what the map asks for unbidden. A view the
    /// spyglass is not keeping up with is a window with a circle of stars in
    /// the middle of it, which is the one thing a reach that follows the view
    /// has undertaken not to show.
    #[test]
    fn the_reach_follows_the_camera_the_whole_way_out() {
        let mut app = linked(Spyglass::CEILING, false, true);

        app.update();

        let (reach, _) = linkage(&mut app);
        assert_eq!(reach, following(Spyglass::CEILING));
        assert!(reach > Spyglass::UNASKED, "stopped at {reach}");
    }

    /// And no further than the whole galaxy
    ///
    /// The camera may stand further off than the galaxy is wide, and a reach
    /// past that is a larger number asking for exactly the same systems.
    #[test]
    fn the_reach_stops_at_the_edge_of_the_galaxy() {
        let mut app = linked(1e6, false, true);

        app.update();

        let (reach, _) = linkage(&mut app);
        assert_eq!(reach, Spyglass::CEILING);
    }

    /// And never falls under the shortest reach worth offering
    #[test]
    fn the_reach_stops_where_it_would_show_one_system() {
        let mut app = linked(1e-3, false, true);

        app.update();

        let (reach, _) = linkage(&mut app);
        assert_eq!(reach, Spyglass::FLOOR);
    }

    /// Not following, the reach is left where it was set
    #[test]
    fn a_reach_set_by_hand_is_left_alone() {
        let mut app = linked(100., false, false);

        app.update();

        let (reach, _) = linkage(&mut app);
        assert_eq!(reach, Spyglass::OPENING);
    }

    /// Locking the camera does nothing while the camera is what sets the
    /// reach
    ///
    /// The two are the same link read in opposite directions. Were both in
    /// force the camera would be told to stand where the reach says while the
    /// reach was being taken from where it stands, and which of them the map
    /// ended up obeying would come down to the order they ran in.
    #[test]
    fn the_camera_is_not_locked_to_a_reach_it_is_setting() {
        let mut app = linked(100., true, true);

        app.update();

        let (reach, back) = linkage(&mut app);
        assert_eq!(back, 100., "the camera was moved to {back}");
        assert!((reach - following(100.)).abs() < 1e-3, "reached {reach}");
    }

    /// Locked and not following, the camera goes where the reach says
    #[test]
    fn a_locked_camera_still_follows_the_reach() {
        let mut app = linked(100., true, false);

        app.update();

        let (reach, back) = linkage(&mut app);
        assert_eq!(reach, Spyglass::OPENING);
        assert_eq!(back, Spyglass::OPENING * 3.);
    }
}
