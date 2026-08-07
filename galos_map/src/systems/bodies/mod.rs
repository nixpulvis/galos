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

pub mod fetch;
pub mod orbit;
pub mod spawn;

pub fn plugin(app: &mut App) {
    app.init_resource::<Contents>();
    app.add_plugins(fetch::plugin);
    app.add_plugins(spawn::plugin);
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
    held: Held,
}

/// How far along the asking has got
#[derive(Default)]
enum Held {
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

    /// Whether the database has answered about `address`
    ///
    /// What the shell asks before it begins to clear: an answer of nothing is
    /// still an answer, and a system with no bodies on record is one the map
    /// knows about rather than one it has yet to ask after.
    pub fn known(&self, address: i64) -> bool {
        self.of == Some(address) && matches!(self.held, Held::Known { .. })
    }

    /// The stars of the system being held
    pub fn stars(&self) -> &[DbStar] {
        match &self.held {
            Held::Known { stars, .. } => stars,
            _ => &[],
        }
    }

    /// The bodies of the system being held
    pub fn bodies(&self) -> &[DbBody] {
        match &self.held {
            Held::Known { bodies, .. } => bodies,
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
    /// Nothing is drawn for one. What they are worth is that everything under
    /// one can be placed: a body measures its orbit about its nearest
    /// ancestor, and where that ancestor stands is the rest of the answer.
    pub fn barycenters(&self) -> &[DbBarycenter] {
        match &self.held {
            Held::Known { centers, .. } => centers,
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
        let Held::Known { stars, bodies, .. } = &self.held else {
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
        // The barycenters go in as well, though nothing is drawn for one.
        // A close pair names its center and the center names the star, so
        // leaving them out breaks the chain at its first step and drops the
        // pair at the middle of the system with its whole outer orbit lost.
        for center in self.barycenters() {
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
fn recorded_body(body: &DbBody) -> Orbit {
    Orbit::recorded(
        body.orbit.semi_major_axis,
        body.orbit.eccentricity,
        body.orbit.orbital_inclination,
        body.orbit.periapsis,
        body.orbit.ascending_node,
        body.orbit.mean_anomaly,
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
            orbit.ascending_node,
            orbit.mean_anomaly,
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
            orbit.ascending_node,
            orbit.mean_anomaly,
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
    fn body(a: f32) -> DbBody {
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
            temperature: 0.,
            surface: None,
            orbit: JournalOrbit {
                semi_major_axis: a,
                eccentricity: 0.,
                orbital_inclination: 0.,
                periapsis: 0.,
                orbital_period: 0.,
                ascending_node: 0.,
                mean_anomaly: 0.,
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
            ascending_node: 0.,
            mean_anomaly: 0.,
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
            held: Held::Known {
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

    /// Contents that came back holding `bodies`
    fn known(bodies: Vec<DbBody>) -> Contents {
        Contents {
            of: Some(1),
            held: Held::Known { stars: vec![], bodies, centers: vec![] },
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
        assert_eq!(known(vec![]).extent(), None);
    }

    /// The extent reaches the furthest body, not the last one read
    #[test]
    fn the_extent_reaches_the_outermost_body() {
        let contents = known(vec![body(1e11), body(5e12), body(3e11)]);

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

        assert_eq!(known(vec![eccentric]).extent(), Some(3e12));
    }

    /// A body's own size counts, so the shell holds the whole of it
    #[test]
    fn the_extent_takes_in_the_body_standing_at_it() {
        let mut wide = body(5e12);
        wide.radius = 7e7;

        assert_eq!(known(vec![wide]).extent(), Some(5e12 + 7e7));
    }

    /// A body sitting at the centre does not make an extent of nothing
    ///
    /// The primary star has no orbit and no distance from itself, so it comes
    /// back as a zero, and a system of nothing but that has nothing to say
    /// about how far it reaches.
    #[test]
    fn a_body_at_the_centre_leaves_the_extent_unsaid() {
        assert_eq!(known(vec![body(0.)]).extent(), None);
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
            held: Held::Known {
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

        let extent = known(vec![escaping]).extent().unwrap();
        assert!(extent.is_finite(), "the extent ran away to {extent}");
        assert!(extent < 2e12, "the extent reached {extent}");
    }
}
