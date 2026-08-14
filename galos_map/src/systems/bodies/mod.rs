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
use galos_db::barycenters::Barycenter as DbBarycenter;
use galos_db::bodies::Body as DbBody;
use galos_db::stars::Star as DbStar;
use orbit::{Orbit, Orbits};
use std::collections::HashSet;

pub mod fetch;
pub mod orbit;
pub mod spawn;

pub fn plugin(app: &mut App) {
    app.init_resource::<Contents>();
    app.init_resource::<Clock>();
    app.add_plugins(fetch::plugin);
    app.add_plugins(spawn::plugin);
}

/// How long the map has run a system on from when its things were scanned
///
/// Seconds, and one reading for the whole system, so what is drawn is always a
/// single moment rather than an arrangement composed body by body.
///
/// Set from a body's own panel rather than from one control over the system,
/// because a system has no span that suits all of it: the slowest body of one
/// takes a median 993 times as long to come round as its fastest, and in Sol it
/// is four million times. Each panel gears this to its own body's period, so
/// a slider is one orbit of the body it stands under whatever that body's
/// orbit is worth in seconds.
///
/// Zero draws every thing where its own scan put it. The bodies of one system
/// are scanned a median one minute forty-five apart against periods mostly over
/// a year, so zero is very nearly one moment rather than a smear -- which is
/// also why one reading serves the whole system instead of each body being
/// advanced from an epoch of its own.
#[derive(Resource, Default)]
pub struct Clock {
    /// The seconds themselves
    pub at: f64,
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
    /// Whether `at` still stands in the turn this was taken for
    ///
    /// Measured rather than trusted. A drag that never sees its own end leaves
    /// the anchor standing, and the clock may have been wound anywhere since by
    /// another body's slider, so an anchor is only worth measuring from where
    /// the reading could have come from it.
    ///
    /// The far end counts. A slider run to it lands exactly on the beginning of
    /// the next turn and is held there, which is what the anchor is for.
    fn holds(&self, at: f64) -> bool {
        at >= self.turns * self.period && at <= (self.turns + 1.) * self.period
    }
}

impl Clock {
    /// Where `period` stands in its own turn, from none of it to all
    ///
    /// What a slider geared to one body reads.
    pub fn through(&self, period: f64) -> f64 {
        if period <= 0. {
            return 0.;
        }
        let turns = self.at / period;
        turns - turns.floor()
    }

    /// Take hold of the turn a slider over `period` is setting out from
    ///
    /// Said when a drag begins, so that [`Self::wind_to`] measures from where
    /// the slider started rather than from where it has since put the clock.
    pub fn hold(&mut self, period: f64) {
        if period > 0. {
            self.held =
                Some(Held { period, turns: (self.at / period).floor() });
        }
    }

    /// Let go of it, the drag being over
    pub fn release(&mut self) {
        self.held = None;
    }

    /// Move to where `period` stands `through` of the way round its turn
    ///
    /// Within the turn the slider set out from, so dragging one moves the map
    /// by at most a single period of the body it is geared to, and moves it
    /// evenly: a slider run from end to end runs the clock on by exactly one
    /// turn of that body, with nothing anywhere in the system jumping on the
    /// way.
    ///
    /// A moon's slider therefore barely stirs the planet it goes round.
    /// Reaching for the first turn instead would throw the whole system back to
    /// the beginning every time a moon was nudged.
    pub fn wind_to(&mut self, period: f64, through: f64) {
        if period <= 0. {
            return;
        }
        let whole = match &self.held {
            Some(held) if held.period == period && held.holds(self.at) => {
                held.turns
            }
            _ => (self.at / period).floor(),
        };
        self.at = (whole + through) * period;
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
/// nothing until it is dragged.
pub(crate) fn mark_if_wound<T>(
    clock: &mut impl DetectChangesMut<Inner = Clock>,
    with: impl FnOnce(&mut Clock) -> T,
) -> T {
    let wound = clock.bypass_change_detection();
    let was = wound.at;
    let drawn = with(&mut *wound);
    let moved = wound.at != was;

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
    Known {
        stars: Vec<DbStar>,
        bodies: Vec<DbBody>,
        centers: Vec<DbBarycenter>,
    },
}

impl Contents {
    /// Which system is being held, if any
    pub fn of(&self) -> Option<i64> {
        self.of
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
    pub(super) fn hold(
        &mut self,
        stars: Vec<DbStar>,
        bodies: Vec<DbBody>,
        centers: Vec<DbBarycenter>,
    ) {
        if let FetchState::Known { stars: had, bodies: were, centers: about } =
            &self.state
            && *had == stars
            && *were == bodies
            && *about == centers
        {
            return;
        }

        self.state = FetchState::Known { stars, bodies, centers };
        self.revision = self.revision.wrapping_add(1);
    }

    /// Whether the database has answered about `address`
    ///
    /// What the shell asks before it begins to clear: an answer of nothing is
    /// still an answer, and a system with no bodies on record is one the map
    /// holds rather than one it has yet to ask after.
    pub fn holds(&self, address: i64) -> bool {
        self.of == Some(address)
            && matches!(self.state, FetchState::Known { .. })
    }

    /// The stars of the system being held
    pub fn stars(&self) -> &[DbStar] {
        match &self.state {
            FetchState::Known { stars, .. } => stars,
            _ => &[],
        }
    }

    /// The bodies of the system being held
    pub fn bodies(&self) -> &[DbBody] {
        match &self.state {
            FetchState::Known { bodies, .. } => bodies,
            _ => &[],
        }
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

    /// The points a close pair of the system being held goes round
    ///
    /// No sphere stands at one, and its ellipse is drawn: a close pair rides
    /// that ellipse, so it is the whole of how far out the pair sits. What
    /// they are worth besides is that everything under one can be placed. A
    /// body measures its orbit about its nearest ancestor, and where that
    /// ancestor stands is the rest of the answer.
    pub fn barycenters(&self) -> &[DbBarycenter] {
        match &self.state {
            FetchState::Known { centers, .. } => centers,
            _ => &[],
        }
    }

    /// Which star the system arrives at
    ///
    /// The one every distance inside a system is quoted from, and where the
    /// map puts the middle of it: a scan records the arrival star at no
    /// distance from arrival, arriving being what happens at it.
    ///
    /// The nearest to arrival rather than the one recorded at exactly nothing,
    /// so a system whose arrival star was never scanned still has a middle.
    /// Nothing at all for one with no star on record, which is then drawn
    /// about the point its contents go round, there being nothing else to
    /// offer.
    pub fn primary(&self) -> Option<i16> {
        self.stars()
            .iter()
            .min_by(|one, other| {
                one.distance_from_arrival_ls
                    .total_cmp(&other.distance_from_arrival_ls)
            })
            .map(|star| star.id)
    }

    /// Where the middle of the system falls, as [`Orbits`] measures
    ///
    /// Which is to say where the arrival star stands about the point the
    /// system's stars go round. Everything drawn inside a system is placed
    /// short of this, so the star lands at the system's own position and
    /// flying to a system arrives at its star rather than at a point in
    /// between two of them.
    pub(super) fn middle(&self, orbits: &Orbits, since: f64) -> DVec3 {
        self.primary().map_or(DVec3::ZERO, |id| orbits.place(id, since))
    }

    /// What the thing with `id` goes round, as the bodies under it say
    ///
    /// A barycenter is recorded with an orbit and nothing about what that
    /// orbit is around, `ScanBaryCentre` not naming any ancestor of its own.
    /// What does name one is every body beneath it, each of which carries the
    /// whole chain back to its star, so the link is read off there: find the
    /// barycenter in an ancestry and take whatever the scan put behind it.
    fn goes_round(&self, id: i16) -> Option<i16> {
        self.bodies().iter().find_map(|body| {
            let mut ancestry =
                body.parents.iter().skip_while(|parent| parent.id != id);
            ancestry.next()?;
            Some(ancestry.next()?.id)
        })
    }

    /// How far the system reaches from its middle, in metres, and never less
    /// than [`STAND_IN`]
    ///
    /// Measured from the arrival star, which is where the shell is drawn and
    /// where everything inside is placed short of. An orbit is recorded about
    /// whatever it goes round, so its own apoapsis says how far a thing gets
    /// from its parent and nothing about how far the parent stands from the
    /// middle. Both are needed: in a wide binary the two stars are ten billion
    /// kilometres apart, and reading the orbits alone leaves everything about
    /// the far one outside the shell drawn about the near one.
    ///
    /// To the far side of what is drawn rather than to where it stands. What
    /// is drawn for a thing is a sphere at its own place and the whole ellipse
    /// of its orbit, and the ellipse reaches its apoapsis on the far side of
    /// the parent from the middle, which is further out than the thing itself
    /// ever gets.
    ///
    /// Nothing until the rows are in, and nothing for a system that has none.
    /// Both are the same picture to whoever is drawing: the map cannot say how
    /// far this system reaches.
    pub fn extent(&self) -> Option<f32> {
        let FetchState::Known { stars, bodies, centers } = &self.state else {
            return None;
        };

        let orbits = self.orbits();
        let middle = self.middle(&orbits, 0.);
        // How far from the middle the orbit itself is centred, which is where
        // whatever it goes round stands.
        let about = |parent: Option<i16>| {
            let anchor = parent.map_or(DVec3::ZERO, |id| orbits.place(id, 0.));
            (anchor - middle).length() as f32
        };

        let reaches = bodies
            .iter()
            .map(|b| {
                about(b.parent_id())
                    + reach(b.orbit.semi_major_axis, b.orbit.eccentricity)
                    + b.radius.max(0.)
            })
            .chain(stars.iter().map(|s| {
                // A primary goes round nothing, so it reaches only as far as
                // it is wide.
                about(s.parent_id())
                    + s.orbit.as_ref().map_or(0., |o| {
                        reach(o.semi_major_axis, o.eccentricity)
                    })
                    + s.radius.max(0.)
            }))
            .chain(centers.iter().filter(|c| orbits.holds(c.id)).map(|c| {
                // A barycenter has no size of its own, so its ellipse is the
                // whole of what is drawn for it. The pair riding it says only
                // where it stands today, which on an eccentric orbit is short
                // of where the line reaches.
                //
                // Whichever of them the orbits kept, so that the shell is
                // measured against exactly what is drawn.
                about(orbits.parent(c.id))
                    + c.orbit.as_ref().map_or(0., |o| {
                        reach(o.semi_major_axis, o.eccentricity)
                    })
            }))
            .filter(|r| r.is_finite() && *r > 0.);

        reaches
            .fold(None, |widest: Option<f32>, r| {
                Some(widest.map_or(r, |w| w.max(r)))
            })
            .map(|widest| widest.max(STAND_IN))
    }

    /// Where everything in the system stands, `since` seconds after the epoch
    ///
    /// Worked out for the system as a whole rather than a body at a time,
    /// since placing a moon means placing its planet too and doing that once
    /// per moon would place the planet again for each of them.
    ///
    /// Stars go in alongside bodies. A body's `parent_id` may name either, and
    /// the two share a numbering, so a chain that stepped over the stars would
    /// lose the planet's own place about its sun in a system that has more
    /// than one.
    pub fn orbits(&self) -> Orbits {
        let mut orbits = Orbits::default();
        for star in self.stars() {
            orbits.insert(star.id, star.parent_id(), recorded_star(star));
        }
        for body in self.bodies() {
            orbits.insert(body.id, body.parent_id(), recorded_body(body));
        }
        // The barycenters go in as well. A close pair names its center and the
        // center names the star, so leaving them out breaks the chain at its
        // first step and drops the pair at the middle of the system with its
        // whole outer orbit lost.
        //
        // Only the ones something rides. A center and the pair that goes round
        // it arrive as separate scans, so the database holds a great many
        // points with nothing yet under them. Neither of the two things a
        // center is worth applies to one of those: no chain runs through it,
        // and the ellipse drawn for it would be a ring with nothing on it.
        let ridden: HashSet<i16> = self
            .bodies()
            .iter()
            .flat_map(|body| body.parents.iter().map(|parent| parent.id))
            .collect();
        for center in self.barycenters() {
            if !ridden.contains(&center.id) {
                continue;
            }
            orbits.insert(
                center.id,
                self.goes_round(center.id),
                recorded_center(center),
            );
        }
        orbits
    }

    /// Where the thing with `id` stands, in metres from the system's middle
    ///
    /// For one answer. Anything placing the whole system at once should build
    /// the [`Orbits`] once and ask it, rather than calling this per body.
    pub fn place(&self, id: i16, since: f64) -> DVec3 {
        let orbits = self.orbits();
        orbits.place(id, since) - self.middle(&orbits, since)
    }
}

/// The orbit a body was recorded on
///
/// TODO: A node and an anomaly nobody reported are read as zero, here and in
/// the two below, which draws the thing at periapsis. Worth answering properly
/// if that stops being rare: the path is known and the place along it is not,
/// a null `mean_anomaly` is how to tell the two apart, and the panel is where
/// it can be said in words rather than guessed at in space.
fn recorded_body(body: &DbBody) -> Orbit {
    Orbit::recorded(
        body.orbit.semi_major_axis,
        body.orbit.eccentricity,
        body.orbit.orbital_inclination,
        body.orbit.periapsis,
        body.orbit.ascending_node.unwrap_or(0.),
        body.orbit.mean_anomaly.unwrap_or(0.),
        body.orbit.orbital_period,
    )
}

/// The orbit a barycenter was recorded on
///
/// The one at the root of a multi-star system goes round nothing and is
/// recorded without an orbit, as a primary star is, and stands at the middle
/// of the system for the same reason.
fn recorded_center(center: &DbBarycenter) -> Orbit {
    center.orbit.as_ref().map_or_else(Orbit::still, |orbit| {
        Orbit::recorded(
            orbit.semi_major_axis,
            orbit.eccentricity,
            orbit.orbital_inclination,
            orbit.periapsis,
            orbit.ascending_node.unwrap_or(0.),
            orbit.mean_anomaly.unwrap_or(0.),
            orbit.orbital_period,
        )
    })
}

/// The orbit a star was recorded on
///
/// A primary star goes round nothing and is recorded without an orbit, which
/// comes back as one of no size: it stands at the middle of its system, which
/// is what everything else there is measured from.
fn recorded_star(star: &DbStar) -> Orbit {
    star.orbit.as_ref().map_or_else(Orbit::still, |orbit| {
        Orbit::recorded(
            orbit.semi_major_axis,
            orbit.eccentricity,
            orbit.orbital_inclination,
            orbit.periapsis,
            orbit.ascending_node.unwrap_or(0.),
            orbit.mean_anomaly.unwrap_or(0.),
            orbit.orbital_period,
        )
    })
}

/// How far a system reaches when the map has not been told, in metres
///
/// Five thousand light seconds, near the middle of what a system comes to.
/// Stands for one nobody has asked about and one with nothing on record alike,
/// both being the map not knowing, and neither being worth telling apart on
/// screen.
///
/// A floor under [`Contents::extent`] as much as a stand-in for it. Every
/// shell but the one system being held is drawn at this, so a system whose rows
/// land saying it reaches less would collapse out of the mark it had been drawn
/// as on the frame they arrive. A star with nothing on record around it is the
/// far end of that: its own radius is a twenty-five thousandth of this, and the
/// shell became a skin on the star rather than a mark around the system.
pub const STAND_IN: f32 = 1.5e12;

/// How far an orbit gets from what it goes round, in metres
///
/// The apoapsis, which is the far end of the ellipse rather than its average.
/// A body on an eccentric orbit spends most of its time out there, and a shell
/// drawn to the average would leave it outside for most of its year.
///
/// Eccentricity is clamped short of one. The database records a scan rather
/// than a solution, and a parabola read literally would reach forever.
fn reach(semi_major_axis: f32, eccentricity: f32) -> f32 {
    semi_major_axis.max(0.) * (1. + eccentricity.clamp(0., 0.99))
}

#[cfg(test)]
mod tests {
    use super::*;
    use elite_journal::body::{
        Discovery as JournalDiscovery, Orbit as JournalOrbit,
        Spin as JournalSpin,
    };
    use galos_db::bodies::Parent;

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
            discovery: JournalDiscovery { discovered: false, mapped: false },
        }
    }

    /// A circular orbit `a` metres across, as the journal records one
    fn circle(a: f32) -> JournalOrbit {
        JournalOrbit {
            semi_major_axis: a,
            eccentricity: 0.,
            orbital_inclination: 0.,
            periapsis: 0.,
            orbital_period: 1.,
            ascending_node: Some(0.),
            mean_anomaly: Some(0.),
        }
    }

    /// A star `away` light seconds from arrival, `a` metres out around
    /// whatever `parents` names
    fn star(id: i16, away: f32, a: f32, parents: Vec<Parent>) -> DbStar {
        DbStar {
            system_address: 1,
            id,
            name: String::new(),
            parents,
            updated_at: chrono::DateTime::UNIX_EPOCH,
            updated_by: String::new(),
            absolute_magnitude: 0.,
            age_my: 0,
            distance_from_arrival_ls: away,
            luminosity: String::new(),
            star_class: String::new(),
            stellar_mass: 0.,
            subclass: 0,
            orbit: Some(circle(a)),
            spin: JournalSpin { period: 0., tilt: 0. },
            radius: 0.,
            temperature: 0.,
            discovery: JournalDiscovery { discovered: false, mapped: false },
        }
    }

    /// A barycenter with `id`, `a` metres out around whatever holds it
    fn center(id: i16, a: f32) -> DbBarycenter {
        DbBarycenter {
            system_address: 1,
            id,
            updated_at: chrono::DateTime::UNIX_EPOCH,
            updated_by: String::new(),
            orbit: Some(circle(a)),
        }
    }

    /// One ancestor a scan named
    fn parent(ty: &str, id: i16) -> Parent {
        Parent { ty: Some(ty.to_owned()), id }
    }

    /// A system of two stars and one close pair, as Ross 248 is
    ///
    /// Star one is the arrival star and star two is further out from the point
    /// they both go round. Body eleven goes round a barycenter that goes round
    /// star one, which is the chain that has to be walked whole.
    fn binary(with_center: bool) -> Contents {
        let mut close = body(1e9);
        close.id = 11;
        close.parents =
            vec![parent("Null", 10), parent("Star", 1), parent("Null", 0)];

        Contents {
            of: Some(1),
            revision: 0,
            state: FetchState::Known {
                stars: vec![
                    star(1, 0., 1e13, vec![parent("Null", 0)]),
                    star(2, 1e5, 2e13, vec![parent("Null", 0)]),
                ],
                bodies: vec![close],
                centers: if with_center {
                    vec![center(10, 1e11)]
                } else {
                    vec![]
                },
            },
        }
    }

    /// The middle of a system is the star it arrives at
    ///
    /// Not the point its stars go round, which in a wide binary is ten billion
    /// kilometres of empty sky. A system's recorded position is where the map
    /// sends the camera and what it zooms towards, so what stands there has to
    /// be what the user came for.
    #[test]
    fn the_middle_of_a_system_is_the_star_it_arrives_at() {
        let contents = binary(true);

        assert_eq!(contents.place(1, 0.), DVec3::ZERO);
        // Every orbit here is a circle read at the same angle, so the two
        // stars lie the same way and stand their orbits apart. Both have moved
        // in by the arrival star's own orbit, which is the whole of this.
        let far = contents.place(2, 0.).length();
        assert!(
            (far - 1e13).abs() < 1e13 * 1e-6,
            "the far star stood {far}m off, not the 1e13 between them"
        );
    }

    /// The arrival star is the one recorded at no distance from arrival
    #[test]
    fn the_arrival_star_is_the_one_arrived_at() {
        assert_eq!(binary(true).primary(), Some(1));
        assert_eq!(Contents::default().primary(), None);
    }

    /// A body under a barycenter is placed out where the barycenter is
    ///
    /// The case Ross 248 is made of: a close pair goes round a point that goes
    /// round a star. The pair names the point and nothing else does, so a map
    /// that does not hold the points breaks the chain at its first step.
    ///
    /// Measured from the star, which is the middle, so the star's own orbit
    /// about the point its pair goes round drops out of the sum and what is
    /// left is the barycenter's orbit and the body's own.
    #[test]
    fn a_body_under_a_barycenter_stands_out_where_it_belongs() {
        // Every orbit is a circle read at the same angle, so they stack up.
        let out = binary(true).place(11, 0.).length();
        let wanted = 1e11 + 1e9;

        assert!(
            (out - wanted).abs() < wanted * 1e-6,
            "the body stood {out}m out, not {wanted}m"
        );
    }

    /// And loses its way entirely without it
    ///
    /// The walk ends at the missing point and only the body's own orbit is
    /// left, measured from where the walk stopped. In a system of one star
    /// that is the star, and the body is merely drawn too near it; in a binary
    /// it is the point the two stars go round, and the body is thrown out into
    /// the space between them.
    #[test]
    fn a_body_under_a_missing_barycenter_loses_its_way() {
        let out = binary(false).place(11, 0.).length();

        assert!(
            (out - 1e13).abs() < 1e13 * 1e-3,
            "the body stood {out}m from the star, not the 1e13 of its orbit"
        );
    }

    /// Every kind of thing in a system is offered a line
    ///
    /// The lines are drawn from what [`Contents::orbits`] holds, so a kind
    /// missing here is a kind placed on the map with no orbit drawn for it.
    /// The barycenter is the one that costs something: nothing is drawn at
    /// one, but a close pair rides its ellipse, and without that the pair is
    /// two small rings around a point nothing leads to.
    #[test]
    fn every_kind_of_thing_is_offered_a_line() {
        let orbits = binary(true).orbits();
        let mut held: Vec<_> = orbits.circling().map(|(id, _)| id).collect();
        held.sort();

        // Two stars, one body, one barycenter.
        assert_eq!(held, vec![1, 2, 10, 11]);
        assert!(
            orbits.path(10, 64).is_some(),
            "the barycenter was offered no path"
        );
    }

    /// A barycenter nothing rides is left out, and does not stretch the system
    ///
    /// A center and the pair that goes round it arrive as separate scans, so
    /// the database holds a great many points with nothing yet under them. The
    /// ellipse drawn for one would be a ring with nothing on it, and a shell
    /// drawn out to that ring is a system with one star in the middle of a
    /// great deal of nothing.
    #[test]
    fn a_barycenter_nothing_rides_is_left_out() {
        let mut lone = star(1, 0., 0., vec![]);
        lone.radius = 5.9e7;
        let contents = Contents {
            of: Some(1),
            revision: 0,
            state: FetchState::Known {
                stars: vec![lone],
                bodies: vec![],
                centers: vec![center(10, 4e12)],
            },
        };

        assert!(
            !contents.orbits().holds(10),
            "the center was kept with nothing riding it"
        );
        assert_eq!(contents.extent(), Some(STAND_IN));
    }

    /// A system whose outermost thing is a close pair on an eccentric orbit
    ///
    /// One star at the middle, a barycenter going round it, and one body
    /// riding the barycenter. A mean anomaly of nothing stands the pair at
    /// periapsis, so where it is today and how far its line reaches are as far
    /// apart as the orbit allows.
    fn eccentric_pair() -> Contents {
        let mut close = body(1e8);
        close.id = 11;
        close.parents = vec![parent("Null", 10), parent("Star", 1)];

        let mut wide = center(10, 4e12);
        wide.orbit.as_mut().expect("a center with an orbit").eccentricity = 0.5;

        Contents {
            of: Some(1),
            revision: 0,
            state: FetchState::Known {
                stars: vec![star(1, 0., 0., vec![])],
                bodies: vec![close],
                centers: vec![wide],
            },
        }
    }

    /// The extent reaches the far end of a barycenter's ellipse
    ///
    /// The pair riding it says only where it stands today. Measured from that
    /// alone the shell is drawn at the periapsis and the far half of the line
    /// hangs outside the system it belongs to.
    #[test]
    fn the_extent_takes_in_the_whole_of_a_barycenters_ellipse() {
        let reaches =
            eccentric_pair().extent().expect("the pair reaches somewhere");

        assert!(
            (reaches - 6e12).abs() < 6e12 * 1e-6,
            "the system reached {reaches}m, not the 6e12 of the apoapsis"
        );
    }

    /// And no part of that ellipse falls outside it
    #[test]
    fn a_barycenters_line_stays_inside_the_extent() {
        let contents = eccentric_pair();
        let orbits = contents.orbits();
        let middle = contents.middle(&orbits, 0.);
        let about = orbits.place(1, 0.) - middle;
        let reaches =
            contents.extent().expect("the pair reaches somewhere") as f64;

        for point in orbits.path(10, 64).expect("the center has a path") {
            let out = (about + point).length();
            assert!(
                out <= reaches,
                "the line reached {out}m out, past a {reaches}m extent"
            );
        }
    }

    /// Contents that came back holding `bodies`
    fn holding(bodies: Vec<DbBody>) -> Contents {
        Contents {
            of: Some(1),
            revision: 0,
            state: FetchState::Known { stars: vec![], bodies, centers: vec![] },
        }
    }

    /// A system nobody has asked about has no extent to give
    ///
    /// Which is the same answer as one that came back empty, and deliberately
    /// so: in both the map cannot say how far the system reaches.
    #[test]
    fn a_system_not_asked_about_has_no_extent() {
        assert_eq!(Contents::default().extent(), None);
    }

    /// Nor has one that came back with nothing in it
    #[test]
    fn a_system_with_nothing_on_record_has_no_extent() {
        assert_eq!(holding(vec![]).extent(), None);
    }

    /// The extent reaches the furthest body, not the last one read
    #[test]
    fn the_extent_reaches_the_outermost_body() {
        let contents = holding(vec![body(1e11), body(5e12), body(3e11)]);

        assert_eq!(contents.extent(), Some(5e12));
    }

    /// An eccentric orbit is measured to the far end of its ellipse
    ///
    /// Where the body actually gets to. Measuring to the semi-major axis
    /// would leave it outside the shell for the half of its year it spends
    /// beyond that.
    #[test]
    fn an_eccentric_orbit_is_measured_where_it_reaches() {
        let mut eccentric = body(2e12);
        eccentric.orbit.eccentricity = 0.5;

        assert_eq!(holding(vec![eccentric]).extent(), Some(3e12));
    }

    /// A body's own size counts, so the shell holds the whole of it
    #[test]
    fn the_extent_takes_in_the_body_standing_at_it() {
        let mut wide = body(5e12);
        wide.radius = 7e7;

        assert_eq!(holding(vec![wide]).extent(), Some(5e12 + 7e7));
    }

    /// A body sitting at the centre does not make an extent of nothing
    ///
    /// The primary star has no orbit and no distance from itself, so it comes
    /// back as a zero, and a system of nothing but that has nothing to say
    /// about how far it reaches.
    #[test]
    fn a_body_at_the_centre_leaves_the_extent_unsaid() {
        assert_eq!(holding(vec![body(0.)]).extent(), None);
    }

    /// The extent is measured from the middle, not from what a thing goes round
    ///
    /// The whole of a wide binary is drawn about its arrival star, and the far
    /// star's orbit reaches its apoapsis on the other side of the point the
    /// pair goes round. That point stands the arrival star's own orbit away
    /// from the middle, so the two add. Read as orbits alone the extent stops
    /// at the wider of them and the shell cuts through the far half of the
    /// system, which is where the bodies out there were coming from.
    #[test]
    fn the_extent_is_measured_from_the_middle() {
        let reaches =
            binary(true).extent().expect("a binary reaches somewhere");

        assert!(
            (reaches - 3e13).abs() < 3e13 * 1e-6,
            "the binary reached {reaches}m, not the 3e13 out to the far side \
             of the outer star's orbit"
        );
    }

    /// Nothing drawn in a system stands outside the extent
    #[test]
    fn nothing_in_a_binary_stands_outside_its_extent() {
        let contents = binary(true);
        let reaches = contents.extent().expect("a binary reaches somewhere");

        for id in [1, 2, 11] {
            let out = contents.place(id, 0.).length();
            assert!(
                out <= reaches as f64,
                "{id} stood {out}m out, past a {reaches}m extent"
            );
        }
    }

    /// A system of nothing but its star still reaches the stand-in
    ///
    /// Its own radius is a twenty-five thousandth of it. Every shell but the
    /// held one is drawn at the stand-in, so without this the shell collapses
    /// to a skin on the star at the moment the rows land.
    #[test]
    fn a_system_of_one_star_still_reaches_the_stand_in() {
        let mut lone = star(1, 0., 0., vec![]);
        lone.radius = 5.9e7;
        let contents = Contents {
            of: Some(1),
            revision: 0,
            state: FetchState::Known {
                stars: vec![lone],
                bodies: vec![],
                centers: vec![],
            },
        };

        assert_eq!(contents.extent(), Some(STAND_IN));
    }

    /// A near-parabolic orbit is held to something finite
    ///
    /// What the database holds is a scan rather than a solution, so an
    /// eccentricity of one is a reading rather than an escape, and reading it
    /// literally would put the shell at infinity.
    #[test]
    fn an_eccentricity_of_one_does_not_reach_forever() {
        let mut escaping = body(1e12);
        escaping.orbit.eccentricity = 1.;

        let extent = holding(vec![escaping]).extent().unwrap();
        assert!(extent.is_finite(), "the extent ran away to {extent}");
        assert!(extent < 2e12, "the extent reached {extent}");
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
        contents.hold(vec![], vec![body(1e9)], vec![]);

        let first = contents.revision();
        contents.hold(vec![], vec![body(1e9)], vec![]);

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
        contents.hold(vec![], vec![body(1e9)], vec![]);
        let first = contents.revision();

        let mut arriving = body(2e9);
        arriving.id = 2;
        contents.hold(vec![], vec![body(1e9), arriving], vec![]);

        assert_ne!(
            contents.revision(),
            first,
            "a body that was not there before read as the same rows"
        );
        assert_eq!(contents.bodies().len(), 2);
    }

    /// A slider reads where its own body stands in the turn it is in
    #[test]
    fn a_slider_reads_its_own_bodys_turn() {
        let day = 86_400.;
        // A quarter through its second turn of a four hundred day orbit.
        let clock = Clock { at: 500. * day, ..default() };

        assert_eq!(clock.through(400. * day), 0.25);
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
        let mut clock = Clock { at: 500. * day, ..default() };

        clock.wind_to(day, 0.5);

        assert_eq!(
            clock.at,
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
        let mut clock = Clock { at: 500. * day, ..default() };

        clock.hold(period);
        clock.wind_to(period, 1.);
        let once = clock.at;
        for _ in 0..30 {
            clock.wind_to(period, 1.);
        }

        assert_eq!(
            clock.at, once,
            "the clock ran away while the slider was held"
        );
    }

    /// An anchor left standing by a drag that never ended is not measured from
    ///
    /// `drag_stopped` may never arrive: a panel shut with the pointer down
    /// leaves the anchor where it is. The clock can be wound anywhere else
    /// before that body's slider is touched again, and measuring from a turn
    /// the system left long ago throws the whole of it back to that turn.
    #[test]
    fn an_anchor_from_a_drag_that_never_ended_is_let_go_of() {
        let day = 86_400.;
        let period = 400. * day;
        let mut clock = Clock { at: 500. * day, ..default() };

        // A drag that begins and never sees its own end.
        clock.hold(period);
        // And the clock moves on, wound by some other body's slider.
        clock.at = 900. * day;

        clock.wind_to(period, 0.5);

        assert_eq!(
            clock.at,
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

    /// A panel that only reads the clock does not count as winding it
    ///
    /// A panel is handed the clock every frame it is open, and being handed a
    /// [`ResMut`] is what marks a resource written. What reads that mark
    /// rebuilds every star, body and orbit line in the held system, so a panel
    /// left standing open would rebuild the whole of it every frame.
    #[test]
    fn a_panel_reading_the_clock_does_not_wind_it() {
        let mut world = holding_a_clock();

        mark_if_wound(&mut world.resource_mut::<Clock>(), |clock| {
            clock.through(86_400.);
        });

        assert!(!written(&world), "an untouched slider wound the clock");
    }

    /// Taking hold of the slider does not either, until it is dragged
    ///
    /// A drag begins on the press, and the turn it sets out from is worked out
    /// then. Nothing has moved yet, so nothing needs redrawing.
    #[test]
    fn taking_hold_of_the_slider_does_not_wind_the_clock() {
        let mut world = holding_a_clock();

        mark_if_wound(&mut world.resource_mut::<Clock>(), |clock| {
            clock.hold(86_400.);
        });

        assert!(!written(&world), "holding the slider wound the clock");
    }

    /// Dragging one does
    #[test]
    fn dragging_the_slider_winds_the_clock() {
        let mut world = holding_a_clock();

        mark_if_wound(&mut world.resource_mut::<Clock>(), |clock| {
            clock.wind_to(86_400., 0.5);
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
        let mut clock = Clock { at: 500. * day, ..default() };
        clock.hold(period);

        let mut readings = Vec::new();
        for step in 0..=20 {
            clock.wind_to(period, step as f64 / 20.);
            readings.push(clock.at);
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
        let mut clock = Clock { at: 500. * day, ..default() };

        // A drag of the planet's slider that never ends.
        clock.hold(400. * day);
        // Then the moon's slider is touched.
        clock.wind_to(day, 0.5);

        assert_eq!(
            clock.at,
            500.5 * day,
            "the moon's slider measured from the planet's turn",
        );
    }

    /// A thing whose period nobody recorded has no turn to be a fraction of
    #[test]
    fn an_unrecorded_period_has_no_phase() {
        let mut clock = Clock { at: 500. * 86_400., ..default() };

        assert_eq!(clock.through(0.), 0.);
        clock.wind_to(0., 0.5);
        assert_eq!(clock.at, 500. * 86_400., "the map moved on nothing");
    }
}
