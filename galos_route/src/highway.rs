//! The boost stars alone, and the coarse graph a long route is planned on.
//!
//! A supercharged crossing of the galaxy is what the flat search in
//! [`crate::graph`] is worst at. The reach of a boosted jump is four times
//! the range, so its sphere holds sixty-four times the systems, and the
//! estimate has to divide by that widest jump everywhere to stay honest
//! about the fewest — while two systems in a hundred can actually offer one.
//! Measured on the real 200 M-system index, Sol to Colonia charged: the
//! flat search takes **610 s** to prove its 137 jumps. The plan below takes
//! 6.5 s to find 139.
//!
//! What the route is actually made of is a chain of jet cones. So this holds
//! the boost stars as a graph of their own — 3,846,802 nodes of 200,071,629
//! systems, on a 250 light year grid because its queries are hundreds of
//! light years wide rather than tens — and an edge is one supercharged hop,
//! or a supercharged hop plus a few ordinary jumps where the next cone is
//! further out than one hop reaches. A coarse plan over that is a chain of
//! waypoints; [`crate::graph::JumpGraph`] then flies each leg with the
//! search it already has.
//!
//! **What the chain is not is proven.** A coarse edge says how few jumps
//! *could* cross a gap, not that a chain of systems exists to cross it that
//! way, and the boost stars it steps through are a guess at which cones are
//! worth taking. Which is why only [`crate::graph::Routing::QUICK`] plans
//! here: a setting that claims the fewest jumps cannot be answered off a
//! plan over two per cent of the galaxy. See
//! [`crate::graph::Routing::highway`].
//!
//! The same shape as EDDA's `long_range.rs`, read against its
//! implementation, and the numbers agree where they can be compared: its
//! sub-index holds 3,853,782 boost stars over 199 M systems on the same 250
//! light year grid, and 475 k occupied cells against our 475,068. What is
//! not here is everything its fuel model buys — the scoop edges, the tank
//! carried in the coarse state, the refuel rounds over the refined legs —
//! because a galos route is jumps and a ship's range, with no tank in it.

use crate::Boosts;
use crate::graph::{Drive, Routing, Sampler, Tuning, WHOLE, Weigh};
use galos_index::meta::Boost;
use glam::DVec3;
use rustc_hash::FxHashMap;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// How wide the cells of the highway's own grid are, in light years.
///
/// Sized against its own queries and not against a jump: a coarse hop
/// reaches a boosted jump plus a few ordinary ones, some four hundred light
/// years for a standard drive at fifty, so a query sweeps a handful of cells
/// to a side. At the 64 ly the flat search buckets by, the same query would
/// be thousands of cell lookups. EDDA sizes its own sub-index at 250 for
/// the same reason (`long_range.rs:59-61`).
pub const CELL_LY: f64 = 250.0;

/// How wide the gaps in the highway are to begin with, in light years
///
/// Not read by anything — [`Tuning::reach`] is the setting, and this is
/// where its default comes from. The one number the data decides.
///
/// Flooding the real table from Sol with supercharged hops alone — 200
/// light years for a standard drive at fifty — reaches **135 of
/// 3,846,802** boost stars and stops 196 light years out: the chain of
/// cones is not connected at a single hop, anywhere. Allowing four
/// ordinary jumps after each hop, which at 50 ly is a 400 ly reach,
/// reaches **2,419,398** of them, out past 61,000 light years.
///
/// **Where the cliff is, is a fact about the cones and not about the
/// ship**, which is why this is a distance: at 350 ly the plan fails
/// outright at 10, 25 *and* 50 ly of range, and at 380 all three plan.
/// EDDA's `MAX_BRIDGE_JUMPS` is four jumps with no such scaling
/// (`long_range.rs:27`), which at 25 ly is a 200 ly reach and no plan at
/// all.
///
/// **Five hundred, and four hundred was too low.** A wider reach only
/// *adds* edges to the cone graph — a hop may be any distance up to it —
/// so it can never cost a route stops, and what it does cost is the scan
/// width of one coarse expansion. Measured over `.index/full` at 45 ly
/// with a standard drive, whole routes:
///
/// | reach | Sol → 2 kly | Sol → 3 kly | Sol → Colonia | Sol → 16 kly |
/// |---|---|---|---|---|
/// | 405 ly | 60 stops, **32.3 s** | 67, **50.3 s** | 166, 214 ms | 137, 418 ms |
/// | 450 ly | 47, 33.6 ms | 54, 8.1 ms | 166, 97 ms | 137, 423 ms |
/// | **495 ly** | **32, 6.2 ms** | **40, 4.8 ms** | **164, 101 ms** | **136, 473 ms** |
/// | 585 ly | 32, 5.8 ms | 40, 5.6 ms | 164, 116 ms | 136, 594 ms |
/// | 765 ly | 31, 6.9 ms | 39, 5.5 ms | 164, 175 ms | 136, 913 ms |
///
/// Monotone in stops on every corridor, and the curve flattens at 495:
/// everything above it buys one stop in thirty and costs 13% at 16 kly
/// and 74% at 765. What the old four hundred did on the two short
/// corridors was not plan slowly — the coarse search never reached the
/// goal, spent [`Tuning::stall`], handed over the seven cones it had, and
/// the stretch left became one enormous gap for the legs to fly. Five
/// hundred is the same sort of number as four hundred, measured against
/// routes rather than against a flood out of one system.
pub const GAPS_LY: u32 = 500;

/// How many ordinary jumps wider the plan may try when a reach does not
/// connect
///
/// **The escape hatch that used to be a rail.** No reach is right
/// everywhere: the cones join up between 360 and 380 ly out of the bubble
/// and at 225 or less in the sampled deep, and a corridor whose own
/// bottleneck is wider than [`GAPS_LY`] has no chain at all — which costs
/// seconds to minutes, the leftover stretch falling to the legs or to the
/// flat galaxy-wide search. So a plan that does not close on the goal is
/// tried again a jump wider, and again, up to this many rungs. See
/// [`Highway::plan`].
///
/// A rung is only ever paid where the narrower one failed, and what it
/// costs there is that failure's own stall. Eight, which is the travel the
/// panel's rail used to offer before it was replaced by this: at 45 ly it
/// carries a 500 ly reach out to 860.
const RUNGS: u32 = 8;

/// Ordinary jumps the first coarse hop may spend, the highway rarely
/// starting at the door.
///
/// Reached by widening rather than by sweeping sixty jumps of sphere at
/// every start: the first scan is [`Tuning::reach`] and this is the last
/// resort, so a start in the bubble pays a small query and one out in the
/// dark pays for what it is.
const START_BRIDGE: u32 = 60;

/// And the last, closing the approach from the final cone.
///
/// Shorter than the start's on purpose: a goal off the highway is flown to
/// by the refined leg, and a coarse edge that may run twelve jumps is
/// already long enough to reach past whatever the last cone left.
const GOAL_BRIDGE: u32 = 12;

/// How long a coarse search may make no progress before it gives up.
///
/// The bound on the answer that is not a route. A crossing that is going
/// somewhere improves steadily — Sol to Colonia settles in 81,034
/// expansions — where a goal the highway cannot reach expands the whole
/// connected component, 2.4 M nodes and some tens of seconds, to say so.
/// Measured against progress rather than against a total, so a long
/// crossing is never cut short for being long.
pub const STALL: u64 = 200_000;

/// Expansions an exact coarse plan may spend before the plan is worked
/// leaned instead.
///
/// Not read by anything — [`Tuning::allowance`] is the setting, and this
/// is where its default comes from. Measured over `.index/full` from Sol
/// at 45 ly, an exact plan either lands in 512 expansions or wants
/// between 97,792 and 276,480 of them, so every corridor sampled is a
/// factor of forty-eight either side of this. See [`Highway::plan`].
pub const ALLOWANCE: u64 = 2_048;

/// The start and the goal, which are systems rather than boost stars.
///
/// Nodes of the coarse graph like any other, so the plan is one search
/// rather than a search with two special cases stitched either side. The
/// top of the index space, which a table of four million cannot reach.
const START: u32 = u32::MAX - 1;
const GOAL: u32 = u32::MAX;

/// The boost stars, cell-sorted, and the directory over them.
pub struct Highway {
    /// Where each boost star sits, in cell order.
    ///
    /// At the `f32` the names table carries, which is what these are read
    /// out of. A coarse plan is waypoints on a 250 light year grid and the
    /// leg that flies to one finds the record by descending to the place
    /// ([`galos_index::Sky::node_of`]), so a light year's worth of
    /// rounding changes nothing and the table is half the bytes.
    place: Vec<[f32; 3]>,
    /// Which system each is, parallel to [`Self::place`].
    address: Vec<i64>,
    /// What each supercharges, parallel to [`Self::place`].
    boost: Vec<Boost>,
    /// The run of [`Self::place`] each occupied cell holds.
    cells: FxHashMap<[i32; 3], (u32, u32)>,
}

impl Highway {
    /// The boost stars of a published supercharge table, sorted into cells.
    ///
    /// [`None`] where the index publishes no table at all, or publishes an
    /// empty one — there is then no coarse graph and nothing plans on one.
    ///
    /// Nothing is joined and nothing is looked up: the published rows carry
    /// the place ([`galos_index::SystemBoost`]), so all this does is sort
    /// four million of them into cells, measured at 213 ms. It used to find
    /// the places by walking the names table's whole address column — 4 GB
    /// of mapping faulted and 7.9 s before a galactic route could begin
    /// planning, once a session, and the click sat there while it did.
    pub fn over(boosts: &Boosts) -> Option<Highway> {
        if !boosts.published() {
            return None;
        }
        let rows = boosts.table();
        if rows.is_empty() {
            return None;
        }
        // Sorted by cell so a cell's stars are a run of the arrays and a
        // query reads them in one sweep, which measured 1.6x the neighbour
        // query against an address-ordered directory.
        let mut order: Vec<(u64, u32)> = rows
            .iter()
            .enumerate()
            .map(|(at, row)| (key_of(row.position), at as u32))
            .collect();
        order.sort_unstable_by_key(|&(key, _)| key);
        let mut cells: FxHashMap<[i32; 3], (u32, u32)> = FxHashMap::default();
        for (at, &(_, which)) in order.iter().enumerate() {
            let cell = cell_of(rows[which as usize].position);
            let run = cells.entry(cell).or_insert((at as u32, 0));
            run.1 += 1;
        }
        Some(Highway {
            place: order
                .iter()
                .map(|&(_, which)| rows[which as usize].position)
                .collect(),
            address: order
                .iter()
                .map(|&(_, which)| rows[which as usize].address)
                .collect(),
            boost: order
                .iter()
                .map(|&(_, which)| rows[which as usize].boost)
                .collect(),
            cells,
        })
    }

    /// How many boost stars it holds.
    pub fn len(&self) -> usize {
        self.place.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.place.is_empty()
    }

    /// How many cells of the grid are occupied.
    #[cfg(test)]
    pub fn cells(&self) -> usize {
        self.cells.len()
    }

    /// Where a coarse node sits, the two ends included.
    fn at(&self, node: u32, from: [f64; 3], to: [f64; 3]) -> [f64; 3] {
        match node {
            START => from,
            GOAL => to,
            node => widen(self.place[node as usize]),
        }
    }

    /// Every boost star within `radius` light years of `at`.
    ///
    /// The cells the sphere touches, each pruned by its nearest corner
    /// before a single star is read, then the stars of the survivors
    /// measured. No allocation: a coarse expansion sweeps hundreds of
    /// cells and there are tens of thousands of expansions in a crossing.
    fn each_near(
        &self,
        at: [f64; 3],
        radius: f64,
        mut found: impl FnMut(u32, [f64; 3], f64),
    ) {
        let span = (radius / CELL_LY).ceil() as i32;
        let home = cell_of([at[0] as f32, at[1] as f32, at[2] as f32]);
        let reach = radius * radius;
        for dx in -span..=span {
            for dy in -span..=span {
                for dz in -span..=span {
                    let cell = [home[0] + dx, home[1] + dy, home[2] + dz];
                    if corner_away(cell, at) > reach {
                        continue;
                    }
                    let Some(&(from, count)) = self.cells.get(&cell) else {
                        continue;
                    };
                    for node in from..from + count {
                        let place = widen(self.place[node as usize]);
                        let away = dist2(at, place);
                        if away <= reach {
                            found(node, place, away.sqrt());
                        }
                    }
                }
            }
        }
    }

    /// Which systems within `radius` light years of `at` can supercharge,
    /// by address and in ascending order
    ///
    /// **What keeps the fanout cap affordable in a dense sky.** The cap
    /// keeps cones before ordinary systems, so it has to know which
    /// candidates are cones — and asking that of the supercharge table is a
    /// binary search of 3.8 M rows a candidate. Measured over
    /// `.index/full`, one charged expansion at Sagittarius A\*: the sphere
    /// holds 538,898 systems and the per-candidate lookups took **32.78 ms
    /// to find that none of them is a cone**, against 4.10 ms to sweep
    /// their places.
    ///
    /// This answers the same question of the cell buckets these stars are
    /// already sorted into: tens of cells rather than millions of rows, and
    /// where it comes back empty — which is most of the galaxy, the core
    /// included — the caller has no candidate to look up at all.
    ///
    /// Ascending by address, so a caller tests membership by binary search
    /// over the handful this hands back — and their places come with them,
    /// because a cell that holds a cone is a cell no nearness rule may
    /// skip.
    pub fn cones_near(
        &self,
        at: [f64; 3],
        radius: f64,
    ) -> Vec<(i64, [f64; 3])> {
        let mut found = Vec::new();
        self.each_near(at, radius, |node, place, _| {
            found.push((self.address[node as usize], place))
        });
        found.sort_unstable_by_key(|&(address, _)| address);
        found
    }

    /// The boost star nearest `to` within `within` light years, if any.
    #[cfg(test)]
    pub fn nearest(&self, to: [f64; 3], within: f64) -> Option<i64> {
        let mut best: Option<(f64, u32)> = None;
        self.each_near(to, within, |node, _, away| {
            if best.is_none_or(|(held, _)| away < held) {
                best = Some((away, node));
            }
        });
        best.map(|(_, node)| self.address[node as usize])
    }

    /// A run of coarse nodes as the cones the caller flies by
    ///
    /// The sentinels are not cones and are not in the table, so a chain is
    /// only ever the real nodes of it: the caller knows its own two ends.
    fn cones(&self, chain: Vec<u32>) -> Vec<(i64, [f64; 3])> {
        chain
            .into_iter()
            .map(|node| {
                (self.address[node as usize], widen(self.place[node as usize]))
            })
            .collect()
    }

    /// What a stalled search has to hand over: the chain to `nearest`
    ///
    /// [`Planned::Nothing`] where it closed on nothing — a plan that got
    /// no nearer than the door is no plan, and the caller searches flat as
    /// it always did.
    ///
    /// The sentinels go, as [`Reached::chain`] drops them: the start is the
    /// caller's own and is not a row of the table, so indexing it would
    /// read off the end.
    fn chained(
        &self,
        reached: &Reached,
        nearest: Option<u32>,
        closest: f64,
        closed: bool,
    ) -> Planned {
        let Some(nearest) = nearest.filter(|_| closed) else {
            return Planned::Nothing;
        };
        let mut chain = reached.chain_to(nearest);
        chain.retain(|&node| node != START && node != GOAL);
        Planned::Stalled { cones: self.cones(chain), closest }
    }

    /// The chain of cones between two places: which boost stars to fly by,
    /// in order.
    ///
    /// **A ladder of reaches, each a plan of its own.** No gap width is
    /// right everywhere — the cones join up between 360 and 380 light
    /// years out of the bubble and at 225 or less in the sampled deep —
    /// and a corridor whose own bottleneck is wider than the reach in hand
    /// does not fail cheaply: the coarse search never closes on the goal,
    /// spends [`Tuning::stall`], hands over the cones it did reach, and
    /// the stretch left over becomes one enormous gap for the legs.
    /// Measured at 45 ly, Sol to a system 2 kly out: **60 stops in 32.3 s
    /// at a 405 ly reach against 32 stops in 6.2 ms at 495**.
    ///
    /// So a plan that does not close is tried again one ordinary jump
    /// wider, up to [`RUNGS`] of them. A wider reach only *adds* edges to
    /// the cone graph, so a rung can never answer worse than the one below
    /// it; what it costs is the scan width of an expansion, and it is only
    /// ever paid where the narrower reach had already failed. This is what
    /// the panel's `Gap allowed` rail used to be, and the reason it is no
    /// longer a question anybody is asked.
    ///
    /// **Two passes at each rung, because how hard the plan is worked is
    /// its own setting.** [`Tuning::planning`] says what the coarse
    /// estimate is leaned by and opens at *exact*, which is worth nothing
    /// to six percent of a route's stops for two to nineteen times the
    /// wait — decided by how large the cone plateau beside the corridor
    /// is, which no distance predicts. So it is tried rather than
    /// promised: the exact pass runs first inside [`Tuning::allowance`]
    /// expansions, and where it runs past that the plan is worked again
    /// leaned by the route's own percent ([`Routing::over`]), which is
    /// what this did before there was a setting. Measured, that costs
    /// 40 ms of dropped search at the worst.
    ///
    /// A chain either pass *stalls* into is kept rather than returned: the
    /// rung above may close on the goal outright, and only if none does is
    /// the closest stalled chain the answer — which is still a plan for
    /// most of the way and much better than the flat fallback.
    ///
    /// Every pass draws into the one sampler, so the ladder carries the
    /// picture on rather than restarting it: what [`Sampler::reached`]
    /// keeps is the closest anything has come, which only ever improves.
    ///
    /// `boost` is what the *start* can supercharge, which the caller knows
    /// and this table does not: a route can begin in a system with a cone
    /// and the first hop is then a long one.
    ///
    /// [`None`] where no rung connects the two, which is the answer for a
    /// goal the highway does not reach and the one the caller falls back
    /// to the flat search on.
    #[allow(clippy::too_many_arguments)]
    pub fn plan(
        &self,
        from: ([f64; 3], Option<Boost>),
        to: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        tune: Tuning,
        mut watched: Option<&mut Sampler>,
    ) -> Option<Vec<(i64, [f64; 3])>> {
        if self.is_empty() {
            return None;
        }
        // How many ordinary jumps a hop may add to span [`Tuning::reach`],
        // measured off the *widest* hop the drive could make rather than
        // off each system's own cone. One number for the plot: derived per
        // node instead, a white dwarf or an unboosted start is allowed a
        // far wider scan to reach the same distance, and the 50 ly
        // crossing measured 4.4 s against 2.2 s for one jump in a hundred
        // and forty. At 50 ly a 500 ly reach is six of them; at 25 it is
        // eighteen, which is the same five hundred light years.
        //
        // At least one, because a hop *is* a supercharged jump and the
        // ordinary ones after it: asked for less, the reach would quietly
        // be more than the setting said.
        let widest = range * drive.widest();
        let bridge =
            ((tune.reach as f64 - widest) / range).ceil().max(1.) as u32;

        // The closest a stalled rung came, and the chain it had. Kept
        // rather than returned: a rung above may close outright.
        let mut closest = f64::INFINITY;
        let mut stalled = Vec::new();
        for rung in 0..=RUNGS {
            match self.passes(
                from,
                to,
                range,
                drive,
                how,
                tune,
                bridge + rung,
                watched.as_deref_mut(),
            ) {
                Planned::Chain(cones) => return Some(cones),
                // **A rung that comes no closer ends the climb**, which is
                // the same progress rule [`Tuning::stall`] is one level
                // down, and it is what keeps a corridor no reach can plan
                // from paying for all of them. Measured over
                // `.index/full` at 45 ly, the plan alone, rung by rung:
                //
                // ```text
                // far rim   0: stalled 5,460 Ly, 6.19 s
                //           1: stalled 4,574 Ly, 7.74 s
                //         2-8: stalled 4,574 Ly, 48 s for nothing
                // under     0: stalled 6,090 Ly, 1.40 s
                //         1-8: stalled 6,090 Ly, 20 s for nothing
                // ```
                //
                // So the far rim is 21 s against 63 and the corridor
                // under the disc 3.0 s against 21.9, both keeping the
                // chain they kept before — and a corridor that closes on
                // the goal never gets here, Colonia answering off rung
                // zero in 89 ms.
                Planned::Stalled { cones, closest: came } => {
                    if came >= closest {
                        break;
                    }
                    closest = came;
                    stalled = cones;
                }
                // Nothing reached at this reach: a wider scan may still
                // find a cone the narrower one could not, so the ladder
                // goes on. It is the cheap answer too — a start with no
                // cone in reach empties the heap at once.
                Planned::Nothing | Planned::Spent => {}
            }
        }
        (!stalled.is_empty()).then_some(stalled)
    }

    /// One rung of the ladder: the exact plan if it lands inside its
    /// allowance, and the leaned one if it does not
    #[allow(clippy::too_many_arguments)]
    fn passes(
        &self,
        from: ([f64; 3], Option<Boost>),
        to: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        tune: Tuning,
        bridge: u32,
        mut watched: Option<&mut Sampler>,
    ) -> Planned {
        let asked = tune.weight();
        let leaned = how.weight();
        let allowance = tune.allowance.filter(|_| asked < leaned);
        match self.coarse(
            from,
            to,
            range,
            drive,
            how,
            tune,
            bridge,
            asked,
            allowance,
            watched.as_deref_mut(),
        ) {
            // The allowance spent, which only an exact-enough pass can do:
            // the plan the route's own percent asks for, as it always was.
            Planned::Spent => self.coarse(
                from, to, range, drive, how, tune, bridge, leaned, None,
                watched,
            ),
            answered => answered,
        }
    }

    /// One pass of the coarse search, its estimate times `weight` and its
    /// expansions inside `allowance`
    ///
    /// A\* over the coarse graph: a hop costs the jumps it takes, the
    /// estimate divides the distance left by the widest jump the drive
    /// could make, and that estimate is weighted by `weight` against
    /// [`WHOLE`] for a hop — the route's own percent over the fewest on
    /// the cheap pass, and [`Tuning::planning`] on the one that tries to
    /// better it.
    ///
    /// **How much leaning costs, measured over the real index** (Sol to
    /// Colonia, jumps flown after the legs, warm):
    ///
    /// | the estimate times | 50 ly | 80 ly |
    /// |---|---|---|
    /// | 1 — exact in the coarse graph | 139 in 6.7 s | 80 in 9.4 s |
    /// | **21/20** | **140 in 2.0 s** | **81 in 0.7 s** |
    /// | 11/10 | 142 in 1.8 s | 83 in 0.6 s |
    /// | 6/5 | 148 in 1.8 s | 87 in 0.7 s |
    /// | 3/2 | 163 in 1.7 s | 95 in 1.4 s |
    ///
    /// The knee is at the first row past exact and it is sharp: a
    /// twentieth buys 3.3× at 50 ly and **13× at 80** for one jump, and
    /// everything past it buys nothing at all — a fifth costs eight more
    /// jumps than a twentieth and saves fifty milliseconds. Which is the
    /// travel [`Tuning::planning`]'s own rail is drawn over, and why the
    /// exact row at the top of it is worth trying for: the knee is one
    /// notch wide.
    ///
    /// **Thinning is still a loss**, separately measured: the 64 cones
    /// nearest the goal made the search *bigger* — 461,462 expansions
    /// against 178,910, and 160 jumps against 139 — because the hop a
    /// chain needs is often the one pointing sideways at a gap. So is a
    /// goal-direction prune: 6.5 s to 7.6 s at 50 ly for the same route.
    ///
    /// [`Planned::Nothing`] where no chain of hops connects the two, which
    /// is the answer for a goal the highway does not reach and the one the
    /// caller falls back to the flat search on, and where the route was
    /// taken back while this was still planning.
    #[allow(clippy::too_many_arguments)]
    fn coarse(
        &self,
        from: ([f64; 3], Option<Boost>),
        to: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        tune: Tuning,
        bridge: u32,
        weight: u32,
        allowed: Option<u64>,
        mut watched: Option<&mut Sampler>,
    ) -> Planned {
        let (start, boost) = from;
        let widest = range * drive.widest();
        // Weighted by `weight`: a hop costs [`WHOLE`] and the estimate is
        // multiplied by that plus whatever percent this pass leans by,
        // which keeps the arithmetic exact integers rather than a float
        // rounded twice.
        // **Whether the chain's own length is part of the ask.** [`hops`]
        // answers whole jumps, so a hop of one light year and a hop of two
        // hundred cost the same and a chain that wanders is charged
        // nothing for wandering. Until this the coarse plan did not read
        // [`Weigh`] at all, so *every* planned route was the fewest-jumps
        // chain whatever the form asked for — and a route asked for as the
        // shortest of the fewest jumps came back byte for byte identical
        // to one asked for in the fewest jumps alone. That is the half of
        // it that was wrong, and it is this.
        //
        // **Least fuel is not the other half, and the cap is why.**
        // [`crate::graph::burned`] charges a whole tank for any jump at or
        // past the ship's range, because that is what a maximum fuel per
        // jump means — so on a chain of cones every hop costs one tank
        // however far the jet throws the ship, and the fuel *is* the hop
        // count. There is nothing for a distance tie-break to save there.
        // Measured over the real index, Sagittarius A\* to Quemaae JN-K
        // D8-3 at 50 ly with a standard drive, it does not save anything:
        // 49 hops against 48 for the same tank a hop, which is one tank
        // worse inside what the leaning already allows. So a least-fuel
        // plan is the fewest-hops plan, and that is the right answer
        // rather than a missing feature.
        //
        // What none of it buys is a straight line. The cones are not on
        // one: the shortest chain the coarse graph admits inside ±200 ly
        // of the straight line costs 51 jumps against this plan's 47, and
        // only a ±800 ly corridor matches it. The bow is where the boost
        // stars are, and it is paid for.
        let flown = |d: f64| match how.weigh {
            Weigh::Jumps | Weigh::Fuel { .. } => 0.,
            Weigh::Shortest => d,
        };
        // Taken off a distance the caller already measured: the estimate
        // wants it in whole hops for the ordering and in light years for
        // the tie-break. Both understate what is left — the hops by
        // dividing by the widest jump the drive could make, the distance
        // by being the straight line — so the chain that comes back is
        // the fewest hops there are and, of those, the shortest. See
        // [`Coarse`].
        let estimate = |left: f64| Coarse {
            jumps: ((left / widest).ceil() as u32).saturating_mul(weight),
            flown: flown(left) as u32,
        };
        // What the search has reached, and what is left to expand.
        let mut reached = Reached::over(self.len());
        let opening = dist2(start, to).sqrt();
        reached.begin(estimate(opening));

        // The stall rule: how close anything has come, when, and which cone
        // it was. A search whose frontier has stopped closing on the goal
        // for [`Tuning::stall`] expansions is one the highway cannot
        // finish — and what it *has* reached is worth handing over all the
        // same; see below.
        let began = opening;
        let mut closest = f64::INFINITY;
        let mut nearest = None;
        let mut since = 0u64;
        let mut expansions = 0u64;

        while let Some((was, node)) = reached.pop() {
            if node == GOAL {
                return Planned::Chain(self.cones(reached.chain()));
            }

            // Told to give up, the route having been taken back while it
            // was still being planned. Seconds of a pool thread otherwise,
            // with nobody left to read the chain. See
            // [`crate::graph::Frontier::abandon`].
            if watched.as_ref().is_some_and(|drawn| drawn.stopped()) {
                return Planned::Nothing;
            }

            let at = self.at(node, start, to);
            let left = dist2(at, to).sqrt();
            expansions += 1;
            // What the allowance bounds, and the whole of why an exact
            // pass can be tried at all: a pass that has spent more than it
            // was allowed hands back [`Planned::Spent`] and the caller
            // keeps the chain it already has. The cheap pass is handed no
            // allowance and never reaches this.
            if allowed.is_some_and(|allowance| expansions > allowance) {
                return Planned::Spent;
            }
            // Drawn while it runs, as the flat search is. The plan is most
            // of the wait on a galactic route and it used to draw nothing:
            // seconds of an empty sky before the legs began. The chain is
            // the cones back to the start, which is what the search would
            // hand over if this cone were the last one.
            if let Some(sampler) = watched.as_deref_mut() {
                sampler.reached(DVec3::from(at), || {
                    reached
                        .chain_to(node)
                        .into_iter()
                        .map(|node| DVec3::from(self.at(node, start, to)))
                        .collect()
                });
            }
            if left < closest {
                closest = left;
                nearest = Some(node);
                since = expansions;
            } else if expansions - since > tune.stall {
                // **Stalled, not failed.** The chain to the closest cone
                // reached is a plan for most of the way, and throwing it
                // away hands a galactic crossing to the flat search — which
                // is minutes. Reported exactly that way: a 45 ly Colonia to
                // Sgr A\* plot at five percent over stalled here, answered
                // nothing, and the flat search spent **227 s**, where the
                // same plot at twenty-five percent planned in 0.93 s.
                //
                // The stretch left over becomes the last hop of the plan,
                // and is crossed by whatever
                // [`crate::graph::Tuning::crossing`] says — stepping first,
                // which is cheap over any distance, and the search only if
                // that cannot close. It is EDDA's rule: truncate the coarse
                // chain at the closest node reached and hand the rest on
                // (`long_range.rs:3273-3281`).
                //
                // Nothing to hand over if the search never closed on the
                // goal at all — no cone was nearer than the door — and the
                // caller then falls back as it did.
                return self.chained(
                    &reached,
                    nearest,
                    closest,
                    closest < began,
                );
            }

            // What one hop out of here reaches: the cone this system has,
            // or the range alone where it has none.
            let charged = range
                * match node {
                    START => drive.factor(boost),
                    GOAL => 1.,
                    node => drive.factor(Some(self.boost[node as usize])),
                };

            // The goal, where the ordinary jumps after the hop can close
            // the rest of the way to it.
            if let Some(step) =
                hops(left, charged, range, GOAL_BRIDGE.max(bridge))
            {
                reached.relax(
                    GOAL,
                    node,
                    was.and(step * WHOLE, flown(left)),
                    Coarse::NONE,
                );
            }

            // And every cone in reach. The start widens where its first
            // scan finds nothing: the highway rarely starts at the door,
            // and sweeping sixty jumps of sphere at every expansion to
            // find that out is what makes a coarse search slow.
            let widening = [bridge, bridge * 3, START_BRIDGE.max(bridge)];
            let mid = [bridge];
            let allowances: &[u32] =
                if node == START { &widening } else { &mid };
            for &allowance in allowances {
                let mut relaxed = false;
                self.each_near(
                    at,
                    charged + range * allowance as f64,
                    |next, place, away| {
                        // A cone at the place being expanded is no hop:
                        // the start carries its own cone already, and
                        // paying a jump to arrive where the search stands
                        // would price every plan out of a boost system one
                        // jump high.
                        if next == node || away < 0.01 {
                            return;
                        }
                        let Some(step) = hops(away, charged, range, allowance)
                        else {
                            return;
                        };
                        let step = step * WHOLE;
                        // The cone's own distance to the goal, which the
                        // estimate wants in whole hops and the tie-break
                        // wants as it is — one square root either way.
                        let left = dist2(place, to).sqrt();
                        relaxed |= reached.relax(
                            next,
                            node,
                            was.and(step, flown(away)),
                            estimate(left),
                        );
                    },
                );
                if relaxed {
                    break;
                }
            }
        }
        Planned::Nothing
    }
}

/// What one pass of the coarse search came back with
///
/// Four answers because [`Highway::plan`] may run a second pass and the
/// choice turns on *which* of them it is: a chain either way is the
/// answer, nothing is nothing however the plan is weighted, and only a
/// pass that spent its allowance is worth replacing with a leaned one.
enum Planned {
    /// A chain of cones that closed on the goal
    Chain(Vec<(i64, [f64; 3])>),
    /// The chain to the closest cone the search reached, its frontier
    /// having stopped closing on the goal for [`Tuning::stall`] expansions
    Stalled { cones: Vec<(i64, [f64; 3])>, closest: f64 },
    /// No chain at all: the highway does not join these two ends, or the
    /// route was taken back while this was still planning
    Nothing,
    /// The pass ran past the allowance it was handed and was dropped
    Spent,
}

/// What a coarse chain has cost: the hops, and how far they fly
///
/// **Two numbers because one of them prices nothing.** [`hops`] answers
/// whole jumps, so a hop of one light year and a hop of two hundred cost
/// the same — and a chain of forty-one hops that wanders five hundred
/// light years off the straight line costs *exactly* what a straight one
/// of forty-one hops costs. Nothing in the coarse search preferred the
/// straight one, which is why a route asked for as the shortest of the
/// fewest jumps used to come back as whichever chain of that many the
/// boost table's own ordering happened to reach first. See
/// [`Highway::plan`] for which asks read the second number and why least
/// fuel is not one of them.
///
/// The order of the fields is the whole of it, as it is in the flat
/// search's own [`crate::graph::Cost`]: a hop is never traded for any
/// amount of distance, and between two chains of the same hop count the
/// shorter one wins. Derived [`Ord`] compares them in that order.
///
/// Where the ask does not weigh distance the second number stays at
/// nothing for every chain, and the ordering is the hop count it always
/// was — the same answer, and the compare costs one more integer.
///
/// Light years as whole integers, which is finer than any chain worth
/// telling apart — the hops are hundreds of them — and keeps the key an
/// integer compare rather than a float one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Coarse {
    /// Hops, each [`WHOLE`] so the leaning is exact integer arithmetic.
    jumps: u32,
    /// How far the chain flies, in light years.
    flown: u32,
}

impl Coarse {
    /// Nothing spent, which is what the start has spent.
    const NONE: Coarse = Coarse { jumps: 0, flown: 0 };
    /// Dearer than any chain, which is what an unreached node holds.
    const UNREACHED: Coarse = Coarse { jumps: UNREACHED, flown: u32::MAX };

    /// This chain with one more hop of `step` on the end of it.
    fn and(self, step: u32, flown: f64) -> Coarse {
        Coarse {
            jumps: self.jumps.saturating_add(step),
            flown: self.flown.saturating_add(flown as u32),
        }
    }
}

/// What a coarse search has reached, and what is left to expand
///
/// Two flat arrays over the boost stars with the two ends on the end of
/// them — what each node has been reached in, and where from — beside the
/// heap of what to expand next. Eight bytes a star of cost and four of
/// parent is 46 MB over the real table, which is affordable exactly
/// because the coarse graph is two systems in a hundred: the flat search
/// cannot do this over 200 M, which is why it keeps a map of what it
/// reached instead. What it buys is the per-candidate hash gone — an
/// expansion measures hundreds of cones and asks this of every one of
/// them.
struct Reached {
    /// The cheapest chain anything has reached each node with.
    best: Vec<Coarse>,
    /// Which node each was reached from.
    came: Vec<u32>,
    /// Cheapest estimated total first, and of those the cheapest reached:
    /// a heap pops the greatest, so the whole key is reversed.
    open: BinaryHeap<Reverse<(Coarse, Coarse, u32)>>,
    /// How many boost stars there are, which is where [`START`] sits.
    nodes: usize,
}

impl Reached {
    /// Room for a graph of `nodes` boost stars and the two ends.
    fn over(nodes: usize) -> Reached {
        Reached {
            best: vec![Coarse::UNREACHED; nodes + 2],
            came: vec![UNREACHED; nodes + 2],
            open: BinaryHeap::new(),
            nodes,
        }
    }

    /// Which slot of the arrays a node keeps its cost in.
    ///
    /// The boost stars are their own indices and the two ends sit on the
    /// end, which is the whole of why the state can be an array at all:
    /// [`START`] and [`GOAL`] are named from the top of the index space so
    /// they cannot collide with a star, and they are the only two nodes
    /// that are not one.
    fn slot(&self, node: u32) -> usize {
        match node {
            START => self.nodes,
            GOAL => self.nodes + 1,
            node => node as usize,
        }
    }

    /// Stand at the start, `estimate` away from the goal.
    fn begin(&mut self, estimate: Coarse) {
        let at = self.slot(START);
        self.best[at] = Coarse::NONE;
        self.open.push(Reverse((estimate, Coarse::NONE, START)));
    }

    /// The next node to expand, and what it was reached in.
    ///
    /// A node can be pushed more than once, a cheaper way to it having
    /// been found after the first; the dearer entries are still in the
    /// heap and are nothing to expand again.
    fn pop(&mut self) -> Option<(Coarse, u32)> {
        while let Some(Reverse((_, was, node))) = self.open.pop() {
            if was <= self.best[self.slot(node)] {
                return Some((was, node));
            }
        }
        None
    }

    /// Take a hop, answering whether it was worth taking.
    fn relax(
        &mut self,
        next: u32,
        from: u32,
        cost: Coarse,
        estimate: Coarse,
    ) -> bool {
        let at = self.slot(next);
        if cost < self.best[at] {
            self.best[at] = cost;
            self.came[at] = from;
            self.open.push(Reverse((
                Coarse {
                    jumps: cost.jumps.saturating_add(estimate.jumps),
                    flown: cost.flown.saturating_add(estimate.flown),
                },
                cost,
                next,
            )));
            return true;
        }
        false
    }

    /// The chain that reached `node`, as the boost stars along it.
    ///
    /// What the picture draws while the plan is still running: the cones
    /// back to the start, in the order they would be flown. Bounded by the
    /// hops in it, and asked only when the plan has come closer than it
    /// ever had.
    fn chain_to(&self, node: u32) -> Vec<u32> {
        let mut chain = vec![node];
        loop {
            let before = self.came[self.slot(*chain.last().expect("a hop"))];
            if before == UNREACHED {
                break;
            }
            chain.push(before);
            if chain.len() > self.nodes + 2 {
                break;
            }
        }
        chain.reverse();
        chain
    }

    /// The chain that reached the goal, as the boost stars along it.
    ///
    /// The two ends are left out: the caller asked about them and has them
    /// already. What it wants is the cones in between, in the order they
    /// are flown.
    fn chain(&self) -> Vec<u32> {
        let mut chain = vec![GOAL];
        loop {
            let before = self.came[self.slot(*chain.last().expect("a hop"))];
            if before == UNREACHED {
                break;
            }
            chain.push(before);
        }
        chain.reverse();
        chain.retain(|&node| node != START && node != GOAL);
        chain
    }
}

/// What a node has cost before anything has reached it.
///
/// A jump count no route can have, so "unreached" and "dearer than this"
/// are the one comparison. It is also what says a chain has reached its
/// start when it is walked back.
const UNREACHED: u32 = u32::MAX;

/// How few jumps could cross `d` light years, leaving a star whose cone
/// reaches `charged` and with `range` unaided
///
/// One jump where the cone reaches, and one jump plus the ordinary jumps
/// the rest of the gap takes where it does not. [`None`] where that is more
/// bridging than allowed, which is what keeps the coarse graph's edges to
/// the gaps a leg can actually be expected to fly.
///
/// A lower bound on the real jumps and not a promise: nothing here says a
/// chain of systems exists along the gap. That is the leg's business, and
/// the whole of why a plan over this graph is not a fewest-jumps claim.
fn hops(d: f64, charged: f64, range: f64, bridge: u32) -> Option<u32> {
    if d <= charged {
        return Some(1);
    }
    let over = ((d - charged) / range).ceil() as u32;
    (over <= bridge).then_some(1 + over)
}

/// Which cell of the grid a place falls in.
fn cell_of(at: [f32; 3]) -> [i32; 3] {
    [
        (at[0] as f64 / CELL_LY).floor() as i32,
        (at[1] as f64 / CELL_LY).floor() as i32,
        (at[2] as f64 / CELL_LY).floor() as i32,
    ]
}

/// A cell key that sorts the stars into their cells.
///
/// Row-major over the grid rather than Z-ordered: what the sort has to do
/// is put a cell's stars together, and Morton's ordering measured nothing
/// over a directory that is asked for cells by name. Twenty-one bits an
/// axis holds the galaxy at 250 light years with room over.
fn key_of(at: [f32; 3]) -> u64 {
    let cell = cell_of(at);
    let bias = |n: i32| (n + (1 << 20)).clamp(0, (1 << 21) - 1) as u64;
    (bias(cell[0]) << 42) | (bias(cell[1]) << 21) | bias(cell[2])
}

/// The squared distance from `at` to the nearest corner of `cell`.
fn corner_away(cell: [i32; 3], at: [f64; 3]) -> f64 {
    let mut away = 0.;
    for axis in 0..3 {
        let lo = cell[axis] as f64 * CELL_LY;
        let hi = lo + CELL_LY;
        let d = if at[axis] < lo {
            lo - at[axis]
        } else if at[axis] > hi {
            at[axis] - hi
        } else {
            0.
        };
        away += d * d;
    }
    away
}

/// A place as the arithmetic wants it.
fn widen(at: [f32; 3]) -> [f64; 3] {
    [at[0] as f64, at[1] as f64, at[2] as f64]
}

/// The squared distance between two places.
fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_index::SystemBoost;

    /// The ship every plan below is flown by: fifty light years unaided,
    /// two hundred off a neutron star.
    const RANGE: f64 = 50.;
    const DRIVE: Drive = Drive::Standard;

    /// A published supercharge table over `cones`: which systems have a
    /// jet cone, and where each of them is.
    ///
    /// One table, not two. The place is in the published row, which is the
    /// whole of what the highway is built from.
    fn table(cones: &[(i64, [f32; 3])]) -> Boosts {
        Boosts::of(
            cones
                .iter()
                .map(|&(address, position)| SystemBoost {
                    address,
                    boost: Boost::Neutron,
                    position,
                })
                .collect(),
        )
    }

    /// Cones every `apart` light years out along x, `count` of them, the
    /// first at `apart`.
    fn line(count: i64, apart: f32) -> Vec<(i64, [f32; 3])> {
        (1..=count).map(|k| (k, [k as f32 * apart, 0., 0.])).collect()
    }

    /// The plan over a chain of cones is the chain
    ///
    /// Two hundred light years apart is exactly one supercharged hop for
    /// this ship, so every cone is a node of the plan and the jumps are the
    /// hops. The start has no cone of its own, so its first hop is the one
    /// that bridges: fifty light years of range and three ordinary jumps
    /// after it.
    #[test]
    fn a_chain_of_cones_is_a_plan() {
        let cones = line(10, 200.);
        let highway = Highway::over(&table(&cones)).expect("a highway");
        assert_eq!(highway.len(), 10, "a cone was not placed");

        let plan = highway
            .plan(
                ([0., 0., 0.], None),
                [2_200., 0., 0.],
                RANGE,
                DRIVE,
                Routing::QUICK,
                Tuning::default(),
                None,
            )
            .expect("a chain");
        assert_eq!(
            plan.iter().map(|&(address, _)| address).collect::<Vec<_>>(),
            (1..=10).collect::<Vec<_>>(),
            "the plan did not follow the cones out",
        );
    }

    /// A shortest ask takes the shorter of two chains the same length
    ///
    /// [`hops`] prices whole jumps, so a hop of 130 light years and one of
    /// 200 both cost one — and until the coarse cost carried the distance
    /// flown ([`Coarse`]), *nothing* preferred the straighter chain. A
    /// route asked for as the shortest of the fewest jumps came back as
    /// whichever chain of that many the boost table's own ordering reached
    /// first, which is what "shortest" is there to decide.
    ///
    /// Two chains here, both three hops from a start that carries its own
    /// cone: one nearly straight out along x, one sixty light years off it
    /// and twenty-six light years longer. The hops are well under the two
    /// hundred a supercharged jump reaches, which is where the slack lives
    /// — a chain can wander inside the bracket its hop count already pays
    /// for, and on the real table that slack is thousands of light years.
    ///
    /// The wide cones are numbered *below* the straight ones on purpose.
    /// Nothing but the node index separated the two chains before, and the
    /// index is the order the boost table happens to sort in: unweighed by
    /// distance the search takes the wide chain, which is what makes this
    /// a test rather than a restatement of the code.
    ///
    /// Least fuel is deliberately on the other side of it: a boosted jump
    /// costs a whole tank however far the cone throws the ship, so the
    /// fuel on a chain of cones *is* the hop count and the shorter of two
    /// equal chains saves nothing. See [`Highway::plan`].
    #[test]
    fn a_shortest_plan_takes_the_shorter_of_two_equal_chains() {
        let highway = Highway::over(&table(&[
            (1, [120., 60., 0.]),
            (2, [260., 60., 0.]),
            (7, [134., 0., 0.]),
            (8, [268., 0., 0.]),
        ]))
        .expect("a highway");

        let planned = |weigh: Weigh| {
            highway
                .plan(
                    ([0., 0., 0.], Some(Boost::Neutron)),
                    [400., 0., 0.],
                    RANGE,
                    DRIVE,
                    Routing::at(95, weigh),
                    Tuning::default(),
                    None,
                )
                .expect("a chain")
                .iter()
                .map(|&(address, _)| address)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            planned(Weigh::Shortest),
            vec![7, 8],
            "the shortest of the fewest jumps took the long way round",
        );
        // And the two asks that count hops alone keep the answer they
        // always had, which is the half of this that must not move.
        for weigh in [Weigh::Jumps, Weigh::Fuel { hop: 50, expand: 0 }] {
            assert_eq!(
                planned(weigh),
                vec![1, 2],
                "{weigh:?} is no longer the chain it was",
            );
        }
    }

    /// A plan that stalls hands over what it reached
    ///
    /// The reported trouble. A coarse search whose frontier stops closing
    /// on the goal used to answer nothing, and the caller then searched the
    /// whole galaxy flat: a 45 ly Colonia to Sgr A\* plot at five percent
    /// over spent **227 s** that way, where the same plot at twenty-five
    /// percent planned in 0.93 s. The chain to the closest cone reached is
    /// a plan for most of the way, and the stretch left over is one more
    /// gap for the caller to cross.
    ///
    /// Stalled deliberately here, which is not the same as exhausted: one
    /// cone toward the goal and three behind the start, with a stall
    /// allowance of one expansion. The search closes once on the cone
    /// ahead, then pops the ones behind — which get no nearer — and gives
    /// up with a chain in hand. A search that runs *out* of cones is the
    /// other case and still answers nothing; see
    /// [`a_gap_nothing_can_cross_has_no_plan`].
    #[test]
    fn a_stalled_plan_hands_over_what_it_reached() {
        let cones = vec![
            (1i64, [200., 0., 0.]),
            (2, [-200., 0., 0.]),
            (3, [-400., 0., 0.]),
            (4, [-600., 0., 0.]),
        ];
        let highway = Highway::over(&table(&cones)).expect("a highway");
        let stalling = Tuning { stall: 1, ..Tuning::default() };

        let plan = highway
            .plan(
                ([0., 0., 0.], None),
                [1_400., 900., 0.],
                RANGE,
                DRIVE,
                Routing::QUICK,
                stalling,
                None,
            )
            .expect("the cones it did reach");

        assert_eq!(
            plan.iter().map(|&(address, _)| address).collect::<Vec<_>>(),
            vec![1],
            "the plan is the cone it closed on",
        );
        assert!(
            plan.iter().all(|&(address, _)| (1..=4).contains(&address)),
            "the plan carried something that is not a cone: {plan:?}"
        );
    }

    /// And a plan that closed on nothing is still no plan
    ///
    /// What the flat search is the fallback *for*: a goal the highway does
    /// not reach at all. Handing over a chain that got no nearer than the
    /// door would be a route out into nothing.
    #[test]
    fn a_plan_that_closed_on_nothing_is_no_plan() {
        // One cone, and it is the wrong way: the goal is behind the start.
        let cones = vec![(1i64, [500., 0., 0.])];
        let highway = Highway::over(&table(&cones)).expect("a highway");
        let stalling = Tuning { stall: 1, ..Tuning::default() };

        assert!(
            highway
                .plan(
                    ([0., 0., 0.], None),
                    [-9_000., 0., 0.],
                    RANGE,
                    DRIVE,
                    Routing::QUICK,
                    stalling,
                    None,
                )
                .is_none(),
            "a plan that closed on nothing was handed over anyway",
        );
    }

    /// The reach is a distance, so halving the range keeps the plan
    ///
    /// The reported trouble, in miniature. Cones 300 light years apart are
    /// too far for one supercharged hop at either range, so each hop has to
    /// bridge — and a *jump count* allowance is a different distance at
    /// each range: four bridging jumps is 400 ly at 50 and 200 ly at 25, and
    /// at 200 ly this chain is not connected. Over the real table that was
    /// Sol to Colonia answering nothing in 0.3 ms and the flat search
    /// spending 233 s on the fallback. [`Tuning::reach`] is light years, so
    /// both ranges plan.
    #[test]
    fn a_plan_holds_when_the_ship_jumps_half_as_far() {
        // Long enough that the chain has to be flown: the start may bridge
        // [`START_BRIDGE`] jumps, which at this range is three thousand
        // light years, and a shorter chain is crossed in one leap from the
        // door whatever the mid-route reach is.
        let cones = line(30, 300.);
        let highway = Highway::over(&table(&cones)).expect("a highway");
        let goal = [9_200., 0., 0.];

        for range in [RANGE, RANGE / 2.] {
            let plan = highway
                .plan(
                    ([0., 0., 0.], None),
                    goal,
                    range,
                    DRIVE,
                    Routing::QUICK,
                    Tuning::default(),
                    None,
                )
                .unwrap_or_else(|| panic!("no plan at {range} ly"));
            assert_eq!(
                plan.iter().map(|&(address, _)| address).collect::<Vec<_>>(),
                (1..=30).collect::<Vec<_>>(),
                "the plan at {range} ly did not follow the cones out",
            );
        }

        // And a reach under the gaps is **recovered by the ladder**, which
        // is what the setting stopped being a question for: a 200 ly reach
        // strings nothing at either range — 300 ly apart is more than one
        // supercharged hop plus what it allows — and the plan tries again
        // a jump wider until it does. Over the real table this is a 2 kly
        // corridor going from 60 stops in 32.3 s to 32 in 6.2 ms.
        let narrow = Tuning { reach: 200, ..Tuning::default() };
        for range in [RANGE, RANGE / 2.] {
            let plan = highway
                .plan(
                    ([0., 0., 0.], None),
                    goal,
                    range,
                    DRIVE,
                    Routing::QUICK,
                    narrow,
                    None,
                )
                .unwrap_or_else(|| {
                    panic!("the ladder did not climb at {range} ly")
                });
            assert_eq!(
                plan.iter().map(|&(address, _)| address).collect::<Vec<_>>(),
                (1..=30).collect::<Vec<_>>(),
                "the ladder's chain at {range} ly is not the cones",
            );
        }

        // What the ladder cannot reach is still no plan: [`RUNGS`] jumps
        // past the reach asked for is the end of it, and a chain of cones
        // a thousand light years apart is past that at this range — the
        // widest rung strings 900.
        let sparse = Highway::over(&table(&line(30, 1_000.))).expect("cones");
        assert!(
            sparse
                .plan(
                    ([0., 0., 0.], None),
                    [30_500., 0., 0.],
                    RANGE,
                    DRIVE,
                    Routing::QUICK,
                    Tuning::default(),
                    None
                )
                .is_none(),
            "the ladder claimed a chain past its last rung",
        );
    }

    /// An exact plan that spends its allowance leaves the leaned one
    /// standing
    ///
    /// The failure this guards is not a worse chain, it is **no chain**:
    /// the plan opens at exact ([`Tuning::planning`]) and is abandoned the
    /// moment it has spent [`Tuning::allowance`] expansions, and a caller
    /// handed nothing falls back to the flat galaxy-wide search — which
    /// across the galaxy is the minutes this whole module exists to avoid.
    /// So an allowance of nothing, which no exact pass can survive, must
    /// still answer the chain the route's own percent asks for.
    ///
    /// And the allowance belongs to the *try* rather than to every pass: a
    /// reader who asked for a leaned plan asked for it, so a plan leaning
    /// harder than the route does is run with no allowance at all and an
    /// allowance of nothing does not touch it.
    #[test]
    fn a_plan_that_spends_its_allowance_falls_back_to_the_leaned_one() {
        let cones = line(30, 300.);
        let highway = Highway::over(&table(&cones)).expect("a highway");
        let goal = [9_200., 0., 0.];
        let chain = (1..=30).collect::<Vec<_>>();

        for (what, tune) in [
            // Exact asked for, and not a single expansion to try it in.
            (
                "no allowance",
                Tuning { allowance: Some(0), ..Tuning::default() },
            ),
            // Exact asked for and paid for, which is the rail's last stop.
            ("paid for", Tuning { allowance: None, ..Tuning::default() }),
            // Leaned harder than the route: the reader's own ask, which
            // the allowance has no say over.
            (
                "leaned by hand",
                Tuning {
                    planning: 20,
                    allowance: Some(0),
                    ..Tuning::default()
                },
            ),
        ] {
            let plan = highway
                .plan(
                    ([0., 0., 0.], None),
                    goal,
                    RANGE,
                    DRIVE,
                    Routing::QUICK,
                    tune,
                    None,
                )
                .unwrap_or_else(|| panic!("{what}: no plan at all"));
            assert_eq!(
                plan.iter().map(|&(address, _)| address).collect::<Vec<_>>(),
                chain,
                "{what} did not follow the cones out",
            );
        }
    }

    /// What a hop between two cones costs, and when there is no hop
    ///
    /// The whole of the coarse graph's arithmetic. One jump where the cone
    /// reaches, one jump and the ordinary jumps the rest of the gap takes
    /// where it does not, and nothing at all past the bridging allowed.
    ///
    /// The measurement this module turns on is what makes that middle case
    /// necessary: flooding the real table with supercharged hops *alone*
    /// reaches 135 of 3,846,802 stars and stops 196 light years out, where
    /// four ordinary jumps after each hop reaches 2,419,398 of them past
    /// 61,000 light years. The bridging is what connects the graph.
    #[test]
    fn a_hop_is_priced_by_the_bridging_it_needs() {
        // Inside the cone's reach: one jump, however short.
        assert_eq!(hops(200., 200., 50., 4), Some(1));
        assert_eq!(hops(10., 200., 50., 4), Some(1));
        // Past it: the hop, and one ordinary jump per range over.
        assert_eq!(hops(250., 200., 50., 4), Some(2));
        assert_eq!(hops(400., 200., 50., 4), Some(5));
        // And past the bridging allowed, no edge at all.
        assert_eq!(hops(450., 200., 50., 4), None);
        assert_eq!(hops(450., 200., 50., GOAL_BRIDGE), Some(6));
    }

    /// A gap nothing can cross has no plan
    ///
    /// The coarse graph is disconnected where the cones are, and a plan
    /// has to answer nothing rather than promise a leg with no chain of
    /// systems to fly it. Ten thousand light years between the near cone
    /// and the far one: past the four ordinary jumps a hop may bridge,
    /// past the sixty the first hop out may spend, and past the twelve the
    /// approach may take.
    #[test]
    fn a_gap_nothing_can_cross_has_no_plan() {
        let highway = Highway::over(&table(&[
            (1, [200., 0., 0.]),
            (2, [10_200., 0., 0.]),
        ]))
        .expect("a highway");

        assert!(
            highway
                .plan(
                    ([0., 0., 0.], None),
                    [10_300., 0., 0.],
                    RANGE,
                    DRIVE,
                    Routing::QUICK,
                    Tuning::default(),
                    None
                )
                .is_none(),
            "a ten thousand light year gap was crossed",
        );
    }

    /// The last hop closes the approach on ordinary jumps
    ///
    /// A goal is a system somebody named rather than a jet cone, so the
    /// end of every plan is plain flying: up to [`GOAL_BRIDGE`] jumps of
    /// it. Five hundred light years past the last cone here, which is ten
    /// of them — more than the start's own first scan would allow and
    /// exactly what the approach is for. Nothing else can be the way: the
    /// start cannot reach the goal on its own allowance.
    #[test]
    fn the_last_hop_closes_the_approach_on_ordinary_jumps() {
        let highway =
            Highway::over(&table(&[(1, [200., 0., 0.])])).expect("a highway");

        let plan = highway
            .plan(
                ([0., 0., 0.], None),
                [700., 0., 0.],
                RANGE,
                DRIVE,
                Routing::QUICK,
                Tuning::default(),
                None,
            )
            .expect("a chain by way of the cone");
        assert_eq!(
            plan.iter().map(|&(address, _)| address).collect::<Vec<_>>(),
            vec![1],
            "the approach was not flown from the cone",
        );
    }

    /// The first hop may bridge much further than the rest
    ///
    /// The highway rarely starts at the door. A start a thousand light
    /// years from the nearest cone is twenty ordinary jumps off it — past
    /// what any hop between cones may spend — and the plan is still to fly
    /// there, which is what [`START_BRIDGE`] allows and the widening finds
    /// without sweeping sixty jumps of sphere at every expansion.
    #[test]
    fn the_first_hop_out_may_bridge_further_than_the_rest() {
        let highway =
            Highway::over(&table(&[(1, [1_000., 0., 0.])])).expect("a highway");

        let plan = highway
            .plan(
                ([0., 0., 0.], None),
                [1_150., 0., 0.],
                RANGE,
                DRIVE,
                Routing::QUICK,
                Tuning::default(),
                None,
            )
            .expect("a chain out to the cone");
        assert_eq!(
            plan.iter().map(|&(address, _)| address).collect::<Vec<_>>(),
            vec![1],
            "the far cone was not the way",
        );
    }

    /// An empty table is no highway either
    ///
    /// Two systems in a hundred have a cone and the table is the two: an
    /// index that publishes none of them has nothing to plan on, and the
    /// caller searches flat. Which is not the same as publishing no table
    /// at all — that is a galaxy the map has been told nothing about, and
    /// it refuses a supercharged route rather than answering the unaided
    /// one under its name.
    #[test]
    fn a_table_with_nothing_in_it_is_no_highway() {
        assert!(Highway::over(&table(&[])).is_none(), "an empty table");
        assert!(Highway::over(&Boosts::absent()).is_none(), "no table");
    }

    /// Every published row is a node, at the place the row carries
    ///
    /// The place comes with the row now, so there is nothing to join and
    /// nothing to drop: what the builder published is what the coarse
    /// graph is made of, cell by cell.
    #[test]
    fn a_published_row_is_a_node_where_it_says_it_is() {
        let highway = Highway::over(&table(&[
            (1, [200., 0., 0.]),
            (7, [9_000., 0., 0.]),
        ]))
        .expect("a highway");

        assert_eq!(highway.len(), 2);
        assert_eq!(highway.cells(), 2, "two cones, two cells apart");
        assert_eq!(highway.nearest([200., 0., 0.], 10.), Some(1));
        assert_eq!(highway.nearest([9_000., 0., 0.], 10.), Some(7));
    }
}
