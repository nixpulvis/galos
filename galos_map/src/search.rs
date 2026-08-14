use crate::Db;
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future::poll_once;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use elite_journal::system::Coordinate;
use galos_db::Database;
use galos_db::systems::System as DbSystem;
use std::time::{Duration, Instant};

pub fn plugin(app: &mut App) {
    app.add_message::<Search>();
    app.init_resource::<SearchNote>();
    app.init_resource::<SearchResults>();
    app.init_resource::<Plot>();
    app.init_resource::<Searching>();
    app.init_resource::<Locating>();
    app.add_systems(Update, searched.in_set(MapSet::Search));
}

/// How many systems a search answers with
///
/// Enough that the one wanted is among them, and few enough that the database
/// is asked for a screenful rather than for every system holding a common
/// fragment. `%col%` matches better than a hundred thousand of them.
const RESULTS: i64 = 25;

/// The systems the last search found, for the bar to draw
///
/// The database's own rows rather than the map's [`System`]s. Roughly three
/// quarters of the systems on record have no position, and those are worth
/// showing: a user searching a name wants to know it exists even where the
/// map cannot draw it. A map [`System`] is a thing with somewhere to be, so
/// the unplaceable ones would have to be dropped to hold them here, and the
/// answer would be missing the systems it most needs to account for.
#[derive(Resource, Default)]
pub struct SearchResults(Vec<DbSystem>);

impl SearchResults {
    /// What was found, best first
    pub fn iter(&self) -> impl Iterator<Item = &DbSystem> {
        self.0.iter()
    }

    /// Whether anything was found
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Hold `found` in place of what the last search found
    pub fn set(&mut self, found: Vec<DbSystem>) {
        self.0 = found;
    }

    /// Put the list away
    ///
    /// What a search turned up answers the name it was asked about, so it is
    /// no answer at all once that name is being typed over. Picking one of
    /// them out does not put the list away: choosing is what it is for, and
    /// several may be chosen from the one list.
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// What to tell the user about the last name they searched for
///
/// Roughly three quarters of the systems on record have no coordinates, Sol
/// among them. Flying to one is impossible, and doing nothing at all reads
/// exactly like the name not being in the database.
#[derive(Resource, Default)]
pub struct SearchNote(pub Option<String>);

/// How the route last asked for is getting on
///
/// Routing is worked out against the database in the background, and until
/// it comes back nothing is drawn. Neither is anything drawn for a route
/// that was asked for and does not exist, so without somewhere to say which
/// is which, a plot still being worked out and one that failed look exactly
/// alike: nothing happens either way.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub enum Plot {
    /// Nothing has been asked for, or what was asked for is drawn
    #[default]
    Nothing,
    /// Asked for, and not yet come back
    Working,
    /// Why the route could not be plotted
    Failed(String),
}

/// A collection of search messages for responding to the user's UI
/// interactions.
///
/// A filter is not one of these. Asking for a filter names something and the
/// map neither goes there, fetches it, nor picks it out, so it is asked for
/// by [`crate::systems::filter::Lookup`] instead.
#[derive(Message, Debug)]
pub enum Search {
    System { name: String },
    Route { start: String, end: String, range: String },
}

/// The row for a named system a route may run to, or why it may not
///
/// Both ends go through this before a route is drawn between them, so that a
/// plot which comes back with nothing says which end it could not have rather
/// than leaving the user to guess.
async fn locate(db: &Database, name: &str) -> Result<DbSystem, String> {
    match DbSystem::fetch_by_name(db, name).await {
        Ok(system) if system.position.is_some() => Ok(system),
        Ok(system) => Err(format!("{} has no position on record", system.name)),
        Err(_) => Err(format!("No system named {name}")),
    }
}

/// How long an answer may be coming before the field says it is waiting
///
/// Under this a search reads as instant, and a spinner that came and went
/// inside a twentieth of a second would be a flicker rather than an answer:
/// most searches land in a millisecond or two. Over it the user is waiting on
/// the database, and a field that says nothing while they wait is a field
/// that looks like it did not hear.
const PATIENCE: Duration = Duration::from_millis(50);

/// A question the bar has put to the database, and how long it has been out
///
/// One at a time, whichever field is asking. A name typed over the last one
/// replaces the question rather than racing it: two answers landing would
/// leave whichever finished last on screen, and that is not the one being
/// waited on.
///
/// `A` is whatever the answer will have to be read against, which is usually
/// the name that was asked about.
///
/// Waited for off the main thread, unlike the name that started it. A search
/// for a letter or two matches most of the systems on record and is sorted
/// before it is cut to [`RESULTS`], which is a third of a second against the
/// database here. Waited for in the frame, that is a third of a second of a
/// map that does not move.
#[derive(Resource)]
pub struct Pending<A, T> {
    out: Option<(A, Instant, Task<T>)>,
    waiting: bool,
}

impl<A, T> Default for Pending<A, T> {
    fn default() -> Self {
        Pending { out: None, waiting: false }
    }
}

impl<A, T> Pending<A, T> {
    /// Put `task` to the database, in place of whatever was already out
    pub(crate) fn ask(&mut self, about: A, now: Instant, task: Task<T>) {
        self.out = Some((about, now, task));
    }

    /// What came back, if it has, and what it is about
    ///
    /// Also settles whether the field should say it is waiting, this being
    /// where both the clock and the question are in hand.
    pub(crate) fn answered(&mut self, now: Instant) -> Option<(A, T)> {
        let Some((about, since, mut task)) = self.out.take() else {
            self.waiting = false;
            return None;
        };

        match block_on(poll_once(&mut task)) {
            Some(answer) => {
                self.waiting = false;
                Some((about, answer))
            }
            None => {
                self.waiting = now.duration_since(since) >= PATIENCE;
                self.out = Some((about, since, task));
                None
            }
        }
    }

    /// Whether the answer has been long enough coming to say so
    ///
    /// What the spinner in the field is drawn from. Read rather than worked
    /// out where it is drawn, since the bar draws during egui's own pass and
    /// has no clock of its own to hand.
    pub fn waiting(&self) -> bool {
        self.waiting
    }
}

/// What the search box has out, and the name it is about
///
/// The name is what its note is written against, a search that found nothing
/// having to say which name found it.
pub type Searching = Pending<String, Vec<DbSystem>>;

/// The pair of names a route is being worked out between, while they are
/// being looked up
///
/// One at a time for the reason a search is, and asked apart from the route
/// itself, which `systems::fetch` has already sent off. This settles only
/// which of the two ends the user got wrong, so what it answers with is that
/// trouble or nothing at all.
type Locating = Pending<(), Option<String>>;

/// Say what a pair of names being looked up had to say about the plot
///
/// Only trouble. Whether a route is still being worked out is the route's own
/// business: it is asked for in the same breath as this and comes back with a
/// line drawn or with nothing, either of which is a later and better answer
/// than that the two names were spelled right.
///
/// So a pair that resolved says nothing rather than saying the plot is still
/// working. The two are raced, and there is no order to them: the names are
/// two indexed lookups and the route is a search through the systems between
/// them, so the names usually land first, but nothing holds them to it. Landing
/// second and writing [`Plot::Working`] would take back the answer the route
/// had already given, and nothing would put it right again, the only thing
/// that clears `Working` being a route landing while it stands. That is a
/// spinner that turns for the rest of the session over a route that is drawn.
fn located(plot: &mut Plot, trouble: Option<String>) {
    if let Some(why) = trouble {
        *plot = Plot::Failed(why);
    }
}

/// Answer what the user asked for
///
/// A system for responding to [`Search`] messages.
/// - On [`Search::System`] the systems that might be meant are looked up and
/// left as a list for the user to choose from. Nothing is picked out by the
/// search itself, however exactly the name was typed: a search says which
/// systems are on record under that name and a click says which of them is
/// meant. Keeping the two apart is what lets a set be gathered across
/// searches, since a search that picked something out would let go of
/// everything gathered before it. The camera is left where it is for the same
/// reason, and the map has a control of its own for going there.
/// - On [`Search::Route`] both ends are resolved, and which of them could
/// not be is what the form is told.
///
/// Every one of them is asked of the database off the main thread and read
/// back here when it lands, so that a search the database takes its time over
/// is a list that arrives late rather than a map that stops.
///
/// The search is measured from where the camera is looking, so that a common
/// fragment answers with the systems in front of the user rather than with
/// whichever ones the database reached first.
fn searched(
    mut search_events: MessageReader<Search>,
    mut searching: ResMut<Searching>,
    mut locating: ResMut<Locating>,
    mut note: ResMut<SearchNote>,
    mut results: ResMut<SearchResults>,
    mut plot: ResMut<Plot>,
    time: Res<Time<Real>>,
    camera: Query<&OrbitCamera>,
    db: Res<Db>,
) {
    let now = time.last_update().unwrap_or(time.startup());
    let near = camera
        .single()
        .map(|camera| Coordinate {
            x: camera.center.x,
            y: camera.center.y,
            z: camera.center.z,
        })
        .ok();
    let pool = AsyncComputeTaskPool::get();

    for event in search_events.read() {
        match event {
            Search::System { name, .. } => {
                let db = db.0.clone();
                let asked = name.clone();
                searching.ask(
                    name.clone(),
                    now,
                    pool.spawn(async move {
                        DbSystem::search_by_name(&db, &asked, near, RESULTS)
                            .await
                            .unwrap_or_default()
                    }),
                );
            }
            // A route needs both ends. Say which one is the problem rather
            // than drawing nothing and leaving the user to guess.
            Search::Route { start, end, .. } => {
                let db = db.0.clone();
                let (start, end) = (start.clone(), end.clone());
                locating.ask(
                    (),
                    now,
                    pool.spawn(async move {
                        // The one nearer the start of the form, and only it:
                        // an end looked up after the one before it turned out
                        // to be wrong is a lookup whose answer nothing reads.
                        match locate(&db, &start).await {
                            Err(why) => Some(why),
                            Ok(_) => locate(&db, &end).await.err(),
                        }
                    }),
                );
            }
        };
    }

    if let Some((name, found)) = searching.answered(now) {
        answered(&name, found, &mut note, &mut results);
    }

    if let Some((_, trouble)) = locating.answered(now) {
        located(&mut plot, trouble);
    }
}

/// Put what came back for `name` on screen
///
/// Whatever is listed answered the last name asked about, and this is a
/// new one, so it goes whether or not anything came back to replace it. What
/// is picked out is left alone: it was picked out by a click rather than by
/// the search before it, and a search is not a reason to let go of it.
///
/// A name that found nothing is said in the note rather than left as an empty
/// list. Nothing on screen is what the map looks like before anything has
/// been asked, and the two have to be told apart.
fn answered(
    name: &str,
    found: Vec<DbSystem>,
    note: &mut SearchNote,
    results: &mut SearchResults,
) {
    results.clear();
    note.0 = if found.is_empty() {
        Some(format!("No system named {name}"))
    } else {
        results.set(found);
        None
    };
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A system the database might answer a search with
    pub(crate) fn row(name: &str) -> DbSystem {
        DbSystem {
            address: name.len() as i64,
            name: name.to_owned(),
            position: Some(Coordinate { x: 0., y: 0., z: 0. }),
            population: 0,
            security: None,
            government: None,
            allegiance: None,
            economies: None,
            factions: vec![],
            body_count: None,
            non_body_count: None,
            updated_at: chrono::DateTime::UNIX_EPOCH,
            updated_by: String::new(),
        }
    }

    /// A name that found nothing is said, rather than left as an empty list
    #[test]
    fn a_name_that_found_nothing_is_said() {
        let mut note = SearchNote(None);
        let mut results = SearchResults::default();

        answered("NOWHERE", Vec::new(), &mut note, &mut results);

        assert_eq!(note.0.as_deref(), Some("No system named NOWHERE"));
        assert!(results.is_empty());
    }

    /// What was found is listed, and nothing is said about it
    #[test]
    fn what_was_found_is_listed() {
        let mut note = SearchNote(Some("No system named SOL".to_owned()));
        let mut results = SearchResults::default();

        answered(
            "SOL",
            vec![row("SOL"), row("SOLATI")],
            &mut note,
            &mut results,
        );

        assert_eq!(note.0, None);
        assert_eq!(results.iter().count(), 2);
    }

    /// A name that could not be had is said
    ///
    /// Which is the whole of what looking the two ends up is for: a plot that
    /// came back with nothing says only that, and this says which end it could
    /// not have had.
    #[test]
    fn an_end_that_could_not_be_had_is_said() {
        let mut plot = Plot::Working;

        located(&mut plot, Some("No system named NOWHERE".to_owned()));

        assert_eq!(plot, Plot::Failed("No system named NOWHERE".to_owned()));
    }

    /// Both ends resolving leaves the plot as it stands
    ///
    /// The route is asked for in the same breath and is the better answer
    /// wherever it has landed. Saying anything here would only ever say
    /// something the route has already said better, or take back what it said.
    #[test]
    fn a_pair_that_resolved_says_nothing() {
        let mut working = Plot::Working;
        let mut drawn = Plot::Nothing;
        let mut refused =
            Plot::Failed("No route from SOL to BARNARD at 10 Ly".to_owned());

        located(&mut working, None);
        located(&mut drawn, None);
        located(&mut refused, None);

        assert_eq!(working, Plot::Working);
        assert_eq!(drawn, Plot::Nothing);
        assert_eq!(
            refused,
            Plot::Failed("No route from SOL to BARNARD at 10 Ly".to_owned())
        );
    }

    /// The last answer goes whether or not this one replaces it
    ///
    /// Both halves of it: the list and the note answered a name that is no
    /// longer the name being asked about.
    #[test]
    fn a_fresh_answer_takes_the_last_one_away() {
        let mut note = SearchNote(None);
        let mut results = SearchResults::default();
        answered("SOL", vec![row("SOL")], &mut note, &mut results);

        answered("NOWHERE", Vec::new(), &mut note, &mut results);

        assert!(results.is_empty());
        assert_eq!(note.0.as_deref(), Some("No system named NOWHERE"));
    }
}
