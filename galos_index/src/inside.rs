//! What a system is made of, and where each piece sits
//!
//! One system's rows — its stars, its bodies and the points a close pair goes
//! round — read as an arrangement: what goes round what, where each thing
//! stands, and how far the whole of it reaches.
//!
//! Held here rather than in whoever draws it because two programs ask. The map
//! reads it to place a system's insides on the way in; the builder reads it to
//! write the reach table the map sizes every system in the sky by. Those two
//! answers have to agree — a shell drawn smaller than the orbits inside it is
//! the one thing a reach cannot be — and the only way to be sure of that is
//! for both to be the same code, which is this.
//!
//! # Units
//!
//! Metres and seconds, as [`crate::orbit`] is. The journal records lengths in
//! metres and distances from arrival in light seconds, so the one conversion
//! stands at [`LIGHT_SECOND`] below.

use crate::meta::{Barycenter, Body, Parent, Star, SystemBodies};
use crate::orbit::{Orbit, Orbits, made_up_direction};
use elite_journal::body::Orbit as JournalOrbit;
use glam::DVec3;
use std::collections::{HashMap, HashSet};

/// Metres in a light second
///
/// Exact, the speed of light being defined. What a distance from arrival is
/// recorded in, and the one unit here that is not already metres.
pub const LIGHT_SECOND: f64 = 2.99792458e8;

impl SystemBodies {
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
        self.stars
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
    pub fn middle(&self, orbits: &Orbits, since: f64) -> DVec3 {
        self.primary().map_or(DVec3::ZERO, |id| orbits.place(id, since))
    }

    /// Every ancestry the system's rows carry, stars and bodies alike
    ///
    /// A chain is where the map learns that a barycentre is there, what kind
    /// of thing it is and what it goes round. A close pair of stars carries
    /// one exactly as a moon does, so both are read: a centre with nothing
    /// but stars riding it would otherwise go unheard of.
    fn ancestries(&self) -> impl Iterator<Item = &[Parent]> + '_ {
        self.stars
            .iter()
            .map(|star| star.parents.as_slice())
            .chain(self.bodies.iter().map(|body| body.parents.as_slice()))
    }

    /// What the thing with `id` goes round, as the rows under it say
    ///
    /// A barycenter is recorded with an orbit and nothing about what that
    /// orbit is around, `ScanBaryCentre` not naming any ancestor of its own.
    /// What does name one is everything beneath it, each of which carries the
    /// whole chain back to its star, so the link is read off there: find the
    /// barycenter in an ancestry and take whatever the scan put behind it.
    ///
    /// Nothing where it is the last name in every chain that mentions it,
    /// which is the barycentre at the root of a multi-star system: it goes
    /// round nothing, and that is a reading rather than a gap.
    fn goes_round(&self, id: i16) -> Option<i16> {
        self.ancestries().find_map(|ancestry| {
            let mut behind =
                ancestry.iter().skip_while(|parent| parent.id != id);
            behind.next()?;
            Some(behind.next()?.id)
        })
    }

    /// Every place the rows name, nearest the root first
    ///
    /// A chain names its parents by type as well as by id, so what a name
    /// stands for is known whether or not its own scan ever landed: `Null` is
    /// a barycentre, where nothing stands at all, and anything else is a
    /// thing nobody scanned. Both are held and both are marked the same, a
    /// place a chain names and nothing describes being the one thing they
    /// have in common; which kind it was is said in words by
    /// [`SystemBodies::guessed_under`].
    ///
    /// `Ring` is left out. The map carries no ring rows at all, so a ring is
    /// not a gap in what was scanned but a thing this index does not hold,
    /// and a mark at every one would say the wrong thing about all of them.
    ///
    /// Ordered by how many ancestors stand behind each, which is its depth
    /// read the other way up: a parent always has fewer than its child, so
    /// walking this order stands each of them up after whatever it goes
    /// round. Every chain through one carries the same tail, so the count is
    /// the same wherever it is read.
    pub fn named_places(&self) -> Vec<i16> {
        let mut behind: HashMap<i16, usize> = HashMap::new();
        for ancestry in self.ancestries() {
            for (step, parent) in ancestry.iter().enumerate() {
                if parent.ty.as_deref() == Some("Ring") {
                    continue;
                }
                behind.insert(parent.id, ancestry.len() - step - 1);
            }
        }

        let mut named: Vec<(usize, i16)> =
            behind.into_iter().map(|(id, deep)| (deep, id)).collect();
        named.sort_unstable();
        named.into_iter().map(|(_, id)| id).collect()
    }

    /// How far the barycentre `id` stands from the arrival star, in metres
    ///
    /// Read off the things riding it, which is the only thing on record about
    /// a centre nobody scanned. Each of them says how far it stands from
    /// arrival and how far its own orbit carries it from the centre, and the
    /// two together bracket the centre: it cannot be nearer than a rider's
    /// distance less its own reach, nor further than that distance plus it.
    /// Every rider narrows both ends, and the middle of what is left is the
    /// answer.
    ///
    /// Exact for a close pair, which is what this is really for: two things
    /// either side of a point they nearly touch put both ends of the bracket
    /// on it. Loosest for a centre with a whole system going round it at
    /// wildly different distances, where the widest orbit is what says the
    /// centre cannot be at the middle of the system after all.
    ///
    /// The mean over the riders was the first answer here and is worse: a
    /// centre with three planets round it came out at the mean of *their*
    /// distances, which is a figure about the planets rather than about the
    /// point, and put every one of them half a system too far out.
    ///
    /// Nothing where no rider says a distance, which leaves the centre with
    /// nothing to be placed by.
    fn stands_off(&self, id: i16) -> Option<f64> {
        let stars =
            self.stars.iter().filter(|star| star.parent_id() == Some(id)).map(
                |star| {
                    let carried = star.orbit.as_ref().map_or(0., |orbit| {
                        reach(orbit.semi_major_axis, orbit.eccentricity)
                    });
                    (Some(star.distance_from_arrival_ls), carried)
                },
            );
        let bodies = self
            .bodies
            .iter()
            .filter(|body| body.parent_id() == Some(id))
            .map(|body| {
                (
                    body.distance_from_arrival,
                    reach(body.orbit.semi_major_axis, body.orbit.eccentricity),
                )
            });

        let mut nearest = f64::NEG_INFINITY;
        let mut furthest = f64::INFINITY;
        for (away, carried) in stars.chain(bodies) {
            let Some(away) = away else { continue };
            let away = away as f64 * LIGHT_SECOND;
            nearest = nearest.max(away - carried as f64);
            furthest = furthest.min(away + carried as f64);
        }
        if !nearest.is_finite() || !furthest.is_finite() {
            return None;
        }

        // Nothing where the bracket does not rule out the middle. A rider
        // whose own orbit is wider than its distance from arrival says only
        // that what it goes round is somewhere within that orbit of the
        // arrival star, and standing it there is the answer with nothing made
        // up in it — which is exactly the arrival star's own case, where the
        // unscanned thing at the end of every chain *is* the middle.
        (nearest > 0.).then_some((nearest + furthest) / 2.)
    }

    /// A path standing the place `id` where its riders say it is, if they do
    ///
    /// How far out it is is a reading ([`SystemBodies::stands_off`]); which way
    /// nobody measured, so the bearing is taken from the system's address and
    /// its own id and comes out the same every time. The circle through that
    /// point is what carries it, and is drawn by nobody: its size is a
    /// reading and its shape is not.
    ///
    /// Nothing where the riders do not say, which leaves it standing on
    /// whatever it goes round. That is the one answer with nothing invented
    /// in it, and it is still better than the walk ending at the missing row
    /// and putting everything above it at the root of the system.
    fn stood_off(
        &self,
        orbits: &Orbits,
        address: i64,
        id: i16,
        parent: i16,
    ) -> Option<Orbit> {
        let away = self.stands_off(id)?;
        // The distance is from the arrival star, which is where the middle
        // stands; the orbit is about the parent, which is somewhere else.
        let stands =
            self.middle(orbits, 0.) + away * made_up_direction(address, id);

        Some(Orbit::circle_to(stands - orbits.place(parent, 0.)))
    }

    /// How far the tightest ring around `id` reaches, in metres
    ///
    /// What says whether a mark at `id` is worth drawing and how large it may
    /// be drawn: a mark is a few pixels of nothing, and one standing inside a
    /// ring only a few pixels across is a mark over the very thing it stands
    /// beside. The periapsis rather than the mean, that being how near the
    /// ring ever comes.
    ///
    /// Nothing where nothing on record goes round it at any distance. A place
    /// its riders sit on top of is a place they already say is there.
    pub fn ridden_at(&self, id: i16) -> Option<f32> {
        let near = |orbit: &JournalOrbit| {
            let close = orbit.semi_major_axis.max(0.)
                * (1. - orbit.eccentricity.clamp(0., 0.99));
            (close > 0.).then_some(close)
        };
        let stars = self
            .stars
            .iter()
            .filter(|star| star.parent_id() == Some(id))
            .filter_map(|star| star.orbit.as_ref().and_then(near));
        let bodies = self
            .bodies
            .iter()
            .filter(|body| body.parent_id() == Some(id))
            .filter_map(|body| near(&body.orbit));

        stars.chain(bodies).fold(None, |tightest: Option<f32>, ring| {
            Some(tightest.map_or(ring, |had| had.min(ring)))
        })
    }

    /// What kind of unscanned place `id`'s own is measured from, where that
    /// place is one the map made up
    ///
    /// Not only barycentres. A close pair's centre is the commonest one to
    /// arrive without a scan of its own, but a star or a body named in a
    /// chain with no row behind it is stood up the same way and is guessed at
    /// the same way — 55,072 chains on record name a star like that and
    /// 23,393 a body, against 32,909 naming a centre. So the word is read
    /// off the type the chain gave it rather than assumed.
    ///
    /// Nothing where nothing about where it stands was made up, which is the
    /// ordinary case.
    pub fn guessed_under(
        &self,
        orbits: &Orbits,
        id: i16,
    ) -> Option<&'static str> {
        let at = orbits.guessed_under(id)?;
        let mut named = self.ancestries().flat_map(|ancestry| ancestry.iter());
        let ty = named
            .find(|parent| parent.id == at)
            .and_then(|parent| parent.ty.as_deref().filter(|ty| *ty != "Null"));

        Some(match ty {
            None => "barycentre",
            Some("Star") => "star",
            // A `Planet` parent is any body with something going round it,
            // moons and rings alike, so it is said as the journal's own
            // wider word rather than as the narrower one it writes.
            Some("Planet") => "body",
            // A kind the journal has since grown. Said as what is true of all
            // of them rather than in a word the map cannot vouch for.
            Some(_) => "place",
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
    /// Nothing for a system with nothing on record, which to whoever is
    /// drawing is the same picture as one nobody has asked about yet: the map
    /// cannot say how far this system reaches.
    pub fn extent(&self, address: i64) -> Option<f32> {
        let (stars, bodies, centers) =
            (&self.stars, &self.bodies, &self.barycenters);
        let orbits = self.orbits(address);
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
    pub fn orbits(&self, address: i64) -> Orbits {
        let mut orbits = Orbits::default();
        for star in &self.stars {
            orbits.insert(star.id, star.parent_id(), recorded_star(star));
        }
        for body in &self.bodies {
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
            .ancestries()
            .flat_map(|ancestry| ancestry.iter().map(|parent| parent.id))
            .collect();
        for center in &self.barycenters {
            if !ridden.contains(&center.id) {
                continue;
            }
            orbits.insert(
                center.id,
                self.goes_round(center.id),
                recorded_center(center),
            );
        }

        // And every place the chains name that no scan ever landed for, which
        // is the common case rather than the odd one: a centre and the pair
        // riding it arrive as separate scans and the centre's own may never
        // come, and the same goes for the star a system's bodies hang from.
        // 212,021 systems on record name a root nobody scanned; some tens of
        // thousands name a close pair's centre or a star with nothing but
        // bodies under it.
        //
        // Held even where nothing can be said about where it stands, because
        // holding it is what keeps the walk running: a chain that steps over
        // a missing row loses every orbit above it and drops what rides it at
        // the root of the system. Nearest the root first, so each is stood up
        // after whatever it goes round is in.
        for id in self.named_places() {
            if orbits.holds(id) {
                continue;
            }
            // A place at the root goes round nothing, which is the whole of
            // its path rather than a gap in it: it is what the system's stars
            // go round and the middle everything is measured from.
            let stood = self.goes_round(id).and_then(|parent| {
                self.stood_off(&orbits, address, id, parent)
                    .map(|orbit| (parent, orbit))
            });
            match stood {
                // A direction was made up to place it, and everything above
                // it rests on that.
                Some((parent, orbit)) => orbits.guess(id, Some(parent), orbit),
                // Nothing was: it stands on whatever it goes round, or at the
                // middle where it goes round nothing.
                None => orbits.insert(id, self.goes_round(id), Orbit::still()),
            }
        }
        orbits
    }
}

/// The orbit a body was recorded on
///
/// TODO: A node and an anomaly nobody reported are read as zero, here and in
/// the two below, which draws the thing at periapsis. Worth answering properly
/// if that stops being rare: the path is known and the place along it is not,
/// a null `mean_anomaly` is how to tell the two apart, and the panel is where
/// it can be said in words rather than guessed at in space.
fn recorded_body(body: &Body) -> Orbit {
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
fn recorded_center(center: &Barycenter) -> Orbit {
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
fn recorded_star(star: &Star) -> Orbit {
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
/// A floor under [`SystemBodies::extent`] as much as a stand-in for it. Every
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
    use crate::orbit::Spacing;
    use elite_journal::body::Spin as JournalSpin;

    /// The system every fixture here is about
    ///
    /// Only ever read as the seed a made-up direction is taken from, so what
    /// it is does not matter and that it holds still does: the same rows read
    /// twice have to come out in the same place.
    const ADDRESS: i64 = 1;

    /// A body `a` metres out on a circle, with no size of its own
    fn body(a: f32) -> Body {
        Body {
            system_address: ADDRESS,
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
            mapped: false,
            discovered_at: None,
        }
    }

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
    fn star(id: i16, away: f32, a: f32, parents: Vec<Parent>) -> Star {
        Star {
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
            mapped: false,
            discovered_at: None,
        }
    }

    /// A barycenter with `id`, `a` metres out around whatever holds it
    fn center(id: i16, a: f32) -> Barycenter {
        Barycenter {
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
    fn binary(with_center: bool) -> SystemBodies {
        let mut close = body(1e9);
        close.id = 11;
        close.parents =
            vec![parent("Null", 10), parent("Star", 1), parent("Null", 0)];

        SystemBodies {
            stars: vec![
                star(1, 0., 1e13, vec![parent("Null", 0)]),
                star(2, 1e5, 2e13, vec![parent("Null", 0)]),
            ],
            bodies: vec![close],
            barycenters: if with_center {
                vec![center(10, 1e11)]
            } else {
                vec![]
            },
        }
    }

    /// Where the thing with `id` stands, in metres from the system's middle
    ///
    /// The sum `spawn::draw` writes into a transform: the walk up the chain,
    /// measured from the arrival star rather than from the point the system's
    /// stars go round.
    fn place(rows: &SystemBodies, id: i16, since: f64) -> DVec3 {
        let orbits = rows.orbits(ADDRESS);
        orbits.place(id, since) - rows.middle(&orbits, since)
    }

    /// The middle of a system is the star it arrives at
    ///
    /// Not the point its stars go round, which in a wide binary is ten billion
    /// kilometres of empty sky. A system's recorded position is where the map
    /// sends the camera and what it zooms towards, so what stands there has to
    /// be what the user came for.
    #[test]
    fn the_middle_of_a_system_is_the_star_it_arrives_at() {
        let rows = binary(true);

        assert_eq!(place(&rows, 1, 0.), DVec3::ZERO);
        // Every orbit here is a circle read at the same angle, so the two
        // stars lie the same way and stand their orbits apart. Both have moved
        // in by the arrival star's own orbit, which is the whole of this.
        let far = place(&rows, 2, 0.).length();
        assert!(
            (far - 1e13).abs() < 1e13 * 1e-6,
            "the far star stood {far}m off, not the 1e13 between them"
        );
    }

    /// The arrival star is the one recorded at no distance from arrival
    #[test]
    fn the_arrival_star_is_the_one_arrived_at() {
        assert_eq!(binary(true).primary(), Some(1));
        assert_eq!(SystemBodies::default().primary(), None);
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
        let out = place(&binary(true), 11, 0.).length();
        let wanted = 1e11 + 1e9;

        assert!(
            (out - wanted).abs() < wanted * 1e-6,
            "the body stood {out}m out, not {wanted}m"
        );
    }

    /// And hangs on its parent where nothing says how far out it is
    ///
    /// Which is the first thing holding it buys: the walk runs on to the star
    /// rather than ending at the missing row, so the pair is drawn beside the
    /// thing it belongs to instead of out in the empty space between the
    /// system's stars. Ross 248 is what showed it, four of its bodies
    /// gathered at the middle with their whole outer orbit dropped.
    #[test]
    fn a_missing_barycenter_falls_back_on_its_parent() {
        let out = place(&binary(false), 11, 0.).length();

        assert!(
            (out - 1e9).abs() < 1e9 * 1e-6,
            "the body stood {out}m from the star, not the 1e9 it goes round it"
        );
    }

    /// And is stood off by what rides it where something does
    ///
    /// The other thing holding it buys, and the reason it is worth guessing at
    /// all: how far out the centre is is on record — a scan says how far the
    /// thing it is about stands from arrival, and a pair sits either side of
    /// its centre — so the pair lands at that distance rather than on top of
    /// its star. Which way round it lies is nobody's reading, and the panel
    /// says as much.
    #[test]
    fn a_missing_barycenter_is_stood_off_by_what_rides_it() {
        let mut rows = binary(false);
        let away = 4e11;
        rows.bodies[0].distance_from_arrival =
            Some((away / LIGHT_SECOND) as f32);

        // Its own orbit either side of the point the map stood up, that being
        // the whole of what is known about where along its own ring it is.
        let out = place(&rows, 11, 0.).length();
        assert!(
            (out - away).abs() <= 1e9 * 1.001,
            "the body stood {out}m from the star, not the {away}m on record"
        );

        let orbits = rows.orbits(ADDRESS);
        assert!(
            orbits.guessed_under(11).is_some(),
            "the body's place was not owned as a guess"
        );
        assert!(
            orbits.guessed_under(1).is_none(),
            "the arrival star's own place was called a guess"
        );
    }

    /// And a guess is not only ever a barycentre
    ///
    /// A star or a body a chain names with no row behind it is stood up the
    /// same way and guessed at the same way, and what a panel says has to say
    /// which: 55,072 chains on record name a star like that and 23,393 a
    /// body, against 32,909 naming a centre. `Screakoo GH-V f2-0 AB 8 a` is
    /// one of them — a moon of a star nobody scanned, which the map drew
    /// 3,800 light seconds out of place until its star was stood up.
    #[test]
    fn an_unscanned_star_is_named_as_the_guess() {
        let mut moon = body(1e9);
        moon.id = 17;
        moon.parents = vec![parent("Star", 16), parent("Null", 0)];
        moon.distance_from_arrival = Some((4e11 / LIGHT_SECOND) as f32);

        let rows = SystemBodies {
            stars: vec![star(1, 0., 0., vec![])],
            bodies: vec![moon],
            barycenters: vec![],
        };
        let orbits = rows.orbits(ADDRESS);

        assert_eq!(rows.guessed_under(&orbits, 17), Some("star"));
        // The arrival star's own place is a reading, and stays one.
        assert_eq!(rows.guessed_under(&orbits, 1), None);
    }

    /// Every kind of thing in a system is offered a line
    ///
    /// The lines are drawn from what [`SystemBodies::orbits`] holds, so a kind
    /// missing here is a kind placed on the map with no orbit drawn for it.
    /// The barycenter is the one that costs something: nothing is drawn at
    /// one, but a close pair rides its ellipse, and without that the pair is
    /// two small rings around a point nothing leads to.
    ///
    /// The root of the system is held as well and offered nothing. It goes
    /// round nothing, so there is no ring to be drawn for it; what it is held
    /// for is the chain running whole through it and the mark drawn where it
    /// stands.
    #[test]
    fn every_kind_of_thing_is_offered_a_line() {
        let orbits = binary(true).orbits(ADDRESS);
        let mut held: Vec<_> = orbits.circling().map(|(id, _)| id).collect();
        held.sort();

        // Two stars, one body, the pair's centre, and the root they all go
        // round.
        assert_eq!(held, vec![0, 1, 2, 10, 11]);
        assert!(
            orbits.path(10, &Spacing::even(0., 512)).is_some(),
            "the barycenter was offered no path"
        );
        assert!(
            orbits.path(0, &Spacing::even(0., 512)).is_none(),
            "the root was offered a ring to go round nothing on"
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
        let rows = SystemBodies {
            stars: vec![lone],
            bodies: vec![],
            barycenters: vec![center(10, 4e12)],
        };

        assert!(
            !rows.orbits(ADDRESS).holds(10),
            "the center was kept with nothing riding it"
        );
        assert_eq!(rows.extent(ADDRESS), Some(STAND_IN));
    }

    /// A system whose outermost thing is a close pair on an eccentric orbit
    ///
    /// One star at the middle, a barycenter going round it, and one body
    /// riding the barycenter. A mean anomaly of nothing stands the pair at
    /// periapsis, so where it is today and how far its line reaches are as far
    /// apart as the orbit allows.
    fn eccentric_pair() -> SystemBodies {
        let mut close = body(1e8);
        close.id = 11;
        close.parents = vec![parent("Null", 10), parent("Star", 1)];

        let mut wide = center(10, 4e12);
        wide.orbit.as_mut().expect("a center with an orbit").eccentricity = 0.5;

        SystemBodies {
            stars: vec![star(1, 0., 0., vec![])],
            bodies: vec![close],
            barycenters: vec![wide],
        }
    }

    /// The extent reaches the far end of a barycenter's ellipse
    ///
    /// The pair riding it says only where it stands today. Measured from that
    /// alone the shell is drawn at the periapsis and the far half of the line
    /// hangs outside the system it belongs to.
    #[test]
    fn the_extent_takes_in_the_whole_of_a_barycenters_ellipse() {
        let reaches = eccentric_pair()
            .extent(ADDRESS)
            .expect("the pair reaches somewhere");

        assert!(
            (reaches - 6e12).abs() < 6e12 * 1e-6,
            "the system reached {reaches}m, not the 6e12 of the apoapsis"
        );
    }

    /// And no part of that ellipse falls outside it
    #[test]
    fn a_barycenters_line_stays_inside_the_extent() {
        let rows = eccentric_pair();
        let orbits = rows.orbits(ADDRESS);
        let middle = rows.middle(&orbits, 0.);
        let about = orbits.place(1, 0.) - middle;
        let reaches =
            rows.extent(ADDRESS).expect("the pair reaches somewhere") as f64;

        for point in orbits
            .path(10, &Spacing::even(0., 512))
            .expect("the center has a path")
        {
            let out = (about + point).length();
            assert!(
                out <= reaches,
                "the line reached {out}m out, past a {reaches}m extent"
            );
        }
    }

    /// Rows holding `bodies` and nothing else
    fn holding(bodies: Vec<Body>) -> SystemBodies {
        SystemBodies { bodies, ..Default::default() }
    }

    /// Nor has one that came back with nothing in it
    #[test]
    fn a_system_with_nothing_on_record_has_no_extent() {
        assert_eq!(holding(vec![]).extent(ADDRESS), None);
    }

    /// A missing centre is bracketed by its riders, not averaged over them
    ///
    /// Each rider says the centre is within its own reach of where it stands,
    /// and the tightest pair of those bounds is the answer. Averaging instead
    /// puts a centre with a whole system round it at the mean of *their*
    /// distances, which is a figure about the planets rather than about the
    /// point: here the mean is twice the answer, and everything under it
    /// would be drawn half a system too far out. Taken from a real one —
    /// `Screakoo GH-V f2-0 AB`, whose pair's centre the mean put 580 light
    /// seconds out of place.
    #[test]
    fn a_missing_centre_is_bracketed_by_what_rides_it() {
        let ls = |metres: f64| (metres / LIGHT_SECOND) as f32;
        let under = |id: i16, a: f32, away: f64| {
            let mut rider = body(a);
            rider.id = id;
            rider.parents = vec![parent("Null", 10), parent("Star", 1)];
            rider.distance_from_arrival = Some(ls(away));
            rider
        };

        let rows = SystemBodies {
            stars: vec![star(1, 0., 0., vec![])],
            // One close in, saying the centre is within a hundredth of 1e11;
            // one far out on an orbit nearly as wide as its own distance,
            // which says almost nothing.
            bodies: vec![under(11, 1e9, 1e11), under(12, 2.05e11, 3e11)],
            barycenters: vec![],
        };

        let stands = rows.stands_off(10).expect("the riders say where");
        assert!(
            (stands - 1e11).abs() < 1e11 * 1e-3,
            "the centre stood {stands}m out, not the 1e11 its riders bracket"
        );
    }

    /// The extent reaches the furthest body, not the last one read
    #[test]
    fn the_extent_reaches_the_outermost_body() {
        let rows = holding(vec![body(1e11), body(5e12), body(3e11)]);

        assert_eq!(rows.extent(ADDRESS), Some(5e12));
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

        assert_eq!(holding(vec![eccentric]).extent(ADDRESS), Some(3e12));
    }

    /// A body's own size counts, so the shell holds the whole of it
    #[test]
    fn the_extent_takes_in_the_body_standing_at_it() {
        let mut wide = body(5e12);
        wide.radius = 7e7;

        assert_eq!(holding(vec![wide]).extent(ADDRESS), Some(5e12 + 7e7));
    }

    /// A body sitting at the centre does not make an extent of nothing
    ///
    /// The primary star has no orbit and no distance from itself, so it comes
    /// back as a zero, and a system of nothing but that has nothing to say
    /// about how far it reaches.
    #[test]
    fn a_body_at_the_centre_leaves_the_extent_unsaid() {
        assert_eq!(holding(vec![body(0.)]).extent(ADDRESS), None);
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
            binary(true).extent(ADDRESS).expect("a binary reaches somewhere");

        assert!(
            (reaches - 3e13).abs() < 3e13 * 1e-6,
            "the binary reached {reaches}m, not the 3e13 out to the far side \
             of the outer star's orbit"
        );
    }

    /// Nothing drawn in a system stands outside the extent
    #[test]
    fn nothing_in_a_binary_stands_outside_its_extent() {
        let rows = binary(true);
        let reaches = rows.extent(ADDRESS).expect("a binary reaches somewhere");

        for id in [1, 2, 11] {
            let out = place(&rows, id, 0.).length();
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
        let rows = SystemBodies { stars: vec![lone], ..Default::default() };

        assert_eq!(rows.extent(ADDRESS), Some(STAND_IN));
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

        let extent = holding(vec![escaping]).extent(ADDRESS).unwrap();
        assert!(extent.is_finite(), "the extent ran away to {extent}");
        assert!(extent < 2e12, "the extent reached {extent}");
    }
}
