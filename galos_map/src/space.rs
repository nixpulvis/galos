//! The high precision space the map is drawn in
//!
//! Star positions run to a hundred thousand light years from the galactic
//! centre, and a single float carrying such a number has about a thousandth
//! of a light year left over for everything to its right. That is coarser
//! than a whole star system is wide, so anything smaller than a system
//! cannot be placed at all, and meshes drawn out on the rim jitter as the
//! camera moves.
//!
//! [`big_space`] answers this by splitting a position in two: an integer
//! [`CellCoord`] naming a cell of a [`Grid`], and a [`Transform`] holding the
//! remainder within that cell. The remainder never grows past half a cell, so
//! the float carrying it never runs out of digits, however far from the
//! centre the cell is. Rendering positions are then computed relative to
//! whichever entity holds [`FloatingOrigin`] — the camera — which keeps the
//! error that is left too far away to see.
//!
//! # What a number means here
//!
//! Metres. Not light years, which is what the map talks in everywhere else:
//! the spyglass, the routes, a system's position, every distance the UI says
//! out loud.
//!
//! Two things settle it. The database records a body in metres already, and
//! bevy's lighting is in physical units, so a light year would mean writing
//! our own. And one unit has to serve the whole hierarchy — composing one
//! grid's transform onto another drops any scale between them, so a nested
//! grid may say how large its cells are but never what a number means.
//!
//! So [`LIGHT_YEAR`] stands at the one place the two meet: where a position
//! becomes a cell.
use crate::systems::Spyglass;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_plugins(BigSpaceDefaultPlugins);
    app.add_systems(Startup, spawn_galaxy);
}

/// Metres in a light year
///
/// Exact: a light year is defined as a year of Julian days at the defined
/// speed of light, so this is a whole number of metres rather than a measured
/// one.
pub const LIGHT_YEAR: f64 = 9.4607304725808e15;

/// Where `position` light years from the galactic centre falls, in metres
///
/// The one conversion in the map, and the only place a light year is spoken
/// to the grid. Everything above it says light years; everything below says
/// metres.
pub fn metres(position: DVec3) -> DVec3 {
    position * LIGHT_YEAR
}

/// Edge length of a galaxy grid cell, in metres
///
/// Two to the fifty-third, which is a little under a light year. A power of
/// two so that multiplying a cell by it is exact in both a float and a double;
/// a literal light year is neither, and the error, compounded over the seventy
/// thousand cells it takes to reach the far rim, would bend the star map by
/// some thousandths of a light year.
///
/// This grid holds systems and nothing else, and a system is only recorded to
/// an eighth of a light year, so the quarter of a million kilometres a float
/// has left over inside a cell of this size is a million times finer than
/// anything it is asked to place.
///
/// The old grid was one light year, chosen so that a cell's coordinate and a
/// position in light years were the same number. That went when the unit did.
const GALAXY_CELL_EDGE: f32 = 9007199254740992.;

/// Edge length of a cell in a system's own grid, in metres
///
/// One metre. Bodies sit from light seconds out to light hours, so the widest
/// system on record needs some hundreds of trillions of cells, which an `i64`
/// carries with four orders of magnitude to spare. What is left over inside a
/// cell this size is about thirty nanometres, which is far finer than the
/// float vertices of the sphere a body is drawn with, and leaves the mesh
/// rather than the grid as the thing that gives out first.
const SYSTEM_CELL_EDGE: f32 = 1.;

/// How far past its cell an entity may drift before it is moved to the next
///
/// Recentring the moment a cell edge is crossed would send anything sitting
/// on the boundary back and forth between two cells.
const SWITCHING_THRESHOLD: f32 = 0.1;

/// The grid every star is placed in
///
/// Held as a resource because stars are spawned by systems that have no way
/// to reach the builder the grid was created with. Everything drawn in the
/// galaxy has to be a child of this entity to be positioned by its grid.
#[derive(Resource)]
pub struct Galaxy(pub Entity);

/// The grid a system's own contents are placed in
///
/// Handed to whatever puts a `Grid` on a system, so that neither the size of
/// a cell nor the threshold is named anywhere but here.
///
/// A system carries one of these only while its contents are loaded. Grids are
/// walked every frame — the whole tree of them, since the order that walk takes
/// is for precision rather than for skipping any — so one per system on the map
/// would be tens of thousands of them at a wide spyglass, and one at a time is
/// none.
pub fn system_grid() -> Grid {
    Grid::new(SYSTEM_CELL_EDGE, SWITCHING_THRESHOLD)
}

/// Create the galaxy grid, and the camera that looks at it
///
/// The camera is spawned here rather than alongside the rest of its own
/// module because it has to be a child of the grid, and because it carries
/// [`FloatingOrigin`]: every other entity's rendered position is computed
/// relative to it.
fn spawn_galaxy(mut commands: Commands, spyglass: Res<Spyglass>) {
    commands.spawn_big_space(
        Grid::new(GALAXY_CELL_EDGE, SWITCHING_THRESHOLD),
        |galaxy| {
            let entity = galaxy.id();
            galaxy.commands().insert_resource(Galaxy(entity));
            galaxy.spawn_spatial(crate::camera::camera(&spyglass));
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Light seconds in a light year
    const LIGHT_SECONDS: f64 = 3.15576e7;

    /// The galaxy grid can separate two points a hundred light seconds apart,
    /// out where a float has given up entirely
    ///
    /// This is the whole reason for the grid. A body sits light seconds from
    /// its star, and a star sits tens of thousands of light years from the
    /// galactic centre. Neither number is hard on its own. Holding both at
    /// once is what a single float cannot do.
    #[test]
    fn resolves_light_seconds_at_the_rim() {
        let grid = Grid::new(GALAXY_CELL_EDGE, SWITCHING_THRESHOLD);
        // Deliberately not on a cell boundary, so the remainder within the
        // cell is as large as it realistically gets.
        let star = DVec3::new(20_000.371, 0., 30_000.628);
        let body = star + DVec3::X * 100. / LIGHT_SECONDS;

        // One step of a float's precision this far out is worth about
        // seventy five thousand light seconds, so it places the two on top
        // of each other.
        assert_eq!(star.as_vec3(), body.as_vec3());

        let placed = |p: DVec3| {
            let (cell, offset) = grid.translation_to_grid(metres(p));
            cell.as_dvec3(&grid) + offset.as_dvec3()
        };
        let separation =
            (placed(body) - placed(star)).length() / LIGHT_YEAR * LIGHT_SECONDS;
        assert!(
            (separation - 100.).abs() < 5.,
            "expected about 100 light seconds apart, got {separation}"
        );
    }

    /// A system's own grid separates two points a metre apart, anywhere a
    /// body may be
    ///
    /// The same demonstration one level down, and the reason a system carries
    /// a grid of its own rather than leaving its bodies to the galaxy's. Out
    /// at the far edge of a wide system a float has kilometres of precision
    /// left, which is no way to draw a planet.
    #[test]
    fn a_system_grid_separates_two_points_a_metre_apart() {
        let grid = system_grid();
        // A light hour out, which is about as far as a body is ever found.
        let far = DVec3::new(1.08e12, 0., 0.);
        let beside = far + DVec3::X;

        assert_eq!(far.as_vec3(), beside.as_vec3());

        let placed = |p: DVec3| {
            let (cell, offset) = grid.translation_to_grid(p);
            cell.as_dvec3(&grid) + offset.as_dvec3()
        };
        let separation = (placed(beside) - placed(far)).length();
        assert!(
            (separation - 1.).abs() < 0.01,
            "expected about a metre apart, got {separation}"
        );
    }

    /// A galaxy cell is a whole number of metres however many of them are
    /// counted
    ///
    /// Which is what the edge length being a power of two buys. An edge a
    /// float rounds is an edge that compounds: the seventy thousandth cell
    /// would stand somewhere other than seventy thousand times the first.
    #[test]
    fn a_galaxy_cell_is_exact_however_far_out_it_is() {
        let edge = GALAXY_CELL_EDGE as f64;
        // Past the far rim, so any cell the map can reach is covered.
        for cells in [1i64, 1_000, 68_272, 100_000] {
            let reached = cells as f64 * edge;
            assert_eq!(
                reached as f32 as f64, reached,
                "cell {cells} did not survive being carried in a float"
            );
        }
    }
}
