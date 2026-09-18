use crate::schedule::MapSet;
use crate::systems::route::graph::{Drive, Routing, Tuning};
use crate::systems::selection::Selection;
use crate::systems::spawn::system_at;
use crate::systems::{System, route::fetch::fetch_route};
use crate::{Names, Populated, search::Search};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

pub fn plugin(app: &mut App) {
    app.insert_resource(Poll(Some(10.)));

    app.init_resource::<FetchTasks>();
    // The router's graph, and the table it is weighted by: both are read
    // when a route is first asked for, which may be before a read has
    // landed. An absent supercharge table is what the map says where it
    // cannot say where a jet cone is, which is exactly the right answer
    // before the read — see [`crate::Boosts::absent`].
    app.init_resource::<crate::Boosts>();
    app.init_resource::<crate::systems::route::graph::Jumps>();
    // And how the form says a plot is getting on, which the route fetch
    // writes: a click that takes a route back leaves nothing to wait on,
    // and the spinner has to stop. The form's own plugin inits this too;
    // said here as well so the system that writes it cannot be registered
    // without it.
    app.init_resource::<crate::search::Plot>();

    // Neither of these asks about a piece of sky: a route is walked over the
    // resident jump graph and a picked-out system is read from the resident
    // names table, and both answer whatever the walk has loaded around them.
    app.add_systems(Update, fetch_searched.in_set(MapSet::Fetch));
    app.add_systems(Update, fetch_selected.in_set(MapSet::Fetch));
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
/// Map-wide, and the reason it is not filed under the spyglass. Three things
/// go back over what the map already holds and all three keep this beat:
/// [`super::bodies::fetch`] asks a system's interior again,
/// [`super::filter`] re-cuts the time filter as its span slides, and
/// [`crate::refresh`] picks up an index that has been published again.
#[derive(Resource)]
pub struct Poll(pub Option<f64>);

impl Poll {
    /// Whether enough has passed since `last` to ask again at `now`
    ///
    /// The one reading of the setting, put to it by everything refreshing on
    /// it. [`crate::systems::bodies::fetch`] asks about the inside of one
    /// system and [`crate::refresh`] asks whether what the map reads from has
    /// moved, and the two share nothing else; what they do share is that the
    /// user set one number for how often the map goes back over what it
    /// holds, and it means the same thing to both.
    ///
    /// Never when the poll is off, so a caller only has to ask this to honour
    /// the checkbox.
    pub fn elapsed(&self, last: Instant, now: Instant) -> bool {
        self.0.is_some_and(|wait| {
            last + Duration::from_secs_f64(wait.max(0.)) < now
        })
    }
}

// TODO: once we have a hash impl let's save f64 instead of String for route
// range.
#[derive(Hash, Eq, PartialEq, Clone)]
pub enum FetchIndex {
    // System<String>
    // View<Frustum>,
    /// One leg of a trip: from one named system to the next, at a jump range
    ///
    /// A leg rather than a whole trip, so that a trip through several stops is
    /// several of these, asked and answered one per leg. The range as it was
    /// typed, since it is part of what tells one route from another and a
    /// float is no kind of key, and the drive beside it for the same reason:
    /// the same pair supercharged and unsupercharged are two routes through
    /// different systems. The search mode last, on the same argument again: a
    /// route that was not asked to prove the fewest jumps may not take them.
    Route(String, String, String, Option<String>, Drive, Routing, Tuning),
    /// Named systems, by address
    ///
    /// What the map is asked for a row at a time rather than by where it is:
    /// a system the user picked out of a list is one the map may never have
    /// been near.
    Systems(Vec<i64>),
}

impl fmt::Debug for FetchIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use FetchIndex::*;

        match self {
            Route(start, end, range, trip, drive, how, _) => {
                let boosted = drive.named().unwrap_or("unaided");
                let how = how.named();
                match trip {
                    Some(trip) => write!(
                        f,
                        "<{start}-{end}>{range},{boosted},{how}>{trip}"
                    ),
                    None => {
                        write!(f, "<{start}-{end}>{range},{boosted},{how}>")
                    }
                }
            }
            Systems(addresses) => write!(f, "<{} named>", addresses.len()),
        }
    }
}

/// Tasks for systems in the DB which will be spawned
#[derive(Resource, Default)]
pub struct FetchTasks {
    pub fetched: HashMap<FetchIndex, (Task<Fetched>, Instant)>,
}

/// A system as the cells give it, before the resident tables name and color
/// it: an address and where it sits, in light years.
///
/// The cells carry position and photometry and nothing political, so a fetch
/// task turns each point into one of these and then joins it against
/// [`Populated`] and [`Names`] to build a drawable [`System`] — all on its own
/// thread, so the main thread only ever applies the finished rows.
pub struct RawSystem {
    pub address: i64,
    pub position: [f64; 3],
    /// The payload point's combined absolute magnitude and temperature bucket,
    /// for the realistic view's photometry. [`None`] on the paths that carry no
    /// point — a route's stops, a searched system flown to.
    pub magnitude: Option<f32>,
    pub temp_bucket: Option<u8>,
    /// When the system was last updated, as the payload point carries it.
    ///
    /// What the filter on time is asked of. [`None`] alongside the photometry
    /// and for the same reason: no point behind this one, so nothing on record
    /// here says when the system was last heard from.
    pub updated_at: Option<DateTime<Utc>>,
}

/// What a fetch came back with: already-built [`System`]s.
///
/// Naming and coloring happen in the task off the main thread (see
/// [`RawSystem`]), so [`super::spawn`] has only to queue what arrives.
pub type Fetched = Vec<System>;

/// Ask for whatever a search named
///
/// A route is walked over the resident jump graph and named out of the
/// resident names table; it asks the sky for nothing. It was registered on
/// the spyglass region fetch once and gated with it, so the moment the walk
/// became the map's source plotting a route resolved its two ends and then
/// did nothing at all — no hops, no line, no framing. Its own system, gated
/// by nothing, is what keeps that from happening again.
pub fn fetch_searched(
    mut search_events: MessageReader<Search>,
    mut tasks: ResMut<FetchTasks>,
    mut searching: ResMut<crate::systems::route::frontier::Frontiers>,
    time: Res<Time<Real>>,
    mut jumps: ResMut<crate::systems::route::graph::Jumps>,
    names: Res<Names>,
    boosts: Res<crate::Boosts>,
    populated: Res<Populated>,
    mut plot: ResMut<crate::search::Plot>,
    tune: Res<crate::systems::route::graph::Tuning>,
    mut filters: ResMut<crate::systems::filter::Filters>,
    mut selected: ResMut<crate::systems::route::SelectedFilter>,
) {
    for event in search_events.read() {
        match event {
            // A search finds and picks out nothing, so there is nothing
            // here to fetch yet. Whatever the user picks out of what it
            // found is asked for by `fetch_selected`.
            Search::System { .. } => {}
            // The mode comes with the ask rather than off the setting, as
            // the range and the drive do: what the route is, is what was
            // asked for, and the setting may have moved since.
            // Stop what is running and ask for nothing. Its own gesture
            // rather than a second meaning for the plot button; see
            // [`crate::search::Search::Stop`].
            Search::Stop => {
                crate::systems::route::fetch::stop_routes(
                    &mut tasks,
                    &mut searching,
                    &mut filters,
                );
                // Nothing is left for the form to wait on, and the only
                // other thing that clears `Working` is a route landing.
                *plot = crate::search::Plot::Nothing;
            }
            Search::Route { stops, range, drive, how } => {
                fetch_route(
                    stops.clone(),
                    range.into(),
                    *drive,
                    &mut tasks,
                    &mut searching,
                    &time,
                    &mut jumps,
                    *how,
                    // Read where the route is asked for, as the range and
                    // the drive are: what a plot is, is what was asked for,
                    // and a knob moved while it runs does not change the
                    // answer under it.
                    *tune,
                    &names,
                    &boosts,
                    &populated,
                    &mut filters,
                    &mut selected,
                );
            }
            // And one leg asked again, on its own ask rather than the
            // form's. The form is told it is waiting again, as pressing
            // plot tells it: the row's own spinner is the bar's, and the
            // stop button is the one way out of either.
            Search::Replot(route) => {
                if crate::systems::route::fetch::replot(
                    route,
                    &mut tasks,
                    &mut searching,
                    &time,
                    &mut jumps,
                    &names,
                    &boosts,
                    &populated,
                    &mut filters,
                ) {
                    *plot = crate::search::Plot::Working;
                }
            }
        };
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
/// through a ready task all the same, so it lands the way a route's own stops
/// do and [`super::spawn`] has one queue to drain.
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
    // read from the index; handed through a ready task so it lands the way a
    // route's stops do and [`super::spawn`] has one queue to drain.
    let systems: Vec<System> = wanted
        .iter()
        .filter_map(|&address| system_at(address, &populated, &names))
        .collect();
    let task = task_pool.spawn(async move { systems });
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::systems::filter::Plotted;

    /// A world wired as the map wires it, holding two systems a jump apart
    ///
    /// The map's own [`plugin`], so a run condition put back on the route
    /// fetch fails in a test rather than in the app.
    ///
    /// The directory comes back with the app: the router reads the galaxy's
    /// cell payloads where they lie, so the built index has to outlive every
    /// route the app plots over it.
    fn plotting() -> (App, crate::testing::Scratch) {
        use galos_index::NameEntry;
        // Addresses minted from the places, so the names table and the
        // built sky agree about where these three systems are: the table
        // answers with the middle of the boxel an address names, and the
        // router resolves a leg's ends through it. See
        // [`crate::testing::boxel_at`].
        //
        // A stop every ten light years, because that is a class `A`
        // boxel's side: two places closer than that can fall in the one
        // boxel and so be minted the one address, and the second would
        // then overwrite the first in the log.
        let entries = vec![
            NameEntry {
                address: crate::testing::boxel_at([0., 0., 0.]),
                name: "Start".into(),
                position: [0., 0., 0.],
            },
            NameEntry {
                address: crate::testing::boxel_at([10., 0., 0.]),
                name: "End".into(),
                position: [10., 0., 0.],
            },
            NameEntry {
                address: crate::testing::boxel_at([20., 0., 0.]),
                name: "Onward".into(),
                position: [20., 0., 0.],
            },
        ];

        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::time::TimePlugin,
        ));
        app.add_message::<Search>();
        app.init_resource::<Selection>();
        app.init_resource::<crate::systems::route::graph::Routing>();
        app.init_resource::<crate::systems::route::graph::Tuning>();
        app.init_resource::<crate::systems::route::frontier::Frontiers>();
        // The rows a plot puts up, and which route is the one being looked
        // at: a leg's row goes up when it is asked for, so the ask writes
        // both. See [`crate::systems::filter::Plotted`].
        app.init_resource::<crate::systems::filter::Filters>();
        app.init_resource::<crate::systems::route::SelectedFilter>();
        // The same systems twice over: the names table the search box reads,
        // and the built galaxy the router walks.
        let dir = crate::testing::Scratch::new("fetch");
        let sky = crate::testing::sky_of(dir.path(), &entries);
        let names = Names::reaching(entries, Vec::new());
        app.insert_resource(crate::systems::route::graph::Jumps::over(sky));
        app.insert_resource(names);
        app.insert_resource(Populated::default());
        app.add_plugins(plugin);
        (app, dir)
    }

    /// Ask for a route through the systems [`plotting`] holds
    fn plot(app: &mut App) {
        app.world_mut().write_message(Search::Route {
            stops: vec!["Start".into(), "End".into()],
            range: "15".into(),
            drive: Drive::Unaided,
            how: Routing::default(),
        });
        app.update();
    }

    /// Ask for a trip through `stops`
    fn trip(app: &mut App, stops: &[&str]) {
        app.world_mut().write_message(Search::Route {
            stops: stops.iter().map(|stop| stop.to_string()).collect(),
            range: "10".into(),
            drive: Drive::Unaided,
            how: Routing::default(),
        });
        app.update();
    }

    /// Every leg the map is asking about, as its ends
    fn legs(app: &App) -> Vec<(String, String)> {
        let mut asked: Vec<(String, String)> = app
            .world()
            .resource::<FetchTasks>()
            .fetched
            .keys()
            .filter_map(|index| match index {
                FetchIndex::Route(start, end, ..) => {
                    Some((start.clone(), end.clone()))
                }
                _ => None,
            })
            .collect();
        asked.sort();
        asked
    }

    /// A trip through several stops is asked for a leg at a time
    ///
    /// Each leg is its own question, so each comes back with its own answer
    /// about whether the ship can fly it, and one that cannot takes nothing
    /// with it. Asked at once rather than in turn: they share the graph
    /// behind an `Arc`, and a trip that walked its legs one after another
    /// would take as long as the sum of them.
    #[test]
    fn a_trip_is_asked_for_a_leg_at_a_time() {
        let (mut app, _dir) = plotting();

        trip(&mut app, &["Start", "End", "Onward"]);

        assert_eq!(
            legs(&app),
            vec![
                ("End".to_owned(), "Onward".to_owned()),
                ("Start".to_owned(), "End".to_owned()),
            ]
        );
    }

    /// A trip asked for again while it runs is left to finish
    ///
    /// The same question twice is one question: a leg already under way
    /// keeps the seconds it has spent rather than starting over. Asking is
    /// only ever asking now — stopping is [`crate::search::Search::Stop`]'s
    /// own gesture, for the reason that variant gives at length.
    #[test]
    fn a_trip_asked_again_while_it_runs_is_left_alone() {
        let (mut app, _dir) = plotting();

        trip(&mut app, &["Start", "End", "Onward"]);
        let under_way = legs(&app);
        assert_eq!(under_way.len(), 2, "the trip was never asked for");

        trip(&mut app, &["Start", "End", "Onward"]);

        assert_eq!(
            legs(&app),
            under_way,
            "asking again disturbed the legs already searching",
        );
    }

    /// And the stop takes every leg of it back
    ///
    /// Every leg at once, because that is what the gesture means: the form
    /// waits on the plot as a whole, and a trip half stopped is a spinner
    /// nothing will ever clear. Until this the plot button carried the
    /// meaning, and on a trip it half worked — the legs land at different
    /// moments, so a second click took back the ones still searching and
    /// *re-asked the ones that had already landed*.
    ///
    /// The searches are told to give up as well as dropped: a body the pool
    /// has begun does not stop for being dropped. See
    /// [`crate::systems::route::graph::Frontier::abandon`].
    #[test]
    fn a_stop_takes_back_every_leg_of_a_trip() {
        let (mut app, _dir) = plotting();

        trip(&mut app, &["Start", "End", "Onward"]);
        assert_eq!(legs(&app).len(), 2, "the trip was never asked for");

        app.world_mut().write_message(crate::search::Search::Stop);
        app.update();

        assert!(legs(&app).is_empty(), "a leg was left searching");
        assert_eq!(
            *app.world().resource::<crate::search::Plot>(),
            crate::search::Plot::Nothing,
            "the form is still saying it is working",
        );
    }

    /// And a route whose form has moved cancels the old one and asks the new
    ///
    /// The other meaning of the same click: what is under way is dropped
    /// either way, and what is asked for is whatever the form now says. Here
    /// it is the range that moved, which is part of the leg's key.
    #[test]
    fn a_route_asked_at_another_range_replaces_the_one_running() {
        let (mut app, _dir) = plotting();

        plot(&mut app);
        assert_eq!(legs(&app).len(), 1, "the route was never asked for");

        app.world_mut().write_message(Search::Route {
            stops: vec!["Start".into(), "End".into()],
            range: "20".into(),
            drive: Drive::Unaided,
            how: Routing::default(),
        });
        app.update();

        assert_eq!(
            legs(&app),
            vec![("Start".to_owned(), "End".to_owned())],
            "the new range was not asked for",
        );
        let ranges: Vec<String> = app
            .world()
            .resource::<FetchTasks>()
            .fetched
            .keys()
            .filter_map(|index| match index {
                FetchIndex::Route(_, _, range, ..) => Some(range.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ranges, vec!["20".to_owned()], "the old range still runs");
    }

    /// And a trip replaces the one before it, leg for leg
    ///
    /// Two trips landing at once would draw lines nobody asked for together,
    /// and the form has room to say how one of them is getting on. A leg the
    /// new trip shares with the old is kept rather than walked again.
    #[test]
    fn a_new_trip_drops_the_legs_of_the_one_before() {
        let (mut app, _dir) = plotting();

        trip(&mut app, &["Start", "End", "Onward"]);
        trip(&mut app, &["Start", "End"]);

        assert_eq!(legs(&app), vec![("Start".to_owned(), "End".to_owned())]);
        // And the legs of the trip it replaced say they were stopped rather
        // than losing their rows. Both of them: a leg of a trip and a route
        // asked for on its own are different questions — the trip is part of
        // the key the router walks under and part of the row's own ask — so
        // the second plot is a row of its own rather than the first's leg
        // carried over.
        assert_eq!(
            rows(&app),
            vec![
                ("START -> END".to_owned(), Some(Plotted::Stopped)),
                ("END -> ONWARD".to_owned(), Some(Plotted::Stopped)),
                ("START -> END".to_owned(), Some(Plotted::Searching)),
            ],
        );
    }

    /// What the bar holds, in the rows' own order, and how each is getting on
    fn rows(app: &App) -> Vec<(String, Option<Plotted>)> {
        app.world()
            .resource::<crate::systems::filter::Filters>()
            .iter()
            .map(|entry| (entry.filter.name().to_owned(), entry.plotted))
            .collect()
    }

    /// A leg's row goes up when it is asked for, not when it lands
    ///
    /// Which is what gives a search something on screen: the row picks out
    /// the two ends, the sky dims to them, and the search's own layers are
    /// drawn against that rather than against the whole star field.
    #[test]
    fn a_leg_has_a_row_while_it_is_being_searched() {
        let (mut app, _dir) = plotting();

        trip(&mut app, &["Start", "End", "Onward"]);

        assert_eq!(
            rows(&app),
            vec![
                ("START -> END".to_owned(), Some(Plotted::Searching)),
                ("END -> ONWARD".to_owned(), Some(Plotted::Searching)),
            ],
            "a leg being searched has no row of its own",
        );
    }

    /// And a trip stopped part way keeps a row for every leg of it
    ///
    /// The reported trouble. A trip of three legs stopped before the last
    /// landed read as a "3 Leg Route" standing over **two** rows: a leg that
    /// never landed left no row at all, so the count over them named a trip
    /// the rows could not account for. The rows stand and say they were
    /// stopped; what is not there is the route, which is the truth of it.
    #[test]
    fn a_trip_stopped_part_way_keeps_a_row_for_every_leg() {
        let (mut app, _dir) = plotting();

        trip(&mut app, &["Start", "End", "Onward"]);
        app.world_mut().write_message(crate::search::Search::Stop);
        app.update();

        assert_eq!(
            rows(&app),
            vec![
                ("START -> END".to_owned(), Some(Plotted::Stopped)),
                ("END -> ONWARD".to_owned(), Some(Plotted::Stopped)),
            ],
            "a stopped leg's row went with its search",
        );
        // And each goes on naming the two ends it was asked between: the
        // route is what is missing, not the systems. Three of them over two
        // legs, the middle stop being both legs' own.
        assert_eq!(
            app.world().resource::<crate::systems::filter::Filters>().routed(),
            std::collections::HashSet::from([
                crate::testing::boxel_at([0., 0., 0.]),
                crate::testing::boxel_at([10., 0., 0.]),
                crate::testing::boxel_at([20., 0., 0.]),
            ]),
            "a stopped leg let go of where it was going",
        );
    }

    /// The route a row names, for asking it over
    fn row_of(app: &App, at: usize) -> crate::systems::filter::Filter {
        app.world()
            .resource::<crate::systems::filter::Filters>()
            .get(at)
            .expect("a row")
            .filter
            .clone()
    }

    /// Ask the route `route` again, as its row's own mark does
    fn again(app: &mut App, route: crate::systems::filter::Filter) {
        app.world_mut().write_message(Search::Replot(route));
        app.update();
    }

    /// A leg that was stopped can be asked again, from its own row
    ///
    /// The whole of what the row standing there is for: the ask is the row's
    /// — its two ends, its ship, how hard the search was told to work — so a
    /// leg that was stopped, or one that came back with no route, is tried
    /// again without the form having to be filled in a second time.
    #[test]
    fn a_stopped_leg_can_be_asked_again() {
        let (mut app, _dir) = plotting();

        trip(&mut app, &["Start", "End", "Onward"]);
        app.world_mut().write_message(crate::search::Search::Stop);
        app.update();
        assert!(legs(&app).is_empty(), "the stop left a search running");

        let leg = row_of(&app, 1);
        again(&mut app, leg);

        assert_eq!(
            legs(&app),
            vec![("END".to_owned(), "ONWARD".to_owned())],
            "the leg was not asked again",
        );
        assert_eq!(
            rows(&app),
            vec![
                ("START -> END".to_owned(), Some(Plotted::Stopped)),
                ("END -> ONWARD".to_owned(), Some(Plotted::Searching)),
            ],
            "asking one leg again disturbed the other",
        );
        // And the form is waiting again, as it is when the plot button is
        // pressed: the spinner turns and the stop gesture is the way out.
        assert_eq!(
            *app.world().resource::<crate::search::Plot>(),
            crate::search::Plot::Working,
        );
    }

    /// And a leg asked again while it runs is taken back first
    ///
    /// From nothing is the whole of the gesture: one search over that leg,
    /// not two racing to land in the one row. The search it takes back is
    /// found by what it asks and not by the string it was asked with — the
    /// row is named off the names table and the leg is keyed on what the
    /// reader typed, which here is neither spelled the other's way.
    #[test]
    fn a_leg_asked_again_while_it_runs_is_taken_back_first() {
        let (mut app, _dir) = plotting();

        app.world_mut().write_message(Search::Route {
            stops: vec!["start".into(), "end".into()],
            range: "10".into(),
            drive: Drive::Unaided,
            how: Routing::default(),
        });
        app.update();
        assert_eq!(legs(&app), vec![("start".to_owned(), "end".to_owned())]);

        let leg = row_of(&app, 0);
        again(&mut app, leg);

        assert_eq!(
            legs(&app),
            vec![("START".to_owned(), "END".to_owned())],
            "the leg is being searched twice over",
        );
        assert_eq!(
            rows(&app),
            vec![("START -> END".to_owned(), Some(Plotted::Searching))],
            "asking again left a second row",
        );
    }

    /// And a filter that was never plotted has nothing to ask again
    #[test]
    fn a_faction_cannot_be_asked_again() {
        let (mut app, _dir) = plotting();

        again(
            &mut app,
            crate::systems::filter::Filter::Faction {
                id: 7,
                name: "Some Lot".to_owned(),
            },
        );

        assert!(legs(&app).is_empty(), "a faction was searched for");
        assert_eq!(
            *app.world().resource::<crate::search::Plot>(),
            crate::search::Plot::Nothing,
            "the form was told it was waiting on a faction",
        );
    }

    /// A leg asked for again while it runs keeps the one row it has
    #[test]
    fn a_leg_asked_again_is_the_row_it_already_has() {
        let (mut app, _dir) = plotting();

        plot(&mut app);
        plot(&mut app);

        assert_eq!(
            rows(&app),
            vec![("START -> END".to_owned(), Some(Plotted::Searching))],
        );
    }

    /// Asking for a route takes back whatever was picked out
    ///
    /// A route just asked for is the one the user is looking at, so the one
    /// they had picked out stands down and the fall back does the rest. At
    /// the ask rather than when the answer lands, the row being up from the
    /// ask onwards.
    #[test]
    fn asking_for_a_route_takes_back_what_was_picked_out() {
        let (mut app, _dir) = plotting();
        app.insert_resource(crate::systems::route::SelectedFilter(vec![
            crate::systems::filter::Filter::Faction {
                id: 7,
                name: "Some Lot".to_owned(),
            },
        ]));

        plot(&mut app);

        assert!(
            app.world()
                .resource::<crate::systems::route::SelectedFilter>()
                .0
                .is_empty(),
        );
    }

    /// The stops a plotted route came back with, or nothing if it was never
    /// asked for
    fn walked(app: &mut App) -> Option<Vec<i64>> {
        let mut tasks = app.world_mut().resource_mut::<FetchTasks>();
        let (_, (task, _)) = tasks
            .fetched
            .iter_mut()
            .find(|(index, _)| matches!(index, FetchIndex::Route(..)))?;
        let hops = bevy::tasks::block_on(task);
        Some(hops.iter().map(|hop| hop.address).collect())
    }

    /// A route is walked off the resident graph, on its own ask
    ///
    /// It used to be registered on the spyglass region fetch and carried that
    /// system's run condition, so the moment the walk became the map's source
    /// plotting a route resolved its two ends and then asked for nothing at
    /// all: no hops, no line, no framing. A route belongs to no source — it is
    /// walked over the resident jump graph and named out of the resident names
    /// table — so it is asked for on its own and gated by nothing.
    #[test]
    fn a_route_is_walked_on_its_own_ask() {
        let (mut app, _dir) = plotting();

        plot(&mut app);

        assert_eq!(
            walked(&mut app),
            Some(vec![
                crate::testing::boxel_at([0., 0., 0.]),
                crate::testing::boxel_at([10., 0., 0.]),
            ]),
            "no way across the resident graph",
        );
    }

    /// The map holding a star for each of `addresses`
    fn on_the_map(addresses: &[i64]) -> HashSet<i64> {
        addresses.iter().copied().collect()
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
}
