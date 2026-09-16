use super::Route;
use crate::space::Galaxy;
use crate::systems::System;
use crate::systems::filter::Filter;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;

/// What a route's line is painted, at `strength` of the full
///
/// White, and faint even at full strength: the line crosses systems the user
/// is meant to go on seeing. What colour each *jump* of it is drawn is
/// [`jump_color`]'s, carried per vertex and multiplied by this — so this is
/// the fade and that is the reading.
///
/// The hue is left alone here and the alpha carries the strength, so that a
/// route held behind another reads as further off rather than as some other
/// kind of route.
pub fn line_color(strength: f32) -> Color {
    Color::srgba(1., 1., 1., FAINT * strength)
}

/// How faint a route's line is drawn at full strength
///
/// The alpha every jump is multiplied into, and the ceiling on how bright
/// any of them can be: a route crosses systems that are meant to go on
/// being seen. A charged jump takes all of it and an ordinary jump half,
/// which is [`jump_color`]'s business — so this is twice the quarter the
/// whole line used to be drawn at, and an ordinary jump still comes out at
/// exactly that quarter.
///
/// The headroom is what lets the two be told apart by *light* as well as by
/// hue. A saturated blue carries about a third of white's luminance at the
/// same alpha, so a blue drawn at the same faintness reads as a dimmer
/// white rather than as blue; drawn at twice the alpha it comes out at
/// about the same brightness, and the difference left is the one worth
/// seeing.
const FAINT: f32 = 0.5;

/// What one jump of a route is painted, by whether it was flown on a cone
///
/// **Blue where the ship charged off a jet cone**, white where it jumped
/// unaided. The thing about a drawn route a reader cannot otherwise see:
/// two routes between the same two systems look alike and are not — one is
/// a hundred and forty jumps and the other four hundred and fifty-eight —
/// and *within* one route the boosted stretches are where the distance went.
///
/// **Told apart twice over: by hue and by alpha.** A gentle blue at the
/// faintness a route is drawn at was next to unreadable against the white
/// beside it — reported as lines that could hardly be told apart — and the
/// reason is that blending toward black takes a faint line's colour before
/// it takes its light. So the blue keeps almost none of its red, and a
/// charged jump is drawn at the whole of [`FAINT`] where an ordinary one is
/// drawn at half. That extra alpha only buys back what the hue costs: the
/// two come out at about the same light, differing in colour alone.
///
/// The alpha here is a share of [`line_color`]'s, the two being multiplied,
/// so a route held behind another dims whole and keeps its reading.
pub fn jump_color(charged: bool) -> [f32; 4] {
    match charged {
        true => [0.12, 0.55, 1., 1.],
        // A quarter once [`FAINT`] has multiplied it, which is what the
        // whole line was drawn at before any of this.
        false => [1., 1., 1., 0.5],
    }
}

/// Which jumps of a route could only have been flown on a cone
///
/// One flag a jump, from the places alone: a jump longer than the ship
/// reaches unaided **is** a supercharged jump, there being no other way to
/// cross it. That makes this a fact about the jump rather than about the
/// ask — a route plotted for a supercharging drive is mostly ordinary
/// jumps, and colouring the whole line blue for it would say the ship flew
/// a cone at every stop.
///
/// The other direction is not decidable here and is not claimed: a jump
/// *inside* the ship's plain range may still have set out from a cone, the
/// charge going to waste. What the line says is "this one needed the cone",
/// which is the reading that answers where a route's length came from.
///
/// `range` is what the ship reaches unaided, in light years, as it was
/// asked for. Nothing charged where it will not parse — a route whose range
/// is not a number is one the map cannot say anything about.
pub fn charged(stops: &[DVec3], range: Option<f64>) -> Vec<bool> {
    let jumps = stops.len().saturating_sub(1);
    let Some(range) = range.filter(|range| *range > 0.) else {
        return vec![false; jumps];
    };

    stops
        .windows(2)
        .map(|leg| {
            // A shade over, so a jump flown at exactly the ship's range is
            // an ordinary jump: the router's own test admits it, and float
            // arithmetic on light years is not exact.
            leg[0].distance(leg[1]) > range * 1.000_01
        })
        .collect()
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
    systems: &[System],
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
        .map(|system| (system.address, system.position()))
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
    // Which jumps needed a cone, settled here off the places and the range
    // the route was asked for — in light years, before any of this is
    // turned into metres from the midpoint. A fact about the route, so it
    // is settled once; see [`charged`].
    let flown = charged(
        &stops.iter().map(|(_, at)| *at).collect::<Vec<DVec3>>(),
        route.range().and_then(|range| range.parse::<f64>().ok()),
    );
    let path = super::Path::new(
        stops
            .iter()
            .map(|(address, at)| {
                (*address, crate::space::metres(*at - midpoint).as_vec3())
            })
            .collect(),
        flown.clone(),
    );

    // Whole to begin with, every stop taken as drawn. `super::trim` cuts it
    // back to what is on the map on the frame it is spawned, which is before
    // anything is seen of it.
    let whole = path.whole();
    let shown = vec![true; whole.len()];
    commands.spawn((
        // Whole, the spawn taking every stop as drawn, so no dash is
        // wanted yet: `super::trim` cuts it to the map and to the view
        // before the frame is presented.
        Mesh3d(meshes.add(super::legs(&whole, &shown, &flown, 0.))),
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

    /// A jump that needed a cone is drawn blue, and an ordinary one white
    ///
    /// Per jump, not per route: a route plotted for a supercharging drive is
    /// mostly ordinary jumps, and one colour over the whole line would say
    /// the ship flew a cone at every stop.
    #[test]
    fn a_charged_jump_is_drawn_blue() {
        let hue = |color: [f32; 4]| (color[0], color[1], color[2]);
        let (charged, plain) = (jump_color(true), jump_color(false));

        let (red, green, blue) = hue(charged);
        assert!(blue > green && green > red, "{charged:?} is not blue");
        assert_eq!(hue(plain), (1., 1., 1.), "an ordinary jump left white");

        // Told apart twice over: the charged jump is bluer *and* drawn at
        // more alpha, a faint line losing its hue to the black behind it
        // before it loses its light.
        assert!(
            charged[3] > plain[3],
            "a charged jump is not drawn at the more of the two",
        );
        assert!(red < 0.25, "the blue kept too much red to read as blue");
        assert!(line_color(0.5).alpha() < line_color(1.).alpha());

        // And the extra alpha only buys back what the hue costs: a
        // saturated blue carries about a third of white's luminance, so the
        // two come out at roughly the same light and the difference left is
        // the one worth seeing.
        let light = |color: [f32; 4]| {
            (0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2])
                * color[3]
        };
        let (lit, white) = (light(charged), light(plain));
        assert!(
            (lit - white).abs() < white * 0.35,
            "a charged jump carries {lit} of light against {white}",
        );

        // And an ordinary jump comes out where the whole line used to:
        // a quarter, once the material's own faintness has multiplied it.
        assert!(
            (line_color(1.).alpha() * plain[3] - 0.25).abs() < 0.01,
            "an ordinary jump moved: {}",
            line_color(1.).alpha() * plain[3],
        );
    }

    /// Which jumps needed a cone is read off the places and the range
    ///
    /// A jump longer than the ship reaches unaided is a supercharged jump,
    /// there being no other way across it. The other direction is not
    /// claimed: a short jump may still have set out from a cone.
    #[test]
    fn a_jump_longer_than_the_ship_reaches_was_charged() {
        let stops = [
            DVec3::ZERO,
            DVec3::new(40., 0., 0.),
            DVec3::new(240., 0., 0.),
            DVec3::new(280., 0., 0.),
        ];

        assert_eq!(
            charged(&stops, Some(50.)),
            vec![false, true, false],
            "the 200 Ly jump is the only one a 50 Ly ship could not make",
        );

        // A jump at exactly the range is an ordinary jump, the router's own
        // test admitting it.
        assert_eq!(charged(&[stops[0], stops[1]], Some(40.)), vec![false]);

        // And nothing is claimed where there is no range to compare with.
        assert_eq!(charged(&stops, None), vec![false; 3]);
        assert!(charged(&stops[..1], Some(50.)).is_empty());
    }
}
