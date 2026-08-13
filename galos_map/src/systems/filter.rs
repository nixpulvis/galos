//! Picking the map out against the rest of the sky
//!
//! A filter is a question asked of every system, and several are asked at
//! once: a system is admitted when any one of them admits it. Each adds to
//! what is picked out rather than cutting into it, every filter being
//! something the user asked to see.
//!
//! Except the one on time, which is asked of all of them. A span names no
//! systems, only how lately one was heard from, so two factions and a span
//! mean either faction heard from within it. Counted alongside the factions it
//! would put the whole of the last hour onto a map asked for two factions.
//!
//! This is a layer over the map rather than a mode. The spyglass goes on
//! fetching by region, the camera stays where it is, and nothing is
//! despawned.
//!
//! [`DimTo`] says how faintly what none of them admits is drawn, and answers
//! for the whole of it. Above zero the excluded systems are wanted on screen,
//! so they are fetched to be dimmed: what was never asked for cannot be drawn
//! faintly, and a faction read against the space around it is the thing being
//! drawn. At zero they are not drawn at all, which is the other thing a filter
//! is asked for: this kind of system and none of the rest.

use crate::Db;
use crate::schedule::MapSet;
use crate::search::Pending;
use crate::systems::System;
use crate::systems::fetch::Poll;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::tasks::AsyncComputeTaskPool;
use chrono::{DateTime, Duration, Utc};
use galos_db::Database;
use galos_db::factions::Faction as DbFaction;
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.init_resource::<Filters>();
    app.init_resource::<LastCutAt>();
    app.init_resource::<Watch>();
    app.init_resource::<Standstill>();
    app.init_resource::<DimTo>();
    app.init_resource::<LookupNote>();
    app.init_resource::<Resolving>();
    app.init_resource::<FactionResults>();
    app.add_message::<Lookup>();
    // Answering what the user asked for, so with the rest of that.
    app.add_systems(Update, resolve.in_set(MapSet::Search));
    // After the systems it marks exist. A system spawned this frame is
    // marked by `spawn` itself, since commands do not land until the next
    // sync point and nothing here could see it in time.
    app.add_systems(
        Update,
        mark.in_set(MapSet::Populate).after(super::spawn::spawn),
    );
}

/// When the filters were last cut afresh against the clock
///
/// Its own memory rather than the fetch's. A span's near edge moves with the
/// clock, so what it admits has to be settled again as time passes, and that
/// is a walk of every system on the map rather than a query.
#[derive(Resource)]
struct LastCutAt(Instant);

impl Default for LastCutAt {
    fn default() -> LastCutAt {
        LastCutAt(Instant::now())
    }
}

/// How many systems a question about time is answered with at most
///
/// An hour of the feed touches about nine thousand of them, so this is a few
/// hours of it. The bound is there because the question has a far end that means
/// every system on record, and neither the map nor a list of rows is any use
/// holding a million of them.
pub(crate) const MOST: i64 = 10_000;

/// How far back the control over time offers to look, longest first
///
/// Named spans rather than a bare number of seconds. The interesting end of
/// this is the last few minutes and the far end is a database going back years,
/// so a rail laid out evenly in seconds would spend nearly all of its length in
/// country nobody wants and cross the useful part in a pixel.
///
/// Nothing at the near end. A span reaching back forever admits every system on
/// record, which is what asking nothing does, and putting the question anyway
/// would fetch ten thousand rows to say so.
pub(crate) const SPANS: [(&str, Option<i64>); 9] = [
    ("Off", None),
    ("30 days", Some(30 * 24 * 60 * 60)),
    ("7 days", Some(7 * 24 * 60 * 60)),
    ("1 day", Some(24 * 60 * 60)),
    ("6 hours", Some(6 * 60 * 60)),
    ("1 hour", Some(60 * 60)),
    ("15 minutes", Some(15 * 60)),
    ("5 minutes", Some(5 * 60)),
    ("1 minute", Some(60)),
];

/// Where the control over time stands, by its place in [`SPANS`]
///
/// Kept because [`Filter::Recency`] holds the moment the span worked out to be
/// rather than the span itself, and a minute later those are different things.
/// Reading the control back off the filter would have it drift a step to the
/// left every step of the way.
#[derive(Resource, Default)]
pub struct Watch(pub usize);

impl Watch {
    /// What the control says it is set to
    pub fn name(&self) -> &'static str {
        SPANS.get(self.0).map(|(name, _)| *name).unwrap_or("Off")
    }

    /// How far back it reaches, or nothing where it is off
    pub fn span(&self) -> Option<Duration> {
        SPANS.get(self.0).and_then(|(_, secs)| *secs).map(Duration::seconds)
    }
}

/// One question asked of every system
///
/// The id is what a system is tested against and the name is what the filter
/// row says it is. Both are settled when the faction is picked out of a list,
/// so neither has to be looked up again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Filter {
    /// Systems the named faction is present in
    Faction { id: i32, name: String },
    /// The systems a plotted route runs through
    ///
    /// Unlike a faction, this is nothing a system knows about itself. A route
    /// is worked out rather than recorded, so the filter carries the answer it
    /// came back with: the addresses it runs through, in the order they are
    /// travelled.
    ///
    /// That order is the whole of what a route is, so it is what is kept.
    /// Holding them sorted instead would make asking whether a system is on
    /// the route a search rather than a walk, and would lose the sequence in
    /// exchange: a route is tens of systems long, so the walk costs little,
    /// and nothing else could put them back in order afterwards.
    ///
    /// `label` is what its row says, settled when the route landed, since it
    /// names the two ends as the database spells them rather than as they
    /// were typed.
    ///
    /// `range` is how far the ship it was plotted for reaches in one jump, in
    /// light years, as the user typed it. The two ends are what a route is
    /// named for and they are not the whole of what it is: the same pair
    /// plotted for a ship that reaches further is a different route through
    /// different systems, and with nothing but the ends to go on the two are
    /// one name over two answers. Kept as the text that was typed, being a
    /// number the map only ever reads back out.
    ///
    /// What the ship can cross rather than what it does. The legs of the route
    /// are shorter, each landing on whatever system lies within reach rather
    /// than out at the limit of it.
    Route { label: String, systems: Vec<i64>, range: String },
    /// The systems the user picked out by hand
    ///
    /// A copy of what was selected rather than a reading of the selection as
    /// it stands. Taking a copy is what makes the filter worth having: the
    /// rings and the rows can be let go of and those systems stay picked out
    /// against the rest.
    ///
    /// `label` says how many, a hand-picked set having no name of its own.
    Systems { label: String, systems: Vec<i64> },
    /// The systems heard from since a moment
    ///
    /// Bounded by [`MOST`], the far end of the control putting this question
    /// being every system on record.
    ///
    /// What the feed is doing, drawn. The others ask something about a system
    /// that holds still while it is asked; this one goes stale as it is
    /// answered, since the moment it names sits at a fixed distance behind now
    /// and everything crosses it eventually.
    ///
    /// So it is the one filter that is edited rather than added. Asking again
    /// with a moment a second later is the same question, not a second one, and
    /// [`Filters::add`] would refuse the second anyway for being a duplicate of
    /// nothing it recognises.
    ///
    /// The database keeps when a system last changed and not a trail of every
    /// time it did, so `moment` bounds a window whose other end is always now.
    /// It cannot be slid back to look at the galaxy as it stood: parked in the
    /// past it picks out what has not been heard from since, which is the same
    /// question read the other way round.
    Recency { label: String, span: Duration },
}

impl Filter {
    /// Whether this filter admits `system`
    fn admits(&self, system: &System, now: DateTime<Utc>) -> bool {
        match self {
            Filter::Faction { id, .. } => system.factions.contains(id),
            Filter::Route { systems, .. } => systems.contains(&system.address),
            Filter::Systems { systems, .. } => {
                systems.contains(&system.address)
            }
            Filter::Recency { span, .. } => system.updated_at >= now - *span,
        }
    }

    /// Whether a panel could say anything about what this admits
    ///
    /// A faction, a route and a hand-picked set each admit a set of systems
    /// worth reading as a list: a few, or a few tens, settled and standing
    /// still while the panel is open.
    ///
    /// A span admits neither. What it holds is scattered across the galaxy and
    /// changes while it is being read, so a list of it is a question over every
    /// system on record answered with an arbitrary slice of the newest. What a
    /// span admits is on the map already, which is where it is worth looking.
    pub fn worth_describing(&self) -> bool {
        !matches!(self, Filter::Recency { .. })
    }

    /// Whether this is a route
    ///
    /// What the bar groups its rows by and what the map draws a line for. Its
    /// own question rather than a reading of [`Self::ordered`] or of
    /// [`Self::range`], which happen to answer the same today and are about
    /// what a route is like rather than about what it is.
    pub fn is_route(&self) -> bool {
        matches!(self, Filter::Route { .. })
    }

    /// Whether what it admits has an order of its own
    ///
    /// A route is travelled from one end to the other, so its systems are a
    /// sequence and reading them in any other order loses what they are. A
    /// faction's are a set, with nothing in them to say which comes first, so
    /// whoever lists those may put them in whatever order suits the reader.
    pub fn ordered(&self) -> bool {
        matches!(self, Filter::Route { .. })
    }

    /// Where the system at `address` falls in what this admits
    ///
    /// Nothing for a filter with no order of its own, and nothing for a
    /// system it does not admit at all.
    pub fn place_of(&self, address: i64) -> Option<usize> {
        match self {
            Filter::Faction { .. }
            | Filter::Systems { .. }
            | Filter::Recency { .. } => None,
            Filter::Route { systems, .. } => {
                systems.iter().position(|on| *on == address)
            }
        }
    }

    /// How many jumps what this admits is flown in
    ///
    /// A route of two systems is one jump, so it is one fewer than what the
    /// route runs through. Nothing for a filter that is not flown at all, and
    /// nothing for a route with nothing in it, there being no leg to count.
    ///
    /// What a row about a route says at its end, as a row about a system says
    /// how far off it is. A route is named for its two ends, and how many
    /// jumps lie between them is what it was plotted to find out.
    pub fn hops(&self) -> Option<usize> {
        match self {
            Filter::Faction { .. }
            | Filter::Systems { .. }
            | Filter::Recency { .. } => None,
            Filter::Route { systems, .. } => systems.len().checked_sub(1),
        }
    }

    /// How far the ship this was plotted for reaches, where one was named
    ///
    /// The one thing a route was asked for that its name does not say. A
    /// faction and a hand-picked set are not plotted, so there is nothing to
    /// ask them.
    ///
    /// Not how far it goes. That is a jump, and there is one of those between
    /// each pair of systems the route runs through.
    pub fn range(&self) -> Option<&str> {
        match self {
            Filter::Faction { .. }
            | Filter::Systems { .. }
            | Filter::Recency { .. } => None,
            Filter::Route { range, .. } => Some(range),
        }
    }

    /// What the filter is asking for, as a row can say it
    pub fn name(&self) -> &str {
        match self {
            Filter::Faction { name, .. } => name,
            Filter::Route { label, .. }
            | Filter::Systems { label, .. }
            | Filter::Recency { label, .. } => label,
        }
    }

    /// Every system this filter admits, as far as the database knows
    ///
    /// What a filter's panel lists. Asked of the database rather than of the
    /// map, since where a faction is, is most of what is being asked, and the
    /// map holds only what the spyglass has dragged in.
    ///
    /// Here rather than beside the panel that draws it, so that a kind of
    /// filter is one arm of each of these rather than something to be traced
    /// through the modules that happen to use it.
    pub async fn systems(&self, db: &Database) -> Vec<DbSystem> {
        match self {
            Filter::Faction { name, .. } => {
                DbSystem::fetch_faction(db, name).await.unwrap_or_default()
            }
            Filter::Route { systems, .. } | Filter::Systems { systems, .. } => {
                DbSystem::fetch_many(db, systems).await.unwrap_or_default()
            }
            // Nothing describes a span, so nothing asks this of one. See
            // [`Self::worth_describing`].
            Filter::Recency { .. } => Vec::new(),
        }
    }
}

/// A name to be resolved into the id a filter tests against
///
/// What is typed is part of a name and what a filter tests against is an id,
/// so something has to find the one from the other. That is a database
/// question, asked here rather than in the bar, which draws during egui's own
/// pass and has no business waiting on anything.
///
/// Its own message rather than a [`crate::search::Search`]: asking for a
/// filter is not searching the map. It goes nowhere, fetches nothing in
/// particular, and picks nothing out.
#[derive(Message, Debug, Clone)]
pub enum Lookup {
    /// Factions whose names hold this
    Faction { name: String },
}

/// What became of the last name looked up
///
/// What was found is a list rather than a state, so this says only what a
/// list cannot: that a name matched nothing at all. Nothing on screen is what
/// the bar looks like before anything has been asked, and the two have to be
/// told apart.
///
/// Its own resource rather than [`crate::search::SearchNote`], which answers a
/// name typed into the search input. Two unrelated answers sharing one line
/// means each wipes the other: asking about a faction would clear a note about
/// a system that was never found, and the note about a faction would be read
/// out under the box that has nothing to do with it.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub enum LookupNote {
    /// Nothing has been asked about, or what was found is standing in a list
    #[default]
    Nothing,
    /// Why there is no list
    Failed(String),
}

/// How many factions a search answers with
///
/// A screenful of the bar, as a search for a system is answered with.
const FACTIONS: i64 = 25;

/// The search under way, if there is one
///
/// One at a time: the field asks about one name and holds what was typed
/// until it is answered, so a second ask is the user having changed their
/// mind rather than a second question.
pub type Resolving = Pending<String, Vec<DbFaction>>;

/// The factions the last search found, for the bar to draw
///
/// Carrying the id as well as the name, which is what a filter tests against.
/// A faction picked out of this list is a filter already, with nothing left to
/// look up: the search asked the question the lookup would have asked.
#[derive(Resource, Default)]
pub struct FactionResults(Vec<DbFaction>);

impl FactionResults {
    /// What was found, best first
    pub fn iter(&self) -> impl Iterator<Item = &DbFaction> {
        self.0.iter()
    }

    /// Whether anything was found
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Stop offering whatever was found
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// Look up the factions a typed name might mean
///
/// Asked of the database off the main thread. A name is matched however it
/// was typed, which the index on the factions cannot answer, so this is a walk
/// of every faction on record. Done in the frame, that is the map stopping for
/// it.
///
/// The answer is a list rather than a filter. A name part way through being
/// typed means whichever factions hold it, and which of those was meant is a
/// question only the user can answer, so the list is offered and a click
/// chooses, exactly as a search for a system is answered.
fn resolve(
    mut lookups: MessageReader<Lookup>,
    mut resolving: ResMut<Resolving>,
    mut results: ResMut<FactionResults>,
    mut note: ResMut<LookupNote>,
    time: Res<Time<Real>>,
    db: Res<Db>,
) {
    let now = time.last_update().unwrap_or(time.startup());
    let pool = AsyncComputeTaskPool::get();

    for lookup in lookups.read() {
        let Lookup::Faction { name } = lookup;
        let db = db.0.clone();
        let asked = name.clone();
        resolving.ask(
            name.clone(),
            now,
            pool.spawn(async move {
                DbFaction::search_by_name(&db, &asked, FACTIONS)
                    .await
                    .unwrap_or_default()
            }),
        );
    }

    if let Some((name, found)) = resolving.answered(now) {
        results.0 = found;
        *note = if results.is_empty() {
            LookupNote::Failed(format!("No faction named {name}"))
        } else {
            LookupNote::Nothing
        };
    }
}

/// A filter, and whether it is being applied
///
/// Off without being taken away, so that one can be lifted to see what it was
/// hiding and put back without being typed in again.
#[derive(Debug, Clone)]
pub struct Entry {
    pub filter: Filter,
    pub enabled: bool,
}

/// Every filter the user has added
#[derive(Resource, Default, Clone)]
pub struct Filters {
    asked: Vec<Entry>,
    /// How many times this has been asked for something
    ///
    /// Counted because a [`ResMut`] reads as written for being handed out, and
    /// the bar is handed the filters every frame it draws whether or not the
    /// user touched anything. What reads that mark puts a query on the wire,
    /// so a set that was only drawn has to be told from one that was asked.
    revision: u32,
}

/// The time filter's row, held back while a control is held
///
/// The rows are drawn above the controls that ask for filters, so a filter
/// asked for part way through a gesture puts a row above the control being
/// used and takes it down a row. Egui follows a drag by the widget it began
/// on, and a control that has moved is one the pointer is no longer over.
///
/// Only a row that was not there when the press landed is held back, and only
/// while the control is held. One that was there is drawn as it stands, so
/// what it says follows the drag: a span dragged from one to the next says the
/// span it has reached, and dragged to the far end says it is off.
///
/// Nothing holds a row the other way, because nothing takes one away mid-drag.
/// A control slid to its far end stops asking without letting go of its row,
/// by [`Filters::turn_time_off`], and the row goes when the control does.
#[derive(Resource, Default)]
pub struct Standstill {
    /// Whether a control is being held
    held: bool,
    /// Whether a filter on time was being asked when the press landed
    asked: bool,
}

impl Standstill {
    /// Hold the rows where the press finds them in `rows`
    pub fn hold(&mut self, rows: &Filters) {
        self.held = true;
        self.asked = rows.timed().is_some();
    }

    /// Let them catch up with what has been asked for
    pub fn release(&mut self) {
        self.held = false;
        self.asked = false;
    }

    /// `rows` as they are to be drawn, where a control is being held
    ///
    /// Nothing where none is, the rows being drawn as they stand.
    pub fn rows(&self, rows: &Filters) -> Option<Filters> {
        if !self.held {
            return None;
        }

        let mut standing = rows.clone();
        if !self.asked
            && let Some(at) = standing.timed()
        {
            standing.asked.remove(at);
        }

        Some(standing)
    }
}

impl Filters {
    /// How many times these have been asked for something
    ///
    /// Compared before and after the bar is drawn, so that a set only drawn is
    /// not reported as one the user changed.
    pub fn revision(&self) -> u32 {
        self.revision
    }

    /// Where the filter on time stands, where one is being asked
    fn timed(&self) -> Option<usize> {
        self.asked
            .iter()
            .position(|active| matches!(active.filter, Filter::Recency { .. }))
    }

    /// Whether the enabled filters admit `system`
    ///
    /// The filters that pick systems out admit between them, so each adds to
    /// what is shown rather than cutting into it. Every one is something the
    /// user asked to see, and a second ask is a second thing wanted rather
    /// than a condition on the first: asking for a faction and then for a
    /// route means both, where taking the systems they share would usually
    /// mean nothing at all, the two rarely overlapping.
    ///
    /// Time is asked of all of them instead. It names no systems, only how
    /// lately one was heard from, so a faction and a span together mean that
    /// faction's systems heard from within it. Counted as one more thing to
    /// show, a span would put the whole of the last hour onto a map asked for
    /// one faction.
    ///
    /// Nothing asked for admits everything. A map with no filter on it is a
    /// map showing the sky rather than an empty one.
    pub fn admit(&self, system: &System, now: DateTime<Utc>) -> bool {
        // Nothing while no filter picks systems out, which is what says a span
        // asked on its own admits whatever it reaches rather than nothing.
        let mut picked = None;

        for active in self.asked.iter().filter(|active| active.enabled) {
            match &active.filter {
                timed @ Filter::Recency { .. } => {
                    if !timed.admits(system, now) {
                        return false;
                    }
                }
                picking => {
                    *picked.get_or_insert(false) |= picking.admits(system, now);
                }
            }
        }

        picked.unwrap_or(true)
    }

    /// Add `filter`, unless it is already being asked
    ///
    /// Asking the same thing twice picks out nothing further and leaves two
    /// rows that have to be turned off one at a time.
    pub fn add(&mut self, filter: Filter) {
        if self.asked.iter().any(|active| active.filter == filter) {
            return;
        }
        self.asked.push(Entry { filter, enabled: true });
        self.revision += 1;
    }

    /// Stop asking the filter at `index`
    pub fn remove(&mut self, index: usize) {
        if index < self.asked.len() {
            self.asked.remove(index);
            self.revision += 1;
        }
    }

    /// The filters in the order they were added
    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.asked.iter()
    }

    /// Turn the filter at `index` on or off
    pub fn toggle(&mut self, index: usize) {
        if let Some(active) = self.asked.get_mut(index) {
            active.enabled = !active.enabled;
            self.revision += 1;
        }
    }

    /// How many filters are being held, turned on or not
    pub fn len(&self) -> usize {
        self.asked.len()
    }

    /// Whether none is held at all, which is a map showing the whole sky
    pub fn is_empty(&self) -> bool {
        self.asked.is_empty()
    }

    /// Whether any filter is turned on
    ///
    /// Which is whether the map is picking anything out. With every filter
    /// off the sky is drawn whole, as it is with none of them held at all.
    pub fn any_enabled(&self) -> bool {
        self.asked.iter().any(|active| active.enabled)
    }

    /// The filter standing in the `index`th place
    pub fn get(&self, index: usize) -> Option<&Entry> {
        self.asked.get(index)
    }

    /// Turn every filter at `rows` off, or every one back on
    ///
    /// Off while any of them is on, since that is the question the control
    /// answers: show me the sky as it is, and then put back what I was
    /// looking at. All of them come back rather than the ones that were on
    /// before, so the two clicks are one gesture and its undo rather than a
    /// state to be remembered.
    ///
    /// Told which rows rather than taking the lot. The bar draws its filters
    /// in sections and each has a row of its own standing over it, so what
    /// this answers is one section's worth: turning the routes off is no
    /// reason to turn the factions off with them.
    pub fn toggle_all(&mut self, rows: &[usize]) {
        let on = rows.iter().any(|index| {
            self.asked.get(*index).is_some_and(|active| active.enabled)
        });
        for index in rows {
            if let Some(active) = self.asked.get_mut(*index) {
                active.enabled = !on;
            }
        }
        self.revision += 1;
    }

    /// Stop asking every filter at `rows`
    ///
    /// Taken from the back, so that removing one does not move the next one
    /// still to be removed out from under its own index.
    pub fn clear(&mut self, rows: &[usize]) {
        let mut rows = rows.to_vec();
        rows.sort_unstable();
        for index in rows.into_iter().rev() {
            self.remove(index);
        }
    }

    /// Ask about what has been heard from since `moment`
    ///
    /// Replacing whatever moment was being asked about, rather than standing
    /// beside it. Two of these are not two things wanted: the earlier one's
    /// answer holds the later one's, so the pair would draw what the wider of
    /// them draws and read as a control that had stopped responding.
    ///
    /// The row says the span it was asked as, `span` being one of the names in
    /// [`SPANS`]. A moment on its own reads as a time of day with no date
    /// against it, which says nothing at all once the span reaching back to it
    /// is longer than a day.
    pub fn ask_within(&mut self, said: &str, span: Duration) {
        let label = format!("Last {said}");
        let asked = Filter::Recency { label, span };
        match self
            .asked
            .iter_mut()
            .find(|active| matches!(active.filter, Filter::Recency { .. }))
        {
            Some(active) => {
                active.filter = asked;
                active.enabled = true;
            }
            None => self.asked.push(Entry { filter: asked, enabled: true }),
        }
        self.revision += 1;
    }

    /// Stop asking about time, leaving the row standing and saying `said`
    ///
    /// What the control does while it is being held. Its row is drawn above
    /// it, so a row going out from under the pointer takes the control up a
    /// row and out from under it too.
    ///
    /// The row says what is filtering, which by now is nothing, so it reads as
    /// the control does. A row left saying the span it was asked as would name
    /// a question that is no longer being put.
    pub fn turn_time_off(&mut self, said: &str) {
        if let Some(active) = self
            .asked
            .iter_mut()
            .find(|active| matches!(active.filter, Filter::Recency { .. }))
        {
            if let Filter::Recency { label, .. } = &mut active.filter {
                *label = said.to_owned();
            }
            active.enabled = false;
            self.revision += 1;
        }
    }

    /// Stop asking about time at all
    ///
    /// Its own way out rather than [`Self::remove`] by index, the control being
    /// slid to nothing rather than a row being let go of.
    pub fn ask_nothing_of_time(&mut self) {
        self.asked
            .retain(|active| !matches!(active.filter, Filter::Recency { .. }));
        self.revision += 1;
    }

    /// The moment an enabled filter on time names, where one is asked
    ///
    /// Read apart from [`Self::admitted`] because what it admits is nowhere in
    /// particular, so it is fetched in its own right rather than as part of a
    /// question about a region.
    ///
    /// The earliest of them where several are somehow asked, that being the one
    /// whose answer holds the others.
    pub fn changed_since(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.asked
            .iter()
            .filter(|active| active.enabled)
            .filter_map(|active| match &active.filter {
                Filter::Recency { span, .. } => Some(now - *span),
                _ => None,
            })
            .min()
    }

    /// What the filters admit, as a query can ask it
    ///
    /// Every enabled filter says either which faction it wants or which
    /// systems by name, so all of them together are two lists. That is the
    /// whole of what a query has to be told, and it is told once however many
    /// filters there are.
    ///
    /// Nothing where they admit everything, which is where none of them is
    /// turned on. A query narrowed by two empty lists answers with nothing at
    /// all, where what is meant is the whole sky.
    pub fn admitted(&self) -> Option<Admitted> {
        let mut admitted = Admitted::default();
        let mut asked = false;

        for active in self.asked.iter().filter(|active| active.enabled) {
            match &active.filter {
                Filter::Faction { id, .. } => {
                    asked = true;
                    admitted.factions.push(*id);
                }
                Filter::Route { systems, .. }
                | Filter::Systems { systems, .. } => {
                    asked = true;
                    admitted.systems.extend(systems.iter().copied());
                }
                // Nothing a region can be narrowed by. What this admits is
                // scattered across the galaxy rather than gathered anywhere,
                // so it is fetched in its own right and not as part of a
                // place. Asked on its own it leaves the region asked for as it
                // stands, rather than narrowing it to two empty lists, which
                // is a question answered with nothing at all.
                Filter::Recency { .. } => {}
            }
        }

        asked.then_some(admitted)
    }
}

/// What a set of filters admits, said as two lists
///
/// Which is as much as the database is told. A faction is a membership to be
/// looked up and a route or a hand-picked set is its addresses outright, and
/// a system is admitted by standing in either list.
///
/// Part of what a region is asked for, so two regions about the same place
/// admitting different things are different questions. Hence [`Eq`] and
/// [`Hash`]: what tells those questions apart is these lists.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct Admitted {
    /// The factions asked for, by id
    pub factions: Vec<i32>,
    /// The systems asked for outright, by address
    pub systems: Vec<i64>,
}

/// A system no enabled filter admits
///
/// The verdict is one bit, whichever of them admitted it and however many
/// did, so it is carried by a marker and how faintly to draw lives apart in
/// [`DimTo`]. Written when the filters change or a system does, rather than
/// worked out afresh by each of the things that draws from it.
#[derive(Component)]
pub struct Filtered;

/// How opaque a system no filter admits is drawn
///
/// A fraction of the alpha it would be drawn at unfiltered, so one is
/// untouched and zero is not drawn at all. Reads as what it does: dim to a
/// fifth, dim to nothing.
///
/// Zero is not merely invisible. A star faded to nothing is still a star
/// being drawn, and still one the pointer can land on, so zero hides it
/// outright, which takes its name, its ring and its hit box with it.
#[derive(Resource)]
pub struct DimTo(pub f32);

impl Default for DimTo {
    fn default() -> Self {
        DimTo(DEFAULT_DIM)
    }
}

impl DimTo {
    /// `color` as it should be drawn for a system, given whether it is
    /// excluded
    ///
    /// The color is left alone and the alpha carries it, so that what is
    /// dimmed reads as standing further back rather than as having changed
    /// into something else.
    ///
    /// For what is painted straight rather than through a material: the two
    /// rings are gizmos, and a gizmo takes its color at the call.
    pub fn as_drawn(&self, color: Srgba, filtered: bool) -> Srgba {
        if filtered {
            Srgba { alpha: color.alpha * self.0, ..color }
        } else {
            color
        }
    }
}

/// How faint an excluded system is to begin with
///
/// Faint enough to read as background rather than as something picked out,
/// and bright enough to still be read: the point of dimming rather than
/// hiding is that the space around a faction stays legible.
const DEFAULT_DIM: f32 = 0.25;

/// Keep the mark on whichever systems the filters exclude
///
/// Only where something has changed. This runs over every system on the map,
/// and the filters are usually quiet, so the common case is a walk that
/// writes nothing.
fn mark(
    filters: Res<Filters>,
    poll: Res<Poll>,
    time: Res<Time<Real>>,
    mut last_cut_at: ResMut<LastCutAt>,
    systems: Query<(Entity, Ref<System>, Has<Filtered>)>,
    mut commands: Commands,
) {
    let now = Utc::now();
    // A span reaches back from now, so the line it draws moves whether or not
    // anything is asked afresh, and systems cross it going the other way. Cut
    // again on the [`Poll`], which is the same beat the answer to it is asked
    // for on, rather than every frame: the line moves by a frame's worth in a
    // frame, and finding out costs a walk of every system on the map.
    let running = time.last_update().unwrap_or(time.startup());
    let recut = filters.changed_since(now).is_some()
        && poll.elapsed(last_cut_at.0, running);
    if recut {
        *last_cut_at = LastCutAt(running);
    }

    let filters_changed = filters.is_changed() || recut;
    for (entity, system, marked) in &systems {
        // A row that has changed may have changed its factions, so it is
        // asked again even while the filters stand still.
        if !filters_changed && !system.is_changed() {
            continue;
        }

        match (filters.admit(&system, now), marked) {
            (false, false) => {
                commands.entity(entity).insert(Filtered);
            }
            (true, true) => {
                commands.entity(entity).remove::<Filtered>();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::tests::{heard, system};

    /// A moment `secs` after the epoch
    fn moment(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).expect("a moment")
    }

    /// The moment these are asked at
    ///
    /// Fixed, so that a span reaching back from it lands somewhere the fixtures
    /// can name. The filters that say nothing about time are asked at it too,
    /// there being one way to ask.
    fn now() -> DateTime<Utc> {
        moment(200)
    }

    /// A filter admitting whatever was heard from within `secs` of [`now`]
    fn within(secs: i64) -> Filter {
        Filter::Recency {
            label: format!("Last {secs}"),
            span: Duration::seconds(secs),
        }
    }

    /// A faction filter, by id, called after it
    fn faction(id: i32) -> Filter {
        Filter::Faction { id, name: format!("Faction {id}") }
    }

    /// A system belonging to each of `factions`
    fn member(address: i64, factions: &[i32]) -> System {
        let mut system = system(address);
        system.factions = factions.to_vec();
        system
    }

    /// A question nobody has asked is answered by nothing at all
    ///
    /// Which is what the bar looks like before a name has been typed, and
    /// what it has to look like again once one has been.
    #[test]
    fn nothing_asked_says_nothing() {
        assert_eq!(LookupNote::default(), LookupNote::Nothing);
    }

    /// With nothing asked, everything passes
    #[test]
    fn no_filter_admits_everything() {
        let filters = Filters::default();
        assert!(filters.admit(&member(1, &[]), now()));
        assert!(filters.admit(&member(2, &[7]), now()));
    }

    /// A faction filter admits the systems that faction is in
    #[test]
    fn a_faction_filter_admits_its_members() {
        let mut filters = Filters::default();
        filters.add(faction(7));

        assert!(filters.admit(&member(1, &[7]), now()));
        assert!(filters.admit(&member(2, &[3, 7]), now()));
        assert!(!filters.admit(&member(3, &[3]), now()));
        assert!(!filters.admit(&member(4, &[]), now()));
    }

    /// Two filters admit what passes either
    ///
    /// Each row is something the user asked to see, so a second row shows a
    /// second thing rather than cutting into the first. Two factions asked
    /// for together and ANDed would answer with the systems both are present
    /// in, which is usually none of them.
    #[test]
    fn filters_add_to_each_other() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));

        assert!(filters.admit(&member(1, &[7, 9]), now()));
        assert!(filters.admit(&member(2, &[7]), now()));
        assert!(filters.admit(&member(3, &[9]), now()));
        assert!(!filters.admit(&member(4, &[3]), now()));
    }

    /// A filter turned off asks nothing
    #[test]
    fn a_disabled_filter_admits_everything() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.toggle(0);

        assert!(filters.admit(&member(1, &[]), now()));
    }

    /// Turning them all off shows the sky whole, and holds on to every filter
    ///
    /// Which is the point of the row: lift the whole set to see what it was
    /// dimming, without having to type any of it in again.
    #[test]
    fn turning_them_all_off_admits_everything() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));

        filters.toggle_all(&[0, 1]);

        assert!(!filters.any_enabled());
        assert_eq!(filters.len(), 2);
        assert!(filters.admit(&member(1, &[3]), now()));
    }

    /// And the same gesture puts every one of them back
    ///
    /// All of them rather than the ones that were on before, so the second
    /// click undoes the first rather than restoring a state nobody chose.
    #[test]
    fn turning_them_all_on_asks_every_one() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));
        filters.toggle(1);

        filters.toggle_all(&[0, 1]);
        filters.toggle_all(&[0, 1]);

        assert!(filters.admit(&member(1, &[7]), now()));
        assert!(filters.admit(&member(2, &[9]), now()));
        assert!(!filters.admit(&member(3, &[3]), now()));
    }

    /// One left on is enough for the set to read as asking
    ///
    /// So the row turns the rest off with it rather than turning the one
    /// that is off back on, which would take two clicks to reach the sky.
    #[test]
    fn one_left_on_turns_them_all_off() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));
        filters.toggle(1);

        filters.toggle_all(&[0, 1]);

        assert!(!filters.any_enabled());
    }

    /// Clearing them takes every filter away
    #[test]
    fn clearing_leaves_nothing_held() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));

        filters.clear(&[0, 1]);

        assert_eq!(filters.len(), 0);
        assert!(filters.admit(&member(1, &[3]), now()));
    }

    /// What the filters admit is two lists a query can be handed
    #[test]
    fn a_faction_filter_admits_by_id() {
        let mut filters = Filters::default();
        filters.add(faction(7));

        let admitted = filters.admitted().expect("something asked for");

        assert_eq!(admitted.factions, vec![7]);
        assert!(admitted.systems.is_empty());
    }

    /// A hand-picked set says its systems outright
    #[test]
    fn a_gathered_filter_admits_by_address() {
        let mut filters = Filters::default();
        filters.add(systems(&[1, 2, 3]));

        let admitted = filters.admitted().expect("something asked for");

        assert_eq!(admitted.systems, vec![1, 2, 3]);
        assert!(admitted.factions.is_empty());
    }

    /// Several of them are gathered into the two lists between them
    ///
    /// Each adds to what is admitted, so the lists are what all of them want
    /// rather than what they have in common.
    #[test]
    fn several_filters_admit_between_them() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));
        filters.add(systems(&[1, 2]));

        let admitted = filters.admitted().expect("something asked for");

        assert_eq!(admitted.factions, vec![7, 9]);
        assert_eq!(admitted.systems, vec![1, 2]);
    }

    /// A filter turned off asks for nothing, and is not asked for
    #[test]
    fn a_disabled_filter_admits_nothing_in_particular() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.toggle(0);

        assert_eq!(filters.admitted(), None);
    }

    /// Nothing held admits the whole sky rather than none of it
    ///
    /// A query narrowed by two empty lists comes back with nothing, where
    /// what is meant is everything, so there is nothing to narrow it by.
    #[test]
    fn no_filters_admit_everything() {
        assert_eq!(Filters::default().admitted(), None);
    }

    /// A filter that admits nothing asks for nothing, and means it
    ///
    /// Which is the one case the two empty lists are the right answer: a
    /// filter is being asked and it admits no system, so a query that comes
    /// back with nothing is what was asked for. It is told apart from nothing
    /// being asked by which of the two it is, and not by what the lists hold.
    ///
    /// What holds this together is that [`Filters::admit`] says the same. The
    /// map dims by that and fetches by this, so a filter the two disagreed
    /// about would be a sky drawn from one answer and fetched from the other.
    #[test]
    fn a_filter_that_admits_nothing_narrows_to_nothing() {
        let mut filters = Filters::default();
        filters.add(route(&[]));

        assert_eq!(filters.admitted(), Some(Admitted::default()));
        assert!(!filters.admit(&member(1, &[7]), now()));
    }

    /// One of two turned off leaves the other asking
    #[test]
    fn disabling_one_filter_leaves_the_rest() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));
        filters.toggle(1);

        assert!(filters.admit(&member(1, &[7]), now()));
        assert!(!filters.admit(&member(2, &[9]), now()));
    }

    /// A hand-picked set holding the systems at `addresses`
    fn systems(addresses: &[i64]) -> Filter {
        Filter::Systems {
            label: format!("{} systems", addresses.len()),
            systems: addresses.to_vec(),
        }
    }

    /// A hand-picked set admits the systems that were picked
    #[test]
    fn a_gathered_set_admits_what_was_picked() {
        let mut filters = Filters::default();
        filters.add(systems(&[7, 3]));

        assert!(filters.admit(&member(3, &[]), now()));
        assert!(filters.admit(&member(7, &[]), now()));
        assert!(!filters.admit(&member(9, &[]), now()));
    }

    /// And has no order for a panel to list it in
    ///
    /// Unlike a route, which is travelled from one end to the other. A set is
    /// gathered up, and the order it happened to be clicked in says nothing
    /// worth holding a list to.
    #[test]
    fn a_gathered_set_has_no_order_of_its_own() {
        assert!(!systems(&[7, 3]).ordered());
        assert_eq!(systems(&[7, 3]).place_of(3), None);
    }

    /// A route between the systems at `addresses`
    fn route(addresses: &[i64]) -> Filter {
        Filter::Route {
            label: "A -> B".to_owned(),
            systems: addresses.to_vec(),
            range: "10".to_owned(),
        }
    }

    /// A route filter admits the systems it runs through
    ///
    /// Nothing a system knows about itself, unlike a faction: a route is
    /// worked out, so the filter carries the answer it came back with.
    #[test]
    fn a_route_filter_admits_what_it_runs_through() {
        let mut filters = Filters::default();
        filters.add(route(&[7, 3, 9]));

        assert!(filters.admit(&member(3, &[]), now()));
        assert!(filters.admit(&member(9, &[]), now()));
        assert!(!filters.admit(&member(4, &[]), now()));
    }

    /// However the addresses are ordered, the answer is the same
    ///
    /// The order a route is kept in is the order it is travelled, which is
    /// what its panel lists it in, and says nothing about who is on it.
    #[test]
    fn a_route_admits_the_same_whatever_order_it_came_in() {
        let mut forwards = Filters::default();
        forwards.add(route(&[1, 5, 9]));
        let mut backwards = Filters::default();
        backwards.add(route(&[9, 5, 1]));

        for address in [1, 5, 9, 2, 7] {
            let system = member(address, &[]);
            assert_eq!(
                forwards.admit(&system, now()),
                backwards.admit(&system, now())
            );
        }
    }

    /// A route says where each of its systems falls along it
    ///
    /// Which is what its panel lists them in. The order a route is travelled
    /// is the whole of what a route is.
    #[test]
    fn a_route_knows_the_order_it_is_travelled() {
        let asked = route(&[9, 3, 7]);

        assert_eq!(asked.place_of(9), Some(0));
        assert_eq!(asked.place_of(3), Some(1));
        assert_eq!(asked.place_of(7), Some(2));
        assert_eq!(asked.place_of(4), None);
    }

    /// A route has an order and a faction has none
    ///
    /// A faction's systems are a set, with nothing in them to say which comes
    /// first, so whoever lists those may order them to suit the reader.
    #[test]
    fn only_a_route_carries_an_order() {
        assert!(route(&[1, 2]).ordered());
        assert!(!faction(7).ordered());
        assert_eq!(faction(7).place_of(1), None);
    }

    /// A route is one jump fewer than the systems it runs through
    ///
    /// The systems are where a jump lands, so two of them are one jump. A
    /// route is what the count is about, so nothing else is counted at all.
    #[test]
    fn a_route_is_flown_in_one_jump_fewer_than_it_holds() {
        assert_eq!(route(&[1, 2]).hops(), Some(1));
        assert_eq!(route(&[1, 2, 3, 4]).hops(), Some(3));
        assert_eq!(route(&[]).hops(), None);
        assert_eq!(faction(7).hops(), None);
        assert_eq!(systems(&[1, 2]).hops(), None);
    }

    /// A second route stands beside the first
    ///
    /// Each keeps its own line and its own row, so plotting another is asking
    /// to see both. What the map is picking out is then either of them, the
    /// filters adding to each other.
    #[test]
    fn a_second_route_stands_beside_the_first() {
        let mut filters = Filters::default();
        filters.add(route(&[1, 2]));
        filters.add(route(&[8, 9]));

        assert_eq!(filters.iter().count(), 2);
        assert!(filters.admit(&member(9, &[]), now()));
        assert!(filters.admit(&member(1, &[]), now()));
    }

    /// The same route asked for twice is one route
    ///
    /// Two rows naming one line would have to be turned off one at a time,
    /// and there is nothing to see twice.
    #[test]
    fn the_same_route_asked_for_twice_is_held_once() {
        let mut filters = Filters::default();
        filters.add(route(&[1, 2]));
        filters.add(route(&[1, 2]));

        assert_eq!(filters.iter().count(), 1);
    }

    /// And leaves the factions where they are
    #[test]
    fn a_route_leaves_the_factions_alone() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));
        filters.add(route(&[1, 2]));

        assert_eq!(filters.iter().count(), 3);
    }

    /// A route stands beside the factions rather than cutting into them
    ///
    /// Both were asked for, and a route through a faction's space rarely
    /// keeps to it. Taking only what the two share would answer a plotted
    /// route with the handful of its systems that faction happens to hold.
    #[test]
    fn a_route_and_a_faction_ask_together() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(route(&[1, 2]));

        let mut on_both = member(1, &[7]);
        on_both.factions = vec![7];
        assert!(filters.admit(&on_both, now()));
        // On the route, though the faction is nowhere near it.
        assert!(filters.admit(&member(2, &[]), now()));
        // The faction's, though the route runs elsewhere.
        assert!(filters.admit(&member(5, &[7]), now()));
        // Neither, so neither asked for it.
        assert!(!filters.admit(&member(6, &[3]), now()));
    }

    /// Two filters that differ are told apart, and equal ones are not
    ///
    /// The bar keys each row on its filter rather than on where the row sits,
    /// so that dropping one does not hand its identity, and whatever egui was
    /// remembering against it, to the row that moves up into its place. That
    /// rests on this: two filters that are not the same must not hash the
    /// same, and one that is must.
    #[test]
    fn filters_are_told_apart_by_what_they_ask() {
        use std::collections::HashSet;

        let asked = [faction(7), faction(9), route(&[1, 2]), route(&[3, 4])];
        let distinct: HashSet<_> = asked.iter().collect();
        assert_eq!(distinct.len(), asked.len());

        let same: HashSet<_> = [faction(7), faction(7)].into_iter().collect();
        assert_eq!(same.len(), 1);
    }

    /// The same filter added twice is asked once
    ///
    /// Two rows saying the same thing narrow nothing and have to be turned
    /// off one at a time.
    #[test]
    fn the_same_filter_is_not_added_twice() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(7));

        assert_eq!(filters.iter().count(), 1);
    }

    /// Removing a filter stops it being asked
    #[test]
    fn a_removed_filter_asks_nothing() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.remove(0);

        assert!(filters.admit(&member(1, &[]), now()));
    }

    /// A world with nothing in it but the filters and the mark
    fn map() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Filters>();
        // What `mark` cuts the spans against, and how often. Zero seconds
        // cuts every frame, which is what a test wants: it steps the world by
        // hand and there is no waiting in it.
        app.insert_resource(Poll(Some(0.)));
        app.init_resource::<LastCutAt>();
        app.init_resource::<Time<Real>>();
        app.add_systems(Update, mark);
        app
    }

    /// The mark lands on what the filters exclude
    #[test]
    fn the_mark_lands_on_what_is_excluded() {
        let mut app = map();
        let inside = app.world_mut().spawn(member(1, &[7])).id();
        let outside = app.world_mut().spawn(member(2, &[3])).id();

        app.world_mut().resource_mut::<Filters>().add(faction(7));
        app.update();

        assert!(!app.world().entity(inside).contains::<Filtered>());
        assert!(app.world().entity(outside).contains::<Filtered>());
    }

    /// Dropping a filter takes the mark off what it excluded
    #[test]
    fn the_mark_goes_when_the_filter_does() {
        let mut app = map();
        let outside = app.world_mut().spawn(member(1, &[3])).id();

        app.world_mut().resource_mut::<Filters>().add(faction(7));
        app.update();
        app.world_mut().resource_mut::<Filters>().remove(0);
        app.update();

        assert!(!app.world().entity(outside).contains::<Filtered>());
    }

    /// A row that changes underneath a filter is asked again
    ///
    /// A fetch replaces the row of a system already on the map, and the
    /// factions in it are what the filter reads. Without this the mark would
    /// go on answering for the row the system arrived with.
    #[test]
    fn a_changed_row_is_asked_again() {
        let mut app = map();
        let joining = app.world_mut().spawn(member(1, &[3])).id();

        app.world_mut().resource_mut::<Filters>().add(faction(7));
        app.update();
        assert!(app.world().entity(joining).contains::<Filtered>());

        app.world_mut().entity_mut(joining).insert(member(1, &[3, 7]));
        app.update();

        assert!(!app.world().entity(joining).contains::<Filtered>());
    }

    /// A system arriving under a standing filter is marked
    #[test]
    fn a_system_arriving_is_asked() {
        let mut app = map();

        app.world_mut().resource_mut::<Filters>().add(faction(7));
        app.update();

        let outside = app.world_mut().spawn(member(1, &[3])).id();
        app.update();

        assert!(app.world().entity(outside).contains::<Filtered>());
    }

    /// A filter on time admits what has been heard from since
    #[test]
    fn a_filter_on_time_admits_what_came_in_since() {
        let mut filters = Filters::default();
        filters.add(within(100));

        assert!(filters.admit(&heard(1, 160), now()), "later than the moment");
        assert!(filters.admit(&heard(2, 100), now()), "the moment itself");
        assert!(
            !filters.admit(&heard(3, 40), now()),
            "earlier than the moment"
        );
    }

    /// A system belonging to each of `factions`, heard from at `secs`
    fn member_heard(address: i64, factions: &[i32], secs: i64) -> System {
        let mut system = heard(address, secs);
        system.factions = factions.to_vec();
        system
    }

    /// Time is a condition on the rest rather than another thing to show
    ///
    /// Two factions and a span mean either faction, heard from within the
    /// span. Counted alongside the factions, the span would put every system
    /// heard from lately onto a map asked for two factions.
    #[test]
    fn time_narrows_what_the_other_filters_admit() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));
        filters.add(within(100));

        assert!(
            filters.admit(&member_heard(1, &[7], 160), now()),
            "the first faction, heard from lately"
        );
        assert!(
            filters.admit(&member_heard(2, &[9], 160), now()),
            "the second faction, heard from lately"
        );
        assert!(
            !filters.admit(&member_heard(3, &[7], 40), now()),
            "a faction asked for, but not heard from since"
        );
        assert!(
            !filters.admit(&member_heard(4, &[3], 160), now()),
            "heard from lately, but no faction asked for"
        );
    }

    /// A span asked on its own admits whatever it reaches
    ///
    /// Nothing else picks systems out, so there is nothing for it to be a
    /// condition on and it stands as the whole question.
    #[test]
    fn a_span_asked_alone_admits_what_it_reaches() {
        let mut filters = Filters::default();
        filters.add(within(100));

        assert!(filters.admit(&heard(1, 160), now()));
        assert!(!filters.admit(&heard(2, 40), now()));
    }

    /// Reading the filters is not asking anything of them
    ///
    /// What the bar leans on. It is handed the filters every frame it draws,
    /// and being handed a [`ResMut`] is what marks a resource written, so the
    /// bar compares this before and after and marks them only where it moved.
    /// Left to the handing alone, every frame would read as the user having
    /// changed a filter, and what reads that puts a query on the wire.
    #[test]
    fn reading_the_filters_asks_nothing_of_them() {
        let mut filters = Filters::default();
        filters.add(faction(7));

        let settled = filters.revision();
        let _ = filters.admit(&member(1, &[7]), now());
        let _ = filters.admitted();
        let _ = filters.changed_since(now());
        let _ = filters.len();
        let _ = filters.any_enabled();

        assert_eq!(filters.revision(), settled, "reading counted as asking");
    }

    /// And every way of asking something of them says so
    #[test]
    fn every_way_of_asking_moves_the_revision() {
        let mut filters = Filters::default();
        let mut settled = filters.revision();

        let mut moved = |filters: &Filters, what: &str| {
            assert_ne!(filters.revision(), settled, "{what} said nothing");
            settled = filters.revision();
        };

        filters.add(faction(7));
        moved(&filters, "adding a filter");

        filters.toggle(0);
        moved(&filters, "turning one off");

        filters.toggle_all(&[0]);
        moved(&filters, "turning the set over");

        filters.ask_within("1 day", Duration::seconds(100));
        moved(&filters, "asking about time");

        filters.turn_time_off("Off");
        moved(&filters, "turning time off");

        filters.ask_nothing_of_time();
        moved(&filters, "letting go of time");

        filters.remove(0);
        moved(&filters, "dropping a filter");
    }

    /// A span reaches back from now, so what it admits changes as the clock does
    ///
    /// The near edge moves, and systems cross it both ways: one heard from
    /// within the span falls out of it by being left alone. A moment worked
    /// out once and kept would say "since 14:32" for as long as it was asked,
    /// so systems could only ever cross into it and the count could only climb.
    #[test]
    fn a_span_lets_go_of_what_has_aged_out_of_it() {
        let mut filters = Filters::default();
        filters.add(within(100));

        let system = heard(1, 150);

        assert!(filters.admit(&system, moment(200)), "inside the span");
        assert!(
            !filters.admit(&system, moment(300)),
            "the span moved on and left it behind"
        );
    }

    /// And the question put to the database moves with it
    ///
    /// Asked again on every poll, so the same span is a later moment each
    /// time. Answered with whatever has crossed into it since.
    #[test]
    fn the_question_asked_of_the_database_moves_with_the_clock() {
        let mut filters = Filters::default();
        filters.add(within(100));

        assert_eq!(filters.changed_since(moment(200)), Some(moment(100)));
        assert_eq!(filters.changed_since(moment(300)), Some(moment(200)));
    }

    /// It narrows no region, what it admits being gathered nowhere
    ///
    /// A faction and a route say which systems a region should be asked for. A
    /// question about time is answered from across the galaxy, so it has
    /// nothing to add to a question about a place.
    ///
    /// Nothing at all rather than two empty lists. The region is asked with
    /// whatever these hold, and a pair of empty lists asks it for no faction
    /// and no system, which is a question the database answers with nothing.
    #[test]
    fn a_filter_on_time_narrows_no_region() {
        let mut filters = Filters::default();
        filters.add(within(100));

        assert!(filters.admitted().is_none(), "the region was narrowed");
    }

    /// Beside a faction it leaves that faction narrowing the region
    ///
    /// The two are asked together: the faction says which systems the region
    /// is wanted for, and time is asked of what comes back.
    #[test]
    fn a_filter_on_time_leaves_a_faction_narrowing() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(within(100));

        let admitted = filters.admitted().expect("the faction asked");
        assert_eq!(admitted.factions, vec![7]);
    }

    /// Asking again about time replaces the question rather than adding one
    ///
    /// Two of these would draw what the wider of them draws, since the earlier
    /// moment's answer holds the later one's. The control would look like it
    /// had stopped responding once it had been moved twice.
    #[test]
    fn asking_about_time_twice_asks_one_question() {
        let mut filters = Filters::default();
        filters.ask_within("1 day", Duration::seconds(100));
        filters.ask_within("1 hour", Duration::seconds(50));

        assert_eq!(filters.len(), 1, "the first question was left standing");
        assert_eq!(filters.changed_since(now()), Some(moment(150)));
    }

    /// The row a filter on time draws says the span it was asked as
    ///
    /// A moment on its own reads as a time of day with no date against it, so
    /// a row naming one says nothing the user can act on once the span
    /// reaching back to it is longer than a day.
    #[test]
    fn a_filter_on_time_is_named_for_its_span() {
        let mut filters = Filters::default();
        filters.ask_within("30 days", Duration::seconds(100));

        assert_eq!(
            filters.get(0).map(|active| active.filter.name()),
            Some("Last 30 days")
        );
    }

    /// Sliding the control off stops asking about time and nothing else
    #[test]
    fn asking_nothing_of_time_leaves_the_rest() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.ask_within("1 day", Duration::seconds(100));

        filters.ask_nothing_of_time();

        assert_eq!(
            filters.changed_since(now()),
            None,
            "still asking about time"
        );
        assert_eq!(filters.len(), 1, "the faction went with it");
        assert!(
            filters.admit(&member(1, &[7]), now()),
            "the faction stopped asking"
        );
    }

    /// Turning time off stops it asking without taking its row away
    ///
    /// What the control does while it is being held, its row standing above
    /// it. The row says what is filtering, which by then is nothing.
    #[test]
    fn time_turned_off_stops_asking_where_it_stands() {
        let mut filters = Filters::default();
        filters.add(within(100));

        filters.turn_time_off("Off");

        assert_eq!(filters.len(), 1, "the row went with the question");
        assert_eq!(
            filters.changed_since(now()),
            None,
            "still asking about time"
        );
        assert!(
            filters.admit(&heard(1, 40), now()),
            "still turning systems away"
        );
        assert_eq!(
            filters.get(0).map(|active| active.filter.name()),
            Some("Off"),
            "the row still named the span it had stopped asking for"
        );
    }

    /// A filter on time turned off asks nothing, as any other does
    ///
    /// The fetch behind it reads this, so a disabled row that still answered
    /// would go on pulling ten thousand rows a poll for a question the user
    /// had switched off.
    #[test]
    fn a_disabled_filter_on_time_asks_nothing() {
        let mut filters = Filters::default();
        filters.ask_within("1 day", Duration::seconds(100));
        filters.toggle(0);

        assert_eq!(filters.changed_since(now()), None);
    }

    /// The control's far end asks nothing, and its near end asks for a minute
    ///
    /// The spans and what they are called are one table read by index, so this
    /// is what says the two halves of it still line up.
    #[test]
    fn the_control_reaches_from_nothing_to_a_minute() {
        assert_eq!(Watch(0).name(), "Off");
        assert_eq!(Watch(0).span(), None);

        let nearest = Watch(SPANS.len() - 1);
        assert_eq!(nearest.name(), "1 minute");
        assert_eq!(nearest.span(), Some(Duration::seconds(60)));
    }
}
