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
//! What the search pays for all of it is in [`crate::map::route::graph::Sampler`]: a
//! counter and a distance per expansion, a cell insert and a jump on one in
//! [`STRIDE`], and a walk back up the search's own parent map when the closest
//! system reached moves. Nothing per neighbour, which is what an earlier cut
//! paid and what put the cost at ninety-eight per cent.

use crate::map::camera::OrbitCamera;
use crate::map::galaxy::fetch::FetchIndex;
use crate::map::route::LineList;
use crate::map::route::graph::{Drawn, Frontier};
use crate::map::screen::world_per_pixel;
use crate::map::space::Galaxy;
use bevy::math::DVec3;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use big_space::prelude::*;
use std::sync::Arc;
use std::time::Duration;

/// How many cells of the closed set span the route being plotted
///
/// The bound on the dimmest layer, and it is a bound by geometry rather than
/// by count: the cells the search touches are the corridor it searched, and a
/// corridor a couple of cells wide across twenty long is a hundred or so
/// marks however long the route is. Fewer and larger reads; more and smaller
/// is a haze over the sky.
pub(crate) const CELLS: f64 = 20.;

/// The most cells the closed set holds before it is drawn coarser
///
/// [`CELLS`] bounds the layer by geometry, which holds while the search stays
/// in a corridor — and it does whenever there is a route to find. There is
/// not always: a leg to a system unreachable at the range asked expands the
/// whole component it can reach, in every direction, and a corridor's worth
/// of cells becomes a region's. So there is a count as well as a geometry,
/// and passing it doubles the cell rather than dropping anything: the picture
/// goes coarser, which is what it should do when a search has stopped being
/// a line and become a volume, and it stays a picture of everywhere the
/// search has been.
///
/// Well clear of what the geometry asks for — a corridor two cells wide by
/// twenty long is a hundred or so — so an ordinary route never reaches it and
/// is drawn exactly as [`CELLS`] says.
pub(crate) const CELL_CEILING: usize = 4096;

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

/// What the stretch still to be crossed is painted
///
/// Cold and faint, and the one layer that is not a route at all: it is the
/// straight line from the furthest the search has got to where it is going,
/// which is the part nothing has planned yet.
///
/// Drawn because without it the picture lies. A search's chain ends at the
/// closest system it has reached, and on a galactic plot that is hundreds
/// of light years short of the goal — a gap of a few hundred light years
/// over twenty-two thousand is sub-pixel, so a route still being worked out
/// read as a finished one, and the wait after it looked like the map doing
/// nothing. Reported exactly that way, and only visible on zooming in.
///
/// Only while there is no coarse plan: once there is one it spans the whole
/// route itself ([`planned_color`]) and a line to the goal would be drawn
/// twice.
fn left_color() -> Color {
    Color::srgba(0.55, 0.60, 0.72, 0.30)
}

/// What the coarse plan is painted
///
/// Deeper and much fainter than the branches drawn over it, so the two read
/// as a promise and the keeping of it. They were one colour to begin with
/// and that was a picture of nothing: most of a plan's legs are a single
/// supercharged jump, so their branch lies exactly along the hop it refines
/// and the ones that do deviate are a few hundred light years out of
/// twenty-two thousand. Same line, same colour, no fork to see.
fn planned_color() -> Color {
    Color::srgba(0.85, 0.45, 0.12, 0.40)
}

/// The stretch still to be crossed, as a chain of two places
///
/// From the furthest anything has reached to the goal. Empty where there is
/// nothing to say: a coarse plan already spans the whole route, a search
/// that has reached nothing has no tip to draw from, and a tip standing on
/// the goal has arrived.
///
/// The tip is the end of the last strand, the strands being the legs flown
/// in order and then the leg under way — so on a flat search it is the
/// closest system reached, and on a plan being searched it is the last cone
/// the coarse graph got to.
fn left(drawn: &Drawn) -> Vec<DVec3> {
    if !drawn.plan.is_empty() {
        return Vec::new();
    }
    let Some(tip) =
        drawn.reaching.iter().rev().find_map(|strand| strand.last())
    else {
        return Vec::new();
    };
    match *tip == drawn.goal {
        true => Vec::new(),
        false => vec![*tip, drawn.goal],
    }
}

/// One chain of places as the jumps between them
///
/// Line segments and not a strip, which is what lets several chains share a
/// layer: a plan and the branches refining it are separate runs, and a strip
/// would join the end of one to the start of the next.
fn jumps(chain: &[DVec3], here: impl Fn(DVec3) -> Vec3) -> Vec<Vec3> {
    let places: Vec<Vec3> = chain.iter().map(|at| here(*at)).collect();
    places.windows(2).flat_map(|leg| [leg[0], leg[1]]).collect()
}

/// The searches running, and what each has drawn
///
/// One per leg: a trip's legs are searched at once and each has its own
/// frontier, so a trip through five stops draws four at once.
#[derive(Resource, Default)]
pub(crate) struct Frontiers(Vec<Watched>);

/// One search being watched, and the three lines drawing it
struct Watched {
    /// Which leg of which trip this is watching, so a leg given up on can be
    /// told apart from one still being searched
    leg: FetchIndex,
    /// What the search is filling in, shared with the task running it
    reached: Arc<Frontier>,
    /// When it was handed to the pool, so the form can say how long it has
    /// been at it
    ///
    /// Taken here rather than off the task's own start: what a user waiting
    /// on a plot is timing is the click, and a leg may sit in the pool's
    /// queue before a thread picks it up.
    asked: Instant,
    /// The closed set's marks, the window's jumps, and the chain
    layers: Option<Layers>,
    /// Where the search set out, once its ends resolved
    ///
    /// Kept because the depth a mark is sized at is measured from here, and
    /// that has to be known before a revision can be weighed against a zoom
    /// that has moved — before, that is, there is any reason to take a copy.
    from: Option<DVec3>,
    /// Which revision of the search the meshes were built from, so a frame
    /// with nothing new to show rebuilds nothing
    ///
    /// A revision and not the sizes of what is held: the edge turns over at a
    /// fixed length and the chain moves without changing how many links it
    /// has, so sizes are equal across frames whose pictures are nothing alike.
    shown: u64,
    /// Which revision of each layer its own mesh was built from, so a flush
    /// that only added a cell does not re-upload the chain
    layered: (u64, u64, u64),
    /// What a pixel was worth when the marks were last sized, so a camera
    /// standing still costs nothing and one zooming re-sizes them
    scaled: f32,
}

/// The five entities one search draws through.
struct Layers {
    closed: Entity,
    edge: Entity,
    /// The coarse plan, where there was one to draw
    planned: Entity,
    /// A branch per leg flown, and the leg under way
    reaching: Entity,
    /// The straight line from the furthest reached to the goal
    left: Entity,
}

impl Frontiers {
    /// Watch `reached`, which a search over `leg` is about to start filling in.
    pub(crate) fn watch(
        &mut self,
        leg: FetchIndex,
        reached: Arc<Frontier>,
        asked: Instant,
    ) {
        self.0.push(Watched {
            leg,
            reached,
            asked,
            layers: None,
            from: None,
            shown: 0,
            layered: (0, 0, 0),
            scaled: 0.,
        });
    }

    /// Give up on every watched search whose leg is not one of `legs`
    ///
    /// Two callers, and one rule between them. A new ask abandons whatever
    /// the form has moved off ([`crate::map::route::fetch::fetch_route`]), and the stop
    /// gesture abandons the lot by keeping nothing
    /// ([`crate::map::route::fetch::stop_routes`]).
    ///
    /// Dropping the task means nothing will read the answer; this is what
    /// stops the work and takes the picture down — a search the pool has
    /// begun reads [`Frontier::stopped`] and gives up, and one it never
    /// began would otherwise have left its layers standing for the rest of
    /// the session. [`draw`] takes them down on the next frame, as it does
    /// for a search that ended of its own accord.
    pub(crate) fn abandon_others(&self, legs: &[FetchIndex]) {
        for watched in &self.0 {
            if !legs.contains(&watched.leg) {
                watched.reached.abandon();
            }
        }
    }

    /// Give up on the search over `leg`, where one is being watched
    ///
    /// The one-leg form of [`Self::abandon_others`], for a leg being asked
    /// again from nothing ([`crate::map::route::fetch::replot`]): what that ask replaces
    /// is that leg alone, the rest of a trip going on as it was. The picture
    /// it had drawn comes down on the next frame, as [`draw`] takes down
    /// every search that has finished.
    pub(crate) fn abandon(&self, leg: &FetchIndex) {
        for watched in self.0.iter().filter(|watched| &watched.leg == leg) {
            watched.reached.abandon();
        }
    }

    /// How many systems the searches under way have expanded between them
    ///
    /// What the form says while it waits. One number over every leg, since
    /// what the user asked for was the trip and the legs are how it is being
    /// worked out.
    pub(crate) fn expanded(&self) -> u64 {
        self.0.iter().map(|watched| watched.reached.expanded()).sum()
    }

    /// How long the searches under way have been at it
    ///
    /// The longest of them, which for a trip is the whole wait: the legs are
    /// searched at once, so the trip is not plotted until the slowest is.
    /// Counted from the click and not from the first expansion, that being
    /// what a user waiting on it is timing.
    ///
    /// A search that has finished still counts until [`draw`] takes its
    /// layers down, a frame later; it is done before the form is, the answer
    /// having to land and be drawn.
    pub(crate) fn asked_for(&self) -> Option<Duration> {
        self.0.iter().map(|watched| watched.asked.elapsed()).max()
    }

    /// How far the whole plot still has to find, in light years
    ///
    /// **Summed over the legs still running, not the nearest of them.** Each
    /// leg closes on its own end, so the *smallest* of those distances is
    /// whichever leg happens to be nearly done — which on a trip through
    /// three stops reads as almost arrived while two legs have twenty
    /// thousand light years between them, and jumps about as each lands.
    /// Added up it is one number about the trip: it falls as every leg
    /// closes, and falls again as a leg finishes and leaves the set.
    ///
    /// [`None`] until something has been reached. A leg whose search has
    /// not closed on anything yet holds an infinite distance, and one
    /// infinity would swallow the sum, so those are left out — the count
    /// [`Self::legs`] is what says the number is not the whole story yet.
    pub(crate) fn left(&self) -> Option<f64> {
        let mut left = None;
        for watched in &self.0 {
            let Some(drawn) = watched.reached.drawn() else { continue };
            if drawn.closest.is_finite() {
                left = Some(left.unwrap_or(0.) + drawn.closest);
            }
        }
        left
    }

    /// How many searches are under way, which for a trip is how many of its
    /// legs are still being worked out.
    pub(crate) fn legs(&self) -> usize {
        self.0.len()
    }

    /// How many legs of `trip` are still being searched
    ///
    /// What a panel about a trip has to know before it describes one: the
    /// legs land one at a time, so a trip read halfway through is a real
    /// route through some of its stops and *not* the thing the panel is
    /// titled after. Counted off the leg each search is watching, which
    /// carries the trip it belongs to.
    pub(crate) fn plotting(&self, trip: &str) -> usize {
        self.0
            .iter()
            .filter(|watched| match &watched.leg {
                FetchIndex::Route(.., under, _, _, _) => {
                    under.as_deref() == Some(trip)
                }
                _ => false,
            })
            .count()
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
        Some((orbit.eye(), camera.clip_from_view().y_axis.y, viewport.y))
    });

    for watched in &mut frontiers.0 {
        // Where it set out, taken once. Absent means a search whose ends could
        // not be resolved at all: there is nothing to draw and nowhere to draw
        // it.
        let from = match watched.from {
            Some(from) => from,
            None => match watched.reached.from() {
                Some(from) => *watched.from.insert(from),
                None => continue,
            },
        };

        // One depth for the whole picture, taken where the search set out: the
        // corridor is narrow against how far away it is at any zoom the marks
        // need help at, and this is a size on screen rather than a projection.
        let per_pixel = seen.map_or(0., |(eye, cot_half_fov, height)| {
            let away = crate::map::space::metres(eye - from).length() as f32;
            world_per_pixel(cot_half_fov, height, away.max(1.))
        });
        // A tenth, so a drag that changes nothing anyone can see rebuilds
        // nothing, and a zoom that crosses a mark's width does.
        let resized = (per_pixel - watched.scaled).abs() > watched.scaled * 0.1;
        // The one number, and the one lock, a frame with nothing to show
        // costs. Asked before [`Frontier::drawn`] because that walks every
        // cell of the closed set and clones the chain, under the lock the
        // search flushes through.
        if watched.reached.revision() == watched.shown && !resized {
            continue;
        }
        let Some(drawn) = watched.reached.drawn() else { continue };

        let layers = match &watched.layers {
            Some(layers) => layers,
            None => {
                let (cell, translation) = grid
                    .translation_to_grid(crate::map::space::metres(drawn.from));
                let layer = |color: Color,
                             commands: &mut Commands,
                             materials: &mut Assets<StandardMaterial>| {
                    commands
                        .spawn((
                            Mesh3d::default(),
                            MeshMaterial3d(materials.add(StandardMaterial {
                                base_color: color,
                                alpha_mode: AlphaMode::Blend,
                                // Drawn in the color it is set to rather than
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
                    planned: layer(
                        planned_color(),
                        &mut commands,
                        &mut materials,
                    ),
                    reaching: layer(
                        reaching_color(),
                        &mut commands,
                        &mut materials,
                    ),
                    left: layer(left_color(), &mut commands, &mut materials),
                });
                watched.layers.as_ref().expect("the layers just spawned")
            }
        };

        // Each layer's own points, in metres from where the search set out.
        // Rebuilt where that layer's own revision moved, or where a zoom has
        // changed what a mark is worth in pixels — which moves every vertex of
        // both sampled layers and none of the chain's.
        let here = |at: DVec3| crate::map::space::metres(at - from).as_vec3();
        let across = across(drawn.across, per_pixel);
        let (closed_at, edge_at, reaching_at) = watched.layered;
        if resized || drawn.cells_at != closed_at {
            let mut points = Vec::with_capacity(drawn.cells.len() * 4);
            for at in &drawn.cells {
                points.extend(mark(here(*at), across));
            }
            put(&mut commands, &mut meshes, layers.closed, points);
        }
        // Half again as wide as the marks behind them, so the edge is the
        // same shape in the same places and reads as the front of one picture
        // rather than as a second one laid over it.
        if resized || drawn.edge_at != edge_at {
            let mut points = Vec::with_capacity(drawn.edge.len() * 4);
            for at in &drawn.edge {
                points.extend(mark(here(*at), across * 1.5));
            }
            put(&mut commands, &mut meshes, layers.edge, points);
        }
        // Chains of jumps rather than lists of places, so each is drawn as
        // the legs between them: what `crate::map::route::legs` does for a route. Both
        // of these move together — a leg flown is a branch gained — so one
        // revision covers the pair.
        if drawn.reaching_at != reaching_at {
            put(
                &mut commands,
                &mut meshes,
                layers.planned,
                jumps(&drawn.plan, here),
            );
            // A branch apiece: the legs flown, and the leg under way. Drawn
            // over the plan and brighter, so a refinement that deviates from
            // the hop it was promised shows as the fork it is.
            let mut points = Vec::new();
            for strand in &drawn.reaching {
                points.extend(jumps(strand, here));
            }
            put(&mut commands, &mut meshes, layers.reaching, points);
            // And what nothing has reached yet: the straight line from the
            // furthest the search has got to where it is going. Without it
            // a chain that stops a few hundred light years short of the
            // goal reads as a finished route, that gap being sub-pixel on a
            // galactic plot. Nothing where a plan is drawn — the plan spans
            // the whole route itself — and nothing once the tip is the
            // goal.
            put(
                &mut commands,
                &mut meshes,
                layers.left,
                jumps(&left(&drawn), here),
            );
        }
        watched.shown = drawn.revision;
        watched.layered = (drawn.cells_at, drawn.edge_at, drawn.reaching_at);
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
            for entity in [
                layers.closed,
                layers.edge,
                layers.planned,
                layers.reaching,
                layers.left,
            ] {
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
    commands.entity(entity).insert(Mesh3d(meshes.add(LineList::plain(points))));
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
    let stipple = crate::map::space::metres(DVec3::X * cell * 0.12).x as f32;
    stipple.max(MARK_PIXELS * per_pixel)
}

/// A cell of the closed set, as two segments crossing at its middle
///
/// Diagonal rather than along the axes, and two segments rather than three.
/// Along the axes it is the same shape the map's own grid is ruled with
/// ([`crate::map::grid`]), and a search drawn in the ruling's shape reads as more
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
    use crate::map::route::graph::Frontier;
    use galos_index::{CellId, Node};
    use rustc_hash::FxHashMap;

    /// A place `along` light years down the x axis
    fn at(along: f64) -> DVec3 {
        DVec3::new(along, 0., 0.)
    }

    /// The `n`th of a run of distinct nodes
    ///
    /// Handed straight to the sampler, which never looks one up: it hashes
    /// the node, follows the search's parent map with it and asks the
    /// caller's closure where it sits. So these need a galaxy behind them
    /// no more than the search's own bookkeeping does — what they must be
    /// is distinct, which one cell and an offset apiece makes them.
    fn node(n: u32) -> Node {
        Node { cell: CellId { level: 13, x: 4096, y: 4096, z: 4096 }, at: n }
    }

    /// A chain of `n` nodes, each reached from the one before it: the record
    /// A* keeps, in the shape [`Sampler::expanded`] reads it.
    fn chain(n: u32) -> FxHashMap<Node, Node> {
        (1..n).map(|i| (node(i), node(i - 1))).collect()
    }

    /// A mark is measured in the same units as the places it stands at
    ///
    /// Which is metres, the vertices being metres from where the search set
    /// out. Sizing one in light years instead draws every cross a hundred
    /// metres wide — a ten-thousand-billionth of a light year, and so an
    /// entire layer that renders and cannot be seen at any zoom.
    #[test]
    fn a_mark_is_drawn_in_metres_like_the_places_it_marks() {
        let ly = |metres: f32| metres as f64 / crate::map::space::LIGHT_YEAR;

        // Near enough that a pixel is a thousandth of a light year: the mark
        // is the fraction of the cell, which is what stipples the region.
        let close = across(100., (crate::map::space::LIGHT_YEAR * 1e-3) as f32);
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
        let far = across(100., (crate::map::space::LIGHT_YEAR * 100.) as f32);
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
        let came = chain(10);

        let mut cells = 0;
        for step in 0..(STRIDE * BATCH as u64 * 40) {
            let here = at(step as f64);
            sampler.expanded(node((step % 10) as u32), here, &came, |it| {
                at(it.at as f64 * 100.)
            });
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

    /// Every leg of a plan shows in the closed set, not just the one drawn
    ///
    /// A plan's legs are refined side by side, each carrying a quiet sampler
    /// ([`super::graph::Sampler::beside`]). The chain and the working edge
    /// each *replace* what the frontier holds, so only the owner hands
    /// those over — but the closed set is a union, and a picture drawn from
    /// one leg stops dead at that leg's corridor. Which reads as a search
    /// that will not expand past a line, over a plot that went somewhere
    /// else entirely.
    #[test]
    fn a_leg_refined_beside_its_neighbours_is_in_the_closed_set() {
        let frontier = Frontier::between(DVec3::ZERO, at(6400.));
        let mut owner = frontier.sampler();
        let mut sibling = owner.beside();
        let came = chain(10);
        let expand = |sampler: &mut crate::map::route::graph::Sampler,
                      along: f64| {
            for step in 0..(STRIDE * BATCH as u64 * 4) {
                sampler.expanded(
                    node((step % 10) as u32),
                    DVec3::new(along, 0., step as f64),
                    &came,
                    |it| at(it.at as f64 * 100.),
                );
            }
        };

        // The owner works one corridor and the sibling another, a thousand
        // light years off it — two legs of the one plan.
        expand(&mut owner, 0.);
        expand(&mut sibling, 1000.);
        sibling.done();
        owner.done();

        let drawn = frontier.drawn().expect("something reached");
        let across = drawn.across;
        assert!(
            drawn.cells.iter().any(|cell| cell.x > across),
            "the sibling's corridor is missing from the {} cells drawn",
            drawn.cells.len(),
        );
        assert!(
            drawn.cells.iter().any(|cell| cell.x <= across),
            "the owner's corridor is missing from the {} cells drawn",
            drawn.cells.len(),
        );
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
        let place = |it: Node| places[it.at as usize];
        // The search's own record: 1 was reached from 0, 2 from 1, 3 from 2.
        let came = chain(4);

        // Expanded in that order, the last of them the closest to the goal.
        for n in 0..3 {
            sampler.expanded(node(n), places[n as usize], &came, place);
        }
        sampler.done();

        let drawn = frontier.drawn().expect("something reached");
        assert_eq!(
            drawn.reaching,
            vec![vec![at(0.), at(100.), at(200.)]],
            "the one chain of a flat search, to the closest system expanded"
        );
        assert!(drawn.plan.is_empty(), "a flat search plans nothing");
        assert_eq!(
            drawn.closest, 200.,
            "and how far that still is from the goal"
        );
    }

    /// A plan is drawn whole, with a branch for every leg refined off it
    ///
    /// The reported trouble, measured over the real index: a galactic route
    /// is planned coarsely and then flown leg by leg, and each leg is its
    /// own search that can only hand over its own two-to-six-jump chain. So
    /// the drawn chain went from 116 links to 2 the moment the first leg
    /// began — the crossing replaced by a stub for the two seconds the legs
    /// take, which reads as the picture being taken down early.
    ///
    /// The plan is said outright now and each leg is a strand beside it, so
    /// what is drawn is the promise and the way the ship can really fly it.
    #[test]
    fn a_plan_is_drawn_with_a_branch_for_every_leg() {
        let goal = at(400.);
        let frontier = Frontier::between(DVec3::ZERO, goal);
        let mut sampler = frontier.sampler();

        // Three waypoints planned; the first leg flown two ways off the
        // straight hop, the second still being searched.
        sampler.planned(vec![at(0.), at(200.), goal]);
        sampler.flew(vec![at(0.), at(90.), at(200.)]);
        sampler.reached(at(300.), || vec![at(200.), at(300.)]);
        sampler.done();

        let drawn = frontier.drawn().expect("something reached");
        assert_eq!(
            drawn.plan,
            vec![at(0.), at(200.), goal],
            "the plan, on its own layer and under the branches"
        );
        assert_eq!(
            drawn.reaching,
            vec![vec![at(0.), at(90.), at(200.)], vec![at(200.), at(300.)],],
            "a branch for the leg flown and one for the leg under way"
        );
        assert_eq!(
            drawn.closest, 100.,
            "how close the route has come, not how close the leg has"
        );
    }

    /// What is left to cross is drawn, so a chain short of the goal reads
    /// as one
    ///
    /// The reported trouble, and it took a zoom to see: a search's chain
    /// ends at the closest thing it has reached, which on a galactic plot
    /// is hundreds of light years short — sub-pixel at that zoom — so a
    /// route still being worked out read as a finished one and the minutes
    /// after it looked like the map doing nothing.
    #[test]
    fn what_is_left_to_cross_is_drawn() {
        let goal = at(400.);
        let frontier = Frontier::between(DVec3::ZERO, goal);
        let mut sampler = frontier.sampler();
        sampler.reached(at(300.), || vec![at(0.), at(300.)]);
        sampler.done();

        let drawn = frontier.drawn().expect("something reached");
        assert_eq!(
            left(&drawn),
            vec![at(300.), goal],
            "the hundred light years nothing has reached were not drawn"
        );
    }

    /// And not drawn twice
    ///
    /// A coarse plan spans the whole route itself, so a second line to the
    /// goal would be the same claim in two colours. Nor is there anything
    /// to say for a search standing on its goal, or one that has reached
    /// nothing at all.
    #[test]
    fn what_is_left_is_not_drawn_twice() {
        let goal = at(400.);
        let frontier = Frontier::between(DVec3::ZERO, goal);
        let mut sampler = frontier.sampler();

        sampler.done();
        let nothing = frontier.drawn().expect("a frontier");
        assert!(left(&nothing).is_empty(), "a search that reached nothing");

        let mut sampler = frontier.sampler();
        sampler.planned(vec![at(0.), at(200.), goal]);
        sampler.reached(at(100.), || vec![at(0.), at(100.)]);
        sampler.done();
        let planned = frontier.drawn().expect("a plan");
        assert!(left(&planned).is_empty(), "the plan already spans it");

        let mut sampler = frontier.sampler();
        sampler.reached(goal, || vec![at(0.), goal]);
        sampler.done();
        let arrived = frontier.drawn().expect("an arrival");
        assert!(left(&arrived).is_empty(), "a tip on the goal has arrived");
    }

    /// A strand of no length is not drawn
    ///
    /// A leg that came back with one place, and a search that has reached
    /// nothing, are both a line of no length — and the renderer handed a
    /// mesh of no vertices says so every frame.
    #[test]
    fn a_strand_of_no_length_is_not_drawn() {
        let frontier = Frontier::between(DVec3::ZERO, at(400.));
        let mut sampler = frontier.sampler();

        sampler.planned(vec![at(0.), at(200.), at(400.)]);
        sampler.flew(vec![at(0.)]);
        sampler.done();

        let drawn = frontier.drawn().expect("something reached");
        assert_eq!(drawn.plan, vec![at(0.), at(200.), at(400.)]);
        assert!(drawn.reaching.is_empty(), "a branch of one place was drawn");
    }

    /// The readouts are about the plot, not about a leg of it
    ///
    /// A trip's legs are searched at once, so every reading the form gives
    /// has to be an aggregate: the longest wait, the expansions added up,
    /// and the distances left added up. The last was a *minimum* over the
    /// legs, which on a trip through three stops reads as almost arrived
    /// while two legs have twenty thousand light years between them — and
    /// jumps about as each lands.
    #[test]
    fn the_readouts_are_about_the_whole_plot() {
        let mut frontiers = Frontiers::default();
        let asked = Instant::now();
        // Three legs: two that have reached something, at 900 and 100 ly
        // from their own ends, and one that has reached nothing yet.
        for (leg, reached) in [(1, Some(900.)), (2, Some(100.)), (3, None)] {
            let frontier = Frontier::between(DVec3::ZERO, at(1_000.));
            if let Some(away) = reached {
                let mut sampler = frontier.sampler();
                sampler.reached(at(1_000. - away), || vec![DVec3::ZERO]);
                sampler.done();
            }
            frontiers.watch(
                FetchIndex::Route(
                    format!("{leg}"),
                    "end".into(),
                    "50".into(),
                    Some("a trip".into()),
                    crate::map::route::Drive::Standard,
                    crate::map::route::Routing::QUICK,
                    crate::map::route::graph::Tuning::default(),
                ),
                frontier,
                asked,
            );
        }

        assert_eq!(frontiers.legs(), 3, "a leg was not watched");
        assert_eq!(
            frontiers.left(),
            Some(1_000.),
            "the distances left were not added up",
        );
        assert_eq!(
            frontiers.plotting("a trip"),
            3,
            "the trip's own legs were not counted",
        );
        assert_eq!(
            frontiers.plotting("another trip"),
            0,
            "another trip's legs were counted",
        );
    }

    /// A running search is drawn, and what it drew goes when it finishes
    ///
    /// Five layers up while it runs and none after — the closed set, the
    /// leading edge, the plan, the branches refining it, and the stretch
    /// still to be crossed: the answer is drawn as a route by then, and
    /// what the search touched on the way is not what the map is for.
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
            .spawn((BigSpace::default(), crate::map::space::galaxy_grid()))
            .id();
        app.insert_resource(Galaxy(galaxy));
        app.add_systems(Update, draw);

        let goal = at(6400.);
        let frontier = Frontier::between(DVec3::ZERO, goal);
        app.world_mut().resource_mut::<Frontiers>().watch(
            FetchIndex::Route(
                "Sol".into(),
                "Colonia".into(),
                "50".into(),
                None,
                crate::map::route::Drive::Standard,
                crate::map::route::Routing::FEWEST,
                crate::map::route::graph::Tuning::default(),
            ),
            Arc::clone(&frontier),
            Instant::now(),
        );
        let mut sampler = frontier.sampler();
        let came = chain(10);

        for step in 0..(STRIDE * BATCH as u64 * 2) {
            let here = at(step as f64);
            sampler.expanded(node((step % 10) as u32), here, &came, |it| {
                at(it.at as f64 * 100.)
            });
        }
        app.update();
        assert_eq!(lines(&mut app), 5, "the five layers of one search");

        app.update();
        assert_eq!(lines(&mut app), 5, "five layers, not five a frame");

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
