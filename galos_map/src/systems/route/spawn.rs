use super::{LineStrip, Route, system_to_vec};
use crate::space::Galaxy;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use galos_db::systems::System as DbSystem;

/// Where a route sits, and how far it reaches from there
///
/// The middle of what it spans and the distance from there to whichever of
/// its systems is furthest, which is what the camera has to take in to show
/// the whole of it. The middle of the span rather than the average of the
/// systems, since a route that crosses a crowded region and then a bare one
/// would otherwise be centered on the crowd and hang off the screen at the far
/// end.
///
/// Nothing for a route with nowhere to be. Systems with no position on record
/// are dropped on the way in, and a route of none is not a route.
pub fn framing(places: &[DVec3]) -> Option<(DVec3, f32)> {
    let low = places.iter().copied().reduce(DVec3::min)?;
    let high = places.iter().copied().reduce(DVec3::max)?;

    let middle = (low + high) / 2.;
    let extent =
        places.iter().map(|place| middle.distance(*place)).fold(0., f64::max);

    Some((middle, extent as f32))
}

// TODO: Save another Local<Option<Handle<Mesh>>>?
pub fn spawn_route(
    systems: &[DbSystem],
    route_query: &Query<Entity, With<Route>>,
    galaxy: &Res<Galaxy>,
    grid: &Grid,
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
) {
    // A strip needs two ends to join. A search that found nothing comes back
    // empty, and handing the renderer a mesh with no vertices leaves its slab
    // allocator referring to something that was never allocated:
    //
    //     ERROR bevy_render::slab_allocator: Use-after-free: attempted to
    //     copy element data for an unallocated key
    let points: Vec<DVec3> = systems.iter().filter_map(system_to_vec).collect();
    if points.len() < 2 {
        return;
    }

    // Asked before the line already drawn is taken away, so that a plot that
    // came back with nothing leaves the last one whole. Taken away first, a
    // route that failed would clear the line and leave the filter naming it
    // standing in the bar, dimming the map down to a route with nothing drawn
    // on it.
    for entity in route_query.iter() {
        commands.entity(entity).despawn();
    }

    // Mesh vertices are floats, with no cell to lean on, so a route drawn in
    // galactic coordinates would be quantised to whatever precision is left
    // at that distance from the center. Hanging the line off its own midpoint
    // leaves the vertices holding only how far each end is from that, which
    // is at most the length of the route.
    let midpoint = points.iter().fold(DVec3::ZERO, |sum, p| sum + *p)
        / points.len() as f64;
    let (cell, translation) = grid.translation_to_grid(midpoint);
    let points = points.iter().map(|p| (*p - midpoint).as_vec3()).collect();

    commands.spawn((
        Mesh3d(meshes.add(LineStrip { points })),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1., 1., 1., 0.25),
            alpha_mode: AlphaMode::Blend,
            ..default()
        })),
        cell,
        Transform::from_translation(translation),
        Route,
        ChildOf(galaxy.0),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A route of nowhere is not framed
    #[test]
    fn nothing_has_no_framing() {
        assert_eq!(framing(&[]), None);
    }

    /// One system is framed on itself, reaching nowhere
    #[test]
    fn one_system_reaches_nothing() {
        let only = DVec3::new(3., -4., 5.);

        assert_eq!(framing(&[only]), Some((only, 0.)));
    }

    /// Two are framed on the point between them
    ///
    /// Which is what the camera is asked to look at, and the extent is the
    /// half of the span it has to hold either side.
    #[test]
    fn two_systems_are_framed_on_the_middle() {
        let start = DVec3::new(0., 0., 0.);
        let end = DVec3::new(100., 0., 0.);

        assert_eq!(
            framing(&[start, end]),
            Some((DVec3::new(50., 0., 0.), 50.))
        );
    }

    /// The order they arrive in says nothing about where they are
    #[test]
    fn the_order_of_a_route_does_not_move_it() {
        let places =
            [DVec3::new(10., 0., 0.), DVec3::ZERO, DVec3::new(4., 0., 0.)];
        let mut backwards = places;
        backwards.reverse();

        assert_eq!(framing(&places), framing(&backwards));
    }

    /// The middle is of what the route spans, not of where its systems fall
    ///
    /// A route crowded at one end and bare at the other would otherwise be
    /// centered on the crowd, leaving the far end off the screen.
    #[test]
    fn a_lopsided_route_is_framed_on_its_span() {
        let places = [
            DVec3::ZERO,
            DVec3::new(1., 0., 0.),
            DVec3::new(2., 0., 0.),
            DVec3::new(100., 0., 0.),
        ];

        assert_eq!(framing(&places), Some((DVec3::new(50., 0., 0.), 50.)));
    }

    /// A route that bows is held whole, not only at its ends
    ///
    /// The extent reaches whichever system is furthest from the middle, so a
    /// jump that wanders off the line between the two ends is still on screen.
    #[test]
    fn a_bowed_route_is_held_by_its_furthest_system() {
        let places =
            [DVec3::ZERO, DVec3::new(50., 40., 0.), DVec3::new(100., 0., 0.)];
        let (middle, extent) = framing(&places).unwrap();

        assert_eq!(middle, DVec3::new(50., 20., 0.));
        for place in places {
            assert!(middle.distance(place) as f32 <= extent);
        }
    }
}
