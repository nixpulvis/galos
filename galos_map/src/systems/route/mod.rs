use crate::camera::{FRAMING_MARGIN, MoveCamera};
use crate::schedule::MapSet;
use crate::systems::Spyglass;
use crate::systems::filter::{Filter, Filters};
use bevy::asset::RenderAssetUsages;
use bevy::math::DVec3;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;

use super::system_to_vec;

pub fn plugin(app: &mut App) {
    app.add_message::<Plotted>();
    // After the fetch it answers has been drawn, and before the camera is
    // pointed, since where it asks the camera to go is what `move_camera`
    // then works out.
    app.add_systems(
        Update,
        (plotted, follow_filters)
            .chain()
            .in_set(MapSet::Populate)
            .after(super::spawn::spawn),
    );
}

#[derive(Component)]
pub struct Route;

/// A route that has landed and been drawn
///
/// Written where the fetch is collected, since that is the one place the
/// systems it runs through are in hand, and answered here so that what a
/// route does to the map is in one place rather than threaded through the
/// system that draws stars.
#[derive(Message, Debug)]
pub struct Plotted {
    /// The two ends, as the database spells them
    pub label: String,
    /// Every system it runs through, by address, in the order travelled
    pub systems: Vec<i64>,
    /// The middle of what it spans
    pub middle: DVec3,
    /// How far it reaches from there, in light years
    pub extent: f32,
}

/// Show a route that has just been plotted
///
/// Three things at once, all of them the same thought: look at the whole of
/// it, reach far enough to hold the whole of it, and pick the whole of it
/// out from everything else.
///
/// The spyglass is set rather than left alone because a route is usually
/// longer than whatever the user was looking at when they asked for it, and a
/// route drawn as a line running out through the edge of an unchanged
/// spyglass is a route with no systems on it.
fn plotted(
    mut plotted: MessageReader<Plotted>,
    mut camera: MessageWriter<MoveCamera>,
    mut spyglass: ResMut<Spyglass>,
    mut filters: ResMut<Filters>,
) {
    for route in plotted.read() {
        camera.write(MoveCamera {
            position: Some(route.middle),
            framing: Some(route.extent),
        });

        // Measured from the middle, which is where the camera is going, so
        // what the spyglass holds is what the camera is about to see. The
        // same room around it that the camera is stood back to leave, since a
        // reach set to the route's own extent puts the two ends exactly on
        // the rim of it: the extent is the distance to the furthest of them,
        // and whether that counts as reaching them comes down to which way an
        // `f32` rounded.
        //
        // Held inside what the map will reach unasked. Everything the
        // spyglass takes in is fetched and spawned, and a route long enough
        // would otherwise set a reach nobody asked the size of.
        spyglass.radius = (route.extent * FRAMING_MARGIN)
            .clamp(Spyglass::OPENING, Spyglass::UNASKED);

        filters.replace(Filter::Route {
            label: route.label.clone(),
            systems: route.systems.clone(),
        });
    }
}

/// Take the line away when the filter naming it goes
///
/// The line and the filter row are two halves of one answer: the row says
/// which route is being shown and the line shows it. Dropping the row is how
/// the user says they are done with the route, so a line left drawn across
/// the map afterwards is an answer to a question nobody is asking, and one
/// with nothing left on screen to say what it is.
///
/// Presence rather than whether it is being asked. A filter turned off is one
/// the user means to come back to, and the route it names is still the route
/// they plotted.
fn follow_filters(
    filters: Res<Filters>,
    lines: Query<Entity, With<Route>>,
    mut commands: Commands,
) {
    if filters
        .iter()
        .any(|active| matches!(active.filter, Filter::Route { .. }))
    {
        return;
    }

    for line in &lines {
        commands.entity(line).despawn();
    }
}

pub mod fetch;
pub mod spawn;

/// A list of points that will have a line drawn between each consecutive points
#[derive(Debug, Clone)]
struct LineStrip {
    points: Vec<Vec3>,
}

impl From<LineStrip> for Mesh {
    fn from(line: LineStrip) -> Self {
        Mesh::new(
            // This tells wgpu that the positions are a list of points
            // where a line will be drawn between each consecutive point
            PrimitiveTopology::LineStrip,
            RenderAssetUsages::RENDER_WORLD,
        )
        // Add the point positions as an attribute
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, line.points)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A route filter over the systems at `addresses`
    fn asking(addresses: &[i64]) -> Filter {
        Filter::Route {
            label: "A -> B".to_owned(),
            systems: addresses.to_vec(),
        }
    }

    /// A world holding nothing but the filters and a drawn route
    fn map(filters: Filters) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(filters);
        app.add_systems(Update, follow_filters);
        let line = app.world_mut().spawn(Route).id();
        (app, line)
    }

    /// Whether the line is still drawn
    fn drawn(app: &App, line: Entity) -> bool {
        app.world().get_entity(line).is_ok()
    }

    /// The spyglass a route centered on `middle` and reaching `extent` leaves
    fn spyglass_for(middle: DVec3, extent: f32) -> Spyglass {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<Plotted>();
        app.add_message::<MoveCamera>();
        app.insert_resource(Spyglass {
            fetch: true,
            radius: Spyglass::OPENING,
            disabled: false,
            lock_camera: false,
        });
        app.init_resource::<Filters>();
        app.add_systems(Update, plotted);

        app.world_mut().write_message(Plotted {
            label: "A -> B".to_owned(),
            systems: vec![1, 2],
            middle,
            extent,
        });
        app.update();

        let held = app.world().resource::<Spyglass>();
        Spyglass {
            fetch: held.fetch,
            radius: held.radius,
            disabled: held.disabled,
            lock_camera: held.lock_camera,
        }
    }

    /// What reach a route of `extent` light years pulls the spyglass out to
    fn reach_for(extent: f32) -> f32 {
        spyglass_for(DVec3::ZERO, extent).radius
    }

    /// A route reaches past its own ends rather than up to them
    #[test]
    fn a_route_reaches_past_its_own_ends() {
        assert!(reach_for(60.) > 60., "{}", reach_for(60.));
    }

    /// So the systems at those ends are drawn
    ///
    /// The extent is the distance from the middle to the furthest of them,
    /// worked out in `f64` and kept as an `f32`. A reach set to exactly that
    /// puts those systems on its rim, where whether they are drawn comes down
    /// to which way the one cast rounded. These coordinates are a case where
    /// it rounds down, so the ends fall outside a reach of their own length.
    #[test]
    fn the_systems_at_a_route_s_ends_are_within_reach() {
        let places = [
            DVec3::ZERO,
            DVec3::new(26.03, 8.676666666666668, 3.718571428571429),
        ];
        let (middle, extent) = spawn::framing(&places).unwrap();
        assert!(
            middle.distance(places[1]) > extent as f64,
            "these coordinates no longer round the way the test is about",
        );

        let spyglass = spyglass_for(middle, extent);

        for place in places {
            assert!(spyglass.reaches(middle, place), "{place} is out of reach");
        }
    }

    /// A short one still leaves the map something to see around it
    #[test]
    fn a_short_route_leaves_room_around_it() {
        assert_eq!(reach_for(2.), Spyglass::OPENING);
    }

    /// A long one is held to what the map will reach unasked
    ///
    /// Everything the spyglass takes in is fetched and spawned, so a reach
    /// set from the length of whatever was plotted is a query nobody asked
    /// the size of.
    #[test]
    fn a_long_route_does_not_reach_as_far_as_it_likes() {
        assert_eq!(reach_for(5000.), Spyglass::UNASKED);
    }

    /// The line stays while a route filter names it
    #[test]
    fn a_route_keeps_its_line() {
        let mut filters = Filters::default();
        filters.replace(asking(&[1, 2]));
        let (mut app, line) = map(filters);

        app.update();

        assert!(drawn(&app, line));
    }

    /// And goes when that filter is dropped
    #[test]
    fn dropping_a_route_takes_its_line() {
        let mut filters = Filters::default();
        filters.replace(asking(&[1, 2]));
        let (mut app, line) = map(filters);
        app.update();

        app.world_mut().resource_mut::<Filters>().remove(0);
        app.update();

        assert!(!drawn(&app, line));
    }

    /// A filter turned off is one to come back to, and keeps its line
    #[test]
    fn a_route_turned_off_keeps_its_line() {
        let mut filters = Filters::default();
        filters.replace(asking(&[1, 2]));
        filters.toggle(0);
        let (mut app, line) = map(filters);

        app.update();

        assert!(drawn(&app, line));
    }

    /// Filters of another kind say nothing about a route's line
    #[test]
    fn a_faction_does_not_keep_a_line() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 7, name: "Some Lot".to_owned() });
        let (mut app, line) = map(filters);

        app.update();

        assert!(!drawn(&app, line));
    }
}
