//! What a search has reached, drawn on the map while it is still searching.
//!
//! A route between two systems twenty thousand light years apart expands
//! hundreds of thousands of them, and takes seconds doing it. The spinner on
//! the plot button says the map is working; it says nothing about where the
//! work has got to, and a search that will not finish looks exactly like one
//! that is about to.
//!
//! Three layers, which are the three things worth knowing, and the three
//! things a pathfinding visualiser conventionally shows:
//!
//! - **Where it has been.** The coarse cells the search has expanded in, dim
//!   and only ever growing: the closed set, a region filling. Cell size is a
//!   [`CELLS`]th of the way from the start to the goal, so the picture is
//!   about that many cells along the route whether it is two hundred light
//!   years or twenty-two thousand. Coarse on purpose — a mark per cell at
//!   sixty-four cells along was a lattice dense enough to read as ruling
//!   rather than as a search, and it stood in front of the star field.
//! - **Where it is.** The last few of those same cells, bright and half again
//!   as wide, replacing themselves as the work moves: the leading edge. The
//!   same quantisation as the layer under it, so the edge holds still while
//!   the search grinds through a region and steps when it moves on. A window
//!   of the last jumps taken is what this drew first, and A* pops from all
//!   over its frontier: what that draws is scattered strokes with no relation
//!   to one another, which flicker where they should move.
//! - **How far it has got.** The chain of jumps it has found to the closest
//!   system it has reached, brightest of the three, redrawn as that system
//!   changes. A search that jumps back and starts somewhere else is the
//!   interesting case, and this is the layer that makes it legible instead of
//!   confusing: the chain moves because the search found a way that gets
//!   nearer, which is the thing that was happening all along.
//!
//! None of the three is thinned as it fills. An earlier cut kept one
//! cumulative sample and halved it whenever it filled — which reads as the
//! search restarting, over and over, and is what this replaced.
//!
//! What the search pays for all of it is in [`super::graph::Sampler`]: a
//! counter and a distance per expansion, a cell insert and a jump on one in
//! [`STRIDE`], and a walk back up the search's own parent map when the closest
//! system reached moves. Nothing per neighbour, which is what an earlier cut
//! paid and what put the cost at ninety-eight per cent.

use super::LineList;
use super::graph::Frontier;
use crate::camera::OrbitCamera;
use crate::space::Galaxy;
use crate::systems::labels::world_per_pixel;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use std::sync::Arc;

/// How many cells of the closed set span the route being plotted
///
/// The bound on the dimmest layer, and it is a bound by geometry rather than
/// by count: the cells the search touches are the corridor it searched, and a
/// corridor a couple of cells wide across twenty long is a hundred or so
/// marks however long the route is. Fewer and larger reads; more and smaller
/// is a haze over the sky.
pub(crate) const CELLS: f64 = 20.;

/// One expansion in how many is drawn
///
/// Only the two sampled layers pay this — the closed set and the window — and
/// what they lose is nothing anyone could see: at half a million expansions,
/// one in sixteen still fills every cell of the corridor several times over.
pub(crate) const STRIDE: u64 = 16;

/// How many cells the leading edge holds
///
/// The last cells the work moved through, so the bright set is a few marks
/// stepping along rather than a second region. Few enough to read as a place
/// and not as an area, more than one so that which way it is going can be
/// seen at all.
pub(crate) const EDGE: usize = 8;

/// How many samples are held before the search hands them over
///
/// The whole of what keeps the lock off the hot loop: a batch of these is one
/// lock. Small enough that the map has something to draw within a frame or two
/// of the search starting.
pub(crate) const BATCH: usize = 32;

/// How few pixels across a mark may be drawn
///
/// The floor under the geometry. A mark sized only in light years is a mark
/// nobody can see from the zoom a twenty-thousand light year route is watched
/// at, and one sized only in pixels loses the stipple that says the closed set
/// is cells rather than a wash. See [`across`].
const MARK_PIXELS: f32 = 4.;

/// What the closed set is painted
///
/// The faintest of the three, since it is the largest and the least urgent —
/// where the search has already been. Faint, but not so faint that a single
/// mark against the star field is a guess: this layer is one pass over the
/// picture and the marks do not stack, so what is set here is what is seen.
fn closed_color() -> Color {
    Color::srgba(0.30, 0.55, 0.95, 0.28)
}

/// What the leading edge is painted: brighter and colder, so it reads as the
/// live edge of the search against the haze behind it.
fn edge_color() -> Color {
    Color::srgba(0.65, 0.90, 1., 0.95)
}

/// What the chain to the closest system reached is painted
///
/// Warm, where the other two are cold and a plotted route is white: it is the
/// one layer that is a chain of real jumps, and it must not be mistaken for
/// the answer — the answer is drawn over it in white the moment it lands.
fn reaching_color() -> Color {
    Color::srgba(1., 0.72, 0.30, 0.95)
}

/// The searches running, and what each has drawn
///
/// One per leg: a trip's legs are searched at once and each has its own
/// frontier, so a trip through five stops draws four at once.
#[derive(Resource, Default)]
pub(crate) struct Frontiers(Vec<Watched>);

/// One search being watched, and the three lines drawing it
struct Watched {
    /// What the search is filling in, shared with the task running it
    reached: Arc<Frontier>,
    /// The closed set's marks, the window's jumps, and the chain
    layers: Option<Layers>,
    /// Which revision of the search the meshes were built from, so a frame
    /// with nothing new to show rebuilds nothing
    ///
    /// A revision and not the sizes of what is held: the edge turns over at a
    /// fixed length and the chain moves without changing how many links it
    /// has, so sizes are equal across frames whose pictures are nothing alike.
    shown: u64,
    /// What a pixel was worth when the marks were last sized, so a camera
    /// standing still costs nothing and one zooming re-sizes them
    scaled: f32,
}

/// The three entities one search draws through.
struct Layers {
    closed: Entity,
    edge: Entity,
    reaching: Entity,
}

impl Frontiers {
    /// Watch `reached`, which a search is about to start filling in.
    pub(crate) fn watch(&mut self, reached: Arc<Frontier>) {
        self.0.push(Watched { reached, layers: None, shown: 0, scaled: 0. });
    }

    /// How many systems the searches under way have expanded between them
    ///
    /// What the form says while it waits. One number over every leg, since
    /// what the user asked for was the trip and the legs are how it is being
    /// worked out.
    pub(crate) fn expanded(&self) -> u64 {
        self.0.iter().map(|watched| watched.reached.expanded()).sum()
    }

    /// How close the nearest search has got to what it is looking for, in
    /// light years, where any of them has reached anything.
    pub(crate) fn closest(&self) -> Option<f64> {
        self.0
            .iter()
            .filter_map(|watched| watched.reached.drawn())
            .map(|drawn| drawn.closest)
            .filter(|closest| closest.is_finite())
            .min_by(f64::total_cmp)
    }
}

/// Draw each running search's three layers, and take them down when it
/// finishes
///
/// One lock per search per frame, to copy out what it has reached. Each mesh
/// is rebuilt only where its layer grew or moved, so a search whose window has
/// not turned over this frame costs a lock and three compares.
///
/// The lines hang off the start of the search, in metres from it, as a route's
/// line hangs off its own midpoint: a vertex is an `f32` and the galaxy is
/// wider than one can say. The start is fixed for the whole of a search, where
/// a midpoint over a growing picture would move under the mesh every frame.
pub(crate) fn draw(
    mut frontiers: ResMut<Frontiers>,
    galaxy: Res<Galaxy>,
    grids: Query<&Grid>,
    camera: Query<(&OrbitCamera, &Camera)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let Ok(grid) = grids.get(galaxy.0) else { return };
    // Where the eye is and what a pixel is worth at a depth, for sizing the
    // marks. Absent without a camera — a test, or the frame before one is
    // spawned — and the marks are then sized by the geometry alone.
    let seen = camera.single().ok().and_then(|(orbit, camera)| {
        let viewport = camera.logical_viewport_size()?;
        Some((orbit.eye, camera.clip_from_view().y_axis.y, viewport.y))
    });

    for watched in &mut frontiers.0 {
        // Nothing reached yet: a search that has only just started, or one
        // whose ends could not be resolved at all.
        let Some(drawn) = watched.reached.drawn() else { continue };

        // One depth for the whole picture, taken where the search set out: the
        // corridor is narrow against how far away it is at any zoom the marks
        // need help at, and this is a size on screen rather than a projection.
        let per_pixel = seen.map_or(0., |(eye, cot_half_fov, height)| {
            let away = crate::space::metres(eye - drawn.from).length() as f32;
            world_per_pixel(cot_half_fov, height, away.max(1.))
        });
        // A tenth, so a drag that changes nothing anyone can see rebuilds
        // nothing, and a zoom that crosses a mark's width does.
        let resized = (per_pixel - watched.scaled).abs() > watched.scaled * 0.1;
        if drawn.revision == watched.shown && !resized {
            continue;
        }

        let layers = match &watched.layers {
            Some(layers) => layers,
            None => {
                let (cell, translation) =
                    grid.translation_to_grid(crate::space::metres(drawn.from));
                let layer = |color: Color,
                             commands: &mut Commands,
                             materials: &mut Assets<StandardMaterial>| {
                    commands
                        .spawn((
                            Mesh3d::default(),
                            MeshMaterial3d(materials.add(StandardMaterial {
                                base_color: color,
                                alpha_mode: AlphaMode::Blend,
                                // Drawn in the colour it is set to rather than
                                // lit to it, as a route's line is: a line has
                                // no surface, and the exposure out here is set
                                // for what a star puts out.
                                unlit: true,
                                ..default()
                            })),
                            cell,
                            Transform::from_translation(translation),
                            Visibility::default(),
                            ChildOf(galaxy.0),
                        ))
                        .id()
                };
                watched.layers = Some(Layers {
                    closed: layer(
                        closed_color(),
                        &mut commands,
                        &mut materials,
                    ),
                    edge: layer(edge_color(), &mut commands, &mut materials),
                    reaching: layer(
                        reaching_color(),
                        &mut commands,
                        &mut materials,
                    ),
                });
                watched.layers.as_ref().expect("the layers just spawned")
            }
        };

        // Each layer's own points, in metres from where the search set out.
        let here = |at: DVec3| crate::space::metres(at - drawn.from).as_vec3();
        let across = across(drawn.across, per_pixel);
        let mut points = Vec::with_capacity(drawn.cells.len() * 4);
        for at in &drawn.cells {
            points.extend(mark(here(*at), across));
        }
        put(&mut commands, &mut meshes, layers.closed, points);
        // Half again as wide as the marks behind them, so the edge is the
        // same shape in the same places and reads as the front of one picture
        // rather than as a second one laid over it.
        let mut points = Vec::with_capacity(drawn.edge.len() * 4);
        for at in &drawn.edge {
            points.extend(mark(here(*at), across * 1.5));
        }
        put(&mut commands, &mut meshes, layers.edge, points);
        // A chain of jumps rather than a list of places, so it is drawn as the
        // legs between them: what `super::legs` does for a route.
        let places: Vec<Vec3> =
            drawn.reaching.iter().map(|at| here(*at)).collect();
        let points = places
            .windows(2)
            .flat_map(|leg| [leg[0], leg[1]])
            .collect::<Vec<Vec3>>();
        put(&mut commands, &mut meshes, layers.reaching, points);
        watched.shown = drawn.revision;
        watched.scaled = per_pixel;
    }

    // A search that has finished has had its answer drawn as a route, or has
    // come back with nothing; either way what it reached on the way is not
    // what the map is for.
    frontiers.0.retain(|watched| {
        if !watched.reached.finished() {
            return true;
        }
        if let Some(layers) = &watched.layers {
            for entity in [layers.closed, layers.edge, layers.reaching] {
                commands.entity(entity).despawn();
            }
        }
        false
    });
}

/// Hang `points` on `entity` as its line mesh
///
/// A mesh of no vertices leaves the renderer's slab allocator holding a key
/// that was never allocated, and it says so every frame, so a layer with
/// nothing in it is drawn as a line of no length instead.
fn put(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    entity: Entity,
    mut points: Vec<Vec3>,
) {
    if points.len() < 2 {
        points = vec![Vec3::ZERO, Vec3::ZERO];
    }
    commands.entity(entity).insert(Mesh3d(meshes.add(LineList { points })));
}

/// How wide a mark is drawn, in metres, for a cell `cell` light years across
///
/// The greater of a fraction of the cell and [`MARK_PIXELS`] pixels. The
/// fraction is what keeps neighbouring marks apart, so the closed set reads as
/// cells filling in rather than as a wash; the pixels are what keep a mark
/// visible when the whole picture is a thumbnail — a search two hundred light
/// years long, seen from outside the galaxy, is a picture whose every mark
/// would otherwise be a fraction of a pixel across.
///
/// In metres because that is what a vertex is in. Light years are what the
/// search and the cells are said in, and the conversion is the whole of the
/// difference between a mark and nothing at all.
fn across(cell: f64, per_pixel: f32) -> f32 {
    let stipple = crate::space::metres(DVec3::X * cell * 0.12).x as f32;
    stipple.max(MARK_PIXELS * per_pixel)
}

/// A cell of the closed set, as two segments crossing at its middle
///
/// Diagonal rather than along the axes, and two segments rather than three.
/// Along the axes it is the same shape the map's own grid is ruled with
/// ([`crate::grid`]), and a search drawn in the ruling's shape reads as more
/// ruling — which is what the first cut of this looked like. Turned off the
/// axes it reads as a tick at a place from every angle instead, and at four
/// vertices against six it costs less to say it.
///
/// `across` is in metres, as the places are.
fn mark(at: Vec3, across: f32) -> [Vec3; 4] {
    // Unit diagonals, so a mark is the size it is asked for however it is
    // turned.
    let a = Vec3::new(1., 1., 1.).normalize() * across;
    let b = Vec3::new(1., -1., -1.).normalize() * across;
    [at - a, at + a, at - b, at + b]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::route::graph::{Frontier, UNSEEN};

    /// A place `along` light years down the x axis
    fn at(along: f64) -> DVec3 {
        DVec3::new(along, 0., 0.)
    }

    /// A mark is measured in the same units as the places it stands at
    ///
    /// Which is metres, the vertices being metres from where the search set
    /// out. Sizing one in light years instead draws every cross a hundred
    /// metres wide — a ten-thousand-billionth of a light year, and so an
    /// entire layer that renders and cannot be seen at any zoom.
    #[test]
    fn a_mark_is_drawn_in_metres_like_the_places_it_marks() {
        let ly = |metres: f32| metres as f64 / crate::space::LIGHT_YEAR;

        // Near enough that a pixel is a thousandth of a light year: the mark
        // is the fraction of the cell, which is what stipples the region.
        let close = across(100., (crate::space::LIGHT_YEAR * 1e-3) as f32);
        // A fraction of the cell: enough of it to be a mark, not so much
        // that neighbouring marks meet. What fraction is taste; that it is
        // light years and not metres is not.
        assert!(
            (1. ..50.).contains(&ly(close)),
            "a cell a hundred light years across marked {} ly wide",
            ly(close)
        );

        // Far enough out that a pixel is a hundred light years, where the
        // fraction of a cell is a fraction of a pixel: the floor takes over.
        let far = across(100., (crate::space::LIGHT_YEAR * 100.) as f32);
        assert!(
            ly(far) >= 100. * MARK_PIXELS as f64,
            "a mark {} ly wide is under a pixel from out here",
            ly(far)
        );
    }

    /// The closed set only grows, and the edge steps along it
    ///
    /// Which is the whole of what the earlier cuts got wrong: a picture that
    /// thinned as it filled read as the search starting again, and one whose
    /// bright layer was scattered jumps read as nothing at all. The cells are
    /// the region searched and never shrink; the edge is the last few of them
    /// and never grows past its own length.
    #[test]
    fn the_closed_set_grows_and_the_edge_steps_along() {
        let frontier = Frontier::between(DVec3::ZERO, at(6400.));
        let mut sampler = frontier.sampler();
        // Ten systems in a chain, each reached from the one before it.
        let came: Vec<u32> =
            (0..10).map(|i| if i == 0 { UNSEEN } else { i - 1 }).collect();

        let mut cells = 0;
        for step in 0..(STRIDE * BATCH as u64 * 40) {
            let here = at(step as f64);
            sampler.expanded(
                (step % 10) as usize,
                here,
                at(6400.),
                &came,
                |i| at(i as f64 * 100.),
            );
            if let Some(drawn) = frontier.drawn() {
                assert!(
                    drawn.cells.len() >= cells,
                    "the closed set shrank from {cells} to {}",
                    drawn.cells.len()
                );
                cells = drawn.cells.len();
                assert!(
                    drawn.edge.len() <= EDGE,
                    "the edge held {} cells",
                    drawn.edge.len()
                );
            }
        }

        let drawn = frontier.drawn().expect("something reached");
        assert!(cells > 1, "the closed set never grew past {cells} cells");
        assert_eq!(drawn.edge.len(), EDGE, "the edge should be full by now");
        assert!(
            drawn.edge.len() < drawn.cells.len(),
            "the edge is the front of the region, not the whole of it"
        );
        // A cell is a twentieth of the way to the goal, so the marks stand
        // three hundred and twenty light years apart over a route of six
        // thousand four hundred.
        assert_eq!(drawn.across, 320.);
    }

    /// The chain is a real run of jumps to the closest system reached
    ///
    /// Every link a neighbour of the last and the first the system the search
    /// set out from, so what is drawn is a way there rather than a line
    /// through space. It moves when the search gets nearer, which is the
    /// answer to "is this getting anywhere".
    #[test]
    fn the_chain_runs_from_the_start_to_the_closest_reached() {
        let goal = at(400.);
        let frontier = Frontier::between(DVec3::ZERO, goal);
        let mut sampler = frontier.sampler();
        let places = [at(0.), at(100.), at(200.), at(300.)];
        let place = |i: usize| places[i];
        // The search's own record: 1 was reached from 0, 2 from 1, 3 from 2.
        let came: Vec<u32> = vec![UNSEEN, 0, 1, 2];

        // Expanded in that order, the last of them the closest to the goal.
        for node in [0usize, 1, 2] {
            sampler.expanded(node, places[node], goal, &came, place);
        }
        sampler.done();

        let drawn = frontier.drawn().expect("something reached");
        assert_eq!(
            drawn.reaching,
            vec![at(0.), at(100.), at(200.)],
            "the chain to the closest system expanded"
        );
        assert_eq!(
            drawn.closest, 200.,
            "and how far that still is from the goal"
        );
    }

    /// A running search is drawn, and what it drew goes when it finishes
    ///
    /// Three layers up while it runs and none after: the answer is drawn as a
    /// route by then, and what the search touched on the way is not what the
    /// map is for.
    #[test]
    fn a_search_is_drawn_while_it_runs_and_taken_down_after() {
        use big_space::prelude::BigSpace;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.init_resource::<Frontiers>();
        let galaxy = app
            .world_mut()
            .spawn((BigSpace::default(), crate::space::galaxy_grid()))
            .id();
        app.insert_resource(Galaxy(galaxy));
        app.add_systems(Update, draw);

        let goal = at(6400.);
        let frontier = Frontier::between(DVec3::ZERO, goal);
        app.world_mut()
            .resource_mut::<Frontiers>()
            .watch(Arc::clone(&frontier));
        let mut sampler = frontier.sampler();
        let came: Vec<u32> =
            (0..10).map(|i| if i == 0 { UNSEEN } else { i - 1 }).collect();

        for step in 0..(STRIDE * BATCH as u64 * 2) {
            let here = at(step as f64);
            sampler.expanded((step % 10) as usize, here, goal, &came, |i| {
                at(i as f64 * 100.)
            });
        }
        app.update();
        assert_eq!(lines(&mut app), 3, "the three layers of one search");

        app.update();
        assert_eq!(lines(&mut app), 3, "three layers, not three a frame");

        sampler.done();
        app.update();
        assert_eq!(lines(&mut app), 0, "a finished search left its lines up");
        assert!(
            app.world().resource::<Frontiers>().0.is_empty(),
            "and is still being watched"
        );
    }

    /// How many frontier lines the map holds
    fn lines(app: &mut App) -> usize {
        app.world_mut().query::<&Mesh3d>().iter(app.world()).count()
    }
}
