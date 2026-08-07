use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use bevy::math::DVec3;
use bevy::prelude::*;
use chrono::{DateTime, Utc};
use elite_journal::{
    // TODO: Fix these imports, they should all be in system.
    Allegiance,
    Government,
    system::Security,
};
use galos_db::systems::{Economies, System as DbSystem};

pub fn plugin(app: &mut App) {
    app.insert_resource(Spyglass {
        radius: Spyglass::OPENING,
        fetch: true,
        clear: true,
        lock_camera: false,
        follow_camera: true,
    });

    app.add_plugins(fetch::plugin);
    app.add_plugins(roundness::plugin);
    app.add_plugins(bodies::plugin);
    app.add_plugins(spawn::plugin);
    app.add_plugins(despawn::plugin);
    app.add_plugins(scale::plugin);
    app.add_plugins(labels::plugin);
    app.add_plugins(pointing::plugin);
    app.add_plugins(route::plugin);
    app.add_plugins(selection::plugin);
    app.add_plugins(filter::plugin);
    app.add_plugins(info::plugin);

    app.init_resource::<InReach>();

    // Both ask the camera for something, and `orbit_camera` then works out
    // where it lands, so both have to have spoken by the time it runs.
    //
    // The two spyglass systems are the same link read in either direction, so
    // they are ordered rather than left to whichever Bevy picked: only one of
    // them is reachable at a time through the settings, and an order spelled
    // out is what keeps that from being the only thing holding them apart.
    app.add_systems(
        Update,
        (zoom_with_spyglass, reach_with_camera)
            .chain()
            .in_set(MapSet::Camera)
            .after(crate::camera::move_camera)
            .before(crate::camera::orbit_camera),
    );
    app.add_systems(Update, visibility.in_set(MapSet::Present));
}

/// Clones because a selection holds one, and a system may be selected before
/// the map has fetched it or after it has been despawned.
#[derive(Component, Clone)]
pub struct System {
    address: i64,
    name: String,
    /// Absolute galactic position, in light years
    ///
    /// The grid this is drawn in splits a position into a cell and an offset
    /// within it, which is what the renderer needs but an awkward thing to
    /// measure distances between. The database's own answer is kept here,
    /// undiminished, and everything that wants to know how far apart two
    /// systems are asks this instead of unpicking the split.
    position: [f64; 3],
    population: u64,
    allegiance: Option<Allegiance>,
    government: Option<Government>,
    security: Option<Security>,
    economies: Option<Economies>,
    /// The factions present in the system, by id
    ///
    /// What [`filter`] asks a system about, so ids rather than names: the
    /// question is put to every system drawn, every frame, and an integer
    /// compare is what that wants. A filter naming a faction has resolved it
    /// to an id already, since it was picked from a list.
    factions: Vec<i32>,
    updated_at: DateTime<Utc>,
}

impl System {
    /// Where the system is, in light years from the galactic center
    ///
    /// A [`System`]'s fields are private to this module, and the camera is
    /// not in it. It has to measure from a system to descend into one.
    pub fn position(&self) -> DVec3 {
        DVec3::from(self.position)
    }

    /// What the system is called
    ///
    /// A [`System`]'s fields are private to this module, and a route names
    /// both of its ends in the bar, which is not.
    pub fn name(&self) -> &str {
        &self.name
    }
}

pub mod bodies;
pub mod despawn;
pub mod fetch;
pub mod filter;
pub mod info;
pub mod labels;
pub mod pointing;
pub mod roundness;
pub mod route;
pub mod scale;
pub mod selection;
pub mod spawn;

/// A global setting which controls the spyglass around the camera
#[derive(Resource)]
pub struct Spyglass {
    /// Ask the database for what is within the reach
    ///
    /// The two halves of what a spyglass does, this and [`Spyglass::clear`],
    /// and each is worth having without the other. Off, the map draws what it
    /// has and asks for nothing more, which is how to look at a sky that
    /// stops changing under you.
    pub fetch: bool,
    pub radius: f32,
    /// Clear away what the reach does not hold
    ///
    /// On to begin with, that being what looking through a spyglass is. Off,
    /// everything loaded is drawn however far off it lies, which is
    /// everywhere the camera has been rather than anywhere it is looking.
    pub clear: bool,
    /// Zoom the camera to whatever the reach is set to
    ///
    /// Only meaningful while [`Spyglass::follow_camera`] is off. The two are
    /// the same link read in opposite directions, and the camera cannot both
    /// be told where to stand and be asked where it is standing.
    pub lock_camera: bool,
    /// Reach as far as the camera can see, rather than as far as it is told
    ///
    /// On to begin with. What the camera is looking at is what the user is
    /// asking about, so a reach taken from it is the map fetching and drawing
    /// the view rather than a circle set beside it and kept in step by hand.
    ///
    /// It runs the whole way to [`Spyglass::CEILING`], which is the galaxy.
    /// Scrolling out that far asks for every system on record, and asks for it
    /// in one gesture, but a map that fetches less than it is showing is a map
    /// with a circle of stars in the middle of an empty window, and a reach
    /// that follows the view has said it will not do that. The fetch is
    /// throttled and the wheel is where the user says when to stop.
    pub follow_camera: bool,
}

impl Spyglass {
    /// How far the map reaches when it opens
    ///
    /// Also the least anything sets it to without being asked. A route
    /// between two neighbours spans a few light years, and a reach drawn in
    /// that far shows the two ends and nothing around them.
    pub const OPENING: f32 = 10.;

    /// The shortest reach worth offering, in light years
    ///
    /// Stars stand far enough apart that a shorter one shows the system at
    /// the middle of it and nothing else, so every setting under it draws the
    /// same picture.
    ///
    /// Measured over a sample of inhabited systems, counting what stands
    /// within reach of each: at 1, 2, 3 and 5 light years the middling answer
    /// is one system, which is the one being stood on. It first rises at 8,
    /// reaches 4 by 10, and 19 by 20.
    pub const FLOOR: f32 = 5.;

    /// The longest, in light years
    ///
    /// The galaxy is 105,700 across, so this reaches the whole of it. Only
    /// ever asked for by hand: everything it takes in is fetched and drawn,
    /// and at this reach that is every system on record.
    pub const CEILING: f32 = 1.1e5;

    /// The furthest the map reaches without being asked to
    ///
    /// What a route may pull it out to. Everything the spyglass takes in is
    /// fetched and spawned, so a reach set from the length of whatever was
    /// plotted is a query nobody asked the size of: measured against the
    /// systems on record, 200 light years takes in about fourteen thousand,
    /// 500 about twenty five, and 1000 about thirty two.
    ///
    /// Past this a route is drawn as a line with stars about the middle of it
    /// and none out at the ends, which is a poorer picture than the one asked
    /// for and a far better one than a map that has stopped answering.
    pub const UNASKED: f32 = 200.;
}

impl Spyglass {
    /// Whether the spyglass reaches `position`, from a camera centered on
    /// `center`
    ///
    /// Everything loaded while it is not clearing, there being no reach to be
    /// outside of then.
    ///
    /// Asked by whatever has to tell the two reasons a system is not drawn
    /// apart. Out of reach is the map saying it is not looking there;
    /// excluded by a filter is the user saying they are not interested, and
    /// what is drawn for a system they picked out by hand answers only the
    /// first.
    pub fn reaches(&self, center: DVec3, position: DVec3) -> bool {
        !self.clear || center.distance(position) <= self.radius as f64
    }

    /// Whether the camera stands wherever the reach puts it
    ///
    /// Locking holds the camera only while the camera is not itself what sets
    /// the reach, the two being the same link read in opposite directions.
    ///
    /// Asked by both ends of that: by what writes the camera's distance from
    /// the reach, and by what would otherwise let a scroll write it too. A
    /// zoom that is going to be written back over on the next frame is a
    /// camera that lurches and returns, so while this holds there is no zoom
    /// at all.
    pub fn locks_camera(&self) -> bool {
        self.lock_camera && !self.follow_camera
    }
}

/// How much of the sky is in reach, and how much of that the filters admit
///
/// Tallied by [`visibility`], which has already settled both for every system
/// on the map. Counted there rather than by whoever draws the number, so what
/// is said is the answer the map acted on rather than a second count taken
/// from the side. Both halves want the same two questions asked of the same
/// system, and that is the one place both are in hand.
///
/// In reach rather than loaded. What the spyglass has dragged in from wherever
/// the camera has been is not what the user is looking at.
#[derive(Resource, Default, PartialEq, Eq)]
pub struct InReach {
    /// Systems the spyglass reaches
    ///
    /// The sky only where the excluded systems are being fetched to be dimmed.
    /// At [`filter::DimTo`] zero the region is asked for what the filters
    /// admit and nothing else, so what is in reach and excluded is whatever
    /// was brought in before the filter was asked for and never despawned.
    /// That is a number about where the camera has been rather than about the
    /// sky, and [`crate::ui`] draws this only where it is the sky.
    pub total: usize,
    /// How many of those the filters admit
    ///
    /// Whole however the region was asked for. Narrowing the query asks for
    /// exactly the systems a filter admits, so what is left out of it is what
    /// this was never counting.
    pub admitted: usize,
}

/// Decide which systems are drawn
///
/// Two questions, and a system has to pass both: the spyglass has to reach it,
/// and the filters have to admit it. One answer written in one place, since
/// two systems each writing a `Visibility` would take turns undoing each
/// other.
///
/// A filtered system is only hidden where it is being dimmed to nothing.
/// Anywhere above that it is drawn faintly, which is the other half of what
/// [`filter`] is for: a faction read against the space around it.
///
/// Runs over every star every frame, so it writes only where the answer
/// actually changed. Assigning regardless would mark the whole sky as
/// changed each frame, and each star drags its name along with it.
pub fn visibility(
    camera: Query<&OrbitCamera>,
    mut systems: Query<(&System, &mut Visibility, Has<filter::Filtered>)>,
    spyglass: Res<Spyglass>,
    dim: Res<filter::DimTo>,
    mut in_reach: ResMut<InReach>,
) {
    let Ok(camera) = camera.single() else { return };
    let excluded_are_drawn = dim.0 > 0.;

    let mut tally = InReach::default();
    for (system, mut visibility, filtered) in &mut systems {
        let within =
            spyglass.reaches(camera.center, DVec3::from(system.position));
        if within {
            tally.total += 1;
            if !filtered {
                tally.admitted += 1;
            }
        }

        visibility.set_if_neq(if within && (!filtered || excluded_are_drawn) {
            Visibility::Visible
        } else {
            Visibility::Hidden
        });
    }

    // Only where it moved, so that a count nobody is watching does not mark
    // itself changed every frame.
    if *in_reach != tally {
        *in_reach = tally;
    }
}

pub fn zoom_with_spyglass(
    spyglass: Res<Spyglass>,
    mut camera: Query<&mut OrbitCamera>,
) {
    if spyglass.locks_camera() {
        if let Ok(mut camera) = camera.single_mut() {
            camera.target_radius = spyglass.radius * 3.;
        }
    }
}

/// How much of the view is left empty around the reach, in hundredths
///
/// The reach is a sphere about what the camera looks at, and stars stop at its
/// surface. Reaching exactly as far as the camera sees stands that surface on
/// the edge of the screen, so the last stars sit hard against the frame with
/// the sky ending along it. Holding it inside the view leaves them short of
/// the edge with empty space beyond, which is what the edge of a reach looks
/// like rather than what the edge of a window looks like.
///
/// Unsigned, and taken off a hundred, so this can only ever bring the reach in
/// from the edge of the view. A margin that would push it past the edge is a
/// margin that does not compile.
///
/// Under thirteen, and not by accident. A camera stood back over a route takes
/// in [`crate::camera::FRAMING_MARGIN`] more than the route, which is a
/// thirteenth of the view left over, so a margin larger than that eats through
/// the room the framing left and puts the ends of every plotted route outside
/// the reach that was taken from the camera framing it.
const FOLLOW_MARGIN: u32 = 10;

/// Reach not quite as far as the camera can see
///
/// [`zoom_with_spyglass`] read the other way. That one stands the camera back
/// to take in the reach; this one takes the reach from where the camera is
/// already standing, so scrolling out fetches and draws more of the sky and
/// scrolling in narrows to what is being looked at.
///
/// Short of what the camera sees by [`FOLLOW_MARGIN`], so that the sky stops
/// inside the window rather than along the edge of it.
///
/// The target rather than the radius the camera has reached, so that the reach
/// is settled the moment a scroll or a move asks for it and the systems are on
/// their way while the camera is still travelling. Reading the radius instead
/// would move the reach a little every frame of a zoom, and every step of it
/// is a region to be fetched.
pub fn reach_with_camera(
    mut spyglass: ResMut<Spyglass>,
    camera: Query<&OrbitCamera>,
    lens: Query<&Projection>,
) {
    if !spyglass.follow_camera {
        return;
    }
    let Ok(camera) = camera.single() else { return };

    // The margin here rather than inside `framed`, which answers what the
    // camera takes in and would be answering something else with room left
    // over folded into it. How far short of that to stop is the spyglass's to
    // say.
    let seen = crate::camera::framed(camera.target_radius, lens.single().ok());
    let inside = seen * (100 - FOLLOW_MARGIN) as f32 / 100.;
    let reach = inside.clamp(Spyglass::FLOOR, Spyglass::CEILING);

    // Only where it moved. Nothing watches this resource for changes today,
    // and writing the same number every frame is how that stops being true
    // without anyone meaning it to.
    if spyglass.radius != reach {
        spyglass.radius = reach;
    }
}

/// Where a system sits, if the database knows
///
/// Roughly three quarters of the systems on record have no coordinates, so
/// this has to be an answer the caller handles rather than an assumption.
pub fn system_to_vec(system: &DbSystem) -> Option<DVec3> {
    system.position.map(|p| DVec3::new(p.x, p.y, p.z))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::DateTime;

    /// A system with nothing on record but the address that names it
    ///
    /// Shared by the modules under [`super`], each of which tests something
    /// that keys off a system without caring what is in it. Fields are set
    /// on the way out by whichever test cares about one.
    pub(crate) fn system(address: i64) -> System {
        System {
            address,
            name: format!("Test {address}"),
            position: [0., 0., 0.],
            population: 0,
            allegiance: None,
            government: None,
            security: None,
            economies: None,
            factions: vec![],
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    /// A system called `name`
    ///
    /// For whoever is testing what gets said about a system rather than what
    /// is done with it. A [`System`]'s fields are private to this module, so
    /// the name has to be set from in here.
    pub(crate) fn named(address: i64, name: &str) -> System {
        let mut system = system(address);
        system.name = name.to_owned();
        system
    }

    /// A system at `away` light years from the origin, on the x axis
    ///
    /// Shared for the same reason [`named`] is: a position is set from in
    /// here or not at all, and what is drawn about a system is tested from
    /// wherever it is drawn.
    pub(crate) fn at(address: i64, away: f64) -> System {
        let mut system = system(address);
        system.position = [away, 0., 0.];
        system
    }

    /// A world holding one camera at the origin, and nothing else
    fn map(radius: f32, clear: bool, dim: f32) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(Spyglass {
            radius,
            clear,
            fetch: false,
            lock_camera: false,
            follow_camera: false,
        });
        app.insert_resource(filter::DimTo(dim));
        app.init_resource::<InReach>();
        app.world_mut().spawn(OrbitCamera::default());
        app.add_systems(Update, visibility);
        app
    }

    /// Whether `entity` came out drawn
    fn drawn(app: &App, entity: Entity) -> bool {
        *app.world().entity(entity).get::<Visibility>().unwrap()
            != Visibility::Hidden
    }

    /// The spyglass reaches what is inside its radius and no further
    #[test]
    fn the_spyglass_reaches_what_is_within_it() {
        let spyglass = Spyglass {
            radius: 10.,
            fetch: false,
            clear: true,
            lock_camera: false,
            follow_camera: false,
        };
        let center = DVec3::ZERO;

        assert!(spyglass.reaches(center, DVec3::new(5., 0., 0.)));
        assert!(spyglass.reaches(center, DVec3::new(10., 0., 0.)));
        assert!(!spyglass.reaches(center, DVec3::new(11., 0., 0.)));
    }

    /// Not clearing, it reaches everything loaded however far off
    #[test]
    fn a_spyglass_that_does_not_clear_reaches_whatever_is_loaded() {
        let spyglass = Spyglass {
            radius: 10.,
            fetch: false,
            clear: false,
            lock_camera: false,
            follow_camera: false,
        };

        assert!(spyglass.reaches(DVec3::ZERO, DVec3::new(5e4, 0., 0.)));
    }

    /// The spyglass draws what it reaches and hides the rest
    #[test]
    fn the_spyglass_decides_what_is_drawn() {
        let mut app = map(10., true, 0.15);
        let near =
            app.world_mut().spawn((at(1, 5.), Visibility::default())).id();
        let far =
            app.world_mut().spawn((at(2, 50.), Visibility::default())).id();

        app.update();

        assert!(drawn(&app, near));
        assert!(!drawn(&app, far));
    }

    /// A spyglass that does not clear draws everything loaded, however far off
    #[test]
    fn a_spyglass_that_does_not_clear_draws_everything() {
        let mut app = map(10., false, 0.15);
        let far =
            app.world_mut().spawn((at(1, 5e4), Visibility::default())).id();

        app.update();

        assert!(drawn(&app, far));
    }

    /// A filtered system is still drawn, so long as it is being dimmed
    ///
    /// Which is the whole of why filters dim rather than hide: a faction is
    /// read against the space around it.
    #[test]
    fn a_filtered_system_is_drawn_to_be_dimmed() {
        let mut app = map(10., true, 0.15);
        let excluded = app
            .world_mut()
            .spawn((at(1, 5.), Visibility::default(), filter::Filtered))
            .id();

        app.update();

        assert!(drawn(&app, excluded));
    }

    /// Dimming to nothing takes the excluded systems off the map
    ///
    /// A star faded to nothing is still a star being drawn, and still one the
    /// pointer can land on, so this is hidden rather than merely invisible.
    #[test]
    fn dimming_to_nothing_hides_what_is_filtered() {
        let mut app = map(10., true, 0.);
        let excluded = app
            .world_mut()
            .spawn((at(1, 5.), Visibility::default(), filter::Filtered))
            .id();
        let included =
            app.world_mut().spawn((at(2, 5.), Visibility::default())).id();

        app.update();

        assert!(!drawn(&app, excluded));
        assert!(drawn(&app, included));
    }

    /// What the tally came to
    fn counted(app: &App) -> (usize, usize) {
        let in_reach = app.world().resource::<InReach>();
        (in_reach.admitted, in_reach.total)
    }

    /// The tally counts what is in reach rather than what has been loaded
    ///
    /// The spyglass drags systems in from everywhere the camera has been,
    /// and those are not what the user is looking at.
    #[test]
    fn the_tally_counts_what_is_in_reach() {
        let mut app = map(10., true, 0.15);
        app.world_mut().spawn((at(1, 5.), Visibility::default()));
        app.world_mut().spawn((at(2, 7.), Visibility::default()));
        app.world_mut().spawn((at(3, 5e3), Visibility::default()));

        app.update();

        assert_eq!(counted(&app), (2, 2));
    }

    /// A filter takes systems out of the count without taking them out of
    /// reach
    #[test]
    fn the_tally_says_how_many_a_filter_admits() {
        let mut app = map(10., true, 0.15);
        app.world_mut().spawn((at(1, 5.), Visibility::default()));
        app.world_mut().spawn((
            at(2, 5.),
            Visibility::default(),
            filter::Filtered,
        ));

        app.update();

        assert_eq!(counted(&app), (1, 2));
    }

    /// The tally counts the excluded, which are drawn along with the rest
    ///
    /// A count over what is drawn would come to the same number twice and say
    /// nothing at all, since what a filter excludes is dimmed rather than
    /// taken away. What is asked of it is how much of the sky is getting
    /// through, which only the marks answer.
    #[test]
    fn the_tally_counts_the_excluded_among_the_drawn() {
        let mut app = map(10., true, 0.15);
        app.world_mut().spawn((at(1, 5.), Visibility::default()));
        app.world_mut().spawn((
            at(2, 5.),
            Visibility::default(),
            filter::Filtered,
        ));
        app.world_mut().spawn((
            at(3, 5.),
            Visibility::default(),
            filter::Filtered,
        ));

        app.update();

        assert_eq!(counted(&app), (1, 3));
    }

    /// Dimming to nothing still counts what it took off the map
    ///
    /// Being hidden is not being gone. A system the spyglass reaches is in
    /// reach whether or not a filter is letting it be drawn, so the two
    /// numbers go on being two numbers here rather than collapsing into one
    /// the moment the sky is put out.
    ///
    /// Which is what the tally is for and not what it is read for: the map
    /// stops fetching the excluded at this opacity, so the systems counted
    /// here are the ones that happened to be loaded already, and the bar says
    /// only how many are getting through. See [`InReach::total`].
    #[test]
    fn the_tally_holds_up_when_nothing_excluded_is_drawn() {
        let mut app = map(10., true, 0.);
        app.world_mut().spawn((at(1, 5.), Visibility::default()));
        app.world_mut().spawn((
            at(2, 5.),
            Visibility::default(),
            filter::Filtered,
        ));
        app.world_mut().spawn((
            at(3, 5.),
            Visibility::default(),
            filter::Filtered,
        ));

        app.update();

        assert_eq!(counted(&app), (1, 3));
    }

    /// The spyglass still hides what it cannot reach, filter or no filter
    #[test]
    fn a_filtered_system_out_of_reach_stays_hidden() {
        let mut app = map(10., true, 0.15);
        let far = app
            .world_mut()
            .spawn((at(1, 50.), Visibility::default(), filter::Filtered))
            .id();

        app.update();

        assert!(!drawn(&app, far));
    }

    /// A spyglass that does not clear leaves the filters clearing
    ///
    /// The two answer different questions, and the one that draws everything
    /// loaded has nothing to say about what was asked for.
    #[test]
    fn a_spyglass_that_does_not_clear_still_honours_a_filter() {
        let mut app = map(10., false, 0.);
        let excluded = app
            .world_mut()
            .spawn((at(1, 5e4), Visibility::default(), filter::Filtered))
            .id();

        app.update();

        assert!(!drawn(&app, excluded));
    }

    /// A world holding a camera `back` light years out and the two systems
    /// that read it
    ///
    /// No lens is spawned, so the default half angle answers, which is what a
    /// window as wide as it is tall would give anyway.
    fn linked(back: f32, lock_camera: bool, follow_camera: bool) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(Spyglass {
            radius: Spyglass::OPENING,
            fetch: false,
            clear: true,
            lock_camera,
            follow_camera,
        });
        app.world_mut().spawn(OrbitCamera {
            radius: back,
            target_radius: back,
            ..default()
        });
        app.add_systems(
            Update,
            (zoom_with_spyglass, reach_with_camera).chain(),
        );
        app
    }

    /// How far the spyglass reaches, and how far back the camera stands
    fn linkage(app: &mut App) -> (f32, f32) {
        let reach = app.world().resource::<Spyglass>().radius;
        let back = app
            .world_mut()
            .query::<&OrbitCamera>()
            .single(app.world())
            .unwrap()
            .target_radius;
        (reach, back)
    }

    /// What the reach comes to at a camera `back` light years out
    fn following(back: f32) -> f32 {
        crate::camera::framed(back, None) * (100 - FOLLOW_MARGIN) as f32 / 100.
    }

    /// Following the camera, the reach is what the camera can see
    #[test]
    fn the_reach_follows_what_the_camera_sees() {
        let mut app = linked(100., false, true);

        app.update();

        let (reach, _) = linkage(&mut app);
        assert!((reach - following(100.)).abs() < 1e-3, "reached {reach}");
        assert!(reach != Spyglass::OPENING, "left where it opened");
    }

    /// The reach stops inside the view rather than along the edge of it
    ///
    /// Stars stop at the surface of the reach, and a surface standing on the
    /// edge of the screen is a sky that ends where the window does.
    #[test]
    fn the_reach_stops_short_of_what_the_camera_sees() {
        let mut app = linked(100., false, true);

        app.update();

        let (reach, _) = linkage(&mut app);
        let seen = crate::camera::framed(100., None);
        assert!(reach < seen, "reached {reach} of the {seen} seen");
    }

    /// A route framed by the camera is still held whole by the reach
    ///
    /// Plotting a route stands the camera back over it, and with the reach
    /// following the camera that framing is what sets the reach. The room the
    /// framing leaves has to outlast the room [`FOLLOW_MARGIN`] takes, or the
    /// ends of every route plotted would fall outside the reach that was taken
    /// from the camera framing it.
    #[test]
    fn a_framed_route_is_held_by_the_reach_taken_from_it() {
        for extent in [10., 50., 150.] {
            let back = crate::camera::stand_back(extent, None);
            let mut app = linked(back, false, true);

            app.update();

            let (reach, _) = linkage(&mut app);
            assert!(
                reach > extent,
                "a route reaching {extent} left a reach of {reach}"
            );
        }
    }

    /// Scrolling out reaches further and scrolling in reaches less
    #[test]
    fn the_reach_moves_with_the_camera() {
        let mut app = linked(100., false, true);
        app.update();
        let (near, _) = linkage(&mut app);

        app.world_mut()
            .query::<&mut OrbitCamera>()
            .single_mut(app.world_mut())
            .unwrap()
            .target_radius = 400.;
        app.update();
        let (far, _) = linkage(&mut app);

        assert!(far > near, "reached {far} from further out than {near}");
    }

    /// Pulled back far enough, the reach takes in the whole galaxy
    ///
    /// Nothing holds it in at what the map asks for unbidden. A view the
    /// spyglass is not keeping up with is a window with a circle of stars in
    /// the middle of it, which is the one thing a reach that follows the view
    /// has undertaken not to show.
    #[test]
    fn the_reach_follows_the_camera_the_whole_way_out() {
        let mut app = linked(Spyglass::CEILING, false, true);

        app.update();

        let (reach, _) = linkage(&mut app);
        assert_eq!(reach, following(Spyglass::CEILING));
        assert!(reach > Spyglass::UNASKED, "stopped at {reach}");
    }

    /// And no further than the whole galaxy
    ///
    /// The camera may stand further off than the galaxy is wide, and a reach
    /// past that is a larger number asking for exactly the same systems.
    #[test]
    fn the_reach_stops_at_the_edge_of_the_galaxy() {
        let mut app = linked(1e6, false, true);

        app.update();

        let (reach, _) = linkage(&mut app);
        assert_eq!(reach, Spyglass::CEILING);
    }

    /// And never falls under the shortest reach worth offering
    #[test]
    fn the_reach_stops_where_it_would_show_one_system() {
        let mut app = linked(1e-3, false, true);

        app.update();

        let (reach, _) = linkage(&mut app);
        assert_eq!(reach, Spyglass::FLOOR);
    }

    /// Not following, the reach is left where it was set
    #[test]
    fn a_reach_set_by_hand_is_left_alone() {
        let mut app = linked(100., false, false);

        app.update();

        let (reach, _) = linkage(&mut app);
        assert_eq!(reach, Spyglass::OPENING);
    }

    /// Locking the camera does nothing while the camera is what sets the
    /// reach
    ///
    /// The two are the same link read in opposite directions. Were both in
    /// force the camera would be told to stand where the reach says while the
    /// reach was being taken from where it stands, and which of them the map
    /// ended up obeying would come down to the order they ran in.
    #[test]
    fn the_camera_is_not_locked_to_a_reach_it_is_setting() {
        let mut app = linked(100., true, true);

        app.update();

        let (reach, back) = linkage(&mut app);
        assert_eq!(back, 100., "the camera was moved to {back}");
        assert!((reach - following(100.)).abs() < 1e-3, "reached {reach}");
    }

    /// Locked and not following, the camera goes where the reach says
    #[test]
    fn a_locked_camera_still_follows_the_reach() {
        let mut app = linked(100., true, false);

        app.update();

        let (reach, back) = linkage(&mut app);
        assert_eq!(reach, Spyglass::OPENING);
        assert_eq!(back, Spyglass::OPENING * 3.);
    }
}
