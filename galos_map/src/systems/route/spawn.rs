use super::{Route, system_to_vec};
use crate::space::Galaxy;
use crate::systems::filter::Filter;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use galos_db::systems::System as DbSystem;

/// What a route's line is painted, at `strength` of the full
///
/// White, so a route reads against a sky of colored stars as a thing drawn
/// over it rather than as more of it, and faint even at full strength: the
/// line crosses systems the user is meant to go on seeing.
///
/// The color is left alone and the alpha carries the strength, so that a
/// route held behind another reads as further off rather than as some other
/// kind of route.
pub fn line_color(strength: f32) -> Color {
    Color::srgba(1., 1., 1., 0.25 * strength)
}

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

/// Draw the line for one route
///
/// `route` is which route this is, and the line carries it so that closing
/// that route's row in the bar takes this line and no other.
///
/// Whatever is already drawn is left alone. Several routes stand at once, and
/// a second plotted is asking to see both; the one already there goes when its
/// own row is closed. A route drawn a second time is the same filter, so
/// [`super::follow_filters`] has nothing to say about it and the two lines
/// would sit on top of each other. Asking whether it is already drawn is what
/// keeps that from happening.
// TODO: Save another Local<Option<Handle<Mesh>>>?
#[allow(clippy::too_many_arguments)]
pub fn spawn_route(
    route: &Filter,
    systems: &[DbSystem],
    drawn: &Query<(Entity, &Route)>,
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
    // The address travels with the place. Which of a route's stops are on the
    // map decides what of its line is drawn, and a place alone cannot say
    // which system it is.
    let stops: Vec<(i64, DVec3)> = systems
        .iter()
        .filter_map(|system| {
            system_to_vec(system).map(|at| (system.address, at))
        })
        .collect();
    if stops.len() < 2 {
        return;
    }

    // The same route plotted again is the line already drawn. Nothing here
    // takes lines away, so a second would stand exactly over the first and
    // only one of them would answer to the row.
    if drawn.iter().any(|(_, line)| line.0 == *route) {
        return;
    }

    // Mesh vertices are floats, with no cell to lean on, so a route drawn in
    // galactic coordinates would be quantised to whatever precision is left
    // at that distance from the center. Hanging the line off its own midpoint
    // leaves the vertices holding only how far each end is from that, which
    // is at most the length of the route.
    //
    // In metres from here down, which is what the grid is laid out in and what
    // a vertex is measured in. The systems arrive in light years, as every
    // position the map states does.
    let midpoint = stops.iter().fold(DVec3::ZERO, |sum, (_, at)| sum + *at)
        / stops.len() as f64;
    let (cell, translation) =
        grid.translation_to_grid(crate::space::metres(midpoint));
    let path = super::Path::new(
        stops
            .iter()
            .map(|(address, at)| {
                (*address, crate::space::metres(*at - midpoint).as_vec3())
            })
            .collect(),
    );

    // Whole to begin with. `super::trim` cuts it back to what is on the map
    // on the frame it is drawn, which is before anything is seen of it.
    let points = super::LineList { points: super::legs_whole(&path) };
    commands.spawn((
        Mesh3d(meshes.add(points)),
        // Its own material rather than one shared between the lines, so that
        // holding one route behind another is a write to that route's color.
        // Drawn as the active one, being the route just plotted;
        // [`super::emphasise`] settles it from there.
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: line_color(super::strength(true)),
            alpha_mode: AlphaMode::Blend,
            // Drawn in the color it is set to rather than lit to it, as the
            // orbit lines inside a system are. A line has no surface, and
            // the only light out here is the ambient one, so a lit line comes
            // out at whatever the camera's exposure makes of that: the
            // exposure is set for what a star puts out, and a route was
            // coming back all but black.
            unlit: true,
            ..default()
        })),
        cell,
        Transform::from_translation(translation),
        // Said outright rather than left to `Mesh3d`, which asks only for a
        // transform. Turning a route's row off hides its line, which is a
        // write to this.
        Visibility::default(),
        Route(route.clone()),
        path,
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
