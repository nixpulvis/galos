use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::space::{Galaxy, Map};
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_message::<Despawn>();
    app.add_systems(
        Update,
        despawn.in_set(MapSet::Populate).after(super::spawn::spawn),
    );
}

#[derive(Message)]
pub struct Despawn;

/// Take the whole map off at once
///
/// The galaxy is thrown away and a bare one hung in its place, rather than
/// each system being taken out of it one at a time. Bevy holds a parent's
/// children in a `Vec` and a child leaving scans it and then shifts down
/// whatever was behind it, so systems leaving one at a time costs the square
/// of how many there are: a hundred thousand measured four seconds, and half a
/// million would leave the window not answering for a minute and a half. A
/// parent going down takes its children with it and they skip that work,
/// knowing the list is going too, which is nineteen milliseconds for the same
/// hundred thousand.
///
/// The route lines go with them. A route is a line from one system to another
/// and joins nothing once they are gone.
///
/// The camera is not touched. It hangs off the [`Map`] rather than off the
/// galaxy, so what it is looking at can be replaced without moving it.
pub fn despawn(
    mut commands: Commands,
    map: Res<Map>,
    galaxy: Res<Galaxy>,
    camera: Query<Entity, With<OrbitCamera>>,
    mut events: MessageReader<Despawn>,
) {
    if events.read().count() == 0 {
        return;
    }

    // Up out of whatever it was standing in first. A camera that has descended
    // into a system is a child of it, and the system is about to go.
    if let Ok(eye) = camera.single() {
        commands.entity(eye).insert(ChildOf(map.0));
    }

    commands.entity(galaxy.0).despawn();
    let fresh = commands.spawn((crate::space::galaxy(), ChildOf(map.0))).id();
    commands.insert_resource(Galaxy(fresh));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::System;
    use crate::systems::tests::system;

    /// A map holding `count` systems, each with a shell drawn under it
    fn sky(count: i64) -> App {
        let mut app = App::new();
        app.add_message::<Despawn>();
        app.add_systems(Update, despawn);

        let map = app.world_mut().spawn_empty().id();
        let galaxy = app.world_mut().spawn(ChildOf(map)).id();
        app.insert_resource(Map(map));
        app.insert_resource(Galaxy(galaxy));

        for address in 0..count {
            let star =
                app.world_mut().spawn((system(address), ChildOf(galaxy))).id();
            app.world_mut().spawn(ChildOf(star));
        }

        app
    }

    fn clear(app: &mut App) {
        app.world_mut().write_message(Despawn);
        app.update();
    }

    fn map(app: &App) -> Entity {
        app.world().resource::<Map>().0
    }

    fn galaxy(app: &App) -> Entity {
        app.world().resource::<Galaxy>().0
    }

    fn parent(app: &App, entity: Entity) -> Option<Entity> {
        app.world().get::<ChildOf>(entity).map(|of| of.parent())
    }

    /// A clear takes every system and what is drawn under it
    #[test]
    fn a_clear_takes_every_system_and_its_own() {
        let mut app = sky(0);
        let galaxy = galaxy(&app);
        let star = app.world_mut().spawn((system(1), ChildOf(galaxy))).id();
        let shell = app.world_mut().spawn(ChildOf(star)).id();

        clear(&mut app);

        assert!(app.world().get_entity(star).is_err(), "the system is left");
        assert!(app.world().get_entity(shell).is_err(), "its shell is left");
    }

    /// A clear takes the route lines along with the systems
    #[test]
    fn a_clear_takes_the_route_lines_too() {
        let mut app = sky(2);
        let galaxy = galaxy(&app);
        let line = app.world_mut().spawn(ChildOf(galaxy)).id();

        clear(&mut app);

        assert!(app.world().get_entity(line).is_err(), "the line is left");
    }

    /// The galaxy hung in its place is bare, and is the one the map names
    #[test]
    fn a_clear_hangs_a_fresh_galaxy_off_the_map() {
        let mut app = sky(3);
        let old = galaxy(&app);

        clear(&mut app);

        let fresh = galaxy(&app);
        assert_ne!(fresh, old, "the galaxy was not replaced");
        assert_eq!(parent(&app, fresh), Some(map(&app)), "not under the map");
        assert!(
            app.world().get::<Children>(fresh).is_none(),
            "the fresh galaxy is holding something"
        );
    }

    /// The camera is left standing where it was
    ///
    /// What hanging it off the map rather than off the galaxy is for: what it
    /// is looking at is replaced without it being moved.
    #[test]
    fn a_clear_leaves_the_camera_standing() {
        let mut app = sky(2);
        let map = map(&app);
        let eye =
            app.world_mut().spawn((OrbitCamera::default(), ChildOf(map))).id();

        clear(&mut app);

        assert_eq!(parent(&app, eye), Some(map), "the camera was moved");
    }

    /// A camera standing inside a system comes up out of it first
    ///
    /// Despawning a system takes its children with it, and the camera is one
    /// of them once it has descended.
    #[test]
    fn a_camera_inside_a_system_comes_up_before_it_goes() {
        let mut app = sky(1);
        let star = app
            .world_mut()
            .query_filtered::<Entity, With<System>>()
            .single(app.world())
            .expect("the one system");
        let eye =
            app.world_mut().spawn((OrbitCamera::default(), ChildOf(star))).id();

        clear(&mut app);

        assert_eq!(
            parent(&app, eye),
            Some(map(&app)),
            "the camera did not come up into the map"
        );
    }
}
