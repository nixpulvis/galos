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

    /// How far the system reaches, in metres
    ///
    /// To the far side of the outermost thing going round it — the apoapsis of
    /// the widest orbit, plus the radius of whatever is sitting at it, so that
    /// the shell drawn at this holds the whole of what it stands for rather
    /// than cutting through the last of it.
    ///
    /// Nothing until the rows are in, and nothing for a system that has none.
    /// Both are the same picture to whoever is drawing: the map cannot say how
    /// far this system reaches.
    pub fn extent(&self) -> Option<f32> {
        let Held::Known { stars, bodies, .. } = &self.held else {
            return None;
        };

        let reaches = bodies
            .iter()
            .map(|b| {
                reach(b.orbit.semi_major_axis, b.orbit.eccentricity)
                    + b.radius.max(0.)
            })
            .chain(stars.iter().map(|s| {
                // A primary goes round nothing, so it reaches only as far as
                // it is wide.
                s.orbit
                    .as_ref()
                    .map_or(0., |o| reach(o.semi_major_axis, o.eccentricity))
                    + s.radius.max(0.)
            }))
            .filter(|r| r.is_finite() && *r > 0.);

        reaches.fold(None, |widest: Option<f32>, r| {
            Some(widest.map_or(r, |w| w.max(r)))
        })
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

    /// Where the thing with `id` stands, in metres from the system's centre
    ///
    /// For one answer. Anything placing the whole system at once should build
    /// the [`Orbits`] once and ask it, rather than calling this per body.
    pub fn place(&self, id: i16, since: f64) -> DVec3 {
        self.orbits().place(id, since)
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

    /// A star with `id`, `a` metres out around whatever `parents` names
    fn star(id: i16, a: f32, parents: Vec<Parent>) -> DbStar {
        DbStar {
            system_address: 1,
            id,
            name: String::new(),
            parents,
            updated_at: chrono::DateTime::UNIX_EPOCH,
            updated_by: String::new(),
            absolute_magnitude: 0.,
            age_my: 0,
            distance_from_arrival_ls: 0.,
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

    /// A body under a barycenter is placed out where the barycenter is
    ///
    /// The case Ross 248 is made of: a close pair goes round a point that goes
    /// round a star that goes round the middle of the system. The pair names
    /// the point and nothing else does, so a map that does not hold the points
    /// breaks the chain at its first step and draws the pair at the middle
    /// with both of the outer orbits lost.
    #[test]
    fn a_body_under_a_barycenter_stands_out_where_it_belongs() {
        let mut close = body(1e9);
        close.id = 11;
        close.parents =
            vec![parent("Null", 10), parent("Star", 1), parent("Null", 0)];

        let contents = Contents {
            of: Some(1),
            held: Held::Known {
                stars: vec![star(1, 1e13, vec![parent("Null", 0)])],
                bodies: vec![close],
                centers: vec![center(10, 1e11)],
            },
        };

        // Every orbit is a circle read at the same angle, so the three stack
        // up and the body stands at their sum.
        let out = contents.place(11, 0.).length();
        let wanted = 1e13 + 1e11 + 1e9;
        assert!(
            (out - wanted).abs() < wanted * 1e-6,
            "the body stood {out}m out, not {wanted}m"
        );
    }

    /// And is at the middle of the system without the barycenter
    ///
    /// What the map was doing, and what the screenshot of Ross 248 showed: the
    /// walk ends at the missing point and only the pair's own small orbit is
    /// left, so it is drawn a million kilometres from the centre rather than
    /// ten billion.
    #[test]
    fn a_body_under_a_missing_barycenter_falls_to_the_middle() {
        let mut close = body(1e9);
        close.id = 11;
        close.parents =
            vec![parent("Null", 10), parent("Star", 1), parent("Null", 0)];

        let contents = Contents {
            of: Some(1),
            held: Held::Known {
                stars: vec![star(1, 1e13, vec![parent("Null", 0)])],
                bodies: vec![close],
                centers: vec![],
            },
        };

        assert!((contents.place(11, 0.).length() - 1e9).abs() < 1e3);
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
        let mut eccentric = body(1e12);
        eccentric.orbit.eccentricity = 0.5;

        assert_eq!(known(vec![eccentric]).extent(), Some(1.5e12));
    }

    /// A body's own size counts, so the shell holds the whole of it
    #[test]
    fn the_extent_takes_in_the_body_standing_at_it() {
        let mut wide = body(1e12);
        wide.radius = 7e7;

        assert_eq!(known(vec![wide]).extent(), Some(1e12 + 7e7));
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
