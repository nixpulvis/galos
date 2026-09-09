//! What a system is made of, and where each piece sits
//!
//! A system is drawn as one sphere from anywhere but inside it, so what fills
//! that sphere is only worth asking about on the way in. This is where it is
//! asked for, and where the answer is kept.
//!
//! One system at a time. Whichever the camera is nearest to is the one held,
//! and the rest are drawn as the marks they are. What is loaded is the rows —
//! stars and bodies both, since a body goes round a star — rather than
//! anything drawn; the entities, the grid a system carries and the camera's
//! descent into it all wait until there is something worth seeing.

use bevy::math::DVec3;
use bevy::prelude::*;
use chrono::{DateTime, Utc};
// Re-exported: the floor under a reach is read all over the map — the zoom
// floor, the shell, the ruled plane — and it belongs beside the reach it
// floors, which is `galos_index::inside`.
pub use galos_index::inside::STAND_IN;
use galos_index::meta::{
    Barycenter as DbBarycenter, Body as DbBody, Star as DbStar, SystemBodies,
};
use galos_index::orbit::Orbits;

// Held in: the map reaches a system's insides through `bodies::plugin`.
pub(crate) mod fetch;
pub(crate) mod spawn;

pub fn plugin(app: &mut App) {
    app.init_resource::<Contents>();
    app.init_resource::<Clock>();
    app.add_plugins(fetch::plugin);
    app.add_plugins(spawn::plugin);
}

/// How far the game's own clock may drift from the map's reading of it, in
/// seconds
///
/// The reading is stepped in whole units of this rather than followed exactly,
/// and everything in a system is placed afresh whenever it moves, so a bound
/// any tighter is the whole of a system's insides rewritten every frame to move
/// nothing anybody can see. A second: the fastest bodies the journal records
/// come round in hours, so a second of one is some ten-thousandth of its turn.
const WITHIN: f64 = 1.;

/// What moment a system is drawn at
///
/// Two numbers make it: where the game's own clock stands, and the offset past
/// that a slider has been dragged to. The reading
/// everything is placed from is the two together, and it is worked out rather
/// than kept, so there is no third number to fall out of step with them.
///
/// Seconds, and one reading for the whole system, so what is drawn is always a
/// single moment rather than an arrangement composed body by body. Which moment
/// each thing is run on from is its own business: a system's rows arrive from
/// as many scans as there were commanders who flew there, so every path carries
/// how long before this reading's zero it was read, and the walk that places a
/// moon runs each step of itself on from the scan that step actually has. See
/// [`galos_index::orbit::Orbit::behind`].
#[derive(Resource)]
pub struct Clock {
    /// Where the game's own clock stands, in seconds past the moment the system
    /// being held was last heard from
    ///
    /// Stepped in whole [`WITHIN`], so a map nobody is touching is still rather
    /// than being rewritten every frame. See [`Self::follows`].
    live: f64,
    /// How far past that the map has been run on, in seconds
    ///
    /// Nothing to begin with and nothing until a slider is dragged, so the map
    /// opens on where the game has carried everything by now.
    ///
    /// The whole of what a slider sets, which is why it is an offset rather
    /// than a place: a slider covers one turn of its own body, so its near end
    /// is that body drawn where it stands and its far end is the same place one
    /// of its own turns later.
    ///
    /// Set from a body's own panel rather than from one control over the
    /// system, because a system has no span that suits all of it: the slowest
    /// body of one takes a median 993 times as long to come round as its
    /// fastest, and in Sol it is four million times.
    offset: f64,
    /// Whether the map stands at the game's own moment or at the scans
    ///
    /// The game's to begin with: a map of where things are is worth more than a
    /// map of where they were seen, and the one thing the reading cannot be
    /// read off the rows is which day it is. Turned off, the map stands where
    /// the system was last heard from — which is what the rows actually
    /// recorded — and the offset runs on from there instead.
    following: bool,
    /// The whole turns the slider being dragged set out from, while one is
    ///
    /// A slider covers one turn of its own body, and a phase is cyclic: its
    /// far end is the same place on the orbit as its near end, one turn later.
    /// Which turn that is has to hold still while the slider is dragged, or
    /// the reading moves under the drag: worked out afresh each frame from the
    /// clock it is itself setting, a slider run to its far end lands on the
    /// next turn, reads back as no phase at all, and asks for the turn after
    /// that.
    ///
    /// One of these for the map rather than one per slider, there being one
    /// pointer and so one slider ever being dragged.
    held: Option<Held>,
}

impl Default for Clock {
    fn default() -> Self {
        Clock { live: 0., offset: 0., following: true, held: None }
    }
}

/// The slider a drag has hold of
///
/// The period is carried so that the anchor is only ever applied to the
/// slider it was taken for. A drag that never sees its own end -- a panel shut
/// while the pointer is down -- would otherwise leave the anchor standing, and
/// the next slider touched would measure a turn of its own body from a count
/// of somebody else's.
struct Held {
    /// What the slider is geared to
    period: f64,
    /// The whole turns it set out from
    turns: f64,
}

impl Held {
    /// Whether `offset` still stands in the turn this was taken for
    ///
    /// Measured rather than trusted. A drag that never sees its own end leaves
    /// the anchor standing, and the offset may have been set anywhere since by
    /// another body's slider, so an anchor is only worth measuring from where
    /// the reading could have come from it.
    ///
    /// The far end counts. A slider run to it lands exactly on the beginning of
    /// the next turn and is held there, which is what the anchor is for.
    fn holds(&self, offset: f64) -> bool {
        offset >= self.turns * self.period
            && offset <= (self.turns + 1.) * self.period
    }
}

impl Clock {
    /// The moment everything is placed at, in seconds past the one the system
    /// was last heard from
    ///
    /// Where the game stands, plus the offset past it.
    /// Worked out rather than kept: the two it is made of are what anything
    /// sets, so there is nothing here to put back in step with them.
    pub fn at(&self) -> f64 {
        let standing = if self.following { self.live } else { 0. };

        standing + self.offset
    }

    /// How far past where the map would otherwise stand it has been run on, in
    /// seconds
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// Whether the map stands at the game's own moment rather than at the scans
    pub fn following(&self) -> bool {
        self.following
    }

    /// Stand at one or the other
    ///
    /// The offset is kept and goes on being measured from wherever this leaves
    /// the map, an offset being a span rather than a place.
    pub fn follow(&mut self, following: bool) {
        self.following = following;
    }

    /// Read where the game's own clock stands, `now` against a system last
    /// heard from at `recorded`
    ///
    /// Read whether the map is standing there or not, so that turning it back
    /// on is the frame it happens on rather than the frame after.
    ///
    /// Stepped in whole [`WITHIN`] rather than followed exactly. Everything in
    /// a system is placed afresh whenever the reading moves, so following it to
    /// the frame is a system rewritten sixty times a second to move nothing
    /// anybody can see.
    ///
    /// Put right rather than added to, so a map left running for an hour is an
    /// hour on rather than however much of one the frames added up to.
    pub fn follows(&mut self, now: DateTime<Utc>, recorded: DateTime<Utc>) {
        let since = (now - recorded).num_milliseconds() as f64 / 1000.;

        if (since - self.live).abs() >= WITHIN {
            self.live = since;
        }
    }

    /// Let go of the offset
    ///
    /// Back to where the map would stand untouched, which under the game's
    /// clock is where everything is now.
    pub fn reset(&mut self) {
        self.offset = 0.;
    }

    /// How far through `period`'s own turn the offset stands, from none of it
    /// to all
    ///
    /// Nothing until a slider is dragged, whatever the reading is: what a
    /// slider sets is a span past where its body already stands, so its near
    /// end is that body drawn where it is and its far end is the same place a
    /// turn later.
    pub fn through(&self, period: f64) -> f64 {
        if period <= 0. {
            return 0.;
        }
        let turns = self.offset / period;
        turns - turns.floor()
    }

    /// Take hold of the turn a slider over `period` is setting out from
    ///
    /// Said when a drag begins, so that [`Self::offset_to`] measures from where
    /// the slider started rather than from where it has since put the clock.
    pub fn hold(&mut self, period: f64) {
        if period > 0. {
            self.held =
                Some(Held { period, turns: (self.offset / period).floor() });
        }
    }

    /// Let go of it, the drag being over
    pub fn release(&mut self) {
        self.held = None;
    }

    /// Run on to `through` of the way round `period`'s own turn
    ///
    /// Within the turn the slider set out from, so dragging one moves the map
    /// by at most a single period of the body it is geared to, and moves it
    /// evenly: a slider run from end to end runs the clock on by exactly one
    /// turn of that body, with nothing anywhere in the system jumping on the
    /// way, and that body left exactly where it was found.
    ///
    /// A moon's slider therefore barely stirs the planet it goes round.
    /// Reaching for the first turn instead would throw the whole system back to
    /// the beginning every time a moon was nudged.
    ///
    /// The map goes on standing where the game's clock puts it under this: an
    /// offset is a span past that and not a place, so the whole system runs on
    /// beneath it rather than it being overruled by them.
    pub fn offset_to(&mut self, period: f64, through: f64) {
        if period <= 0. {
            return;
        }
        let whole = match &self.held {
            Some(held) if held.period == period && held.holds(self.offset) => {
                held.turns
            }
            _ => (self.offset / period).floor(),
        };
        self.offset = (whole + through) * period;
    }
}

/// Draw `with` against the clock, and mark it changed only where it moved
///
/// A [`ResMut`] counts as written for being handed out, and a panel is handed
/// the clock every frame it is open whether or not the slider was touched. What
/// reads the mark rebuilds every star, body and orbit line in the held system,
/// so the handing alone would rebuild them all every frame a panel stood open.
///
/// The reading is what is compared, and not the turn a drag set out from: the
/// places are worked out from the reading, and taking hold of the slider moves
/// nothing until it is dragged. The reading rather than the two numbers behind
/// it, so that the game's clock stepping on while the map is standing at the
/// scans is not a system redrawn for a step nothing was placed from.
pub(crate) fn mark_if_moved<T>(
    clock: &mut impl DetectChangesMut<Inner = Clock>,
    with: impl FnOnce(&mut Clock) -> T,
) -> T {
    let clocking = clock.bypass_change_detection();
    let was = clocking.at();
    let drawn = with(&mut *clocking);
    let moved = clocking.at() != was;

    if moved {
        clock.set_changed();
    }

    drawn
}

/// The one system the map is holding the insides of
///
/// A resource rather than a component, because there is only ever one and
/// because it outlives the system's entity: the spyglass may drag a system
/// off the map while the camera is still standing in it.
#[derive(Resource, Default)]
pub struct Contents {
    /// Which system this is about, if any
    of: Option<i64>,
    /// What has come back about it
    state: FetchState,
    /// How many answers about this system have said something new
    ///
    /// The poll asks over and over and most of what comes back says what the
    /// last one did. This counts only the answers that did not, which is what
    /// whoever drew from the rows compares against to know their picture is
    /// out of date.
    revision: u32,
}

/// How far along the asking has got
#[derive(Default)]
enum FetchState {
    /// Nothing has been asked about
    #[default]
    Nothing,
    /// Asked, and not yet answered
    Asking,
    /// Answered, with whatever the database had — which may be nothing at all
    Known(SystemBodies),
}

impl Contents {
    /// Which system is being held, if any
    pub fn of(&self) -> Option<i64> {
        self.of
    }

    /// Whether the answer about it is still on its way
    ///
    /// [`Contents::extent`] is `None` both for a system with nothing in it and
    /// for one whose rows have not landed yet, and those mean opposite things
    /// to the zoom floor: `zoom_floor` in [`crate::camera`] holds the camera
    /// off a system with nothing to descend into, and reads this so a question
    /// in flight is not taken for an empty answer. Every handover from one
    /// system to the next passes through here, so reading it the other way
    /// would nudge the camera on the way in to anywhere.
    pub fn asking(&self) -> bool {
        matches!(self.state, FetchState::Asking)
    }

    /// Which answer about this system is being held
    ///
    /// Nothing to read into the number itself. It stands still while the
    /// answers repeat and moves when one of them does not, so two readings
    /// that differ mean the rows differ.
    pub fn revision(&self) -> u32 {
        self.revision
    }

    /// Hold what the database said, if it said anything new
    ///
    /// The rows are compared rather than taken as fresh because the poll asks
    /// whether anything changed and the answer is usually no. Everything
    /// inside a system is despawned and drawn again from scratch when what is
    /// held changes, so an answer repeating the last one has to leave both the
    /// rows and the revision exactly as they were.
    pub(super) fn hold(&mut self, rows: SystemBodies) {
        if let FetchState::Known(held) = &self.state
            && *held == rows
        {
            return;
        }

        self.state = FetchState::Known(rows);
        self.revision = self.revision.wrapping_add(1);
    }

    /// The rows of the system being held, if any have landed
    ///
    /// Everything about the arrangement they describe — what goes round what,
    /// where each thing stands, how far the whole of it reaches — is asked of
    /// these rather than of this, and asked through
    /// [`galos_index::inside`], which is the same code the
    /// builder works the reach table out with. What is left here is the asking
    /// and the holding.
    pub fn rows(&self) -> Option<&SystemBodies> {
        match &self.state {
            FetchState::Known(rows) => Some(rows),
            _ => None,
        }
    }

    /// The stars of the system being held
    pub fn stars(&self) -> &[DbStar] {
        self.rows().map_or(&[], |rows| &rows.stars)
    }

    /// The bodies of the system being held
    pub fn bodies(&self) -> &[DbBody] {
        self.rows().map_or(&[], |rows| &rows.bodies)
    }

    /// The points a close pair of the system being held goes round
    ///
    /// No sphere stands at one, and its ellipse is drawn: a close pair rides
    /// that ellipse, so it is the whole of how far out the pair sits. What
    /// they are worth besides is that everything under one can be placed. A
    /// body measures its orbit about its nearest ancestor, and where that
    /// ancestor stands is the rest of the answer.
    pub fn barycenters(&self) -> &[DbBarycenter] {
        self.rows().map_or(&[], |rows| &rows.barycenters)
    }

    /// The star with `id`, if what stands there is a star
    ///
    /// What a panel describing one is opened from. The row rather than the
    /// entity, as the panel holds a value and outlives the camera leaving.
    pub fn star(&self, id: i16) -> Option<&DbStar> {
        self.stars().iter().find(|star| star.id == id)
    }

    /// The body with `id`, if what stands there is a body
    pub fn body(&self, id: i16) -> Option<&DbBody> {
        self.bodies().iter().find(|body| body.id == id)
    }

    /// Which star the system arrives at, and where the middle of it falls
    ///
    /// Both [`galos_index::inside`]'s, asked of the rows in hand.
    pub fn primary(&self) -> Option<i16> {
        self.rows().and_then(SystemBodies::primary)
    }

    pub(super) fn middle(&self, orbits: &Orbits, since: f64) -> DVec3 {
        self.rows().map_or(DVec3::ZERO, |rows| rows.middle(orbits, since))
    }

    /// When the system being held was last heard from
    ///
    /// The newest of its scans, which is what [`Clock`]'s zero stands at:
    /// everything on record was read at or before it, so no path has to be run
    /// backwards to meet it, and how far the map has run the system on reads as
    /// how long it has been since anybody looked.
    ///
    /// Nothing until the rows are in, and nothing for a system with none.
    pub fn recorded_at(&self) -> Option<DateTime<Utc>> {
        self.rows().and_then(SystemBodies::recorded_at)
    }

    /// Where everything in the system stands, and how far it reaches
    ///
    /// Worked out from the rows by the same code the index's reach table is
    /// written with, so the shell the map draws around a system and the size
    /// it drew it at from light years off cannot disagree. The address goes in
    /// because a place the map has to stand up itself is pointed in a
    /// direction taken from it; see `galos_index::orbit::made_up_direction`.
    ///
    /// Dated against the moment the system was last heard from, so that one
    /// reading of the clock places rows read years apart: each path is run on
    /// from its own scan and they meet at the moment being asked about. The
    /// builder asks the undated question instead — what it writes is a reach,
    /// which is a fact about the arrangement rather than about the day.
    pub fn orbits(&self) -> Orbits {
        let address = self.of.unwrap_or_default();

        self.rows()
            .map(|rows| match rows.recorded_at() {
                Some(at) => rows.orbits_at(address, at),
                // Nothing was ever read, so there is nothing to date against
                // and nothing held to date.
                None => rows.orbits(address),
            })
            .unwrap_or_default()
    }

    /// How far the system reaches from its middle, in metres
    ///
    /// Nothing until the rows are in, and nothing for a system that has none.
    /// Both are the same picture to whoever is drawing: the map cannot say how
    /// far this system reaches.
    pub fn extent(&self) -> Option<f32> {
        self.rows().and_then(|rows| rows.extent(self.of.unwrap_or_default()))
    }

    /// How near the tightest ring around `id` comes, in metres
    pub(super) fn ridden_at(&self, id: i16) -> Option<f32> {
        self.rows().and_then(|rows| rows.ridden_at(id))
    }

    /// What kind of unscanned place `id`'s own place rests on, where it rests
    /// on one the map made up
    pub(super) fn guessed_under(
        &self,
        orbits: &Orbits,
        id: i16,
    ) -> Option<&'static str> {
        self.rows().and_then(|rows| rows.guessed_under(orbits, id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elite_journal::body::{Orbit as JournalOrbit, Spin as JournalSpin};

    /// A body `a` metres out on a circle, with no size of its own
    ///
    /// Reached from [`super::spawn`]'s tests as well, which drive the same rows
    /// through the systems that draw and move them.
    pub(crate) fn body(a: f32) -> DbBody {
        DbBody {
            system_address: 1,
            id: 1,
            parents: vec![],
            name: String::new(),
            body_type: None,
            distance_from_arrival: None,
            updated_at: chrono::DateTime::UNIX_EPOCH,
            updated_by: String::new(),
            planet_class: String::new(),
            tidal_lock: false,
            mass: 0.,
            radius: 0.,
            gravity: 0.,
            temperature: Some(0.),
            surface: None,
            orbit: JournalOrbit {
                semi_major_axis: a,
                eccentricity: 0.,
                orbital_inclination: 0.,
                periapsis: 0.,
                orbital_period: 0.,
                ascending_node: Some(0.),
                mean_anomaly: Some(0.),
            },
            spin: JournalSpin { period: 0., tilt: 0. },
            discovered_at: None,
            mapped: false,
        }
    }

    /// Rows holding `bodies` and nothing else
    fn holding(bodies: Vec<DbBody>) -> SystemBodies {
        SystemBodies { bodies, ..Default::default() }
    }

    /// A poll finding nothing new leaves the revision where it was
    ///
    /// Most of them find nothing new, nobody being mid scan most of the time.
    /// Everything inside a system is despawned and drawn again when the
    /// revision moves, so a repeat that moved it would be a system blinking
    /// every poll for no reason at all.
    #[test]
    fn a_poll_finding_nothing_new_moves_nothing() {
        let mut contents = Contents::default();
        contents.hold(holding(vec![body(1e9)]));

        let first = contents.revision();
        contents.hold(holding(vec![body(1e9)]));

        assert_eq!(
            contents.revision(),
            first,
            "the same rows again read as something new"
        );
    }

    /// A body arriving mid scan reaches the map
    ///
    /// What the poll is for. The rows land in the database from another
    /// program while the camera stands in the system, and what is drawn has to
    /// follow them rather than stay as the system was when it was first asked
    /// after.
    #[test]
    fn a_body_arriving_mid_scan_is_taken_in() {
        let mut contents = Contents::default();
        contents.hold(holding(vec![body(1e9)]));
        let first = contents.revision();

        let mut arriving = body(2e9);
        arriving.id = 2;
        contents.hold(holding(vec![body(1e9), arriving]));

        assert_ne!(
            contents.revision(),
            first,
            "a body that was not there before read as the same rows"
        );
        assert_eq!(contents.bodies().len(), 2);
    }

    /// A slider reads how far through its own body's turn the offset stands
    #[test]
    fn a_slider_reads_its_own_bodys_turn() {
        let day = 86_400.;
        // Wound a quarter through its second turn of a four hundred day orbit.
        let clock = Clock { offset: 500. * day, ..default() };

        assert_eq!(clock.through(400. * day), 0.25);
    }

    /// And reads nothing while the map stands where the game puts it
    ///
    /// However far past the scans that is. A slider sets a span past where its
    /// body already stands, so untouched it is at its near end whatever the
    /// system's rows were read on and whatever the reading has since come to.
    #[test]
    fn an_untouched_slider_reads_nothing_however_old_the_scans() {
        let day = 86_400.;
        let clock = Clock { live: 3000. * day, ..default() };

        assert_eq!(clock.through(400. * day), 0.);
        assert_eq!(clock.through(day), 0.);
        assert_eq!(clock.at(), 3000. * day, "the map left the game's clock");
    }

    /// A slider run end to end puts its own body back where it found it
    ///
    /// Which is the whole of what one is: the map is run on by exactly one turn
    /// of that body, from wherever the body actually stands, so the far end of
    /// the slider draws it in the same place as the near end. True whatever the
    /// reading is when the drag begins, the offset being a span past it.
    #[test]
    fn a_slider_run_end_to_end_is_one_turn_of_its_own_body() {
        let day = 86_400.;
        let period = 400. * day;
        let mut clock = Clock { live: 3000. * day, ..default() };

        let before = clock.at();
        clock.hold(period);
        clock.offset_to(period, 1.);

        assert_eq!(
            clock.at() - before,
            period,
            "end to end was not one turn of the body it is geared to"
        );
    }

    /// And the map goes on standing where the game's clock puts it under one
    ///
    /// An offset is a span past the game's moment rather than a place, so the
    /// system runs on beneath it: the body stays the span ahead it was set to,
    /// and the slider stays where the hand left it.
    #[test]
    fn the_game_runs_on_under_an_offset_slider() {
        let day = 86_400.;
        let period = 400. * day;
        let mut clock = Clock { live: 3000. * day, ..default() };
        clock.offset_to(period, 0.5);

        let ahead = clock.at() - clock.live;
        clock.live += 7. * day;

        assert_eq!(clock.at() - clock.live, ahead, "the offset was overruled");
        assert_eq!(clock.through(period), 0.5, "the slider moved on its own");
    }

    /// Standing at the scans instead keeps the offset
    ///
    /// An offset is a span, so it is measured from wherever the map is left
    /// standing rather than being thrown away with the moment it was set at.
    #[test]
    fn standing_at_the_scans_keeps_the_offset() {
        let day = 86_400.;
        let mut clock = Clock { live: 3000. * day, ..default() };
        clock.offset_to(400. * day, 0.5);

        clock.follow(false);

        assert_eq!(clock.at(), 200. * day, "the map did not reach the scans");
        clock.reset();
        assert_eq!(clock.at(), 0., "letting go of the offset left some of it");
    }

    /// Dragging a slider stays in the turn its body is already in
    ///
    /// The point of gearing a slider to one body: a moon's covers one of its
    /// own orbits, so dragging it moves the map by at most that. Winding to the
    /// fraction of the first turn instead would throw the whole system back to
    /// the beginning every time a moon was nudged.
    #[test]
    fn dragging_a_moons_slider_barely_moves_the_map() {
        let day = 86_400.;
        let mut clock = Clock { offset: 500. * day, ..default() };

        clock.offset_to(day, 0.5);

        assert_eq!(
            clock.offset,
            500.5 * day,
            "the map went back to the first turn"
        );
    }

    /// A slider held at its far end leaves the map where it is
    ///
    /// A phase is cyclic, so the far end of a slider is the same place on the
    /// orbit as its near end, one turn on. Worked out afresh from the clock
    /// each frame, that reads back as no phase at all and asks for the turn
    /// after it, and a slider held there ran the whole system on a period every
    /// frame.
    #[test]
    fn a_slider_held_at_its_far_end_stays_put() {
        let day = 86_400.;
        let period = 400. * day;
        let mut clock = Clock { offset: 500. * day, ..default() };

        clock.hold(period);
        clock.offset_to(period, 1.);
        let once = clock.offset;
        for _ in 0..30 {
            clock.offset_to(period, 1.);
        }

        assert_eq!(
            clock.offset, once,
            "the clock ran away while the slider was held"
        );
    }

    /// An anchor left standing by a drag that never ended is not measured from
    ///
    /// `drag_stopped` may never arrive: a panel shut with the pointer down
    /// leaves the anchor where it is. The offset can be set anywhere else
    /// before that body's slider is touched again, and measuring from a turn
    /// the system left long ago throws the whole of it back to that turn.
    #[test]
    fn an_anchor_from_a_drag_that_never_ended_is_let_go_of() {
        let day = 86_400.;
        let period = 400. * day;
        let mut clock = Clock { offset: 500. * day, ..default() };

        // A drag that begins and never sees its own end.
        clock.hold(period);
        // And the offset moves on, set by some other body's slider.
        clock.offset = 900. * day;

        clock.offset_to(period, 0.5);

        assert_eq!(
            clock.offset,
            2.5 * period,
            "the anchor threw the system back to the turn it was taken in"
        );
    }

    /// A world holding a clock nothing has written to yet
    fn holding_a_clock() -> World {
        let mut world = World::new();
        world.init_resource::<Clock>();
        // Making it is a write like any other, and this is about what happens
        // after it exists.
        world.clear_trackers();
        world.increment_change_tick();

        world
    }

    /// Whether anything has written to the clock `world` holds
    fn written(world: &World) -> bool {
        world.get_resource_ref::<Clock>().unwrap().is_changed()
    }

    /// A panel that only reads the clock does not count as moving it
    ///
    /// A panel is handed the clock every frame it is open, and being handed a
    /// [`ResMut`] is what marks a resource written. What reads that mark
    /// rebuilds every star, body and orbit line in the held system, so a panel
    /// left standing open would rebuild the whole of it every frame.
    #[test]
    fn a_panel_reading_the_clock_does_not_move_it() {
        let mut world = holding_a_clock();

        mark_if_moved(&mut world.resource_mut::<Clock>(), |clock| {
            clock.through(86_400.);
        });

        assert!(!written(&world), "an untouched slider moved the clock");
    }

    /// Taking hold of the slider does not either, until it is dragged
    ///
    /// A drag begins on the press, and the turn it sets out from is worked out
    /// then. Nothing has moved yet, so nothing needs redrawing.
    #[test]
    fn taking_hold_of_the_slider_does_not_move_the_clock() {
        let mut world = holding_a_clock();

        mark_if_moved(&mut world.resource_mut::<Clock>(), |clock| {
            clock.hold(86_400.);
        });

        assert!(!written(&world), "holding the slider moved the clock");
    }

    /// Dragging one does
    #[test]
    fn dragging_the_slider_moves_the_clock() {
        let mut world = holding_a_clock();

        mark_if_moved(&mut world.resource_mut::<Clock>(), |clock| {
            clock.offset_to(86_400., 0.5);
        });

        assert!(written(&world), "a dragged slider left the map where it was");
    }

    /// A slider run from end to end runs the clock on by one turn of its body
    ///
    /// Evenly, and that is the point of it. The clock is shared, so every other
    /// body moves by whatever span this slider asks for; a slider that reached
    /// backwards at its far end would leave its own body where it was, the two
    /// ends being one place on its orbit, and jump everything else in the
    /// system by nearly a whole turn of it.
    #[test]
    fn a_slider_run_end_to_end_moves_the_map_evenly() {
        let day = 86_400.;
        let period = 400. * day;
        let mut clock = Clock { offset: 500. * day, ..default() };
        clock.hold(period);

        let mut readings = Vec::new();
        for step in 0..=20 {
            clock.offset_to(period, step as f64 / 20.);
            readings.push(clock.at());
        }

        let ran_on = readings[20] - readings[0];
        assert_eq!(ran_on, period, "end to end was not one turn");
        // Nothing anywhere in the system jumps, which is this being monotone.
        for pair in readings.windows(2) {
            let step = pair[1] - pair[0];
            assert!(step > 0., "the clock went backwards by {}", -step);
            assert!(step <= period / 20. + 1., "the clock jumped {step}");
        }
    }

    /// An anchor moves the slider it was taken for and no other
    ///
    /// A drag that never sees its own end leaves the anchor standing: a panel
    /// shut with the pointer down draws no slider that frame, so nothing says
    /// the drag is over. The next slider touched must measure its own body's
    /// turn rather than a count of somebody else's, which for a moon holding a
    /// planet's count is a clock thrown a long way from anywhere.
    #[test]
    fn an_anchor_moves_only_the_slider_it_was_taken_for() {
        let day = 86_400.;
        let mut clock = Clock { offset: 500. * day, ..default() };

        // A drag of the planet's slider that never ends.
        clock.hold(400. * day);
        // Then the moon's slider is touched.
        clock.offset_to(day, 0.5);

        assert_eq!(
            clock.at(),
            500.5 * day,
            "the moon's slider measured from the planet's turn",
        );
    }

    /// A moment out in the galaxy, `days` after the epoch
    fn read(days: i64) -> DateTime<Utc> {
        DateTime::UNIX_EPOCH + chrono::TimeDelta::days(days)
    }

    /// The map opens on the game's clock rather than on the scans
    ///
    /// A map of where things are is worth more than a map of where they were
    /// seen, and which day it is is the one thing the reading cannot be read
    /// off the rows.
    #[test]
    fn the_map_opens_on_the_games_clock() {
        let clock = Clock::default();

        assert!(clock.following());
        assert_eq!(clock.offset(), 0., "the map opened already run on");
    }

    /// Reading the game's clock puts the map however long it has been past the
    /// scans
    #[test]
    fn the_game_puts_the_map_past_the_scans_by_how_long_it_has_been() {
        let mut clock = Clock::default();

        clock.follows(read(20_000), read(19_000));

        assert_eq!(clock.at(), 1000. * 86_400.);
    }

    /// And is read whether the map is standing there or not
    ///
    /// So that standing back on it is the frame that happens on rather than the
    /// frame after, which is a system drawn once at the scans on the way.
    #[test]
    fn the_game_is_read_even_while_the_map_stands_at_the_scans() {
        let mut clock = Clock::default();
        clock.follow(false);

        clock.follows(read(20_000), read(19_000));
        assert_eq!(clock.at(), 0., "the map left the scans on its own");

        clock.follow(true);
        assert_eq!(clock.at(), 1000. * 86_400.);
    }

    /// A reading already near enough is left exactly as it was
    ///
    /// What reads the mark writes every star, body and orbit line in the held
    /// system back where it stands, and this is asked every frame. Nothing on
    /// screen moves through a second of a real orbit, so a reading followed to
    /// the frame is a system's insides rewritten sixty times over to move
    /// nothing.
    #[test]
    fn a_reading_near_enough_is_left_alone() {
        // A hair under the second the clock is stepped in.
        let drifted = 1000. * 86_400. - WITHIN / 2.;
        let mut clock = Clock { live: drifted, ..default() };

        clock.follows(read(20_000), read(19_000));

        assert_eq!(
            clock.at(),
            drifted,
            "the clock was put right for a drift nobody could see"
        );
    }

    /// And is put right rather than stepped on, so a drift is not carried
    ///
    /// The reading is worked out from the two moments every time rather than
    /// added to, so a map left running for an hour is an hour on rather than
    /// however much of one its frames added up to.
    #[test]
    fn a_reading_left_far_behind_is_put_right() {
        let mut clock = Clock { live: 5., ..default() };

        clock.follows(read(20_000), read(19_000));

        assert_eq!(clock.at(), 1000. * 86_400.);
    }

    /// A frame that finds the clock where it belongs does not count as winding
    /// it
    ///
    /// The clock is read every frame, and what reads the mark rebuilds every
    /// place in the held system. A frame that moved nothing has to leave the
    /// resource unwritten or the whole of a system is written back sixty times
    /// a second.
    #[test]
    fn a_frame_that_moves_nothing_does_not_move_the_clock() {
        let mut world = holding_a_clock();
        world.resource_mut::<Clock>().bypass_change_detection().live =
            1000. * 86_400.;
        world.clear_trackers();
        world.increment_change_tick();

        mark_if_moved(&mut world.resource_mut::<Clock>(), |clock| {
            clock.follows(read(20_000), read(19_000));
        });

        assert!(!written(&world), "a clock already right moved itself");
    }

    /// And one that finds it out of date does
    #[test]
    fn a_frame_that_moves_it_moves_the_clock() {
        let mut world = holding_a_clock();

        mark_if_moved(&mut world.resource_mut::<Clock>(), |clock| {
            clock.follows(read(20_000), read(19_000));
        });

        assert!(written(&world), "the map stood at its scans");
    }

    /// The game's clock stepping on while the map stands at the scans is not a
    /// redraw
    ///
    /// It is read either way, so that standing back on it is instant. Nothing
    /// is placed from it while the map is at the scans, and a system rebuilt
    /// once a second for a number nothing is drawing from is the whole of that
    /// system's insides for nothing.
    #[test]
    fn the_game_stepping_on_behind_the_scans_is_not_a_redraw() {
        let mut world = holding_a_clock();
        world.resource_mut::<Clock>().bypass_change_detection().follow(false);
        world.clear_trackers();
        world.increment_change_tick();

        mark_if_moved(&mut world.resource_mut::<Clock>(), |clock| {
            clock.follows(read(20_000), read(19_000));
        });

        assert!(!written(&world), "a reading nothing draws from redrew it");
    }

    /// A system is counted from the newest of its scans
    ///
    /// Which is what every path in it is dated against, so nothing has to be
    /// run backwards to meet the reading.
    #[test]
    fn a_system_is_counted_from_its_newest_scan() {
        let mut contents = Contents::default();
        let mut newer = body(2e9);
        newer.id = 2;
        newer.updated_at = read(20_000);
        contents.hold(holding(vec![body(1e9), newer]));

        assert_eq!(contents.recorded_at(), Some(read(20_000)));
        // And a system with nothing on record has no moment to count from.
        assert_eq!(Contents::default().recorded_at(), None);
    }

    /// A thing whose period nobody recorded has no turn to be a fraction of
    #[test]
    fn an_unrecorded_period_has_no_phase() {
        let mut clock = Clock { offset: 500. * 86_400., ..default() };

        assert_eq!(clock.through(0.), 0.);
        clock.offset_to(0., 0.5);
        assert_eq!(clock.at(), 500. * 86_400., "the map moved on nothing");
    }
}
