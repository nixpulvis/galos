//! Where a body is, given the ellipse it goes round on
//!
//! The database records what a scan saw: the shape of an orbit, its tilt, and
//! where the body stood along it at the moment of looking. Turning that into a
//! place is Kepler's problem, and this is the whole of the arithmetic — no
//! entities, no world, nothing to run.
//!
//! # Units
//!
//! Metres and radians, because that is what the map is drawn in. The journal
//! records lengths in metres already, so those come straight across, and
//! angles in degrees, so those are turned once at the door. Periods are
//! seconds.
//!
//! # Time
//!
//! Every position is asked for at some number of seconds after the epoch its
//! elements were recorded at. Nothing passes anything but zero yet, so the map
//! stands still, and a knob that lets it turn is a different number rather
//! than a different shape.
//!
//! # The frame
//!
//! Angles are measured against the map's own `y = 0` plane, with `+Y` for the
//! normal. What the journal measures them against is not written down
//! anywhere, so this is a convention rather than a reading: orbits will lie
//! plausibly and a system will not be turned the same way the game turns it.

use bevy::math::{DQuat, DVec3};
use std::collections::HashMap;

/// How many turns of Newton's method to give Kepler's equation
///
/// It roughly squares its own accuracy each turn, so a circle is exact at once
/// and anything the database holds is past a double's precision well before
/// this. The cap is not there for accuracy but so that an orbit the solver
/// cannot settle on — one recorded as very nearly a parabola — costs a fixed
/// amount rather than spinning.
const TURNS: usize = 12;

/// How near an answer has to be before the solving stops, in radians
///
/// A ten-billionth of a turn, which at the widest orbit on record is under a
/// millimetre and everywhere else is far less.
const SETTLED: f64 = 1e-10;

/// The path one body takes about whatever it goes round
///
/// Held in the units the map is drawn in rather than the ones the journal
/// records, so that nothing downstream has to remember which is which.
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    /// Half the long way across the ellipse, in metres
    pub semi_major_axis: f64,
    /// How far from a circle it is, nothing being a circle
    pub eccentricity: f64,
    /// How far the plane is tipped from the map's own
    pub inclination: f64,
    /// Where the near point of the orbit sits, measured within the plane
    pub periapsis: f64,
    /// Where the plane crosses the map's own, measured within that
    pub ascending_node: f64,
    /// Where the body stood when it was looked at
    pub mean_anomaly: f64,
    /// How long it takes to come round, in seconds
    ///
    /// Nothing for a body that does not go round anything, which is what a
    /// system's primary star comes back as.
    pub period: Option<f64>,
}

impl Orbit {
    /// The orbit of something that goes round nothing
    ///
    /// No size and no period, so it stands at whatever it is measured from.
    /// A system's primary star is recorded without an orbit and comes back as
    /// this, which puts it at the middle of its system.
    pub fn still() -> Orbit {
        Orbit {
            semi_major_axis: 0.,
            eccentricity: 0.,
            inclination: 0.,
            periapsis: 0.,
            ascending_node: 0.,
            mean_anomaly: 0.,
            period: None,
        }
    }

    /// The elements as the journal records them: metres, degrees, seconds
    ///
    /// The one place a degree is spoken. An orbit with no size and one with no
    /// period are both things that do not go round, and are kept as such
    /// rather than being turned into a circle of nothing.
    pub fn recorded(
        semi_major_axis: f32,
        eccentricity: f32,
        orbital_inclination: f32,
        periapsis: f32,
        ascending_node: f32,
        mean_anomaly: f32,
        orbital_period: f32,
    ) -> Orbit {
        Orbit {
            semi_major_axis: semi_major_axis.max(0.) as f64,
            // Short of one, since what is recorded is a scan rather than a
            // solution and a parabola read literally never comes back.
            eccentricity: eccentricity.clamp(0., 0.99) as f64,
            inclination: (orbital_inclination as f64).to_radians(),
            periapsis: (periapsis as f64).to_radians(),
            ascending_node: (ascending_node as f64).to_radians(),
            mean_anomaly: (mean_anomaly as f64).to_radians(),
            period: (orbital_period.is_finite() && orbital_period > 0.)
                .then_some(orbital_period as f64),
        }
    }

    /// Where the body is, `since` seconds after the epoch it was recorded at
    ///
    /// Relative to whatever it goes round, which is the parent's problem
    /// rather than this one's.
    pub fn at(&self, since: f64) -> DVec3 {
        if self.semi_major_axis <= 0. {
            return DVec3::ZERO;
        }

        self.place(self.anomaly(since))
    }

    /// How far round the ellipse the body stands, `since` seconds after the
    /// epoch it was recorded at
    ///
    /// The eccentric anomaly, which is the angle [`Orbit::place`] is written
    /// in and the one [`Orbit::path`] is stepped through.
    pub fn anomaly(&self, since: f64) -> f64 {
        let mean = match self.period {
            Some(period) => {
                self.mean_anomaly + std::f64::consts::TAU * since / period
            }
            // Nothing to turn it, so it stands where it was seen.
            None => self.mean_anomaly,
        };

        eccentric_anomaly(mean, self.eccentricity)
    }

    /// The whole ring, as the points `spacing` lays round it
    ///
    /// Stepped through the eccentric anomaly rather than the mean one, which
    /// spreads the points evenly around the ellipse rather than crowding them
    /// where the body dawdles.
    ///
    /// The first and last are the same place, half a turn either way from
    /// where the points are laid closest, so the run closes on itself whatever
    /// the spacing.
    ///
    /// What is drawn is a ring of chords inscribed in the ellipse, and every
    /// chord cuts inside the curve. That is why the middle of the run matters:
    /// at the scale of the point Pluto and Charon go round, five hundred and
    /// twelve even chords sag a hundred and ten thousand kilometres, which is
    /// fifty-four times the radius of Pluto's own orbit about it. A chord
    /// leaves the curve as the square of its length, so the run is laid about
    /// whatever the line has to pass through — the thing riding it, or the
    /// camera — and the sag there falls to nothing.
    pub fn path(&self, spacing: &Spacing) -> Vec<DVec3> {
        if self.semi_major_axis <= 0. {
            return Vec::new();
        }

        spacing.anomalies().map(|anomaly| self.place(anomaly)).collect()
    }

    /// How far apart to lay a ring's points where the camera stands, in radians
    ///
    /// [`CROSSING`] dashes fall across a view, and a dash and the gap after it
    /// are a step each, so a view holds twice that many steps. `across` is how
    /// much sky the camera takes in, and a radian of a ring is about its
    /// semi-major axis of arc, which is what turns the one into the other.
    ///
    /// A ring smaller than the view asks for a step wider than the ring, and
    /// gets one: [`Spacing::round`] lays no ring coarser than evenly.
    pub fn finest(&self, across: f64) -> f64 {
        if self.semi_major_axis <= 0. {
            return std::f64::consts::PI;
        }

        across / (2. * CROSSING as f64 * self.semi_major_axis)
    }

    /// How far round the ring the point nearest `to` sits
    ///
    /// Which is where a piece of the ring is laid out about, `to` being where
    /// the camera stands. Read off the ellipse rather than guessed from the
    /// angle: the two part company as the orbit stretches, and the piece drawn
    /// is a few views wide where the ring is a hundred thousand of them round,
    /// so being out by a degree is being out by everything.
    ///
    /// Swept coarsely and then closed in on, the distance to a point falling
    /// away smoothly on either side of the nearest.
    pub fn nearest(&self, to: DVec3) -> f64 {
        let sweep = 64;
        let step = std::f64::consts::TAU / sweep as f64;
        let away = |turn: f64| self.place(turn).distance(to);

        let coarse = (0..sweep)
            .map(|k| k as f64 * step)
            .min_by(|one, other| away(*one).total_cmp(&away(*other)))
            .unwrap_or(0.);

        // Closed in on from either side of it, which halves what is left over
        // each time round and is under a metre of the ring by the end.
        let (mut low, mut high) = (coarse - step, coarse + step);
        for _ in 0..64 {
            let third = (high - low) / 3.;
            if away(low + third) < away(high - third) {
                high -= third;
            } else {
                low += third;
            }
        }

        (low + high) / 2.
    }

    /// Where the body stands at an eccentric anomaly of `anomaly`
    ///
    /// The ellipse is laid out with its near point along the plane's own `x`,
    /// then turned three times: by the argument of periapsis within the
    /// plane, by the inclination about the line where the planes cross, and
    /// by the ascending node about the map's normal.
    fn place(&self, anomaly: f64) -> DVec3 {
        let (a, e) = (self.semi_major_axis, self.eccentricity);
        // In the plane of the orbit, from the focus the body actually goes
        // round rather than from the middle of the ellipse.
        let flat = DVec3::new(
            a * (anomaly.cos() - e),
            a * (1. - e * e).max(0.).sqrt() * anomaly.sin(),
            0.,
        );

        let turned = DQuat::from_rotation_z(self.ascending_node)
            * DQuat::from_rotation_x(self.inclination)
            * DQuat::from_rotation_z(self.periapsis);
        let placed = turned * flat;

        // The arithmetic above is written the way the textbooks are, with the
        // reference plane as `x`–`y` and the normal along `z`. The map stands
        // the other way up, so the last two are swapped on the way out.
        DVec3::new(placed.x, placed.z, placed.y)
    }
}

/// How the points of a ring are laid round it
///
/// A ring is a closed line and is drawn whole however close the camera stands,
/// so the points cannot simply be spread evenly. The ring Pluto and Charon go
/// round is three hundred thousand views round by the time the camera is beside
/// the pair, and spread evenly there each step of it is hundreds of views long:
/// what is drawn is one dash across the sky, or the gap after it and nothing at
/// all. Laid closest where the camera stands and opening out from there, the
/// same count of points reads as dashes beside it and still closes behind it.
///
/// Each step is a fixed share wider than the one before, so nowhere along the
/// run does the spacing jump. The share falls out of closing the ring in the
/// points there are, so the wider a ring is against the view the faster it
/// opens.
#[derive(Clone, Copy, Debug)]
pub struct Spacing {
    /// Where round the ring the points are laid closest together
    pub at: f64,
    /// How far apart they are there, in radians
    pub finest: f64,
    /// How much wider a step grows per radian round from `at`
    ///
    /// Nothing for a run laid evenly, which is what a ring the view already
    /// holds is given.
    pub flare: f64,
    /// How many steps to either side, the run closing at half a turn
    pub steps: usize,
    /// How many steps make a dash, where the ring is drawn in them
    ///
    /// One wherever the points are laid to the view, a step being a dash and
    /// the next its gap. More where a ring the view holds is laid evenly with
    /// more points than its dashes want. See [`DASHES`].
    pub run: usize,
}

/// How many dashes of a ring fall across the view beside the camera
///
/// A dash and the gap after it are a step each, so a view holds twice this many
/// steps. Eight reads as a dashed line rather than as a chain of ticks, and is
/// about what [`DASHES`] comes to on a ring one view across, so nothing steps as
/// a ring grows past the view.
const CROSSING: usize = 8;

/// How few dashes a ring is ever drawn in
///
/// [`CROSSING`] alone would cut a ring smaller than the view into two or three
/// dashes, which reads as a pair of arcs rather than as a ring. A ring the view
/// holds whole is drawn in this many however small it is, and only past there
/// does the view take over the count.
///
/// A count around the ring rather than a length in the world, which is the other
/// way round from a route's [`crate::systems::route::DASH`]. A route is a run of
/// legs of wildly different lengths and a share of one cannot be read at more
/// than one zoom, so its dashes are held at a distance instead.
const DASHES: usize = 32;

impl Spacing {
    /// Points laid evenly round the ring, `points` of them
    ///
    /// What a ring no wider than the view is drawn with, and what a line with
    /// something standing on it is drawn with at any width: a run laid to the
    /// view is laid about the camera, and such a line has to pass through its
    /// own body instead.
    ///
    /// Cut into as few as [`DASHES`] dashes, which is what a ring smaller than
    /// the view wants.
    pub fn even(at: f64, points: usize) -> Spacing {
        let steps = (points / 2).max(1);
        Spacing {
            at,
            finest: std::f64::consts::PI / steps as f64,
            flare: 0.,
            steps,
            run: (steps / DASHES).max(1),
        }
    }

    /// Points laid `finest` apart at `at`, opening out to close the ring
    ///
    /// `points` is the whole budget for the ring, half of it laid either side.
    /// Where laying them evenly is already closer than `finest` asks, they are
    /// laid evenly and the dashes cut from more than one step apiece.
    pub fn round(at: f64, finest: f64, points: usize) -> Spacing {
        let even = Spacing::even(at, points);
        if !finest.is_finite() || finest <= 0. {
            return even;
        }

        if finest < even.finest {
            let flare = flare(finest, even.steps);
            return Spacing { finest, flare, run: 1, ..even };
        }

        let run = (finest / even.finest).round() as usize;
        Spacing { run: run.min(even.run), ..even }
    }

    /// How wide the step is `from` radians round from where they are closest
    ///
    /// Each step is `finest` plus what it has opened out by, and it opens by a
    /// flat share of how far round it is, which is what a run growing by a fixed
    /// share of itself comes to.
    pub fn step(&self, from: f64) -> f64 {
        self.finest + self.flare * from.abs()
    }

    /// Every anomaly the ring is drawn at, from half a turn behind `at` to half
    /// a turn ahead of it
    pub fn anomalies(&self) -> impl Iterator<Item = f64> + '_ {
        let steps = self.steps as isize;
        (-steps..=steps).map(move |step| self.at + self.along(step))
    }

    /// How far round from `at` the `step`th point lands, signed
    fn along(&self, step: isize) -> f64 {
        let (side, step) = (step.signum() as f64, step.unsigned_abs() as f64);
        if self.flare <= 0. {
            return side * step * self.finest;
        }

        side * self.finest / self.flare * ((step * self.flare).exp() - 1.)
    }
}

/// How fast a run of `steps` laid `finest` apart has to open to close the ring
///
/// Each step being a fixed share wider than the one before, `steps` of them
/// reach `finest/f · (e^(steps·f) - 1)`, and this is the `f` that puts the last
/// of them half a turn round. That reach grows with `f`, so it is closed in on
/// by halving.
///
/// Halved rather than solved outright: the equation has no closed form, and a
/// few dozen of them settle it to the last bit a double holds. It is asked once
/// a ring, and only where the camera has moved far enough to be worth laying one
/// out again.
fn flare(finest: f64, steps: usize) -> f64 {
    let reach = |f: f64| finest / f * ((steps as f64 * f).exp() - 1.);
    // Wide enough for any ring: opening this fast reaches half a turn from a
    // step far smaller than any view asks for.
    let (mut low, mut high) = (0., 600. / steps as f64);

    for _ in 0..64 {
        let middle = (low + high) / 2.;
        if reach(middle) < std::f64::consts::PI {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) / 2.
}

/// Solve `E - e·sin E = M` for `E`
///
/// Newton's method, from a guess that already leans the right way. A circle is
/// answered without turning at all, since there the two anomalies are the same
/// angle.
fn eccentric_anomaly(mean: f64, eccentricity: f64) -> f64 {
    if eccentricity <= 0. {
        return mean;
    }

    let mut anomaly = mean + eccentricity * mean.sin();
    for _ in 0..TURNS {
        let missed = anomaly - eccentricity * anomaly.sin() - mean;
        if missed.abs() < SETTLED {
            break;
        }
        // Never zero: the slope is `1 - e·cos E`, and `e` is held short of
        // one, so it stays above that margin however the cosine falls.
        anomaly -= missed / (1. - eccentricity * anomaly.cos());
    }
    anomaly
}

/// The orbits of everything in one system, by the id its children name it with
///
/// Built from what came back about a system, so that a moon can be placed
/// relative to its planet and its planet relative to the star, without any of
/// them having to know how deep they sit.
#[derive(Default)]
pub struct Orbits(HashMap<i16, (Option<i16>, Orbit)>);

/// How far up a chain of parents to walk before giving up
///
/// A moon of a moon of a planet of a star is four, and the records go no
/// deeper. The cap is there for a chain that points back at itself, which
/// nothing forbids: `parent_id` has no foreign key behind it.
const ANCESTRY: usize = 16;

impl Orbits {
    /// Take in one thing and what it goes round
    pub fn insert(&mut self, id: i16, parent: Option<i16>, orbit: Orbit) {
        self.0.insert(id, (parent, orbit));
    }

    /// What `id` goes round, if it is held and goes round anything
    pub fn parent(&self, id: i16) -> Option<i16> {
        self.0.get(&id).and_then(|(parent, _)| *parent)
    }

    /// Whether `id` is held at all
    ///
    /// Apart from [`Self::parent`], which answers the same for something held
    /// that goes round nothing as for something not held at all.
    pub fn holds(&self, id: i16) -> bool {
        self.0.contains_key(&id)
    }

    /// Everything held, with what each of them goes round
    ///
    /// What the lines are drawn from. Stars, bodies and barycenters all arrive
    /// through [`Self::insert`], so a loop over this draws a line for each of
    /// the three without having to be told there are three.
    pub fn circling(&self) -> impl Iterator<Item = (i16, Option<i16>)> + '_ {
        self.0.iter().map(|(id, (parent, _))| (*id, *parent))
    }

    /// The ring `id` traces about whatever it goes round, as `spacing` lays it
    ///
    /// Relative to its parent rather than to the system, so a moon's line is
    /// the small circle it makes about its planet and belongs wherever the
    /// planet is. Nothing for something that does not go round anything, which
    /// is what a system's primary comes back as.
    pub fn path(&self, id: i16, spacing: &Spacing) -> Option<Vec<DVec3>> {
        let (_, orbit) = self.0.get(&id)?;
        let path = orbit.path(spacing);
        (!path.is_empty()).then_some(path)
    }

    /// How far round `id`'s ring the point nearest `to` sits
    pub fn nearest(&self, id: i16, to: DVec3) -> f64 {
        self.0.get(&id).map_or(0., |(_, orbit)| orbit.nearest(to))
    }

    /// How far round `id`'s ring it stands, `since` seconds after the epoch
    pub fn anomaly(&self, id: i16, since: f64) -> f64 {
        self.0.get(&id).map_or(0., |(_, orbit)| orbit.anomaly(since))
    }

    /// How far apart to lay `id`'s points where the camera stands, in radians
    pub fn finest(&self, id: i16, across: f64) -> f64 {
        self.0
            .get(&id)
            .map_or(std::f64::consts::PI, |(_, orbit)| orbit.finest(across))
    }

    /// Where `id` sits within its system, in metres from where the walk ends
    ///
    /// Each step of the way up adds where that thing stands about its own
    /// parent, so a moon lands beside its planet rather than beside the star.
    ///
    /// Where the walk ends is the point the system's stars go round, which is
    /// the arrival star itself only where there is one of them.
    /// [`super::Contents::place`] measures from the arrival star either way,
    /// that being where the map puts the middle of a system.
    ///
    /// A parent that is not on record ends the walk, and what is left is
    /// measured from the system's centre. An honest shortcut: the body is put
    /// somewhere plausible rather than nowhere, and it is the missing row
    /// rather than this that makes it approximate.
    ///
    /// # Barycentres
    ///
    /// A barycentre is nobody's row in `bodies`, so a chain that names one
    /// steps somewhere else for it: [`super::Contents::orbits`] puts them in
    /// alongside the stars and the bodies, and the chain runs whole.
    ///
    /// One of them stays missing on purpose. The barycentre at the root of a
    /// multi-star system goes round nothing and is the middle of the system,
    /// so ending the walk there and measuring from the centre lands exactly
    /// where following it would have.
    ///
    /// This was read the other way round for a while, with none of them held:
    /// the root came out right by accident and every close pair was drawn at
    /// the middle of its system with its whole outer orbit dropped. Ross 248
    /// is what showed it, its stars ten billion kilometres out on either side
    /// and four of its bodies gathered at the centre between them.
    pub fn place(&self, id: i16, since: f64) -> DVec3 {
        let mut place = DVec3::ZERO;
        let mut at = Some(id);

        for _ in 0..ANCESTRY {
            let Some(this) = at else { break };
            let Some((parent, orbit)) = self.0.get(&this) else { break };
            place += orbit.at(since);
            at = *parent;
        }
        place
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    /// A circle of radius `a`, lying in the map's own plane
    fn circle(a: f64) -> Orbit {
        Orbit {
            semi_major_axis: a,
            eccentricity: 0.,
            inclination: 0.,
            periapsis: 0.,
            ascending_node: 0.,
            mean_anomaly: 0.,
            period: Some(1000.),
        }
    }

    /// A circle is solved without iterating at all
    ///
    /// The two anomalies are the same angle where there is no eccentricity,
    /// and the solver should say so rather than converging on it.
    #[test]
    fn a_circle_is_solved_without_iterating() {
        for mean in [0., 0.3, PI, 5.9] {
            assert_eq!(eccentric_anomaly(mean, 0.), mean);
        }
    }

    /// The solver gives back whatever anomaly the equation was built from
    ///
    /// Kepler's equation is easy to walk forwards and awkward to walk back, so
    /// this goes forwards and checks the way back lands where it started.
    #[test]
    fn kepler_returns_the_anomaly_it_was_given() {
        for e in [0.01, 0.2, 0.5, 0.9, 0.99] {
            for step in 0..24 {
                let wanted = TAU * step as f64 / 24.;
                let mean = wanted - e * wanted.sin();
                let got = eccentric_anomaly(mean, e);
                assert!(
                    (got - wanted).abs() < 1e-8,
                    "at e={e} the solver answered {got}, not {wanted}"
                );
            }
        }
    }

    /// A body comes back to where it started after one whole period
    #[test]
    fn a_body_comes_back_to_where_it_started_after_one_period() {
        let mut orbit = circle(1e11);
        orbit.eccentricity = 0.4;
        orbit.inclination = 0.6;
        orbit.periapsis = 1.2;

        let start = orbit.at(0.);
        let round = orbit.at(1000.);

        assert!(
            start.distance(round) < 1e11 * 1e-9,
            "a year later it stood {} metres away",
            start.distance(round)
        );
    }

    /// And is somewhere else halfway through one
    ///
    /// Otherwise the test above would pass for a body that never moved.
    #[test]
    fn a_body_is_elsewhere_halfway_round() {
        let orbit = circle(1e11);

        assert!(orbit.at(0.).distance(orbit.at(500.)) > 1e11);
    }

    /// A body with no period stands where it was seen, whenever it is asked
    ///
    /// Which is what a system's primary comes back as, and what the map draws
    /// today for everything, since nothing yet passes a time but zero.
    #[test]
    fn an_orbit_with_no_period_stands_still() {
        let mut orbit = circle(1e11);
        orbit.period = None;

        assert_eq!(orbit.at(0.), orbit.at(1e9));
    }

    /// Degrees at the door become radians inside
    #[test]
    fn degrees_become_radians() {
        let orbit = Orbit::recorded(1e11, 0., 90., 180., 270., 45., 1000.);

        assert!((orbit.inclination - FRAC_PI_2).abs() < 1e-12);
        assert!((orbit.periapsis - PI).abs() < 1e-12);
        assert!((orbit.ascending_node - 3. * FRAC_PI_2).abs() < 1e-12);
        assert!((orbit.mean_anomaly - PI / 4.).abs() < 1e-12);
    }

    /// A body that does not go round anything is kept as one
    ///
    /// A period of nothing is a primary star, not a body that comes round
    /// instantly.
    #[test]
    fn no_period_recorded_is_no_period() {
        assert!(Orbit::recorded(1e11, 0., 0., 0., 0., 0., 0.).period.is_none());
    }

    /// An orbit with no tilt lies in the map's own plane
    #[test]
    fn a_flat_orbit_stays_in_the_plane() {
        let orbit = circle(1e11);

        for step in 0..12 {
            let place = orbit.at(1000. * step as f64 / 12.);
            assert!(
                place.y.abs() < 1e11 * 1e-12,
                "it strayed {} metres out of the plane",
                place.y
            );
        }
    }

    /// A quarter turn of tilt stands the orbit on its edge
    ///
    /// Pins which way up the map is, and that the swap on the way out of
    /// [`Orbit::place`] is the right way round. Wrong, an inclined orbit would
    /// stay flat and a flat one would stand up.
    #[test]
    fn a_tilted_orbit_leaves_the_plane() {
        let mut orbit = circle(1e11);
        orbit.inclination = FRAC_PI_2;

        let highest = (0..48)
            .map(|step| orbit.at(1000. * step as f64 / 48.).y.abs())
            .fold(0., f64::max);

        assert!(
            (highest - 1e11).abs() < 1e11 * 1e-6,
            "on edge it reached {highest} out of the plane, not the full 1e11"
        );
    }

    /// The near and far points of an ellipse are where they should be
    ///
    /// `a(1-e)` and `a(1+e)` from the focus. The body goes round the focus,
    /// not the middle of the ellipse, and placing it from the middle would put
    /// both ends at `a`.
    #[test]
    fn an_ellipse_is_measured_from_the_focus() {
        let mut orbit = circle(1e11);
        orbit.eccentricity = 0.5;
        orbit.period = Some(1000.);

        let far = (0..400)
            .map(|s| orbit.at(1000. * s as f64 / 400.).length())
            .fold(0f64, f64::max);
        let near = (0..400)
            .map(|s| orbit.at(1000. * s as f64 / 400.).length())
            .fold(f64::MAX, f64::min);

        assert!((far - 1.5e11).abs() < 1e9, "the far point was {far}");
        assert!((near - 0.5e11).abs() < 1e9, "the near point was {near}");
    }

    /// A path traces the whole ellipse and closes on itself
    #[test]
    fn a_path_goes_all_the_way_round() {
        let mut orbit = circle(1e11);
        orbit.eccentricity = 0.3;

        let path = orbit.path(&Spacing::even(0., 64));

        assert_eq!(path.len(), 65, "a budget of 64 points wants 65 of them");
        assert!(
            path[0].distance(path[64]) < 1.,
            "the path did not close on itself"
        );
    }

    /// A line passes through the thing riding it
    ///
    /// What is drawn is a ring of chords inscribed in the ellipse, and every
    /// chord cuts inside the curve. Started at periapsis the body lands
    /// between two points and floats off its own line: at the scale of the
    /// point Pluto and Charon go round, a hundred and ten thousand kilometres
    /// off it, against the two thousand of Pluto's own orbit.
    #[test]
    fn a_line_passes_through_what_rides_it() {
        let mut orbit = circle(1e11);
        orbit.eccentricity = 0.3;
        orbit.mean_anomaly = 1.1;

        // Whenever it is asked, and wherever the body has got to by then.
        for since in [0., 137., 600., 1e4] {
            let at = orbit.at(since);
            let nearest = orbit
                .path(&Spacing::even(orbit.anomaly(since), 64))
                .into_iter()
                .map(|point| point.distance(at))
                .fold(f64::MAX, f64::min);

            assert!(
                nearest < 1.,
                "{since} on left the body {nearest}m off its own line"
            );
        }
    }

    /// A ring drawn for a camera standing beside it at `across` metres of sky
    ///
    /// The point Pluto and Charon go round, as the database has it, which is
    /// the widest ring the map draws anything on.
    fn seen(across: f64) -> (Orbit, Spacing, Vec<DVec3>) {
        let mut orbit = circle(5.874e12);
        orbit.eccentricity = 0.248;
        let spacing = Spacing::round(0., orbit.finest(across), 512);

        let path = orbit.path(&spacing);
        (orbit, spacing, path)
    }

    /// A ring reads as dashes at every zoom
    ///
    /// The whole of what laying the points closest beside the camera is for.
    /// Spread evenly, a ring this size has steps seven hundred views long by
    /// the time the camera is beside the pair: what is drawn there is one dash
    /// across the sky, or the gap after it and nothing at all.
    ///
    /// Counted as the points that fall across the view, every other pair of
    /// which is a dash. A lower bound and no upper one: seen whole the ring
    /// carries the lot, and what goes wrong is a view with one dash across it
    /// or none.
    #[test]
    fn a_ring_reads_as_dashes_at_every_zoom() {
        // From the ring seen whole down to a view showing the pair alone.
        for across in [1e13, 1e11, 1e9, 1e8, 1e7, 1e6] {
            let (orbit, spacing, path) = seen(across);
            let at = orbit.place(0.);
            let near = path
                .iter()
                .filter(|point| point.distance(at) < across / 2.)
                .count();
            let dashes = near / (2 * spacing.run);

            assert!(
                (CROSSING / 2..=2 * CROSSING).contains(&dashes),
                "{across}m across drew {dashes} dashes across the view"
            );
        }
    }

    /// And is drawn all the way round however close the camera stands
    ///
    /// A ring is a closed line, and the only thing that ends it is coming back
    /// to where it started. Drawn as the piece of it that crosses the view, it
    /// stops wherever the piece runs out, and the gap is in plain sight
    /// whenever the ring is anywhere near the size of the view.
    #[test]
    fn a_ring_is_drawn_whole_at_every_zoom() {
        for across in [1e13, 1e11, 1e9, 1e7, 1e6] {
            let (_, _, path) = seen(across);
            let ends = path[0].distance(path[path.len() - 1]);

            assert!(
                ends < 5.874e12 * 1e-6,
                "{across}m across left the ring open by {ends}m"
            );
        }
    }

    /// And opens out smoothly from where the camera stands to the far side
    ///
    /// The points are laid closest beside the camera and further apart the
    /// further round they are, and how much further has to be a share of what
    /// they already were. Opened out any faster the run reads as a ring of
    /// dashes with a few long chords let into it.
    #[test]
    fn a_rings_points_open_out_smoothly() {
        for across in [1e13, 1e9, 1e6, 1e3] {
            let (_, _, path) = seen(across);
            let steps: Vec<f64> =
                path.windows(2).map(|two| two[0].distance(two[1])).collect();
            let opened = steps
                .windows(2)
                .map(|two| two[1] / two[0])
                .fold(0f64, f64::max);

            assert!(
                opened < 1.25,
                "{across}m across left one step {opened} times the last"
            );
        }
    }

    /// A ring the view holds whole is laid evenly, in a fixed count of dashes
    ///
    /// Laid to the view alone a small ring would be cut into two or three
    /// dashes and read as a pair of arcs rather than as a ring.
    #[test]
    fn a_ring_the_view_holds_is_laid_evenly() {
        let orbit = circle(1e11);
        // A view ten times the width of the ring.
        let spacing = Spacing::round(0., orbit.finest(2e12), 512);

        assert_eq!(spacing.flare, 0., "a ring the view holds was crowded");
        assert_eq!(2 * spacing.steps / (2 * spacing.run), DASHES);
    }

    /// The piece of a ring laid out is the piece the camera stands beside
    ///
    /// Read off the ellipse rather than guessed from the angle round it. The
    /// two part company as an orbit stretches, and the piece drawn is a few
    /// views wide where the ring is a hundred thousand of them round, so being
    /// out by a degree is being out by everything.
    #[test]
    fn the_piece_of_a_ring_beside_the_camera_is_found() {
        let mut orbit = circle(5.874e12);
        orbit.eccentricity = 0.248;

        for turn in [0., 1., 2.5, 4., 6.] {
            let on = orbit.place(turn);
            // A camera standing a little off the ring there.
            let found = orbit.nearest(on * 1.0000001);
            let missed = orbit.place(found).distance(on);

            assert!(
                missed < 1e7,
                "a camera beside {turn} round found {found}, {missed}m off"
            );
        }
    }

    /// A body sitting at the centre has no path to draw
    #[test]
    fn a_body_at_the_centre_has_no_path() {
        assert!(circle(0.).path(&Spacing::even(0., 64)).is_empty());
    }

    /// A moon is placed beside the planet it goes round
    ///
    /// The whole point of walking the parents: a moon a hundred thousand
    /// kilometres from its planet, and the planet a hundred million from its
    /// star, belongs out by the planet rather than that far from the centre.
    #[test]
    fn a_moon_is_placed_relative_to_the_planet_it_goes_round() {
        let mut orbits = Orbits::default();
        orbits.insert(1, None, circle(1.5e11));
        orbits.insert(2, Some(1), circle(3.8e8));

        let planet = orbits.place(1, 0.);
        let moon = orbits.place(2, 0.);

        assert!(
            (planet.distance(moon) - 3.8e8).abs() < 1e3,
            "the moon stood {} from its planet",
            planet.distance(moon)
        );
        assert!(
            moon.length() > 1.4e11,
            "the moon landed {} from the centre, nowhere near its planet",
            moon.length()
        );
    }

    /// A body whose parent is not on record goes round the system instead
    ///
    /// `parent_id` may name a barycentre the database does not keep at all, or
    /// a star the map did not read. Neither is a reason to put the body
    /// nowhere.
    #[test]
    fn a_body_whose_parent_is_not_on_record_orbits_the_system() {
        let mut orbits = Orbits::default();
        orbits.insert(2, Some(99), circle(3.8e8));

        assert!((orbits.place(2, 0.).length() - 3.8e8).abs() < 1.);
    }

    /// Nothing is somewhere, rather than a crash
    #[test]
    fn something_not_on_record_at_all_sits_at_the_centre() {
        assert_eq!(Orbits::default().place(7, 0.), DVec3::ZERO);
    }

    /// Everything held comes back with what it goes round
    ///
    /// The lines are drawn from this, so a thing missing here is a thing
    /// placed on the map with no orbit drawn for it.
    #[test]
    fn everything_held_is_offered_with_what_it_goes_round() {
        let mut orbits = Orbits::default();
        orbits.insert(1, None, circle(1e11));
        orbits.insert(2, Some(1), circle(3.8e8));
        orbits.insert(10, Some(1), circle(1e12));

        let mut held: Vec<_> = orbits.circling().collect();
        held.sort();

        assert_eq!(held, vec![(1, None), (2, Some(1)), (10, Some(1))]);
    }

    /// What one thing goes round is answered on its own
    #[test]
    fn what_a_thing_goes_round_is_answered() {
        let mut orbits = Orbits::default();
        orbits.insert(1, None, circle(1e11));
        orbits.insert(2, Some(1), circle(3.8e8));

        assert_eq!(orbits.parent(2), Some(1));
        // Held and going round nothing, and not held at all, are the same
        // answer, as they are to `place`.
        assert_eq!(orbits.parent(1), None);
        assert_eq!(orbits.parent(9), None);
    }

    /// A chain of parents that points back at itself still answers
    ///
    /// Nothing in the database forbids it: `parent_id` has no foreign key, so
    /// a bad row could name its own child. The walk is capped rather than
    /// trusted.
    #[test]
    fn a_parent_chain_that_loops_still_answers() {
        let mut orbits = Orbits::default();
        orbits.insert(1, Some(2), circle(1e8));
        orbits.insert(2, Some(1), circle(1e8));

        let place = orbits.place(1, 0.);
        assert!(place.is_finite(), "the walk ran off to {place}");
    }
}
