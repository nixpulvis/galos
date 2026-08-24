use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::filter::Admitted;
use crate::systems::selection::Selection;
use crate::systems::spawn::{build_system, system_at};
use crate::systems::{Spyglass, System, route::fetch::fetch_route};
use crate::{Names, Populated, ResidentIndex, Transport, search::Search};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use chrono::{DateTime, Duration as Span, Utc};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

pub fn plugin(app: &mut App) {
    app.insert_resource(Poll(Some(10.)));
    app.insert_resource(Throttle(100));

    app.init_resource::<LastFetchedAt>();
    app.init_resource::<FetchTasks>();

    app.add_systems(Update, (fetch, fetch_selected).in_set(MapSet::Fetch));
}

/// How long the map waits before asking again for what it already has
///
/// Seconds, which is how the question is put: how often should this be
/// refreshed. A rate would want 0.167 of a box that will be typed 6 into.
///
/// `None` never asks again, which is what the checkbox beside it turns off.
/// Zero asks every frame, which the two ends being different values is what
/// makes sayable at all.
///
/// Only what has already been fetched waits this long. Somewhere new is a
/// question the map has not put yet, and waits on [`Throttle`] instead.
#[derive(Resource)]
pub struct Poll(pub Option<f64>);

impl Poll {
    /// Whether enough has passed since `last` to ask again at `now`
    ///
    /// The one reading of the setting, put to it by everything refreshing on
    /// it. The spyglass asks about a region of the galaxy and
    /// [`crate::systems::bodies::fetch`] asks about the inside of one system,
    /// and the two share nothing else; what they do share is that the user set
    /// one number for how often the map goes back to the database, and it
    /// means the same thing to both.
    ///
    /// Never when the poll is off, so a caller only has to ask this to honour
    /// the checkbox.
    pub fn elapsed(&self, last: Instant, now: Instant) -> bool {
        self.0.is_some_and(|wait| {
            last + Duration::from_secs_f64(wait.max(0.)) < now
        })
    }
}

/// The amount to throttle requests for new indices (millis).
#[derive(Resource)]
pub struct Throttle(pub u64);

/// A resource which keeps the instant the last fetch was made
#[derive(Resource)]
pub struct LastFetchedAt(pub Instant);

impl Default for LastFetchedAt {
    fn default() -> LastFetchedAt {
        LastFetchedAt(Instant::now())
    }
}

// TODO: Put region math inside custom Hash impl?
// TODO: once we have a hash impl let's save f64 instead of String for route
// range.
// TODO(#43): fetched regions should be cubes with `region_size` side length, they
// are currently spheres with `region_size` radius.
#[derive(Hash, Eq, PartialEq, Clone)]
pub enum FetchIndex {
    // System<String>
    /// Everywhere within a radius of a point, and what of it is wanted
    ///
    /// Nothing wanted in particular is the whole of what is there. Where the
    /// filters have said what they admit, the region is asked for that alone,
    /// which makes it a different question about the same place: adding or
    /// dropping a filter is somewhere new rather than a refresh, and is
    /// answered at the throttle rather than waiting out the poll.
    ///
    /// The span a filter on time asks for is part of that question, and the
    /// span rather than the moment it reaches back to: a moment is worked out
    /// afresh every frame, so a region carrying one would never match the last
    /// and the map would ask again at the throttle for as long as the filter
    /// stood. A span holds still until the user moves the control.
    Region(IVec3, i32, Option<Admitted>, Option<Span>),
    // View<Frustum>,
    Route(String, String, String),
    /// Named systems, by address
    ///
    /// What the map is asked for a row at a time rather than by where it is:
    /// a system the user picked out of a list is one the map may never have
    /// been near.
    Systems(Vec<i64>),
}

impl FetchIndex {
    /// Whether this asks again for what `last` already fetched
    ///
    /// Which is what decides how long the map waits: asking again for what it
    /// has is a refresh and waits out [`Poll`], while asking for somewhere new
    /// is a question it has not put yet and waits only on [`Throttle`]. Flying
    /// somewhere should bring stars promptly however slowly the map is set to
    /// refresh.
    ///
    /// A region refreshes another when it stands in the same place, takes in no
    /// more sky, and looks back no further. A larger radius takes in systems
    /// that were never asked for, and so does a longer span, so either is a new
    /// question about the same place.
    ///
    /// A question and a predicate rather than an ordering. Two regions about
    /// different centers are each no answer to the other, which is a thing an
    /// [`Ord`] cannot say: it would have to call one of them the greater, and
    /// whichever it called it would be wrong the other way round.
    fn refreshes(&self, last: &FetchIndex) -> bool {
        match (self, last) {
            (
                FetchIndex::Region(center, radius, admitted, span),
                FetchIndex::Region(before, reached, asked, spanned),
            ) => {
                center == before
                    && radius <= reached
                    && admitted == asked
                    && looks_back_no_further(span, spanned)
            }
            // Only the spyglass records what it last fetched, so neither a
            // route nor a named system is ever on either side of this.
            // Somewhere new either way.
            _ => false,
        }
    }

    /// This survey shrunk to what is still held within `keep` light years of
    /// `center`, or [`None`] where the drop has left nothing of it
    ///
    /// A survey the evictor has reached into claims a region now missing its
    /// outskirts. Forgetting it whole would have the map re-fetch and re-spawn
    /// the resident middle it still holds — a zoom in drops the far systems and
    /// then reloads the near ones. So it is clamped to the kept sphere instead:
    /// its radius is brought in to what is provably still resident, so the
    /// region in view stays surveyed while a return to what was dropped asks
    /// again. Only a `Region` is ever surveyed, so nothing else is one to keep.
    ///
    /// The clamp is conservative for a survey off the camera's centre: the part
    /// of it within `keep - distance` of its own centre is within `keep` of the
    /// camera by the triangle inequality, so it may forget a sliver still held
    /// and ask for it again, but it never claims one that is gone.
    pub(crate) fn clamp_to(
        &self,
        center: DVec3,
        keep: f64,
    ) -> Option<FetchIndex> {
        let FetchIndex::Region(at, radius, ..) = self else {
            return None;
        };
        let at = DVec3::new(at.x as f64, at.y as f64, at.z as f64);
        let resident = keep - center.distance(at);
        if resident <= 0. {
            return None;
        }
        let mut clamped = self.clone();
        if let FetchIndex::Region(_, reach, ..) = &mut clamped {
            *reach = (*radius).min(resident.floor() as i32);
        }
        Some(clamped)
    }
}

/// Whether `span` asks about a stretch of time `spanned` already answered
///
/// A span narrows: the shorter of two asks about part of what the longer
/// covered, and asking nothing of time covers the whole of it. So a filter
/// switched on asks for less than is already held and can wait out the poll,
/// while one switched off asks for what was never fetched and is answered at
/// the throttle.
///
/// Part of, at one moment. A radius holds a smaller radius for good; a span
/// slides, so the narrower one asked later reaches systems heard from since the
/// wider one was answered. That is the poll's to bring in, and it brings the
/// same arrivals in for a span nobody has touched, the window having moved
/// under it either way.
///
/// Spelled out rather than compared as [`Option`]s, whose own ordering puts
/// [`None`] below every [`Some`] and would read asking nothing about time as
/// the narrowest question of the lot.
fn looks_back_no_further(span: &Option<Span>, spanned: &Option<Span>) -> bool {
    match (span, spanned) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(span), Some(spanned)) => span <= spanned,
    }
}

impl fmt::Debug for FetchIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use FetchIndex::*;

        match self {
            Region(center, radius, admitted, span) => {
                write!(
                    f,
                    "<({},{},{}),{}",
                    center.x, center.y, center.z, radius
                )?;
                if let Some(admitted) = admitted {
                    write!(
                        f,
                        " admitting {} factions and {} systems",
                        admitted.factions.len(),
                        admitted.systems.len()
                    )?;
                }
                if let Some(span) = span {
                    write!(f, " within {}s", span.num_seconds())?;
                }
                write!(f, ">")
            }
            Route(start, end, range) => {
                write!(f, "<{}-{}>{}>", start, end, range)
            }
            Systems(addresses) => write!(f, "<{} named>", addresses.len()),
        }
    }
}

/// A region the map has an answer for, and the moment it is answered as of
///
/// The map holds every system it has ever been sent and puts a fresh row over
/// an old one, so what it can answer for is the union of everywhere it has
/// asked about. One of these per region asked, and together they are that
/// union.
///
/// Kept as the region was asked for, in whole light years, so that a region
/// asked for again is recognised as the same one. `at` is the database's
/// clock, which is the only clock `updated_at` can be compared against.
#[derive(Clone)]
pub struct Survey {
    /// What was asked, which says what the answer covers
    pub asked: FetchIndex,
    /// The moment the answer is current as of
    pub at: DateTime<Utc>,
}

/// How many regions the map remembers having asked about
///
/// Forgetting one costs a region being read again, and never costs a system
/// being missed, so this is a size rather than a correctness figure. Held
/// short because a region asked again from the same place puts the old one
/// out, so the list only grows while the camera is going somewhere new, and a
/// camera going somewhere new is leaving the old regions behind anyway.
const REMEMBERED: usize = 4;

/// Tasks for systems in the DB which will be spawned
#[derive(Resource, Default)]
pub struct FetchTasks {
    pub fetched: HashMap<FetchIndex, (Task<Fetched>, Instant)>,
    /// The regions already asked about, oldest first
    ///
    /// Written when an answer lands rather than when it is asked for: a
    /// question still on the wire is one the map cannot answer from yet, and
    /// one that came back an error is one it never will.
    pub surveyed: Vec<Survey>,
}

/// A system as the cells give it, before the resident tables name and colour
/// it: an address and where it sits, in light years.
///
/// The cells carry position and photometry and nothing political, so a fetch
/// task turns each point into one of these and then joins it against
/// [`Populated`] and [`Names`] to build a drawable [`System`] — all on its own
/// thread, so the main thread only ever applies the finished rows.
pub struct RawSystem {
    pub address: i64,
    pub position: [f64; 3],
}

/// What a fetch came back with, and the moment it landed.
///
/// Already-built [`System`]s: naming and colouring happen in the task off the
/// main thread (see [`RawSystem`]), so [`super::spawn`] has only to queue what
/// arrives. The cells are static files, so unlike a database read there is no
/// clock to compare a row's age against; the moment only stamps a survey so the
/// region is recognised as one already read. [`None`] where the fetch errored
/// and the region is left to be asked about again.
pub type Fetched = (Vec<System>, Option<DateTime<Utc>>);

impl FetchTasks {
    /// Take `asked` to be answered for as of `at`
    ///
    /// Whatever it covers that an older survey covered is now covered by this
    /// one, so those are dropped: a camera standing still leaves one survey
    /// standing however long it polls for, and only a camera going somewhere
    /// new grows the list.
    ///
    /// The oldest go once the list is full. What that costs is a region read
    /// again, having forgotten it was already held.
    pub fn surveyed(&mut self, asked: FetchIndex, at: DateTime<Utc>) {
        self.surveyed.retain(|survey| {
            !(survey.asked.refreshes(&asked) && survey.at <= at)
        });
        self.surveyed.push(Survey { asked, at });
        if self.surveyed.len() > REMEMBERED {
            self.surveyed.remove(0);
        }
    }
}

/// Spawns tasks to load star systems from the index
pub fn fetch(
    camera_query: Query<&OrbitCamera>,
    mut search_events: MessageReader<Search>,
    mut tasks: ResMut<FetchTasks>,
    mut spyglass: ResMut<Spyglass>,
    time: Res<Time<Real>>,
    mut last_fetched_at: ResMut<LastFetchedAt>,
    throttle: Res<Throttle>,
    poll: Res<Poll>,
    index: Res<ResidentIndex>,
    transport: Res<Transport>,
    jumps: Res<crate::systems::route::graph::Jumps>,
    names: Res<Names>,
    populated: Res<Populated>,
) {
    if spyglass.fetch {
        fetch_spyglass(
            &camera_query,
            &mut tasks,
            &mut spyglass,
            &time,
            &mut last_fetched_at,
            &throttle,
            &poll,
            &index,
            &transport,
            &names,
            &populated,
        );
    }

    for event in search_events.read() {
        match event {
            // A search finds and picks out nothing, so there is nothing
            // here to fetch yet. Whatever the user picks out of what it
            // found is asked for by `fetch_selected`.
            Search::System { .. } => {}
            Search::Route { start, end, range } => {
                fetch_route(
                    start.into(),
                    end.into(),
                    range.into(),
                    &mut tasks,
                    &time,
                    &mut last_fetched_at,
                    &jumps,
                    &names,
                    &populated,
                );
            }
        };
    }
}

/// Ask for every system the spyglass reaches, read from the index cells
///
/// The whole region rather than what the filters admit: the cells are static
/// and cheap to read, so the map draws everything in reach and [`filter`] dims
/// what it excludes, rather than the fetch leaving it out and having nothing to
/// draw faintly.
fn fetch_spyglass(
    camera_query: &Query<&OrbitCamera>,
    tasks: &mut ResMut<FetchTasks>,
    spyglass: &ResMut<Spyglass>,
    time: &Res<Time<Real>>,
    last_fetched_at: &mut ResMut<LastFetchedAt>,
    throttle: &Res<Throttle>,
    poll: &Res<Poll>,
    index: &Res<ResidentIndex>,
    transport: &Res<Transport>,
    names: &Res<Names>,
    populated: &Res<Populated>,
) {
    let Ok(camera) = camera_query.single() else { return };
    let center = camera.center.as_ivec3();
    let key = FetchIndex::Region(center, spyglass.radius as i32, None, None);
    let now = time.last_update().unwrap_or(time.startup());
    if spyglass_condition(&key, tasks, now, last_fetched_at, throttle, poll) {
        debug!("fetching {:?} @ {:?}", key, now.duration_since(time.startup()));

        let task_pool = AsyncComputeTaskPool::get();
        let transport = transport.0.clone();
        // Cheap Arc handles onto the resident tables, so the task names and
        // colours its systems on its own thread rather than handing raw rows
        // back for the main thread to build.
        let names = Names::clone(names);
        let populated = Populated::clone(populated);
        let cent = [center.x as f64, center.y as f64, center.z as f64];
        let range = spyglass.radius.floor() as f64;
        // Which cells the region touches is settled here off the resident
        // index; the task only reads the payloads those cells point at.
        let cells = index.0.region(cent, range);
        let task = task_pool.spawn(async move {
            // Each payload is a blocking read, so a wide region of thousands
            // of cells read one after another on this one task thread is the
            // whole of the fetch's latency. Split the cells across as many
            // reads as the pool has threads and join them, so the reads and
            // the builds run at once rather than in turn.
            let pool = AsyncComputeTaskPool::get();
            let workers =
                std::thread::available_parallelism().map_or(4, |n| n.get());
            let chunk = cells.len().div_ceil(workers).max(1);
            let jobs: Vec<_> = cells
                .chunks(chunk)
                .map(|slice| {
                    let transport = transport.clone();
                    let names = names.clone();
                    let populated = populated.clone();
                    let cells = slice.to_vec();
                    pool.spawn(async move {
                        let mut systems = Vec::new();
                        for cell in cells {
                            let Ok(points) = transport.payload(cell).await
                            else {
                                continue;
                            };
                            for point in points {
                                let pos = cell.dequantize(point.pos);
                                // A cell straddling the sphere carries systems
                                // outside it, so each point is weighed against
                                // the true radius.
                                let dx = pos[0] - cent[0];
                                let dy = pos[1] - cent[1];
                                let dz = pos[2] - cent[2];
                                if dx * dx + dy * dy + dz * dz <= range * range
                                {
                                    let raw = RawSystem {
                                        address: point.id64 as i64,
                                        position: pos,
                                    };
                                    systems.push(build_system(
                                        &raw, &populated, &names,
                                    ));
                                }
                            }
                        }
                        systems
                    })
                })
                .collect();
            let mut systems = Vec::new();
            for job in jobs {
                systems.extend(job.await);
            }
            (systems, Some(Utc::now()))
        });
        tasks.fetched.insert(key.clone(), (task, now));
        **last_fetched_at = LastFetchedAt(now);
    }
}

/// Build the systems that are picked out and have no star on the map
///
/// A system is picked out of a search and flown to, which the map may never
/// have been near. Without this the camera arrives at empty space, and the ring
/// and the name that mark a selection have nothing to hang on.
///
/// Built from the resident [`Names`] table on the spot rather than fetched: a
/// named system's place is already in hand, so nothing is read for it. Handed
/// through a ready task so it lands the same way a region does, which is what
/// [`super::spawn`] already knows how to drain.
///
/// Only when the selection changes, which keeps a system with no place on
/// record from being asked for again every frame.
fn fetch_selected(
    selection: Res<Selection>,
    systems: Query<&System>,
    mut tasks: ResMut<FetchTasks>,
    time: Res<Time<Real>>,
    names: Res<Names>,
    populated: Res<Populated>,
) {
    if !selection.is_changed() {
        return;
    }

    let spawned = systems.iter().map(|system| system.address).collect();
    let wanted = unspawned(&selection.addresses(), &spawned);
    if wanted.is_empty() {
        return;
    }

    let now = time.last_update().unwrap_or(time.startup());
    let task_pool = AsyncComputeTaskPool::get();
    // Built here from the resident tables, a handful at a time, rather than
    // read from the index; handed through a ready task so it lands the same
    // way a region does. No moment, as a route has none: these are systems
    // named outright rather than a region, so nothing about the sky is settled
    // by their arriving.
    let systems: Vec<System> = wanted
        .iter()
        .filter_map(|&address| system_at(address, &populated, &names))
        .collect();
    let task = task_pool.spawn(async move { (systems, None) });
    tasks.fetched.insert(FetchIndex::Systems(wanted), (task, now));
}

/// Which of `selected` the map has no star for, by address
///
/// One query for all of them, so this answers a list rather than a verdict
/// per system.
fn unspawned(selected: &[i64], spawned: &HashSet<i64>) -> Vec<i64> {
    selected
        .iter()
        .copied()
        .filter(|address| !spawned.contains(address))
        .collect()
}

/// Whether the spyglass should ask for `index` now
///
/// One region query at a time. The throttle says how long to wait since the
/// last one was *started*, which is no answer at all when a region takes
/// longer to come back than the throttle waits: flying while zoomed out asks
/// again every throttle, and a query over most of the galaxy takes a second
/// or two, so a handful of them end up on the wire at once, each one a copy
/// of most of the table and each one crowding the rest.
///
/// Waited on rather than replaced, unlike a route. The regions asked for
/// while the camera moves are each a real answer about where it was, and
/// dropping the one under way for the next would leave nothing arriving at
/// all until the camera stopped.
///
/// A refresh of any region already surveyed rather than of the last one only.
/// A camera that goes somewhere and comes back is asking again for what it
/// holds, whatever it looked at in between, and waiting out the poll for it is
/// the difference between asking every tenth of a second and asking every ten
/// seconds.
pub fn spyglass_condition(
    index: &FetchIndex,
    tasks: &ResMut<FetchTasks>,
    now: Instant,
    last_fetched_at: &ResMut<LastFetchedAt>,
    throttle: &Res<Throttle>,
    poll: &Res<Poll>,
) -> bool {
    if region_asked(tasks.fetched.keys()) {
        return false;
    }

    if tasks.surveyed.is_empty() {
        return true;
    }

    if surveyed_already(index, &tasks.surveyed) {
        poll.elapsed(last_fetched_at.0, now)
    } else {
        last_fetched_at.0 + Duration::from_millis(throttle.0) < now
    }
}

/// Whether `index` asks for a region the map can already answer for
///
/// Against every survey rather than the last one. A camera that goes somewhere
/// and comes back is asking again for what it holds, whatever it looked at in
/// between, and read against the last survey alone that is somewhere new and
/// is asked for again every throttle for as long as it stands there.
///
/// Whether any one survey covers it, rather than whether they cover it
/// between them. Two regions may hold a third between them without either
/// holding it, and working that out is a question about spheres that this
/// would have to answer every frame. Said no to wrongly, the region is asked
/// for at the throttle and comes back with what the surveys do cover left out
/// of it, which is a small answer arriving early rather than a wrong one.
fn surveyed_already(index: &FetchIndex, surveyed: &[Survey]) -> bool {
    surveyed.iter().any(|survey| index.refreshes(&survey.asked))
}

/// Whether a region is among the queries already on the wire
///
/// Only a region. A route and a set of named systems are each asked for once
/// by something the user just did, and neither is the map asking again for
/// most of what it already holds.
fn region_asked<'a>(mut asked: impl Iterator<Item = &'a FetchIndex>) -> bool {
    asked.any(|index| matches!(index, FetchIndex::Region(..)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A region of `radius` about `center` on the x axis, asked for whole
    fn region(center: i32, radius: i32) -> FetchIndex {
        FetchIndex::Region(IVec3::new(center, 0, 0), radius, None, None)
    }

    /// The same region, asked only for what was heard from within `secs`
    fn region_within(center: i32, radius: i32, secs: i64) -> FetchIndex {
        FetchIndex::Region(
            IVec3::new(center, 0, 0),
            radius,
            None,
            Some(Span::seconds(secs)),
        )
    }

    /// The same region, narrowed to the faction at `id`
    fn region_admitting(center: i32, radius: i32, id: i32) -> FetchIndex {
        let admitted = Admitted { factions: vec![id], systems: Vec::new() };
        FetchIndex::Region(
            IVec3::new(center, 0, 0),
            radius,
            Some(admitted),
            None,
        )
    }

    /// The map holding a star for each of `addresses`
    fn on_the_map(addresses: &[i64]) -> HashSet<i64> {
        addresses.iter().copied().collect()
    }

    /// A moment, `secs` on from an arbitrary one
    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("a moment")
    }

    /// Standing still leaves one survey however long the map polls
    ///
    /// The region asked again covers what the one before it covered and is
    /// newer, so the older goes. Without that the list fills with the same
    /// region over and over and the oldest of them, which is the one the
    /// database is told about, falls further and further behind.
    #[test]
    fn asking_again_from_the_same_place_puts_the_old_survey_out() {
        let mut tasks = FetchTasks::default();
        for tick in 0..20 {
            tasks.surveyed(region(0, 10), at(tick * 10));
        }

        assert_eq!(tasks.surveyed.len(), 1, "a survey per poll was kept");
        assert_eq!(tasks.surveyed[0].at, at(190), "the newest was not kept");
    }

    /// Going somewhere and coming back leaves both places surveyed
    ///
    /// The case a single remembered region cannot hold. The map holds A and B
    /// both, so coming back to A is a question it can already answer but for
    /// what has changed, and B is still worth remembering for the same reason.
    #[test]
    fn going_away_and_coming_back_holds_both_places() {
        let mut tasks = FetchTasks::default();
        tasks.surveyed(region(0, 10), at(0));
        tasks.surveyed(region(100, 10), at(10));
        tasks.surveyed(region(0, 10), at(20));

        let asked: Vec<_> =
            tasks.surveyed.iter().map(|survey| survey.asked.clone()).collect();
        assert_eq!(asked.len(), 2, "kept {asked:?}");
        assert!(asked.contains(&region(100, 10)), "forgot where it went");
        assert!(asked.contains(&region(0, 10)), "forgot where it came back to");
        // And the one it came back to is held as of when it came back, not as
        // of when it first went.
        let back = tasks
            .surveyed
            .iter()
            .find(|survey| survey.asked == region(0, 10))
            .expect("the region it came back to");
        assert_eq!(back.at, at(20));
    }

    /// A wider region put over a narrower one at the same place
    #[test]
    fn a_wider_survey_puts_out_the_one_inside_it() {
        let mut tasks = FetchTasks::default();
        tasks.surveyed(region(0, 10), at(0));
        tasks.surveyed(region(0, 50), at(10));

        assert_eq!(tasks.surveyed.len(), 1);
        assert_eq!(tasks.surveyed[0].asked, region(0, 50));
    }

    /// And the oldest go once the map has been to more places than it keeps
    ///
    /// Forgetting one costs that region being read again and never costs a
    /// system, so a full list drops rather than grows.
    #[test]
    fn the_oldest_survey_goes_once_the_list_is_full() {
        let mut tasks = FetchTasks::default();
        for step in 0..(REMEMBERED as i32 + 4) {
            tasks.surveyed(region(step * 100, 10), at(step as i64));
        }

        assert_eq!(tasks.surveyed.len(), REMEMBERED);
        assert_eq!(
            tasks.surveyed[0].asked,
            region(400, 10),
            "dropped something other than the oldest"
        );
    }

    /// A region of `radius` about `center`, and when it was answered
    ///
    /// For [`super::despawn`]'s tests, which put surveys on a map to watch a
    /// clear take them off again.
    #[cfg(test)]
    pub(crate) fn surveyed_at(
        center: i32,
        radius: i32,
        secs: i64,
    ) -> (FetchIndex, DateTime<Utc>) {
        (region(center, radius), at(secs))
    }

    /// Coming back to a region already surveyed is a refresh
    ///
    /// So it waits out the poll rather than being asked again at the throttle.
    /// The map holds it, and read against the last survey alone this is
    /// somewhere new: that is the whole of what keeping a list buys.
    #[test]
    fn coming_back_to_a_surveyed_region_is_a_refresh() {
        let mut tasks = FetchTasks::default();
        tasks.surveyed(region(0, 10), at(0));
        tasks.surveyed(region(100, 10), at(10));

        assert!(
            surveyed_already(&region(0, 10), &tasks.surveyed),
            "the place it came back to read as somewhere new"
        );
        assert!(
            surveyed_already(&region(100, 10), &tasks.surveyed),
            "the place it went read as somewhere new"
        );
        assert!(
            !surveyed_already(&region(500, 10), &tasks.surveyed),
            "somewhere it has never been read as already held"
        );
    }

    /// Somewhere never surveyed is a new question however many are held
    #[test]
    fn a_region_between_two_surveys_is_still_a_new_question() {
        let mut tasks = FetchTasks::default();
        tasks.surveyed(region(0, 10), at(0));
        tasks.surveyed(region(10, 10), at(10));

        // Covered by the two of them together and by neither alone, which is
        // asked again rather than worked out.
        assert!(!surveyed_already(&region(5, 10), &tasks.surveyed));
    }

    /// A region narrowed to a filter is a different question about the place
    ///
    /// Not a refresh of the region asked for whole, so it is answered at the
    /// throttle rather than waiting out the poll. Adding a filter while the
    /// excluded are not drawn changes what the map is asking for, and the
    /// user is waiting on the answer.
    #[test]
    fn a_narrowed_region_does_not_refresh_the_whole_one() {
        assert!(!region_admitting(0, 10, 7).refreshes(&region(0, 10)));
        assert!(!region(0, 10).refreshes(&region_admitting(0, 10, 7)));
    }

    /// Nor does one narrowed to something else
    #[test]
    fn two_narrowings_are_two_questions() {
        assert!(
            !region_admitting(0, 10, 7).refreshes(&region_admitting(0, 10, 9))
        );
    }

    /// The same narrowing about the same place is a refresh
    #[test]
    fn the_same_narrowed_region_refreshes() {
        assert!(
            region_admitting(0, 10, 7).refreshes(&region_admitting(0, 10, 7))
        );
    }

    /// Turning a filter on time on is a refresh of what is already held
    ///
    /// A span only narrows, so everything it admits has already been fetched by
    /// the region asked for whole. Nothing new to hurry for, so it waits out
    /// the poll.
    #[test]
    fn asking_about_time_refreshes_the_region_asked_whole() {
        assert!(region_within(0, 10, 60).refreshes(&region(0, 10)));
    }

    /// And turning it off is a new question
    ///
    /// Everything older than the span was never asked for. Left as a refresh,
    /// the map would go on drawing the thinned sky until the poll came round,
    /// or for good where the poll is off.
    #[test]
    fn asking_nothing_of_time_is_a_new_question() {
        assert!(!region(0, 10).refreshes(&region_within(0, 10, 60)));
    }

    /// A shorter span refreshes a longer one
    #[test]
    fn a_shorter_span_refreshes_a_longer_one() {
        assert!(region_within(0, 10, 60).refreshes(&region_within(0, 10, 600)));
    }

    /// And a longer span asks for what the shorter never fetched
    #[test]
    fn a_longer_span_is_not_a_refresh() {
        assert!(
            !region_within(0, 10, 600).refreshes(&region_within(0, 10, 60))
        );
    }

    /// The same span about the same place is a refresh
    ///
    /// Which is what the whole thing rests on: the span is worked out afresh
    /// every frame and must come out equal every time, or the region is a new
    /// question at every frame and the throttle is all that holds it back.
    #[test]
    fn the_same_span_refreshes() {
        assert!(region_within(0, 10, 60).refreshes(&region_within(0, 10, 60)));
    }

    /// A region already on the wire is one the spyglass waits for
    ///
    /// The throttle measures from where the last query was sent rather than
    /// from where it came back, so a region that takes longer to answer than
    /// the throttle waits would otherwise be asked again over the top of
    /// itself, and again, while the camera moves.
    #[test]
    fn a_region_under_way_is_waited_for() {
        assert!(region_asked([region(0, 10)].iter()));
    }

    /// Nothing under way is nothing to wait for
    #[test]
    fn no_query_under_way_is_no_reason_to_wait() {
        assert!(!region_asked([].iter()));
    }

    /// A route does not hold the spyglass up
    ///
    /// It is asked for once by something the user just did, and it is not the
    /// map asking again for most of what it already holds.
    #[test]
    fn a_route_under_way_does_not_hold_the_spyglass_up() {
        let route = FetchIndex::Route("A".into(), "B".into(), "10".into());

        assert!(!region_asked([route].iter()));
    }

    /// Nor does a system that was picked out
    #[test]
    fn a_picked_system_does_not_hold_the_spyglass_up() {
        assert!(!region_asked([FetchIndex::Systems(vec![7])].iter()));
    }

    /// A system picked out with no star on the map is asked for
    ///
    /// The case the whole thing is for: a name searched for, picked out of
    /// what came back, and flown to, from a part of the sky the map has
    /// never fetched.
    #[test]
    fn a_selection_the_map_has_not_reached_is_asked_for() {
        assert_eq!(unspawned(&[7], &on_the_map(&[])), vec![7]);
    }

    /// One already on the map is not asked for again
    #[test]
    fn a_selection_already_drawn_is_left_alone() {
        assert_eq!(unspawned(&[7], &on_the_map(&[7])), Vec::<i64>::new());
    }

    /// A set half on the map asks only for the half that is not
    ///
    /// One query for the lot rather than one each, and the order they were
    /// picked in is what it goes out in.
    #[test]
    fn a_gathered_selection_asks_for_what_is_missing() {
        assert_eq!(unspawned(&[7, 9, 11], &on_the_map(&[9])), vec![7, 11]);
    }

    /// Nothing picked out asks for nothing
    #[test]
    fn an_empty_selection_asks_for_nothing() {
        assert_eq!(unspawned(&[], &on_the_map(&[7])), Vec::<i64>::new());
    }

    /// The same region asked for again is a refresh
    #[test]
    fn the_same_region_refreshes() {
        assert!(region(0, 10).refreshes(&region(0, 10)));
    }

    /// Somewhere else is a new question, whichever way the camera went
    ///
    /// Both directions, since the two regions are symmetric: neither is an
    /// answer to the other, and a test of one direction alone would pass for
    /// something that called one of them the greater.
    #[test]
    fn another_region_is_not_a_refresh() {
        assert!(!region(1, 10).refreshes(&region(0, 10)));
        assert!(!region(0, 10).refreshes(&region(1, 10)));
    }

    /// Reaching no further is still a refresh
    ///
    /// Everything a narrower spyglass asks for has already been fetched, so
    /// there is nothing new to hurry for.
    #[test]
    fn a_smaller_radius_refreshes() {
        assert!(region(0, 5).refreshes(&region(0, 10)));
    }

    /// Reaching further is a new question
    ///
    /// It takes in systems that were never asked for, so it waits on the
    /// throttle rather than the poll and the sky fills as the user widens it.
    #[test]
    fn a_larger_radius_is_not_a_refresh() {
        assert!(!region(0, 20).refreshes(&region(0, 10)));
    }

    /// A route is never a refresh of anything, nor refreshed by one
    #[test]
    fn a_route_is_always_a_new_question() {
        let route = FetchIndex::Route("A".into(), "B".into(), "10".into());
        assert!(!route.refreshes(&region(0, 10)));
        assert!(!region(0, 10).refreshes(&route));
        assert!(!route.refreshes(&route));
    }
}
