use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::filter::{Admitted, DimTo, Filters};
use crate::systems::scale::SIZED_WITHIN;
use crate::systems::selection::Selection;
use crate::systems::{Spyglass, System, route::fetch::fetch_route};
use crate::{Db, search::Search};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use chrono::{DateTime, Duration as Span, Utc};
use galos_db::systems::{Survey as DbSurvey, System as DbSystem};
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

/// What a fetch came back with, and the moment it is current as of
///
/// The moment is read off the database before the question is put, so that
/// anything written while it is being answered is asked for again next time.
///
/// [`None`] where the question was never answered, which is a fetch that
/// errored. Nothing is held on the strength of one, and the region is left to
/// be asked about again.
pub type Fetched = (Vec<DbSystem>, Option<DateTime<Utc>>);

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

    /// The regions worth telling the database about, asking about `range`
    ///
    /// Only the ones asked for whole. A region narrowed by a filter was
    /// answered with the part of it the filter admitted, and telling the
    /// database that region is held would drop every system it turned away.
    ///
    /// And only the ones large enough to pay for themselves. Leaving a survey
    /// out of the answer costs a distance measured against every system in
    /// range, and saves carrying back the ones it reaches. A survey reaching a
    /// tenth as far as the question holds a thousandth of the sky it is asked
    /// against, so it is a measurement per system to save one system in a
    /// thousand. What leaves them behind is a zoom: the map is asked about the
    /// galaxy, and everywhere it surveyed while zoomed in is a pinprick in it.
    pub fn whole(&self, about: IVec3, range: i32) -> Vec<DbSurvey> {
        self.surveyed
            .iter()
            .filter_map(|survey| match &survey.asked {
                FetchIndex::Region(center, radius, None, None)
                    if worth_leaving_out(about, range, *center, *radius) =>
                {
                    Some(DbSurvey {
                        center: [
                            center.x as f64,
                            center.y as f64,
                            center.z as f64,
                        ],
                        range: *radius as f64,
                        at: survey.at,
                    })
                }
                _ => None,
            })
            .collect()
    }
}

/// Whether a survey of `radius` about `center` is worth naming to the database
/// when asking about `range` around `about`
///
/// Two ways one is not. It may stand clear of the region altogether, which is
/// what a camera that has jumped somewhere else leaves behind: a survey that
/// reaches none of what is being asked about holds nothing back and costs a
/// distance measured against every system in range.
///
/// Or it may be too small to pay for itself. A survey reaching a tenth as far
/// as the question covers a thousandth of it, so naming it is a measurement
/// per system to save carrying one system in a thousand. What leaves those
/// behind is a zoom: the map is asked about the galaxy, and everywhere it
/// surveyed while looking at a few light years is a pinprick in it.
///
/// Both are about what is worth doing rather than about what is true. A survey
/// left unsaid costs those systems being read again and never costs a system
/// being missed, so this is free to be wrong in either direction.
fn worth_leaving_out(
    about: IVec3,
    range: i32,
    center: IVec3,
    radius: i32,
) -> bool {
    if (radius as f64) < range as f64 * WORTH_LEAVING_OUT {
        return false;
    }

    // Squared, the distance itself being wanted for nothing but this.
    let away = (about - center).as_dvec3().length_squared();
    let reaching = (range + radius) as f64;

    away < reaching * reaching
}

/// How far a survey must reach to be worth leaving out of an answer
///
/// As a fraction of what is being asked for. A survey holds at most the cube
/// of this of the region it is named against, so half of it is an eighth of
/// the sky and a tenth of it is a thousandth: below about a half the
/// measurement costs more than the systems it saves carrying.
///
/// A figure about what is worth doing rather than about what is true.
/// Forgetting a survey costs those systems being read again and never costs a
/// system being missed, so this is free to be wrong in either direction.
const WORTH_LEAVING_OUT: f64 = 0.5;

/// Spawns tasks to load star systems from the DB
pub fn fetch(
    camera_query: Query<&OrbitCamera>,
    mut search_events: MessageReader<Search>,
    mut tasks: ResMut<FetchTasks>,
    mut spyglass: ResMut<Spyglass>,
    filters: Res<Filters>,
    dim: Res<DimTo>,
    time: Res<Time<Real>>,
    mut last_fetched_at: ResMut<LastFetchedAt>,
    throttle: Res<Throttle>,
    poll: Res<Poll>,
    db: Res<Db>,
) {
    if spyglass.fetch {
        fetch_spyglass(
            &camera_query,
            &mut tasks,
            &mut spyglass,
            &filters,
            &dim,
            &time,
            &mut last_fetched_at,
            &throttle,
            &poll,
            &db,
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
                    &db,
                );
            }
        };
    }
}

/// Ask for the region under the camera, or for what of it is admitted
///
/// Narrowed by the filters only where what they exclude is not drawn at all.
/// Anywhere above that the excluded systems are wanted on screen to be dimmed,
/// and what was never fetched cannot be drawn faintly.
fn fetch_spyglass(
    camera_query: &Query<&OrbitCamera>,
    tasks: &mut ResMut<FetchTasks>,
    spyglass: &ResMut<Spyglass>,
    filters: &Res<Filters>,
    dim: &Res<DimTo>,
    time: &Res<Time<Real>>,
    last_fetched_at: &mut ResMut<LastFetchedAt>,
    throttle: &Res<Throttle>,
    poll: &Res<Poll>,
    db: &Res<Db>,
) {
    let Ok(camera) = camera_query.single() else { return };
    let center = camera.center.as_ivec3();
    let admitted = if dim.0 == 0. { filters.admitted() } else { None };
    // The span rather than the moment it reaches back to. A moment is a
    // different value every frame, so a region carrying one would never match
    // the last and the map would ask again at the throttle for as long as the
    // filter stood.
    let span = if dim.0 == 0. { filters.span() } else { None };
    let index = FetchIndex::Region(
        center,
        spyglass.radius as i32,
        admitted.clone(),
        span,
    );
    let now = time.last_update().unwrap_or(time.startup());
    if spyglass_condition(&index, tasks, now, last_fetched_at, throttle, poll) {
        debug!(
            "fetching {:?} @ {:?}",
            index,
            now.duration_since(time.startup())
        );

        let task_pool = AsyncComputeTaskPool::get();
        let db = db.0.clone();
        let radius = spyglass.radius;
        // What the map can already answer for, which the region is asked
        // around rather than through: everywhere it has been holds systems it
        // would otherwise read again, and zoomed out that is most of them.
        let surveyed = tasks.whole(center, spyglass.radius.floor() as i32);
        let task = task_pool.spawn(async move {
            let cent = [center.x as f64, center.y as f64, center.z as f64];
            let range = radius.floor() as f64;
            let narrowed = admitted.as_ref().map(|admitted| {
                (admitted.factions.as_slice(), admitted.systems.as_slice())
            });
            // Worked out here rather than carried in, so that the moment is
            // taken from the clock the question is actually put at.
            let since = span.map(|span| Utc::now() - span);
            // Read before the question rather than after it, so that a system
            // written while the region is being answered is asked for again
            // rather than taken to be held. The database's own clock, which
            // is the one `updated_at` is written by.
            let Ok(at) = db.now().await else {
                return (Vec::new(), None);
            };
            match DbSystem::fetch_in_range_of_point(
                &db,
                range,
                cent,
                narrowed,
                since,
                Some(SIZED_WITHIN),
                &surveyed,
            )
            .await
            {
                Ok(found) => (found, Some(at)),
                // No moment, so the region is not taken to be held. A question
                // that came back an error is one the map still has to ask, and
                // stamping it here would leave it never asking again.
                Err(_) => (Vec::new(), None),
            }
        });
        tasks.fetched.insert(index.clone(), (task, now));
        **last_fetched_at = LastFetchedAt(now);
    }
}

/// Ask for the systems that are picked out and have no star on the map
///
/// A system is picked out of what the database answered, which the map may
/// never have been near: a name searched for and flown to is exactly that.
/// Without this the camera arrives at empty space, and the ring and the name
/// that mark a selection have nothing to hang on.
///
/// Whatever the spyglass is set to. Fetching by region is what the user turns
/// off to stop the map filling itself in as they fly, and a system they
/// picked out by hand is not the map filling itself in.
///
/// Only when the selection changes, which is what keeps a system the database
/// cannot place from being asked for again every frame. Such a system never
/// spawns, so what is missing would go on being missing.
///
/// The spyglass's own memory of where it last fetched is left alone. This
/// asks for named rows rather than for somewhere, so it says nothing about
/// whether the region under the camera is worth asking for again.
fn fetch_selected(
    selection: Res<Selection>,
    systems: Query<&System>,
    mut tasks: ResMut<FetchTasks>,
    time: Res<Time<Real>>,
    db: Res<Db>,
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
    let asking = wanted.clone();
    let db = db.0.clone();
    // No moment, as a route has none. These are systems named outright rather
    // than a region, so nothing about the sky is settled by their arriving.
    let task = task_pool.spawn(async move {
        (DbSystem::fetch_many(&db, &asking).await.unwrap_or_default(), None)
    });
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

    /// Only the regions asked for whole are told to the database
    ///
    /// A region narrowed by a filter came back with the part of it the filter
    /// admitted. Telling the database that whole region is held would have it
    /// leave out every system the filter turned away, and those would never
    /// arrive.
    #[test]
    fn a_narrowed_survey_is_not_one_the_database_is_told_about() {
        let mut tasks = FetchTasks::default();
        tasks.surveyed(region_admitting(0, 10, 7), at(0));
        tasks.surveyed(region_within(100, 10, 60), at(10));
        tasks.surveyed(region(200, 10), at(20));

        let whole = tasks.whole(IVec3::new(200, 0, 0), 10);
        assert_eq!(whole.len(), 1, "told the database about a narrowed region");
        assert_eq!(whole[0].center, [200., 0., 0.]);
        assert_eq!(whole[0].range, 10.);
        assert_eq!(whole[0].at, at(20));
    }

    /// A survey too small to pay for itself is not told to the database
    ///
    /// What a zoom leaves behind. Everywhere the map surveyed while it was
    /// looking at a few light years is a pinprick in a question about the
    /// galaxy, and naming one costs a distance measured against every system
    /// in range to save carrying back the handful it reaches.
    #[test]
    fn a_survey_too_small_to_pay_for_itself_is_left_unsaid() {
        let mut tasks = FetchTasks::default();
        tasks.surveyed(region(0, 10), at(0));

        let here = IVec3::ZERO;
        assert_eq!(tasks.whole(here, 10).len(), 1, "at the size it was taken");
        assert_eq!(tasks.whole(here, 20).len(), 1, "at twice the size");
        assert_eq!(tasks.whole(here, 100).len(), 0, "at ten times the size");
        assert_eq!(tasks.whole(here, 20000).len(), 0, "zoomed out to a galaxy");
    }

    /// Nor is one standing clear of what is being asked about
    ///
    /// What a jump somewhere else leaves behind. A survey that reaches none of
    /// the region holds nothing back from the answer, and naming it is a
    /// distance measured against every system in range for nothing.
    #[test]
    fn a_survey_standing_clear_of_the_region_is_left_unsaid() {
        let mut tasks = FetchTasks::default();
        tasks.surveyed(region(0, 10), at(0));

        // Twenty-five light years off, which two tens do not reach across.
        assert_eq!(tasks.whole(IVec3::new(25, 0, 0), 10).len(), 0);
        // Nineteen, which they do.
        assert_eq!(tasks.whole(IVec3::new(19, 0, 0), 10).len(), 1);
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
