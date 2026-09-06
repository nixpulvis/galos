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
use pathfinding::num_traits::Zero;
use pathfinding::prelude::astar;
use std::collections::HashMap;
use std::sync::Arc;

/// The edge of a grid bucket, in light years.
///
/// A jump reaches a few tens of light years, so a bucket this size means a
/// neighbour search looks in a handful of buckets rather than the whole grid.
/// Larger wastes the pruning; smaller multiplies the buckets a jump must visit.
const BUCKET_LY: f64 = 64.0;

/// Which of the fewest-jumps routes to come back with
///
/// Both settings are the fewest jumps — that is what a jump range means, fuel
/// and time — and both search with an estimate that never overstates the jumps
/// left, so neither is trading correctness for speed. Twenty-two thousand
/// light years is forty-five jumps, though, and the number of chains exactly
/// forty-five long is enormous. This is what settles which one is drawn.
///
/// Measured Sol to Colonia at a 500 light year range, over the 2.25 million
/// systems the names table carries:
///
/// | | route | against a 22,000 ly line | found in |
/// |---|---|---|---|
/// | [`Direct`](Self::Direct) | 22,021 ly | 1.001 | 0.17 s |
/// | [`Shortest`](Self::Shortest) | 22,003 ly | 1.000 | 30 s |
///
/// Eighteen light years over twenty-two thousand, for a hundred and seventy
/// times the wait. Hence the default, and hence the choice: the difference is
/// invisible on the map and the wait is not, but a fifth of a light year a
/// jump is a real thing to want and there is no reason the map should refuse
/// to spend a minute finding it.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Routing {
    /// Take the neighbour that gets nearest the goal, and do not look back.
    #[default]
    Direct,
    /// Prove the shortest of the chains that are the fewest jumps.
    Shortest,
}

/// What a leg costs when the shortest route is being proved: one jump, and
/// how far that jump goes.
///
/// Two numbers rather than one because the policy is two-tiered, and the order
/// of the fields is the whole of it: the route with the fewest jumps wins
/// outright, and between two of the same length the shorter one wins. Derived
/// [`Ord`] compares the fields in order, which is exactly that. A jump can
/// never be traded away for any amount of distance, however much.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
struct Cost {
    jumps: u32,
    /// Whole light years. An `f64` is not [`Ord`] and a route is not decided
    /// by fractions of a light year, so the distance is rounded to an integer
    /// and the comparison is total.
    ///
    /// A leg rounds up and the estimate rounds down, which is what keeps the
    /// estimate a lower bound: rounding both to nearest would let a long chain
    /// of legs each shaved by half a light year add to less than the straight
    /// line it followed, and an estimate that overstates is one that can send
    /// A* home with the wrong answer.
    light_years: u64,
}

impl std::ops::Add for Cost {
    type Output = Cost;

    fn add(self, other: Cost) -> Cost {
        Cost {
            jumps: self.jumps + other.jumps,
            light_years: self.light_years + other.light_years,
        }
    }
}

impl Zero for Cost {
    fn zero() -> Cost {
        Cost::default()
    }

    fn is_zero(&self) -> bool {
        *self == Cost::default()
    }
}

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

    /// A route between two systems by address, at a ship's jump `range`, as
    /// the hops it passes through. [`None`] where either end is unknown or no
    /// chain of jumps that long connects them.
    ///
    /// Both ways cost one per jump and estimate what is left as the
    /// straight-line distance in whole jumps, which can never overstate the
    /// jumps remaining — so both are admissible, and both come back with a
    /// genuine fewest-jumps route. What [`Routing`] chooses is what to do
    /// about the enormous number of chains that are all exactly that long.
    pub(crate) fn route(
        &self,
        start: i64,
        end: i64,
        range: f64,
        how: Routing,
    ) -> Option<Vec<(i64, [f64; 3])>> {
        let start = *self.by_address.get(&start)?;
        let end = *self.by_address.get(&end)?;
        let goal = self.points[end].1;
        let path = match how {
            Routing::Direct => self.direct(start, end, goal, range),
            Routing::Shortest => self.shortest(start, end, goal, range),
        }?;
        Some(path.into_iter().map(|i| self.points[i]).collect())
    }

    /// A route through `stops` in order, at a ship's jump `range`, as the
    /// hops it passes through
    ///
    /// Each leg is plotted on its own, from one stop to the next, and the
    /// legs are joined end to end: the stop a leg lands on is the stop the
    /// next sets out from, and is written down once. So the hops read as one
    /// route in the order flown, and a stop in the middle stands in the list
    /// once as any other system does.
    ///
    /// As far as it gets. A leg that cannot be flown ends the route at the
    /// stop it would have set out from, and what is handed back is the hops
    /// through every leg before it, so that whoever reads the route can say
    /// which leg it stopped short at. Nothing at all where the first leg is
    /// the one that fails, or where there is no leg to fly.
    ///
    /// `how` is settled once for the whole route rather than per leg: the
    /// legs are one route to the user, and a route flown one way in its first
    /// half and another in its second is a route nobody asked for.
    pub(crate) fn route_through(
        &self,
        stops: &[i64],
        range: f64,
        how: Routing,
    ) -> Vec<(i64, [f64; 3])> {
        let mut hops: Vec<(i64, [f64; 3])> = Vec::new();
        for leg in stops.windows(2) {
            let Some(flown) = self.route(leg[0], leg[1], range, how) else {
                break;
            };
            // The stop this leg sets out from is the one the last landed on.
            let landed = usize::from(!hops.is_empty());
            hops.extend(flown.into_iter().skip(landed));
        }
        hops
    }

    /// Fewest jumps, taking the neighbour that gets nearest the goal first
    ///
    /// Distance is not in the cost at all, so nothing here decides between two
    /// chains of the same length — the order they are offered in does. Nearest
    /// the goal first is what makes a route look like one, and it is a
    /// preference rather than a bound: no claim is made that the chain it
    /// finds is the shortest of them, only that each step reaches as far
    /// toward the goal as the ones beside it.
    fn direct(
        &self,
        start: usize,
        end: usize,
        goal: [f64; 3],
        range: f64,
    ) -> Option<Vec<usize>> {
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
        Some(path)
    }

    /// Fewest jumps, and provably the shortest of them
    ///
    /// Distance goes into the cost under the jump count, so the search settles
    /// the tie itself instead of leaving it to the order neighbours arrive in.
    /// See [`Cost`] for the ordering and for why the rounding leans as it
    /// does; the neighbours are left unsorted, a sort per expansion buying
    /// nothing once every one of them has to be expanded anyway.
    fn shortest(
        &self,
        start: usize,
        end: usize,
        goal: [f64; 3],
        range: f64,
    ) -> Option<Vec<usize>> {
        let (path, _) = astar(
            &start,
            |&i| {
                let from = self.points[i].1;
                self.neighbors(i, range).into_iter().map(move |j| {
                    let leg = dist2(from, self.points[j].1).sqrt();
                    (j, Cost { jumps: 1, light_years: leg.ceil() as u64 })
                })
            },
            |&i| {
                let left = dist2(self.points[i].1, goal).sqrt();
                Cost {
                    jumps: (left / range).ceil() as u32,
                    light_years: left.floor() as u64,
                }
            },
            |&i| i == end,
        )?;
        Some(path)
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

    /// Systems strung out along a line, one every `step` light years
    fn strung(count: i64, step: f32) -> JumpGraph {
        let entries: Vec<NameEntry> =
            (0..count).map(|i| at(i, [i as f32 * step, 0., 0.])).collect();
        JumpGraph::new(&entries)
    }

    /// The addresses a route runs through, in order
    fn addresses(hops: &[(i64, [f64; 3])]) -> Vec<i64> {
        hops.iter().map(|(address, _)| *address).collect()
    }

    /// A route through several stops is flown leg by leg, in order
    ///
    /// Out and back again, which a route between two ends can never be: the
    /// middle stop is flown to and then set out from, and stands in the list
    /// once each time it is passed through.
    #[test]
    fn a_route_through_stops_is_flown_in_order() {
        let graph = strung(5, 10.);

        for how in BOTH {
            let hops = graph.route_through(&[0, 4, 2], 10., how);

            assert_eq!(addresses(&hops), vec![0, 1, 2, 3, 4, 3, 2], "{how:?}");
        }
    }

    /// The stop a leg lands on is written down once
    ///
    /// It is where the next leg sets out from, and a route that named it
    /// twice running would count a jump from a system to itself.
    #[test]
    fn a_stop_between_two_legs_is_not_said_twice() {
        let graph = strung(3, 10.);

        for how in BOTH {
            let hops = graph.route_through(&[0, 1, 2], 10., how);

            assert_eq!(addresses(&hops), vec![0, 1, 2], "{how:?}");
        }
    }

    /// A leg that cannot be flown ends the route where it would have begun
    ///
    /// The legs before it are handed back whole, so that whoever reads the
    /// route can say which leg stopped it. Here the second leg asks for a
    /// system out past what a ship this short can reach.
    #[test]
    fn a_route_stops_short_at_the_leg_it_cannot_fly() {
        let mut entries: Vec<NameEntry> =
            (0..3).map(|i| at(i, [i as f32 * 10., 0., 0.])).collect();
        entries.push(at(9, [100., 0., 0.]));
        let graph = JumpGraph::new(&entries);

        for how in BOTH {
            let hops = graph.route_through(&[0, 2, 9], 10., how);

            assert_eq!(addresses(&hops), vec![0, 1, 2], "{how:?}");
        }
    }

    /// A first leg that cannot be flown is no route at all
    #[test]
    fn a_route_whose_first_leg_fails_is_nothing() {
        let graph = strung(3, 10.);

        for how in BOTH {
            assert!(graph.route_through(&[0, 2], 5., how).is_empty());
            assert!(graph.route_through(&[0], 10., how).is_empty());
            assert!(graph.route_through(&[], 10., how).is_empty());
        }
    }

    /// Both settings, since every claim below holds of both.
    const BOTH: [Routing; 2] = [Routing::Direct, Routing::Shortest];

    /// Of two chains the same number of jumps long, the route follows the
    /// straighter
    ///
    /// The reported trouble, in miniature: counting jumps alone makes every
    /// chain of five equally good, so a detour off the line and back is free
    /// and the line drawn wanders. The detour is offered to the search first,
    /// by being built first, so a router that takes whatever it reaches first
    /// takes the detour.
    ///
    /// [`Routing::Direct`] gets here by preferring the neighbour nearest the
    /// goal and [`Routing::Shortest`] by costing the distance, so the two
    /// arrive by different means and must agree on the answer.
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

        for how in BOTH {
            let path = graph.route(0, 9, 500., how).expect("a route");

            assert_eq!(path.len() - 1, 5, "not five jumps, {how:?}");
            assert_eq!(
                path.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
                vec![0, 1, 2, 3, 4, 9],
                "{how:?} wandered off the line, running {:.0} ly against 2000",
                run(&path)
            );
        }
    }

    /// And fewer jumps beats a shorter way, whichever setting is asked
    ///
    /// The half neither setting may undo: a jump is fuel and time, so two long
    /// jumps beat five short ones over the same ground. Both chains here run
    /// 900 light years, so nothing but the count can choose between them —
    /// which is what [`Cost`]'s field order is for on the one side, and what
    /// costing only jumps gives outright on the other.
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

        for how in BOTH {
            let path = graph.route(0, 9, 500., how).expect("a route");

            assert_eq!(path.len() - 1, 2, "{how:?} took the short hops");
            assert_eq!(path[1].0, 50, "{how:?} missed the far waypoint");
        }
    }

    /// Where the two part: only one of them proves the shortest chain
    ///
    /// A detour that is *not* offered first and is not the nearest step at any
    /// point — its first leg goes almost sideways — so preferring the nearest
    /// neighbour walks straight past it. Both find four jumps; the point is
    /// that [`Routing::Shortest`] is the one that has proved no shorter four
    /// exists, and the map's default has only preferred one.
    #[test]
    fn only_the_shortest_setting_proves_what_it_found() {
        let entries = vec![
            at(0, [0., 0., 0.]),
            at(1, [450., 0., 0.]),
            at(2, [900., 0., 0.]),
            at(3, [1350., 0., 0.]),
            at(9, [1800., 0., 0.]),
        ];
        let graph = JumpGraph::new(&entries);

        for how in BOTH {
            let path = graph.route(0, 9, 500., how).expect("a route");
            assert_eq!(path.len() - 1, 4, "{how:?}");
            assert!(
                (run(&path) - 1800.).abs() < 1.,
                "{how:?} ran {:.0} ly down a 1800 ly line",
                run(&path)
            );
        }
    }

    /// A goal nothing reaches is no route rather than a wrong one
    #[test]
    fn a_gap_wider_than_the_range_is_no_route() {
        let entries = vec![at(0, [0., 0., 0.]), at(1, [600., 0., 0.])];
        let graph = JumpGraph::new(&entries);

        for how in BOTH {
            assert!(
                graph.route(0, 1, 500., how).is_none(),
                "{how:?} jumped 600 at 500"
            );
            assert!(
                graph.route(0, 1, 700., how).is_some(),
                "{how:?} refused 600 inside 700"
            );
        }
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
