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
    Known { stars: Vec<DbStar>, bodies: Vec<DbBody> },
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
        let Held::Known { stars, bodies } = &self.held else { return None };

        let reaches = bodies
            .iter()
            .map(|b| {
                reach(b.semi_major_axis, b.eccentricity) + b.radius.max(0.)
            })
            .chain(stars.iter().map(|s| {
                reach(s.semi_major_axis, s.eccentricity) + s.radius.max(0.)
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
            orbits.insert(star.id, star.parent_id, recorded_star(star));
        }
        for body in self.bodies() {
            orbits.insert(body.id, body.parent_id, recorded_body(body));
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
        body.semi_major_axis,
        body.eccentricity,
        body.orbital_inclination,
        body.periapsis,
        body.ascending_node,
        body.mean_anomaly,
        body.orbital_period,
    )
}

/// The orbit a star was recorded on
///
/// The same seven numbers under the same names. `stars` and `bodies` are two
/// tables holding one idea, and until they are one this says so twice.
fn recorded_star(star: &DbStar) -> Orbit {
    Orbit::recorded(
        star.semi_major_axis,
        star.eccentricity,
        star.orbital_inclination,
        star.periapsis,
        star.ascending_node,
        star.mean_anomaly,
        star.orbital_period,
    )
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

    /// A body `a` metres out on a circle, with no size of its own
    fn body(a: f32) -> DbBody {
        DbBody {
            system_address: 1,
            id: 1,
            parent_id: None,
            name: String::new(),
            updated_at: chrono::DateTime::UNIX_EPOCH,
            updated_by: String::new(),
            planet_class: String::new(),
            tidal_lock: false,
            landable: false,
            terraform_state: None,
            atmosphere: None,
            volcanism: None,
            mass: 0.,
            radius: 0.,
            surface_gravity: 0.,
            surface_temperature: 0.,
            surface: None,
            semi_major_axis: a,
            eccentricity: 0.,
            orbital_inclination: 0.,
            periapsis: 0.,
            orbital_period: 0.,
            rotation_period: 0.,
            axial_tilt: 0.,
            ascending_node: 0.,
            mean_anomaly: 0.,
            was_mapped: false,
            was_discovered: false,
        }
    }

    /// Contents that came back holding `bodies`
    fn known(bodies: Vec<DbBody>) -> Contents {
        Contents { of: Some(1), held: Held::Known { stars: vec![], bodies } }
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
        eccentric.eccentricity = 0.5;

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
        escaping.eccentricity = 1.;

        let extent = known(vec![escaping]).extent().unwrap();
        assert!(extent.is_finite(), "the extent ran away to {extent}");
        assert!(extent < 2e12, "the extent reached {extent}");
    }
}
