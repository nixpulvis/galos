use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use bevy::math::DVec3;
use bevy::prelude::*;
use chrono::{DateTime, Utc};
use elite_journal::{
    // TODO: Fix these imports, they should all be in system.
    Allegiance,
    Government,
    system::{Economy, Security},
};
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.insert_resource(Spyglass {
        radius: 10.,
        fetch: true,
        disabled: false,
        lock_camera: false,
    });

    app.add_plugins(fetch::plugin);
    app.add_plugins(spawn::plugin);
    app.add_plugins(despawn::plugin);
    app.add_plugins(scale::plugin);
    app.add_plugins(labels::plugin);
    app.add_plugins(pointing::plugin);
    app.add_plugins(selection::plugin);
    app.add_plugins(filter::plugin);
    app.add_plugins(info::plugin);

    app.init_resource::<InReach>();

    // Both ask the camera for something, and `orbit_camera` then works out
    // where it lands, so both have to have spoken by the time it runs.
    app.add_systems(
        Update,
        zoom_with_spyglass
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
    primary_economy: Option<Economy>,
    secondary_economy: Option<Economy>,
    /// The factions present in the system, by id
    ///
    /// What [`filter`] asks a system about, so ids rather than names: the
    /// question is put to every system drawn, every frame, and an integer
    /// compare is what that wants. A filter naming a faction has resolved it
    /// to an id already, since it was picked from a list.
    factions: Vec<i32>,
    updated_at: DateTime<Utc>,
}

pub mod despawn;
pub mod fetch;
pub mod filter;
pub mod info;
pub mod labels;
pub mod pointing;
pub mod route;
pub mod scale;
pub mod selection;
pub mod spawn;

/// A global setting which controls the spyglass around the camera
#[derive(Resource)]
pub struct Spyglass {
    pub fetch: bool,
    pub radius: f32,
    pub disabled: bool,
    pub lock_camera: bool,
}

/// How much of the sky is in reach, and how much of that the filters admit
///
/// Tallied by [`visibility`], which has already settled both for every system
/// on the map. Counted there rather than by whoever draws the number, for two
/// reasons: what is said is then the answer the map acted on, and the reach is
/// still counted when the filters have taken their systems off the map
/// entirely. Counting drawn entities would make the two equal at
/// [`filter::DimTo`] zero, where the question matters most.
///
/// In reach rather than loaded. What the spyglass has dragged in from
/// wherever the camera has been is not what the user is looking at.
#[derive(Resource, Default, PartialEq, Eq)]
pub struct InReach {
    /// Systems the spyglass reaches
    pub total: usize,
    /// How many of those the filters admit
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
/// Anywhere above that it is drawn faintly, which is [`filter`]'s whole point:
/// a faction read against the space around it.
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
    // How far the spyglass reaches, or nothing at all when it has been
    // overridden and everything loaded is drawn.
    let reach = if spyglass.disabled {
        None
    } else {
        let Ok(camera) = camera.single() else { return };
        Some((camera.focus, spyglass.radius as f64))
    };
    let excluded_are_drawn = dim.0 > 0.;

    let mut tally = InReach::default();
    for (system, mut visibility, filtered) in &mut systems {
        let within = reach.is_none_or(|(focus, radius)| {
            focus.distance(DVec3::from(system.position)) <= radius
        });
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
    if spyglass.lock_camera {
        if let Ok(mut camera) = camera.single_mut() {
            camera.target_radius = spyglass.radius * 3.;
        }
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
            primary_economy: None,
            secondary_economy: None,
            factions: vec![],
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    /// A system at `away` light years from the origin, on the x axis
    fn at(address: i64, away: f64) -> System {
        let mut system = system(address);
        system.position = [away, 0., 0.];
        system
    }

    /// A world holding one camera at the origin, and nothing else
    fn map(radius: f32, disabled: bool, dim: f32) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(Spyglass {
            radius,
            disabled,
            fetch: false,
            lock_camera: false,
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

    /// The spyglass draws what it reaches and hides the rest
    #[test]
    fn the_spyglass_decides_what_is_drawn() {
        let mut app = map(10., false, 0.2);
        let near =
            app.world_mut().spawn((at(1, 5.), Visibility::default())).id();
        let far =
            app.world_mut().spawn((at(2, 50.), Visibility::default())).id();

        app.update();

        assert!(drawn(&app, near));
        assert!(!drawn(&app, far));
    }

    /// An overridden spyglass draws everything loaded, however far off
    #[test]
    fn an_overridden_spyglass_reaches_everything() {
        let mut app = map(10., true, 0.2);
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
        let mut app = map(10., false, 0.2);
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
        let mut app = map(10., false, 0.);
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
        let mut app = map(10., false, 0.2);
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
        let mut app = map(10., false, 0.2);
        app.world_mut().spawn((at(1, 5.), Visibility::default()));
        app.world_mut().spawn((
            at(2, 5.),
            Visibility::default(),
            filter::Filtered,
        ));

        app.update();

        assert_eq!(counted(&app), (1, 2));
    }

    /// Dimming to nothing still counts what it took off the map
    ///
    /// This is the case the count is most needed for: the sky has emptied,
    /// and how much of it the filter is answerable for is the question. A
    /// tally over what is drawn would make the two numbers equal here and
    /// say nothing at all.
    #[test]
    fn the_tally_holds_up_when_nothing_excluded_is_drawn() {
        let mut app = map(10., false, 0.);
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
        let mut app = map(10., false, 0.2);
        let far = app
            .world_mut()
            .spawn((at(1, 50.), Visibility::default(), filter::Filtered))
            .id();

        app.update();

        assert!(!drawn(&app, far));
    }

    /// Overriding the spyglass does not override the filters
    ///
    /// The two answer different questions, and the one that draws everything
    /// loaded has nothing to say about what was asked for.
    #[test]
    fn an_overridden_spyglass_still_honours_a_filter() {
        let mut app = map(10., true, 0.);
        let excluded = app
            .world_mut()
            .spawn((at(1, 5e4), Visibility::default(), filter::Filtered))
            .id();

        app.update();

        assert!(!drawn(&app, excluded));
    }
}
