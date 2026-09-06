//! Which order to reach a set of destinations in
//!
//! A trip through stops is flown in the order they were picked; a set of
//! destinations is picked in no order at all, and what is wanted is the
//! cheapest way to reach every one of them. That is the open travelling
//! salesman problem: no returning to the start, every destination visited
//! once.
//!
//! The cost is estimated rather than routed. A leg's real cost is the jumps
//! the router comes back with, and asking for those first would mean walking
//! the graph between every pair before anything is drawn -- `n(n-1)/2` walks,
//! which at eight destinations is twenty-eight of them and about five seconds
//! of nothing happening. So a leg is costed at `ceil(distance / range)`, which
//! is the same figure the router's own heuristic uses and can never overstate
//! the jumps a leg takes.
//!
//! Being a lower bound, it can be wrong in one direction: a leg that crosses
//! empty space is flown in more jumps than the estimate, and where two
//! candidate orders are close the sparser one may be chosen. What comes back
//! is a good order rather than a proven one, which is why nothing here claims
//! the trip is optimal — the stops are handed to the same fan-out an ordered
//! trip uses, and the user can reorder by picking again.

use bevy::math::DVec3;

/// How many destinations are ordered exactly
///
/// Held-Karp is `2^n · n^2`, which at ten is about a hundred thousand steps
/// and lands inside a frame. Eleven is a quarter of a million and twelve is
/// six hundred thousand, still fast, but the table is `2^n · n` entries and
/// that is what runs away: ten costs 80 KB and sixteen costs 8 MB.
///
/// Past it the order is grown and then improved, which is not exact and does
/// not claim to be.
const EXACTLY: usize = 10;

/// A leg's cost: jumps first, then how far, in light years
///
/// Jumps decide, since a jump is what a trip is flown in and what it costs.
/// Distance breaks ties, and there are a great many ties: every leg under one
/// range costs one jump however short it is, so without the distance a set of
/// close destinations would be ordered arbitrarily.
type Cost = (u32, f64);

/// What a leg between `from` and `to` costs a ship reaching `range`
///
/// `ceil` because a leg is flown in whole jumps: a ship reaching 10 crosses
/// 11 light years in two of them. The same figure the router estimates with,
/// so the ordering and the routing agree about what is far.
fn leg(from: DVec3, to: DVec3, range: f64) -> Cost {
    let away = from.distance(to);

    ((away / range).ceil() as u32, away)
}

/// Add two costs
fn and(one: Cost, other: Cost) -> Cost {
    (one.0 + other.0, one.1 + other.1)
}

/// The order to reach every one of `places` in, as indices into it
///
/// The first place stays first. A trip has to set out from somewhere and
/// nothing on the map says where the ship is; the first thing picked out is
/// the one the user named first, and holding it still also means picking the
/// same set again gives the same answer.
///
/// Every place exactly once, so the answer is always a permutation of the
/// input however it was reached. Fewer than three places have only one order
/// and are handed straight back.
pub(crate) fn ordered(places: &[DVec3], range: f64) -> Vec<usize> {
    if places.len() < 3 || range <= 0. {
        return (0..places.len()).collect();
    }

    let legs: Vec<Vec<Cost>> = places
        .iter()
        .map(|from| places.iter().map(|to| leg(*from, *to, range)).collect())
        .collect();

    if places.len() <= EXACTLY {
        exactly(&legs)
    } else {
        improved(grown(&legs), &legs)
    }
}

/// The cheapest order there is, by Held-Karp
///
/// `best[mask][last]` is the cheapest way to have reached exactly the places
/// in `mask`, standing at `last`. The start is always in the mask and never
/// the last, so the table is over the other places alone.
///
/// Read back by walking the mask apart, which is why each entry keeps what it
/// came from.
fn exactly(legs: &[Vec<Cost>]) -> Vec<usize> {
    let rest = legs.len() - 1;
    let masks = 1usize << rest;
    // Cost, and which place it came from, per set and per place last stood
    // on. `None` is a set that cannot be reached that way.
    let mut best: Vec<Vec<Option<(Cost, usize)>>> =
        vec![vec![None; rest]; masks];

    // Sets in order of what they hold, so every smaller set a step reads
    // back has already been settled.
    for at in 1..masks {
        for last in 0..rest {
            if at & (1 << last) == 0 {
                continue;
            }
            let without = at & !(1 << last);
            // Straight out of the start, which is where a one-place set is
            // reached from.
            if without == 0 {
                best[at][last] = Some((legs[0][last + 1], usize::MAX));
                continue;
            }
            // Otherwise through whichever of the others it is cheapest to
            // have stood on before this one.
            let mut held: Option<(Cost, usize)> = None;
            for before in 0..rest {
                if without & (1 << before) == 0 {
                    continue;
                }
                let Some((so_far, _)) = best[without][before] else { continue };
                let through = and(so_far, legs[before + 1][last + 1]);
                if held.is_none_or(|(stood, _)| through < stood) {
                    held = Some((through, before));
                }
            }
            best[at][last] = held;
        }
    }

    let whole = masks - 1;
    // Any of them may be where the trip ends: nothing returns to the start,
    // so the last place is whichever is cheapest to finish on.
    let Some(mut last) = (0..rest)
        .filter(|last| best[whole][*last].is_some())
        .min_by(|one, other| {
            let (one, other) =
                (best[whole][*one].unwrap().0, best[whole][*other].unwrap().0);
            one.partial_cmp(&other).unwrap_or(std::cmp::Ordering::Equal)
        })
    else {
        return (0..legs.len()).collect();
    };

    let mut order = Vec::with_capacity(legs.len());
    let mut at = whole;
    while let Some((_, from)) = best[at][last] {
        order.push(last + 1);
        at &= !(1 << last);
        if from == usize::MAX {
            break;
        }
        last = from;
    }
    order.push(0);
    order.reverse();
    order
}

/// An order to start from: always on to the nearest place not yet reached
///
/// Cheap and never terrible, and [`improved`] is what makes it good.
fn grown(legs: &[Vec<Cost>]) -> Vec<usize> {
    let mut order = vec![0];
    let mut left: Vec<usize> = (1..legs.len()).collect();

    while !left.is_empty() {
        let at = *order.last().expect("a place to set out from");
        let (which, _) = left
            .iter()
            .enumerate()
            .min_by(|(_, one), (_, other)| {
                legs[at][**one]
                    .partial_cmp(&legs[at][**other])
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("somewhere left to go");
        order.push(left.remove(which));
    }
    order
}

/// Turn back any stretch of the order that costs less reversed
///
/// Two-opt: a trip that crosses itself is always dearer than the same trip
/// with the crossing undone, and reversing the stretch between the two legs
/// that cross is what undoes it. Run until nothing improves.
///
/// The start is held still, as [`ordered`] promises, so the stretches
/// considered begin at the second place.
fn improved(mut order: Vec<usize>, legs: &[Vec<Cost>]) -> Vec<usize> {
    let mut again = true;
    while again {
        again = false;
        for one in 1..order.len() - 1 {
            for other in one + 1..order.len() {
                let (before, from) = (order[one - 1], order[one]);
                let to = order[other];
                let after = order.get(other + 1).copied();

                let held = match after {
                    Some(after) => and(legs[before][from], legs[to][after]),
                    None => legs[before][from],
                };
                let turned = match after {
                    Some(after) => and(legs[before][to], legs[from][after]),
                    None => legs[before][to],
                };
                if turned < held {
                    order[one..=other].reverse();
                    again = true;
                }
            }
        }
    }
    order
}

/// What an order costs, all told
///
/// Only the tests ask, the ordering itself never needing the whole of a
/// candidate.
#[cfg(test)]
fn costing(order: &[usize], places: &[DVec3], range: f64) -> Cost {
    order.windows(2).fold((0, 0.), |so_far, step| {
        and(so_far, leg(places[step[0]], places[step[1]], range))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Places along the first axis, at each of `along`
    fn along(along: &[f64]) -> Vec<DVec3> {
        along.iter().map(|at| DVec3::new(*at, 0., 0.)).collect()
    }

    /// Every order of `places`, with the first held still
    fn every(places: usize) -> Vec<Vec<usize>> {
        let mut orders = Vec::new();
        let mut rest: Vec<usize> = (1..places).collect();
        permuted(&mut rest, 0, &mut orders);
        orders
    }

    fn permuted(rest: &mut Vec<usize>, at: usize, into: &mut Vec<Vec<usize>>) {
        if at == rest.len() {
            let mut order = vec![0];
            order.extend(rest.iter().copied());
            into.push(order);
            return;
        }
        for swap in at..rest.len() {
            rest.swap(at, swap);
            permuted(rest, at + 1, into);
            rest.swap(at, swap);
        }
    }

    /// A set strung out along a line is reached by walking the line
    ///
    /// Picked in an order that jumps about, which is what an unordered set
    /// is: nothing about how it was gathered says how it should be flown.
    #[test]
    fn a_line_of_destinations_is_walked_in_order() {
        let places = along(&[0., 30., 10., 20.]);

        assert_eq!(ordered(&places, 10.), vec![0, 2, 3, 1]);
    }

    /// The first picked stays first
    ///
    /// A trip sets out from somewhere and nothing on the map says where the
    /// ship is, so the one the user named first is where it starts -- even
    /// where starting elsewhere would be cheaper. Setting out from the end of
    /// this line and walking it would cost three jumps; held to the middle,
    /// the best there is costs four.
    #[test]
    fn the_first_destination_stays_where_it_was_put() {
        let places = along(&[20., 0., 10., 30.]);
        let range = 10.;

        let order = ordered(&places, range);

        assert_eq!(order[0], 0);
        assert_eq!(order, vec![0, 3, 2, 1]);
        assert_eq!(costing(&order, &places, range), (4, 40.));
        assert_eq!(costing(&[1, 2, 0, 3], &places, range), (3, 30.));
    }

    /// Every destination is reached, exactly once
    ///
    /// Whatever the order costs, an answer that dropped one or said one twice
    /// would be a trip to somewhere else.
    #[test]
    fn every_destination_is_reached_once() {
        for count in 2..14 {
            let places: Vec<DVec3> = (0..count)
                .map(|at| {
                    let at = at as f64;
                    DVec3::new(at * 7. % 23., at * 13. % 17., at * 3. % 11.)
                })
                .collect();

            let mut order = ordered(&places, 10.);
            order.sort();

            assert_eq!(order, (0..count).collect::<Vec<_>>(), "{count} places");
        }
    }

    /// Up to ten it is the cheapest order there is
    ///
    /// Held against every order of the same places, the first held still.
    #[test]
    fn a_small_set_is_ordered_as_cheaply_as_it_can_be() {
        let places = vec![
            DVec3::new(0., 0., 0.),
            DVec3::new(14., 3., 0.),
            DVec3::new(4., 19., 2.),
            DVec3::new(21., 17., 5.),
            DVec3::new(9., 8., 11.),
            DVec3::new(30., 2., 7.),
        ];
        let range = 8.;

        let mine = costing(&ordered(&places, range), &places, range);
        let best = every(places.len())
            .into_iter()
            .map(|order| costing(&order, &places, range))
            .min_by(|one, other| {
                one.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("an order");

        assert_eq!(mine, best);
    }

    /// A set too large to order exactly is still ordered well
    ///
    /// Not proven cheapest -- nothing here claims that past ten -- but no
    /// worse than walking to the nearest unreached place every time, which is
    /// what it starts from.
    #[test]
    fn a_large_set_is_no_worse_than_where_it_started() {
        let places: Vec<DVec3> = (0..14)
            .map(|at| {
                let at = at as f64;
                DVec3::new(at * 11. % 37., at * 5. % 29., at * 17. % 41.)
            })
            .collect();
        let range = 9.;

        let legs: Vec<Vec<Cost>> = places
            .iter()
            .map(|from| {
                places.iter().map(|to| leg(*from, *to, range)).collect()
            })
            .collect();
        let started = costing(&grown(&legs), &places, range);
        let mine = costing(&ordered(&places, range), &places, range);

        assert!(mine <= started, "{mine:?} against {started:?}");
    }

    /// Two of them have one order, and are handed back in it
    #[test]
    fn a_pair_has_nothing_to_order() {
        assert_eq!(ordered(&along(&[10., 0.]), 10.), vec![0, 1]);
        assert_eq!(ordered(&along(&[0.]), 10.), vec![0]);
        assert_eq!(ordered(&[], 10.), Vec::<usize>::new());
    }

    /// A range that says nothing orders nothing
    ///
    /// The cost is jumps, and a ship that reaches nowhere makes none of them.
    /// The set comes back as it was rather than in some order arrived at by
    /// dividing by zero.
    #[test]
    fn a_ship_that_reaches_nowhere_leaves_the_set_alone() {
        let places = along(&[0., 30., 10.]);

        assert_eq!(ordered(&places, 0.), vec![0, 1, 2]);
    }

    /// Jumps decide before distance
    ///
    /// A ship reaching 10 crosses 11 light years in two jumps and 10 in one,
    /// so the shorter leg is the cheaper however little separates them. Here
    /// the far place is reached first because doing so costs one jump fewer
    /// all told.
    #[test]
    fn a_jump_saved_beats_a_light_year_saved() {
        let range = 10.;

        assert_eq!(leg(DVec3::ZERO, DVec3::new(10., 0., 0.), range).0, 1);
        assert_eq!(leg(DVec3::ZERO, DVec3::new(10.1, 0., 0.), range).0, 2);
    }
}
