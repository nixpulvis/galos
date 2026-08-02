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
use crate::systems::Spyglass;
use bevy::prelude::*;
use big_space::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_plugins(BigSpaceDefaultPlugins);
    app.add_systems(Startup, spawn_galaxy);
}

/// Edge length of a galaxy grid cell, in light years
///
/// One light year keeps a cell's coordinate and a position in light years
/// the same number, which is the unit the whole map is already written in.
/// Positions within a cell then stay under half a light year, leaving the
/// float that carries them about two light seconds of precision, anywhere.
///
/// That is far finer than the eighth of a light year the game rounds system
/// positions to, and far coarser than a planet. Bodies are not placed in
/// this grid; they belong to a finer one nested at their own system.
const CELL_EDGE: f32 = 1.;

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

/// Create the galaxy grid, and the camera that looks at it
///
/// The camera is spawned here rather than alongside the rest of its own
/// module because it has to be a child of the grid, and because it carries
/// [`FloatingOrigin`]: every other entity's rendered position is computed
/// relative to it.
fn spawn_galaxy(mut commands: Commands, spyglass: Res<Spyglass>) {
    commands.spawn_big_space(
        Grid::new(CELL_EDGE, SWITCHING_THRESHOLD),
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
    use bevy::math::DVec3;

    /// Light seconds in a light year
    const LIGHT_SECONDS: f64 = 3.15576e7;

    /// The grid can separate two points a hundred light seconds apart, out
    /// where a float has given up entirely
    ///
    /// This is the whole reason for the grid. A body sits light seconds from
    /// its star, and a star sits tens of thousands of light years from the
    /// galactic centre. Neither number is hard on its own. Holding both at
    /// once is what a single float cannot do.
    #[test]
    fn resolves_light_seconds_at_the_rim() {
        let grid = Grid::new(CELL_EDGE, SWITCHING_THRESHOLD);
        // Deliberately not on a cell boundary, so the remainder within the
        // cell is as large as it realistically gets.
        let star = DVec3::new(20_000.371, 0., 30_000.628);
        let body = star + DVec3::X * 100. / LIGHT_SECONDS;

        // One step of a float's precision this far out is worth about
        // seventy five thousand light seconds, so it places the two on top
        // of each other.
        assert_eq!(star.as_vec3(), body.as_vec3());

        let placed = |p: DVec3| {
            let (cell, offset) = grid.translation_to_grid(p);
            cell.as_dvec3(&grid) + offset.as_dvec3()
        };
        let separation = (placed(body) - placed(star)).length() * LIGHT_SECONDS;
        assert!(
            (separation - 100.).abs() < 5.,
            "expected about 100 light seconds apart, got {separation}"
        );
    }
}
