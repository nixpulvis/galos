//! The jump graph the router walks.
//!
//! Routing used to be a database question: A* over `ST_3DDWithin` neighbour
//! queries. With the map drawing from the index there is no database, so the
//! same walk runs here over the resident names table, which carries every
//! system's place. The one thing a walk over a million points needs that a
//! database index gave it for free is a way to ask for neighbours without
//! scanning them all, so the positions are bucketed into a coarse spatial grid
//! and a jump looks only in the buckets a ship could reach.

use bevy::prelude::*;
use galos_index::meta::NameEntry;
use pathfinding::prelude::astar;
use std::collections::HashMap;
use std::sync::Arc;

/// The edge of a grid bucket, in light years.
///
/// A jump reaches a few tens of light years, so a bucket this size means a
/// neighbour search looks in a handful of buckets rather than the whole grid.
/// Larger wastes the pruning; smaller multiplies the buckets a jump must visit.
const BUCKET_LY: f64 = 64.0;

/// The jump graph, held behind an [`Arc`] so a route task takes a cheap handle.
#[derive(Resource, Clone)]
pub struct Jumps(pub Arc<JumpGraph>);

/// Every system's place, bucketed in space for neighbour queries.
pub struct JumpGraph {
    /// Each system's address and position, in light years.
    points: Vec<(i64, [f64; 3])>,
    /// Address to its index in `points`.
    by_address: HashMap<i64, usize>,
    /// Grid bucket to the indices of the points that fall in it.
    buckets: HashMap<[i32; 3], Vec<usize>>,
}

/// Which bucket a point falls in.
fn bucket_of(p: [f64; 3]) -> [i32; 3] {
    [
        (p[0] / BUCKET_LY).floor() as i32,
        (p[1] / BUCKET_LY).floor() as i32,
        (p[2] / BUCKET_LY).floor() as i32,
    ]
}

/// The squared distance between two points, the distance itself wanted for
/// nothing here but comparing.
fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dx * dx + dy * dy + dz * dz
}

impl JumpGraph {
    /// Build the graph from the resident names table.
    pub fn new(entries: &[NameEntry]) -> JumpGraph {
        let points: Vec<(i64, [f64; 3])> = entries
            .iter()
            .map(|e| {
                (
                    e.address,
                    [
                        e.position[0] as f64,
                        e.position[1] as f64,
                        e.position[2] as f64,
                    ],
                )
            })
            .collect();
        let by_address =
            points.iter().enumerate().map(|(i, (a, _))| (*a, i)).collect();
        let mut buckets: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
        for (i, (_, p)) in points.iter().enumerate() {
            buckets.entry(bucket_of(*p)).or_default().push(i);
        }
        JumpGraph { points, by_address, buckets }
    }

    /// The systems within `range` light years of the point at `i`, by index.
    fn neighbors(&self, i: usize, range: f64) -> Vec<usize> {
        let p = self.points[i].1;
        let base = bucket_of(p);
        let reach = (range / BUCKET_LY).ceil() as i32;
        let mut out = Vec::new();
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let cell = [base[0] + dx, base[1] + dy, base[2] + dz];
                    let Some(bucket) = self.buckets.get(&cell) else {
                        continue;
                    };
                    for &j in bucket {
                        if j != i && dist2(p, self.points[j].1) <= range * range
                        {
                            out.push(j);
                        }
                    }
                }
            }
        }
        out
    }

    /// A route between two systems by address, at a ship's jump `range`, as the
    /// hops it passes through. [`None`] where either end is unknown or no chain
    /// of jumps that long connects them.
    ///
    /// The cost is one per jump, so the fewest jumps, and the heuristic is the
    /// straight-line distance in whole jumps, which never overstates what is
    /// left and so keeps A* on the shortest chain rather than merely on a
    /// chain.
    ///
    /// Counting jumps says nothing about where they go, though, and that is
    /// the whole of what a route looks like. Twenty thousand light years is
    /// forty-odd jumps and an enormous number of chains exactly that long, so
    /// the answer used to be whichever the neighbour search happened to reach
    /// first — over the real index, a first jump that spent 500 light years to
    /// get 128 nearer, and a line that visibly wandered off course and back.
    ///
    /// So the neighbours of a system are offered nearest-the-goal first. It
    /// costs a sort per system expanded and it is what makes the route look
    /// like a route: measured Sol to Colonia at a 500 light year range, 22,021
    /// light years against a 22,000 straight line, every jump spending itself
    /// on progress.
    ///
    /// What that does not do is *prove* the shortest of the equally-short
    /// chains. Costing distance as a tie-break inside the search does prove
    /// it, and was measured at 22,003 light years — eighteen better over
    /// twenty-two thousand — for thirty seconds against this one's two hundred
    /// milliseconds. The proof is not worth a hundred and seventy times the
    /// wait for a fifth of a light year a jump.
    pub(crate) fn route(
        &self,
        start: i64,
        end: i64,
        range: f64,
    ) -> Option<Vec<(i64, [f64; 3])>> {
        let start = *self.by_address.get(&start)?;
        let end = *self.by_address.get(&end)?;
        let goal = self.points[end].1;
        let (path, _) = astar(
            &start,
            |&i| {
                let mut near = self.neighbors(i, range);
                near.sort_by(|&a, &b| {
                    let a = dist2(self.points[a].1, goal);
                    let b = dist2(self.points[b].1, goal);
                    a.total_cmp(&b)
                });
                near.into_iter().map(|j| (j, 1u32))
            },
            |&i| (dist2(self.points[i].1, goal).sqrt() / range).ceil() as u32,
            |&i| i == end,
        )?;
        Some(path.into_iter().map(|i| self.points[i]).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A system named for its address, at `at`.
    fn at(address: i64, at: [f32; 3]) -> NameEntry {
        NameEntry { address, name: format!("S{address}"), position: at }
    }

    /// How far a route runs, following its legs.
    fn run(path: &[(i64, [f64; 3])]) -> f64 {
        path.windows(2).map(|w| dist2(w[0].1, w[1].1).sqrt()).sum()
    }

    /// Of two chains the same number of jumps long, the route follows the
    /// straighter
    ///
    /// The reported trouble, in miniature: counting jumps alone makes every
    /// chain of five equally good, so a detour off the line and back is free
    /// and the line drawn wanders. The detour is offered to the search first,
    /// by being built first, so a router that takes whatever it reaches first
    /// takes the detour.
    #[test]
    fn a_route_of_equal_jumps_follows_the_straighter_chain() {
        let mut entries = vec![at(0, [0., 0., 0.])];
        // Five jumps off the line and back, all inside a 500 ly range.
        for (k, side) in [(1, 150.), (2, -150.), (3, 150.), (4, -150.)] {
            entries.push(at(100 + k, [400.0 * k as f32, side, 0.]));
        }
        // And five straight down it.
        for k in 1..=4 {
            entries.push(at(k, [400.0 * k as f32, 0., 0.]));
        }
        entries.push(at(9, [2000., 0., 0.]));

        let graph = JumpGraph::new(&entries);
        let path = graph.route(0, 9, 500.).expect("a route");

        assert_eq!(path.len() - 1, 5, "not five jumps");
        assert_eq!(
            path.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 9],
            "wandered off the line, running {:.0} ly against 2000",
            run(&path)
        );
    }

    /// And fewer jumps beats a shorter way
    ///
    /// The other half of the policy, and the one the ordering must not undo: a
    /// jump is fuel and time, so two long jumps beat five short ones covering
    /// the same ground. Both chains here run 900 light years.
    #[test]
    fn a_route_takes_the_fewest_jumps_before_the_straightest() {
        let mut entries = vec![at(0, [0., 0., 0.])];
        // Five short hops.
        for k in 1..=4 {
            entries.push(at(k, [180.0 * k as f32, 0., 0.]));
        }
        // Two long ones, over the same ground.
        entries.push(at(50, [450., 0., 0.]));
        entries.push(at(9, [900., 0., 0.]));

        let graph = JumpGraph::new(&entries);
        let path = graph.route(0, 9, 500.).expect("a route");

        assert_eq!(path.len() - 1, 2, "took the short hops");
        assert_eq!(path[1].0, 50, "not through the far waypoint");
    }

    /// A goal nothing reaches is no route rather than a wrong one
    #[test]
    fn a_gap_wider_than_the_range_is_no_route() {
        let entries = vec![at(0, [0., 0., 0.]), at(1, [600., 0., 0.])];
        let graph = JumpGraph::new(&entries);

        assert!(graph.route(0, 1, 500.).is_none(), "jumped 600 at 500");
        assert!(graph.route(0, 1, 700.).is_some(), "600 is inside 700");
    }

    /// A neighbour search reaches past its own bucket
    ///
    /// The buckets are [`BUCKET_LY`] on a side and a range many times that
    /// has to look many buckets out, or a route would only ever step to the
    /// system next door.
    #[test]
    fn a_range_wider_than_a_bucket_still_finds_its_neighbours() {
        let far = BUCKET_LY as f32 * 6.;
        let entries = vec![at(0, [0., 0., 0.]), at(1, [far, 0., 0.])];
        let graph = JumpGraph::new(&entries);

        assert_eq!(graph.neighbors(0, far as f64 + 1.), vec![1]);
        assert!(graph.neighbors(0, far as f64 - 1.).is_empty());
    }
}
