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
    /// The cost is one per jump, so the fewest-jumps route, and the heuristic is
    /// the straight-line distance in whole jumps, which never overstates what is
    /// left and so keeps A* admissible.
    pub fn route(
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
            |&i| self.neighbors(i, range).into_iter().map(|j| (j, 1u32)),
            |&i| (dist2(self.points[i].1, goal).sqrt() / range).ceil() as u32,
            |&i| i == end,
        )?;
        Some(path.into_iter().map(|i| self.points[i]).collect())
    }
}
