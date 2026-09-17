//! The jump graph the router walks.
//!
//! Routing used to be a database question: A* over `ST_3DDWithin` neighbour
//! queries. With the map drawing from the index there is no database, so the
//! same walk runs here over the resident names table, which carries every
//! system's place. The one thing a walk over a million points needs that a
//! database index gave it for free is a way to ask for neighbours without
//! scanning them all, so the positions are bucketed into a coarse spatial grid
//! and a jump looks only in the buckets a ship could reach.

use super::highway::Highway;
use crate::Boosts;
use bevy::math::DVec3;
use bevy::prelude::*;
use galos_index::meta::Boost;
use galos_index::{CellId, Node, Sky};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// What a route was asked to be
///
/// **Two numbers, not three modes.** A fewest-jumps search with an estimate
/// that never overstates what is left is the honest answer and the slow one:
/// twenty-two thousand light years is hundreds of jumps, and the number of
/// chains exactly that long is enormous — proving the fewest means expanding
/// every system that could have been on an equally short one. So there are
/// two independent questions, and they used to be a three-way choice that
/// could not say what it meant:
///
/// - **How many jumps over the fewest** may it settle for, to come back
///   sooner ([`Self::over`])? Nothing over is the proof; anything over is
///   weighted A\*, which comes back bounded rather than proven.
/// - **Are ties broken by distance** ([`Self::shortest`])? The chains of one
///   length are many, and this is whether the shortest of them is found
///   rather than whichever the tie-break happened to like.
///
/// What used to be three settings is three points in that plane —
/// `FEWEST`, `QUICK`, `SHORTEST`, kept as test fixtures — and the plane
/// holds the combinations they could not express, a quick route whose ties
/// break by distance among them.
///
/// Measured Sol to Colonia at a 50 light year range, over the 2.4 million
/// systems the names table carries:
///
/// | | jumps | found in |
/// |---|---|---|
/// | 5% over | 458, and bounded at 480 | 0.08 s |
/// | nothing over | 458 | 1.8 s |
/// | nothing over, shortest | 458, the shortest of them | 3.2 s |
///
/// The same four hundred and fifty-eight jumps either way here, for a
/// twentieth of the wait — but only the second row *proves* it is the
/// fewest, and that proof is nearly all of the time.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Routing {
    /// How many jumps over the fewest it may settle for, in percent
    ///
    /// Offered as its complement — **optimality**, where 100% is nothing
    /// over and proven — because that is the question a reader is
    /// answering: how good does this have to be. See
    /// [`Self::optimality`].
    ///
    /// Weighted A\*: the estimate is multiplied by `1 + over/100`, which
    /// makes it inadmissible on purpose — it may now overstate what is left,
    /// and the search stops expanding the enormous plateau of systems that
    /// could have been on an equally short chain. What comes back is bounded
    /// rather than proven: no more than that multiple of the fewest jumps,
    ///
    /// ```text
    /// jumps <= fewest + fewest * over / 100, rounded down
    /// ```
    ///
    /// so at five percent a route of under twenty jumps is the fewest there
    /// are and cannot be anything else, and a crossing of the galaxy may be
    /// a jump over for every twenty it takes. Which is the whole of the
    /// claim: it bounds *jumps* and says nothing about light years.
    ///
    /// The bound survives the search expanding each system **once**, which
    /// is what [`JumpGraph::search`] does wherever this is over nothing.
    /// Weighted A\* that never re-expands a settled system still answers
    /// inside the same `1 + over/100` of the fewest, given a heuristic
    /// that is consistent before the weighting — which the estimate here
    /// is, a jump closing at most the widest jump's worth of what is left.
    /// It is the result `ARA*` is built on, and this used to claim the
    /// opposite: that reopening was what made the bound a theorem. What
    /// reopening actually bought was work. See [`JumpGraph::walk`].
    ///
    /// A percent and not a fraction of two integers because that is what the
    /// reader is offered, and the arithmetic stays exact all the same: a
    /// jump costs [`WHOLE`] and the estimate is multiplied by `WHOLE + over`,
    /// so nothing is a float rounded twice. Rounding a *weighted distance*
    /// up to whole jumps is not the same as weighting the whole jumps — it
    /// overstates by up to a jump wherever it lands, which for a system one
    /// jump out is an estimate of two and no five percent about it.
    pub(crate) over: u32,
    /// What the route is weighed by: jumps, distance, or fuel
    ///
    /// The second of the two questions, and the one that says what "best"
    /// means before the first says how hard to prove it. See [`Weigh`].
    pub(crate) weigh: Weigh,
}

/// What a route is weighed by
///
/// **Three asks, and they are not the same route.** A ship crossing the
/// galaxy wants the fewest jumps; one hauling wants the shortest way round;
/// one going somewhere nothing has been wants to arrive with fuel left. The
/// three are genuinely opposed — the fewest jumps means every jump pushed
/// as near the ship's range as the sky allows, and a jump at full range
/// costs the drive's whole maximum fuel by definition, so the
/// fewest-jumps route is the thirstiest route there is.
///
/// Which used to be a `shortest` flag, because there were two. The third
/// does not fit a flag: it is not a tie-break on the jump count, it is
/// another thing to count.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) enum Weigh {
    /// The fewest jumps, ties broken by heading at the goal
    #[default]
    Jumps,
    /// The fewest jumps, and provably the shortest chain of that many
    ///
    /// Distance goes into the cost under the jump count ([`Cost`]), so the
    /// search settles the tie itself instead of leaving it to the
    /// tie-break. It costs: the plateau of equally short chains has to be
    /// walked to know which is shortest, where the tie-break merely
    /// prefers one.
    Shortest,
    /// The least fuel, splitting its jumps down to `hop` percent of the
    /// range and no further, expanding the nearest `expand` systems of
    /// each sphere
    ///
    /// What a deep space route wants, refuelling being the thing there is
    /// none of out there. **The hop is in the ask because the least fuel
    /// there is, is as many hops as the sky offers** — measured, ninety-four
    /// jumps of a third of a light year to cross three hundred, against the
    /// six the fewest-jumps route flies. Splitting always saves fuel and
    /// nothing in the arithmetic ever stops it, so where to stop is the
    /// reader's to say and not the map's: see [`floor`] for the trade, which
    /// is a straight line, and [`Burn`] for the ordering.
    ///
    /// **And `expand` is this weighing's own approximation**, which is why
    /// it rides here rather than in [`Routing`]: a route weighed by fuel
    /// only ever takes short jumps — measured, nothing over 17.8 light
    /// years out of a hundred light year range — so the far half of every
    /// sphere is candidates no answer could use. Asking for fewer is the
    /// one lever that makes such a route quicker without touching what it
    /// is weighed by. Nothing for all of them, which is what a proven
    /// route always takes. See [`EXPAND`] for the measurements and
    /// [`Routing::fanout`] for why the other two weighings have no such
    /// setting.
    Fuel { hop: u32, expand: u32 },
}

impl Default for Routing {
    /// **Eighty percent, which is the knee and not a shrug.** The setting
    /// is the search's greediness — the estimate is multiplied by
    /// `1 + over/100`, so a route at 95% is A\* with a nudge — and what
    /// that costs is wildly nonlinear. Measured over `.index/full`, Sol
    /// outward, unaided at a fifty light year range:
    ///
    /// | optimality | 1 kly | 2 kly |
    /// |---|---|---|
    /// | 100% | 69 ms | 318 ms |
    /// | 95% | 54 ms | 230 ms |
    /// | 90% | 34 ms | 54 ms |
    /// | **80%** | **6.4 ms** | **18 ms** |
    /// | 70% | 4.7 ms | 13 ms |
    /// | 50% | 2.6 ms | 5.7 ms |
    ///
    /// **Every one of those came back with the same route** — 21 jumps and
    /// 41 jumps — so on these two corridors the whole of the slack bought
    /// speed and nothing was given up for it. Eleven times the speed at 80%
    /// against the proof, and the ten percent below that buys a third as
    /// much again. Which is not a claim that the slack is always free: the
    /// promise is still only "within that percent of the fewest", and a
    /// corridor with a genuine choice in it will spend some of it.
    fn default() -> Routing {
        Routing { over: 20, weigh: Weigh::Jumps }
    }
}

/// What a jump costs the search, so a weighted estimate is whole integers
///
/// The estimate is whole jumps multiplied by `WHOLE + `[`Routing::over`], and
/// a jump costs `WHOLE`, so the weighting is exact arithmetic on integers
/// rather than a float rounded at two places. A hundred because the setting
/// is offered in percent; every whole percent lands on its own integer.
pub(crate) const WHOLE: u32 = 100;

/// How many percent over the fewest the quick answer settles for
///
/// The knee of the curve measured on the galaxy: a twentieth still expands
/// most of the plateau, and a fifth buys another factor of three for four
/// times the jumps over. What the form's slider opens at is the user's
/// business — this is the number the measurements in `Routing` were taken
/// at, and the fixture the tests ask for.
#[cfg(test)]
pub(crate) const OVER: u32 = 5;

/// Whether a setting is allowed to approximate.
///
/// The one rule the search obeys: **an approximation belongs to a route that
/// has allowed jumps over the fewest, and to nothing else.** A setting that
/// claims the fewest jumps has to mean it, so everything that trades
/// exactness for speed — the weighted estimate, the fanout cap, the coarse
/// plan over the boost stars, and whatever comes after — is switched on by
/// [`Routing::approximates`] together and nowhere apart.
///
/// Which used to read "belongs to `Routing::QUICK`", a named mode that had
/// to be remembered; now it is the question the user answered.
impl Routing {
    /// The fewest jumps, proven, ties broken by heading at the goal.
    ///
    /// The three named points of the plane are fixtures rather than
    /// settings: what the map asks for is whatever the two controls say,
    /// and these are how the tests and the measurements name the corners
    /// the old three-way choice used to offer.
    #[cfg(test)]
    pub(crate) const FEWEST: Routing = Routing { over: 0, weigh: Weigh::Jumps };

    /// Inside [`OVER`] percent of the fewest, and a hundredth of the time.
    #[cfg(test)]
    pub(crate) const QUICK: Routing =
        Routing { over: OVER, weigh: Weigh::Jumps };

    /// The fewest jumps, and provably the shortest chain of that many.
    #[cfg(test)]
    pub(crate) const SHORTEST: Routing =
        Routing { over: 0, weigh: Weigh::Shortest };

    /// The least fuel, proven.
    #[cfg(test)]
    pub(crate) const ECONOMICAL: Routing =
        Routing { over: 0, weigh: Weigh::Fuel { hop: HOP, expand: 0 } };

    /// Whether anything about the answer is unproven
    ///
    /// One gate for every approximation there is, and the reason they
    /// cannot be switched on one at a time by accident.
    ///
    /// **The percent over the fewest is that question everywhere but one
    /// place.** At an *unpriced* hop the fuel left to burn has no positive
    /// lower bound — a jump can be arbitrarily short — so the estimate is
    /// zero and weighting zero is zero: the percent cannot approximate
    /// anything, measured as the identical route in 6.1 s against 6.3 s at
    /// 100% and 95%. What is left there is the cap, and it is the reader's
    /// own setting ([`Weigh::Fuel`]), so the cap is what decides: every
    /// system in range is the graph a proven route searches, and asking
    /// for fewer is the one thing that makes such a route unprovable.
    ///
    /// Which is what lets the form drop its `Optimal` tick: the top of
    /// each rail *is* the proven ask, `Within` at 100% everywhere the
    /// percent bites and `Expand nearest` at `all` where it does not.
    /// Without this the tick was the only way to say it, because a
    /// percent of nothing switched the coarse plan on regardless — and a
    /// planned route cannot claim to be proven, its edges being lower
    /// bounds on gaps rather than chains anything has flown.
    pub(crate) fn approximates(&self) -> bool {
        match self.weigh {
            Weigh::Fuel { hop: 0, expand } => expand != 0,
            _ => self.over > 0,
        }
    }

    /// What the estimate is multiplied by, against [`WHOLE`] for a jump.
    pub(super) fn weight(&self) -> u32 {
        WHOLE + self.over
    }

    /// How many of an expansion's neighbours may be relaxed, or [`None`]
    /// for all of them.
    ///
    /// A cap is the fix for a search whose work is quadratic in stellar
    /// density — a boosted jump in the core sees thousands of systems — but
    /// it can drop the very neighbour a fewest-jumps chain went through, so
    /// only a route that has not promised the fewest may have one.
    ///
    /// **Where it is the reader's to set, and where it is not.** A route
    /// weighed by fuel takes short jumps and nothing else, so asking for
    /// fewer neighbours costs it almost nothing and saves most of the
    /// wait: measured, the nearest 64 come within 1.8% of the proven fuel
    /// at three to twenty-seven times the speed. It is a setting there
    /// ([`Weigh::Fuel`]), and `expand` of nothing asks for every system in
    /// range, which is what a proven route always gets.
    ///
    /// It is **not** a setting for the two weighings counted in jumps, and
    /// the measurements are why: the same cap tightened to 64 made a
    /// charged three thousand light year crossing *slower* — 320 ms
    /// against 143 — for the same 37 stops, the search having lost
    /// neighbours it needed and wandered for want of them. There it is a
    /// safety valve against stellar density rather than a dial, so it
    /// stays at [`FANOUT`] and says so in the route's own description
    /// ([`Self::named`]) instead of offering a handle that makes things
    /// worse.
    ///
    /// **And it is not a setting at a priced hop either, because the form
    /// does not draw it there.** `Expand nearest` is offered at an
    /// unpriced hop alone — a priced one takes long jumps and few of them
    /// — but the count the rail last held travelled along with the ask
    /// ([`crate::ui::traded`]) and went on biting where nobody could see
    /// it. Measured over `.index/full` at 45 ly, least fuel at a 5% hop,
    /// the remembered 64 against the valve:
    ///
    /// | corridor | the nearest 64 | [`FANOUT`] |
    /// |---|---|---|
    /// | 186 ly out | 47 stops, 0.733 tanks, 206 ms | 49, **0.716**, 487 ms |
    /// | 700 ly out | 197, 2.225, 6.76 s | 199, **2.206**, 11.6 s |
    ///
    /// Two percent of the tank decided by a control that was not on
    /// screen. At a quarter of the range and up it really is inert — 15
    /// stops and 1.251 tanks either way — which is what the form was told
    /// when it stopped drawing the rail, and what made the leak so quiet.
    /// So the cap applies where it is asked for and the valve holds
    /// everywhere else.
    fn fanout(&self) -> Option<usize> {
        if !self.approximates() {
            return None;
        }
        match self.weigh {
            // Every system in range, which is the top of the setting's own
            // travel and the same graph a proven route searches.
            Weigh::Fuel { hop: 0, expand: 0 } => None,
            Weigh::Fuel { hop: 0, expand } => Some(expand as usize),
            Weigh::Fuel { .. } | Weigh::Jumps | Weigh::Shortest => Some(FANOUT),
        }
    }

    /// Whether a long supercharged route may be planned on the boost stars
    ///
    /// The same rule: a chain of jet cones ([`super::highway`]) is a guess
    /// at which cones are worth taking and its edges say how few jumps
    /// *could* cross a gap rather than that a chain of systems crosses it
    /// that way. Nothing about the route it leads to is proven.
    ///
    /// What it buys, measured over the real 200,071,629-system index, Sol
    /// to Colonia at a 50 light year range with a standard drive — and it
    /// is a comparison between two settings rather than one reading:
    /// **140 jumps in 2.0 s at five percent over**, planned on the
    /// highway, against the **137 jumps in 610 s that nothing over
    /// proves** searching flat. Three hundred times the speed for three
    /// jumps in a hundred and forty — 2.2% over the fewest there are,
    /// which is inside what five percent says of itself, though nothing
    /// here proves it and it is one corridor. At an 80 ly range the same
    /// crossing is 81 jumps in 0.8 s.
    fn highway(&self) -> bool {
        self.approximates()
    }

    /// How good the route has to be, in percent, where 100 is proven
    ///
    /// The complement of [`Self::over`], and the way round the setting is
    /// offered: a reader asks for *optimality*, and weighted A\* wants the
    /// slack. Linear in the slack rather than in the bound it implies — the
    /// theorem says a route costs no more than `1 + over/100` times the
    /// fewest, so 95% optimality is a route inside 105% of the fewest
    /// jumps, which is the same claim said the way round people say it.
    pub(crate) fn optimality(&self) -> u32 {
        100 - self.over.min(100)
    }

    /// The setting an optimality asks for.
    pub(crate) fn at(optimality: u32, weigh: Weigh) -> Routing {
        Routing { over: 100 - optimality.min(100), weigh }
    }

    /// What this is called where a route says what it was plotted with
    ///
    /// Read in a line of prose beside the range and the drive rather than as
    /// a heading, so it says what was asked for rather than naming a mode
    /// the reader would have to look up.
    ///
    /// **Including what it did not look at.** A route that has not promised
    /// the fewest expands a bounded number of each sphere's systems, and
    /// what a cap drops the search cannot learn — so a plot that was
    /// capped says so. It is a setting for a fuel-weighed route and a
    /// fixed safety valve for the other two ([`Self::fanout`]), and either
    /// way the reader is entitled to know it happened rather than to
    /// discover it as a route that went the long way.
    ///
    /// The word `optimal` is [`Self::approximates`]'s to give, not the
    /// percent's: at an unpriced hop the percent means nothing and the cap
    /// is the whole of the promise. The middle case is the state the form
    /// does not offer — a cap kept at an unpriced hop with no slack asked
    /// for — and what it gave up is the cap, which the line ends with.
    pub(crate) fn named(&self) -> String {
        let quality = match (self.approximates(), self.over) {
            (false, _) => "optimal".to_owned(),
            (true, 0) => "bounded by what it expanded".to_owned(),
            (true, _) => format!("{}% optimality", self.optimality()),
        };
        let said = match self.weigh {
            Weigh::Jumps => format!("{quality}, fewest jumps"),
            Weigh::Shortest => format!("{quality}, shortest"),
            // **"Least fuel" only where it is true, which is one end of
            // the trade.** Priced, what comes back is the best route under
            // a per-jump toll and it burns more than the untolled optimum
            // — measured over `.index/full`, 0.876 of a tank at a 50%
            // shortest hop against 0.191 at nothing, both of them proven.
            // So a route that paid the toll says where on the trade it
            // stood rather than claiming a superlative the rail moves.
            Weigh::Fuel { hop: 0, .. } => {
                format!("{quality}, the least fuel there is")
            }
            Weigh::Fuel { hop, .. } => {
                format!("{quality}, fuel over jumps at {hop}% hops")
            }
        };
        match self.fanout() {
            Some(cap) => format!("{said}, the nearest {cap} expanded"),
            None => said,
        }
    }
}

/// How far a supercharged route has to be before it is planned on the boost
/// stars rather than searched flat, in light years
///
/// Under this the flat search is already quick — it is the whole reason the
/// grid is sized against a jump — and it is exact, so a coarse plan could
/// only make the answer worse. Over it the two swap places: the flat
/// charged search across the galaxy is ten million expansions and minutes,
/// and the plan is seconds. A thousand five hundred, which is where EDDA
/// draws the same line (`long_range.rs:23-24`).
const LONG_ROUTE_LY: f64 = 1_500.;

/// What a long supercharged plot is allowed to trade, and how much
///
/// The two-level planner has four numbers in it that trade jumps for time,
/// and until this they were constants chosen against a 50 ly ship. They are
/// not the same numbers at 25 ly, and the failure mode is not a slower plan
/// — it is **no plan at all**, and then the flat galaxy-wide search as the
/// fallback. Measured, Sol to Colonia: at 50 ly the plan takes 335 ms and
/// the whole route 2.5 s; at 25 ly the coarse search hits [`Self::stall`],
/// answers nothing, and the flat search spends **233 s** to come back with
/// 317 jumps. EDDA plots the same 25 ly crossing in 284 ms for 331 — 4.5%
/// more jumps for 830× less time — and does it by leaning far harder than
/// this ever has: a coarse weight of 1.5 against our 1.05
/// (`long_range.rs:52-57`, which records 1.3 → 3,120 expansions/6 s/72
/// jumps against 1.5 → 73 expansions/71 ms/66 jumps), candidate thinning,
/// a goal cone, and a hard 30,000-expansion cap.
///
/// So they are settings, with the defaults measured rather than assumed.
/// Only [`Routing::QUICK`] reads any of them: `Direct` and `Shortest` are
/// proven fewest-jumps routes and have nothing to trade.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Tuning {
    /// How large a gap the plan strings together to begin with, in whole
    /// light years
    ///
    /// Whole, because it travels in a route's own identity
    /// ([`crate::systems::filter::Filter::Route`]) and an identity wants
    /// [`Hash`] and [`Eq`] — which a float does not have, and for good
    /// reason. A light year is finer than any gap worth telling apart: a
    /// rung of the ladder is a whole jump, tens of light years.
    ///
    /// **A distance and not a jump count, which is the whole of the 25 ly
    /// bug.** A hop is one supercharged jump plus as many ordinary ones as
    /// this allows, and the number that has to be held constant is the
    /// *reach*: four bridging jumps is 400 ly at 50 ly and 200 ly at 25,
    /// and at 200 ly the boost stars are not a connected graph — the plan
    /// died in 0.3 ms, having expanded its way into a dead end, and the
    /// caller fell back to the flat galaxy-wide search. Where the cliff
    /// is, is a fact about the cones: measured at 10, 25 and 50 ly, 350 ly
    /// strings nothing and 380 plans at every one of them.
    ///
    /// **And it is the first rung rather than the answer**, because no one
    /// distance is right everywhere. A corridor whose own bottleneck is
    /// wider does not fail cheaply — the coarse search never closes on the
    /// goal, spends [`Self::stall`], hands over the cones it reached, and
    /// the stretch left over becomes one enormous gap for the legs.
    /// Measured over `.index/full` at 45 ly, Sol to a system 2 kly out at
    /// `[1400, 0, 1400]`:
    ///
    /// | reach | stops | found in |
    /// |---|---|---|
    /// | 405 ly | 60 | **32.3 s** |
    /// | 450 ly | 47 | 33.6 ms |
    /// | 495 ly | **32** | **6.2 ms** |
    ///
    /// So [`super::highway::Highway::plan`] climbs it — a plan that does
    /// not close is tried again a jump wider — and this is where the climb
    /// starts, which is [`super::highway::GAPS_LY`] and no longer a
    /// question the form asks. A wider reach only adds edges to the cone
    /// graph, so a reader could only ever set this too low, and too low is
    /// a cliff.
    ///
    /// **Floored at one supercharged jump and one ordinary one**, because a
    /// hop *is* those: asked for less, the bridging allowance the plan
    /// derives from this clamps at one jump and the reach is quietly more
    /// than the number said.
    pub(crate) reach: u32,
    /// Expansions the coarse plan spends without closing on the goal before
    /// it gives up
    ///
    /// A stall rule and not a cap: what it bounds is a search going
    /// nowhere. EDDA caps outright instead — 30,000 expansions, then it
    /// takes the closest cone reached and hands the rest to the flat
    /// planner (`long_range.rs:2755-2758`).
    pub(crate) stall: u64,
    /// How many percent over the fewest *hops* the coarse plan leans by,
    /// where nothing is an exact plan over the cone graph
    ///
    /// **Its own number and not [`Routing::over`]'s, because the two are
    /// not the same question.** The route's percent bounds the answer: a
    /// flat search that has allowed that much comes back inside it. A
    /// coarse plan bounds *nothing* — its edges say how few jumps could
    /// cross a gap, not that a chain of systems crosses it that way — so
    /// what this number buys is not a claim but a better chain of cones,
    /// and it is a speed dial with no promise attached either way. Until
    /// this the plan multiplied its estimate by the route's own percent,
    /// so a reader asking for a route within five percent was also asking
    /// for a plan leaned by five, with no way to say the one without the
    /// other, and no way to ask for the exact plan at all.
    ///
    /// **What exact is worth, measured end to end** over `.index/full`
    /// from Sol at a 45 ly range with a standard drive, the route's own
    /// percent against a plan exact over the cone graph:
    ///
    /// | corridor | at 95%, leaned | at 95%, exact | at 80%, leaned | at 80%, exact |
    /// |---|---|---|---|---|
    /// | Colonia | 158 stops, 563 ms | **156**, 2.23 s | 166 stops, 98 ms | **156**, 1.90 s |
    /// | 16 kly out | 137 stops, 697 ms | 137, 1.42 s | 137 stops, 306 ms | 137, 1.06 s |
    /// | 22 kly out | 172 stops, 1.31 s | **170**, 2.99 s | 175 stops, 292 ms | **170**, 2.00 s |
    ///
    /// So an exact plan is worth **nothing to six percent of the stops for
    /// two to nineteen times the wait**, and which of those it is, is
    /// decided by how large the cone plateau beside the corridor is —
    /// which no distance predicts. It is worth *trying* rather than
    /// promising, and [`Self::allowance`] is what makes trying cheap.
    ///
    /// The rail this is asked on runs from exact to fifty percent over,
    /// which is EDDA's own coarse weight (`long_range.rs:52-57`): the
    /// setting is there for the corridor where even the route's own
    /// leaning crawls.
    pub(crate) planning: u32,
    /// Expansions an exact coarse plan may spend before the plan is worked
    /// leaned instead, or [`None`] to pay whatever it costs
    ///
    /// The bound that lets [`Self::planning`] open at exact. The exact
    /// pass runs first and is abandoned the moment it has spent this much;
    /// the plan is then leaned by the route's own percent, which is what
    /// it always was. So what an exact plan can cost is this many
    /// expansions of wasted work, and what it can buy is the rows above.
    ///
    /// **Two thousand and forty-eight, which is where the free cases
    /// are.** An exact plan that lands costs nothing worth measuring —
    /// 512 expansions, and 4.3 ms of whole route against leaning's 4.0 ms
    /// on a 3 kly corridor at a 495 Ly gap — while the ones that do not
    /// land want 97,792 expansions on Colonia, 140,800 on the 16 kly
    /// corridor and 276,480 on the 22 kly one. Every corridor measured is
    /// a factor of forty-eight either side of this, and at some 20 µs an
    /// expansion what the allowance can waste is **40 ms**: measured,
    /// every one of those three answers the same stops it did before,
    /// inside the spread of its own clock.
    ///
    /// **And [`None`] is a rail stop, because the plan it refuses to pay
    /// for is a real answer.** Measured at 45 ly and 80% optimality, the
    /// bounded try against the paid one: Colonia 164 stops in 98 ms
    /// against **154 in 2.83 s**, 22 kly out 174 in 467 ms against **168
    /// in 4.42 s**, and a 2 kly corridor whose exact plan lands inside the
    /// allowance 32 stops in 4.2 ms either way. Three to six percent of
    /// the jumps flown for nine to twenty-nine times the wait, and the
    /// reader who wants them had nowhere to ask: the `Plan` rail only
    /// leans *harder* than exact, and expansions are not a unit to put in
    /// front of anybody. So the rail's last stop is this at nothing — an
    /// exact plan, whole, however long it takes — and the stop below it is
    /// the bounded try that may quietly end up leaned. Two stops because
    /// they are two answers; one word for both is what left the panel
    /// unable to say which it had.
    ///
    /// Far under [`Self::stall`] where it is set at all, deliberately:
    /// the stall rule belongs to the pass that has to answer, and a
    /// bounded exact pass is abandoned long before it could stall.
    pub(crate) allowance: Option<u64>,
    /// How the jumps that cross one gap of the plan are found
    pub(crate) crossing: Crossing,
}

impl Default for Tuning {
    /// What a plot is asked for unless the settings say otherwise
    fn default() -> Self {
        Tuning {
            reach: super::highway::GAPS_LY,
            stall: super::highway::STALL,
            planning: 0,
            allowance: Some(super::highway::ALLOWANCE),
            crossing: Crossing::default(),
        }
    }
}

impl Tuning {
    /// What the coarse plan's estimate is multiplied by, against [`WHOLE`]
    /// for a hop
    ///
    /// The plan's own leaning, where [`Routing::weight`] is the route's.
    /// Exact at nothing, which is what [`Self::planning`] opens at.
    pub(super) fn weight(&self) -> u32 {
        WHOLE + self.planning
    }
}

/// How the jumps that cross one gap of the plan are found
///
/// The plan promises that two boost stars are close enough to string
/// together; something has to say which systems the ship actually flies
/// through to get from the one to the other. Measured at 50 ly, only 8 of a
/// crossing's 117 gaps need searching at all — the rest are a single
/// supercharged jump, answered by arithmetic — but those 8 are 2.0 s of the
/// 2.5 s the whole route takes, and the worst single one is 1.24 s.
///
/// Called a **gap** and not a leg: a leg is what the user means by a part
/// of a multi-stop trip, which the bar says outright ("3 Leg Route"), and a
/// hop is what a row calls one jump. This is neither. The router's own
/// prose still says "leg" internally — [`JumpGraph::leg`],
/// [`JumpGraph::flown`] — which is the word EDDA uses for it too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Crossing {
    /// Step across the gap, on whatever the route is weighed by
    ///
    /// A gap has a boost star at either end — the one the ship stands on
    /// and the next one the plan named — and this walks across it: one
    /// scan of the sphere a jump reaches, the cheapest candidate per light
    /// year of ground closed on that next boost star taken, and again from
    /// there until it is one jump off. No search at all.
    ///
    /// **Cheapest by the route's own metric**, which for a jump-counted
    /// route is the candidate *nearest* the next star — a jump costing one
    /// wherever it lands — and for a fuel-weighed one is emphatically not
    /// that. It was nearness alone until [`JumpGraph::stepped`] read
    /// [`Weigh`], and a supercharged least-fuel crossing's gaps were
    /// therefore not weighed by fuel at all: measured, the same 166 stops
    /// and the same 157.41 tanks at every position of the hop rail, where
    /// scoring the step on fuel spends 142.21 to 148.28.
    ///
    /// Every step has to land closer to the next star than the last, which
    /// is what bounds the walk and what keeps it from stepping back where
    /// it came from. Where no jump lands any closer — a pocket, a wall of
    /// nothing in the way — the gap is searched at the route's own
    /// optimality instead, so nothing is lost but the microseconds the
    /// walk spent.
    ///
    /// **The default, and the measurements are why.** Against searching
    /// every gap at the route's own optimality, over `.index/full`:
    ///
    /// | crossing | stepped | searched | jumps |
    /// |---|---|---|---|
    /// | Sol → Colonia, 50 ly, 95% | 141 in **0.30 s** | 140 in 2.60 s | +1 |
    /// | Sol → Colonia, 25 ly, 95% | 334 in **0.84 s** | 332 in 6.19 s | +2 |
    /// | Sol → Colonia, 80 ly, 95% | 82 in 0.12 s | 81 in 0.14 s | +1 |
    /// | Colonia → Sgr A\*, 45 ly, 75% | 81 in **0.97 s** | 80 in 19.84 s | +1 |
    ///
    /// One jump in a hundred and forty — 0.7%, inside even a 95% ask — for
    /// eight to twenty times the speed. The 80 ly row is where it buys
    /// nothing: a wide jump crosses a gap in one or two anyway, so there
    /// was no plateau to skip.
    ///
    /// **It is a beam search of width one**, scored on the step's own
    /// price rather than on cost-plus-estimate, with no frontier to come
    /// back to and a monotone-progress rule in place of a visited set. A
    /// wider beam would survive the pocket this gives up on, at `k` sphere
    /// scans a step instead of one. Measured, it would buy almost nothing:
    /// of the 32 gaps that needed crossing across three corridors, width
    /// one carried **29**, and the three it handed over were all on the 25
    /// ly crossing — 2.8 ms of walking against a 0.74 s coarse plan in a
    /// 0.90 s route. The place a beam would earn its keep is not speed but
    /// *bounded* time: a beam with a step cap never reaches the unbounded
    /// search at all, trading "sometimes minutes" for "sometimes a worse
    /// route".
    ///
    /// Not to be confused with [`Routing::fanout`], which caps how many
    /// *neighbours of one expansion* a search relaxes. That search keeps
    /// its whole frontier and can still come back to anything in it; a beam
    /// throws the frontier away. EDDA has both, and its greedy cone
    /// (`ED_CONE_CAP` = 8, `long_range.rs:2999-3060`) is the beam-shaped
    /// one. Stepping itself is EDDA's `bridge_leg`
    /// (`long_range.rs:299-410`).
    #[default]
    Stepped,
    /// Search the gap at the route's own optimality
    ///
    /// The same bargain the whole plot was struck at, applied to one gap:
    /// no proof, since the plan the gap belongs to is a guess either way,
    /// and no walk. It is what [`Self::Stepped`] falls back to where a step
    /// cannot close, and a setting in its own right for the corridor where
    /// the walk's one jump in a hundred and forty is not wanted.
    ///
    /// **Proving a gap is not offered, and the measurements are why.** It
    /// never bought a jump: Colonia to Sgr A\* came to 53 jumps proven and
    /// 53 at the route's own optimality (14.64 s against 4.94 s), and Sol
    /// to Colonia at 25 ly came to 355 either way. Which is the same
    /// argument from the other side — a route asked for at 75% optimality
    /// has no use for a gap proven exact inside a plan that bounds nothing,
    /// and a route asked for at 100% has no gaps at all, being searched
    /// flat. See [`Routing::approximates`].
    Searched,
}

/// Which drive is fitted, and so what a jet cone is worth
///
/// Flying the jet of a neutron star or a white dwarf charges a frame shift
/// drive for one jump. What that multiplies the range by is a fact about the
/// drive, not about the star: a standard drive takes four times off a neutron
/// star and half again off a white dwarf, and the Mk II Supercharge Optimised
/// drive takes six and three. Where the boost can be had at all is
/// [`galos_index::Boost`], published per system.
///
/// Asked per route rather than set once, for the reason a jump range is: the
/// same two ends flown by a different ship is a different route through
/// different systems, and a map that redrew the line under an old label would
/// be lying about what it plotted. See [`crate::systems::filter::Filter`].
///
/// White dwarfs are in both settings and worth less than they look. Their
/// exclusion zone is much larger, the wiki calls them not worth the risk for
/// half again, and there are seven thousand of them against ninety-four
/// thousand neutron stars in the data — so what they change is a route with a
/// gap in its neutron chain, and little else.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) enum Drive {
    /// No supercharging: every jump is the range the ship reaches unaided.
    #[default]
    Unaided,
    /// A standard drive: four times off a neutron star, half again off a
    /// white dwarf.
    Standard,
    /// The Mk II Supercharge Optimised drive: six times and three.
    Optimised,
}

impl Drive {
    /// What a jump out of a system offering `boost` is multiplied by
    ///
    /// One where there is no boost to be had or no drive to take it, so a
    /// caller can scale by this unconditionally.
    pub(crate) fn factor(&self, boost: Option<Boost>) -> f64 {
        match (self, boost) {
            (Drive::Unaided, _) | (_, None) => 1.,
            (Drive::Standard, Some(Boost::WhiteDwarf)) => 1.5,
            (Drive::Standard, Some(Boost::Neutron)) => 4.,
            (Drive::Optimised, Some(Boost::WhiteDwarf)) => 3.,
            (Drive::Optimised, Some(Boost::Neutron)) => 6.,
        }
    }

    /// The most any jump can be multiplied by
    ///
    /// What the estimate of the jumps remaining has to divide by. A heuristic
    /// that never overstates what is left is what makes the search come back
    /// with a genuine fewest-jumps route, and any step of the way might be
    /// taken out of a neutron star — so the estimate has to allow the widest
    /// jump the drive could make, however few systems can offer one. It costs
    /// a weaker estimate and more of the graph searched, which is the price of
    /// the answer being true.
    pub(crate) fn widest(&self) -> f64 {
        self.factor(Some(Boost::Neutron))
    }

    /// What the row for a route says it was plotted for, where anything.
    pub(crate) fn named(&self) -> Option<&'static str> {
        match self {
            Drive::Unaided => None,
            Drive::Standard => Some("supercharged"),
            Drive::Optimised => Some("SCO supercharged"),
        }
    }
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
    ///
    /// Whole light years across a galaxy a hundred thousand of them wide, so
    /// a `u32` holds every route there is with room to spare — and a search
    /// holds one of these per system, where the eight bytes a `u64` would add
    /// are twenty megabytes that could never carry a number.
    light_years: u32,
}

/// What a leg costs when the fuel is what is being saved: what the jump
/// burns, and the jump itself
///
/// **Fuel first, and the jump count only to settle a tie.** The other way
/// round from [`Cost`], which is the whole difference between a route flown
/// in the fewest jumps and one flown on the least fuel — and they are
/// genuinely opposed asks: the fewest jumps means every jump pushed as near
/// the ship's range as the sky allows, and a jump at full range costs the
/// drive's whole maximum fuel by definition.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
struct Burn {
    /// What the chain has burned, in hundred-thousandths of a jump's
    /// maximum fuel
    ///
    /// Integer, as every cost here is, so the comparison is total and the
    /// arithmetic exact. A route of four hundred jumps burns at most forty
    /// million of these, which is a hundredth of what a `u32` holds.
    fuel: u32,
    /// How many jumps it took, to settle a tie between two burning the same
    jumps: u32,
}

/// What one jump's maximum fuel is worth, in the integers a search adds up
///
/// A jump at the ship's full range costs the drive's whole maximum fuel —
/// that being what the range *is* — so this is the cost of such a jump and
/// every shorter one is a fraction of it.
///
/// **Hundred-thousandths, and it used to be thousandths.** Two things went
/// wrong at that resolution, both of them at the bottom of the scale where
/// a least-fuel route lives. A jump of a light year at a 45 light year
/// range priced at `(1/45)^2 x 1000 = 0.49`, which rounded to **nothing**:
/// the graph held free edges, so no positive number was a lower bound on
/// the fuel left to burn and a 95% route could come back *cheaper* than
/// the proven one — measured, 2.144 against 2.148 on a 702 light year
/// crossing. And at a 100 light year range the ordinary legs of such a
/// route, three to seven light years, priced at 1 to 5: differences of a
/// fifth in the fuel landing on the same integer.
///
/// A hundred times finer puts those legs at 49 to 476 and the light-year
/// jump at 49. See [`burned`], which will not price any jump at nothing
/// whatever the resolution.
const TANKFUL: u32 = 100_000;

/// What a jump is priced at before it burns anything, for a route that will
/// not split a jump below `hop` percent of the ship's range
///
/// **The least fuel there is, is as many hops as the sky offers.** Fuel
/// goes as the square of a jump at least — `(d/range)^p`, `p` from 2.0 to
/// 2.9 by the drive's class — so splitting *any* jump into two always
/// costs less, and nothing about the arithmetic ever stops splitting. What
/// stops it is the star field running out of stars. Measured over
/// `.index/full`, Sol outward at a fifty light year range with the price
/// set to nothing:
///
/// | crossing | fewest jumps | least fuel, unpriced |
/// |---|---|---|
/// | 85 ly | 2 jumps, 1.445 fuel | **16 jumps**, 0.323 fuel, hops to 0.41 ly |
/// | 300 ly | 6 jumps, 5.465 fuel | **94 jumps**, 0.848 fuel, hops to 0.43 ly |
///
/// So the true optimum is real, is worth six times the fuel, and is
/// ninety-four jumps of a third of a light year each — an hour and a half
/// of flying to cross what six jumps cross in five minutes. It also took
/// 9.2 s to find against 2.1 ms, for the reason given in
/// [`JumpGraph::economical`]: unpriced, there is no lower bound on the fuel
/// left to burn, so A\* has no heading and the walk is Dijkstra's.
///
/// **Which makes this the user's trade and not the map's.** Crossing `D`
/// light years in `k` equal jumps is priced at
/// `k x FLOOR + k x (D / (k x range)) ^ 2 x TANKFUL`, least at a jump of
/// `range x sqrt(FLOOR / TANKFUL)` — so setting the price to the fuel of a
/// `hop`-of-range jump is exactly saying *split no further than that*. The
/// route then flies about `1/hop` times the jumps of the fewest-jumps
/// route and burns about `hop` times the fuel: a straight line between the
/// two, with the fewest jumps at one end and the star field at the other.
///
/// | hop | jumps against the fewest | fuel against the fewest |
/// |---|---|---|
/// | 75% | 1.3x | 75% |
/// | 50% | 2x | 50% |
/// | 25% | 4x | 25% |
///
/// **Nothing at a hop of nothing, which is the honest off position.** Then
/// no jump is priced and what comes back is the least fuel there is, with
/// the star field the only thing stopping the splitting — ninety-four hops
/// across three hundred light years, measured. That is the true optimum and
/// a reader is entitled to ask for it.
///
/// It costs what it costs: with no price on a jump there is no lower bound
/// on the fuel left to burn, so the estimate is nothing and the walk has no
/// heading. 9.2 s over three hundred light years against 2.1 ms weighed by
/// jumps, and the galaxy off the scale. The route's own record says which it
/// was asked for, and the pruning in [`JumpGraph::search`] is what keeps it
/// from being hopeless rather than merely slow.
fn floor(hop: u32) -> u32 {
    let hop = hop.min(100);
    TANKFUL * hop * hop / 10_000
}

/// The middle of the trade, which the measurements and the fixtures are
/// taken at
///
/// Half the range: twice the jumps of the fewest-jumps route for half the
/// fuel, which is the trade most readers would pick if asked, and the one
/// they can move either way once they have seen what it costs.
///
/// A fixture and no longer a default. The form asks for the trade on one
/// rail whose ends are the fewest jumps and the least fuel, so where the
/// handle stands *is* the hop and there is nothing left for a default hop
/// to mean. See [`crate::ui`].
#[cfg(test)]
pub(crate) const HOP: u32 = 50;

/// How many of each sphere's systems a fuel-weighed route expands, unless
/// told otherwise
///
/// The nearest sixty-four, and the measurement is the argument. A route
/// weighed by fuel never takes a long jump — the longest on a 186 light
/// year crossing at a hundred light years of range was 17.8 — so most of
/// what a sphere offers is candidates the answer cannot use, and every one
/// of them is measured and relaxed. Over `.index/full`, least fuel over
/// unpriced hops at 95%, against the fuel of the proven route:
///
/// | expanded | 186 ly at 45 ly | 702 ly at 45 ly | 186 ly at 100 ly |
/// |---|---|---|---|
/// | nearest 8 | +4.9%, 0.15 s | +2.6%, 6.9 s | +0.2%, 0.25 s |
/// | **nearest 64** | **+1.8%, 0.36 s** | **+1.2%, 14.4 s** | **+0.2%, 0.66 s** |
/// | every one in range | +0.0%, 2.68 s | +0.1%, 52.6 s | +0.0%, 0.99 s |
/// | the proven route | — 2.66 s | — 54.0 s | — 18.1 s |
///
/// So sixty-four is within a fiftieth of the fuel at three to twenty-seven
/// times the speed of proving it, where taking every system in range
/// spends the whole wait to buy the last hundredth of a percent. Eight is
/// there for a reader in a hurry and is the bottom of the rail: under it
/// nothing was measured, and a setting that sometimes comes back with no
/// route at all is worse than a slow one.
///
/// **And it is the reader's only at an unpriced hop**, which is the one
/// place the form offers it — and now the one place it applies, because
/// the two were not the same thing. A priced hop takes long jumps and few
/// of them, so from a quarter of the range up a cap really is inert: 15
/// stops and 1.251 tanks at 64 or at 512, measured. **Under a quarter it
/// is not**, and the count the rail last held went on deciding two
/// percent of the tank where nothing on screen said so — 0.733 tanks
/// against 0.716 at a 5% hop. So [`Routing::fanout`] holds the valve
/// there instead, and what this number means is exactly what the rail
/// shows.
///
/// Unpriced, what the cap is worth is decided by how many systems one
/// jump reaches, at 95% over `.index/full`:
///
/// | corridor | in one jump | nearest 8 | nearest 64 | all in range |
/// |---|---|---|---|---|
/// | 186 ly out, 25 ly ship | 172 | 89 ms | 1.22 s | 1.64 s |
/// | 186 ly out, 45 ly ship | 921 | 168 ms | 394 ms | 3.06 s |
/// | 186 ly out, 100 ly ship | 8,261 | 738 ms | 1.66 s | 10.61 s |
/// | 702 ly out, 45 ly ship | 921 | 7.67 s | 16.52 s | 56.08 s |
///
/// Widest where the ship's own jump is widest — a hundred light year
/// sphere holds 8,261 systems against a 25 ly ship's 172, and the search
/// is quadratic in that — and largest in seconds where the route is long:
/// forty of the forty-eight seconds the long crossing saves come from
/// taking sixty-four rather than everything. Against a short-range ship it
/// is worth 1.3x and nothing else, the sphere barely holding more than the
/// cap allows.
pub(crate) const EXPAND: u32 = 64;

/// What a jump of `leg` costs the tank, at a ship whose range is `range`
///
/// `(leg / range) ^ 2` of a jump's maximum fuel, which is the game's own
/// cost with the ship divided out of it: see `info::fuel_rule`. The square
/// rather than the drive's own exponent — 2.0 through 2.9 by its class —
/// because the map is not told which drive is fitted, and the square is the
/// flattest of them: it penalises a long jump least, so an economical route
/// weighed by it splits its jumps *less* than the real drive would want.
/// Conservative in the direction of fewer jumps, which is the direction a
/// reader is not surprised by.
///
/// Capped at one jump's maximum, because that is what the number means: a
/// drive with a maximum fuel per jump does not spend more than it on a
/// jump, however far a jet cone throws the ship. Which makes a supercharged
/// jump the cheapest distance in the game, and an economical route reach
/// for cones rather than avoid them.
///
/// **And floored at one, because a jump is never free.** Rounding decided
/// otherwise at the old resolution — a light year at a 45 light year range
/// came to nothing — and a free edge is not a rounding error but a
/// different problem: it says a route may cross any distance reachable in
/// short enough hops for no fuel at all, which makes the ordering
/// degenerate and the proven answer a fiction. Any jump costs something.
fn burned(leg: f64, range: f64) -> u32 {
    let share = (leg / range).min(1.);
    let burn = (share * share * TANKFUL as f64).round() as u32;
    match leg > 0. {
        true => burn.max(1),
        false => 0,
    }
}

/// What a step and a route cost, as a search adds them up
///
/// A search keeps one of these per system it has reached, in an array over
/// the cell the system belongs to — so "nothing has reached this yet" is a
/// value of the type rather than an absent entry, and the search reads a
/// slot instead of hashing. See [`Ledger`].
trait Metric: Ord + Copy + std::ops::Add<Output = Self> {
    /// What the system the search sets out from has cost so far
    const ZERO: Self;

    /// What a system nothing has reached holds
    ///
    /// Dearer than any route there is, so "unreached" and "dearer than
    /// this" are the one comparison and the relaxation test needs no
    /// branch of its own.
    const UNREACHED: Self;

    /// What the chain has spent, as one number a step can only add to
    ///
    /// **What the pruning turns on, and the one thing every metric here has
    /// to be able to say.** A candidate offered from a chain that has spent
    /// this much will cost strictly more than it, every step costing
    /// something; so a system already reached for no more than this cannot
    /// be bettered through here, and a *cell* whose every system was
    /// reached for no more is a cell that can be skipped without its
    /// payload being read at all. That test is what empties the settled
    /// middle of a search — measured, 45.4 billion systems looked at
    /// against 3.9 million relaxed. See [`Ledger::settled`].
    ///
    /// It used to be called `jumps`, and to be the jump count, on the
    /// argument that one jump more is dearer under either ordering. True of
    /// both orderings that count jumps first, and **false of fuel**: a
    /// chain of five short hops can burn less than a chain of three long
    /// ones, so a fuel-weighed search pruned by jump count would throw away
    /// the very chains it is looking for. What the rule actually needs is a
    /// scalar that rises along every chain, which is the jump count for one
    /// ordering and the fuel for the other.
    fn spent(&self) -> u32;

    /// This cost scaled by `num / den`
    ///
    /// Two uses, both of them arithmetic the search cannot do generically
    /// otherwise: leaning on an estimate by [`Routing::weight`], and
    /// working out from a route already in hand what a better one is
    /// allowed to cost. Saturating and integer, so a galactic route cannot
    /// wrap a ceiling into a small number and prune the answer away.
    fn share(self, num: u32, den: u32) -> Self;
}

/// One integer scaled by `num / den`, saturating rather than wrapping
///
/// In `u64`, because a long route's cost times a weight overruns a `u32` on
/// the way through and fits again when it lands.
fn share(of: u32, num: u32, den: u32) -> u32 {
    let scaled = of as u64 * num as u64 / den.max(1) as u64;
    scaled.min(u32::MAX as u64) as u32
}

impl Metric for u32 {
    const ZERO: u32 = 0;
    const UNREACHED: u32 = u32::MAX;

    fn spent(&self) -> u32 {
        *self
    }

    fn share(self, num: u32, den: u32) -> u32 {
        share(self, num, den)
    }
}

impl Metric for Cost {
    const ZERO: Cost = Cost { jumps: 0, light_years: 0 };
    const UNREACHED: Cost = Cost { jumps: u32::MAX, light_years: u32::MAX };

    fn spent(&self) -> u32 {
        self.jumps
    }

    fn share(self, num: u32, den: u32) -> Cost {
        Cost {
            jumps: share(self.jumps, num, den),
            light_years: share(self.light_years, num, den),
        }
    }
}

impl Metric for Burn {
    const ZERO: Burn = Burn { fuel: 0, jumps: 0 };
    const UNREACHED: Burn = Burn { fuel: u32::MAX, jumps: u32::MAX };

    /// The fuel, which is what this ordering weighs first and what a step
    /// always adds at least [`FLOOR`] of.
    fn spent(&self) -> u32 {
        self.fuel
    }

    fn share(self, num: u32, den: u32) -> Burn {
        Burn {
            fuel: share(self.fuel, num, den),
            jumps: share(self.jumps, num, den),
        }
    }
}

impl std::ops::Add for Burn {
    type Output = Burn;

    fn add(self, other: Burn) -> Burn {
        Burn {
            fuel: self.fuel.saturating_add(other.fuel),
            jumps: self.jumps.saturating_add(other.jumps),
        }
    }
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

/// What a search has reached, as it reaches it
///
/// Shared between the task running the search and the map drawing it: the
/// search fills it in and [`super::frontier`] reads it out a frame at a time.
/// Behind a lock rather than a channel because what the map wants is not every
/// message but the state of the thing.
///
/// Three things, which are the three ways a search is worth drawing. The set
/// of coarse cells it has expanded in, which only ever grows: the closed set,
/// a region filling. The last few of those cells, which replaces itself: the
/// leading edge, where the search is working now. And the chain it has found
/// to the closest system it has reached, which is what says whether any of it
/// is getting anywhere.
///
/// The edge is cells and not jumps for the same reason the closed set is. A
/// window of the last jumps taken is what this held first, and A* pops from
/// all over its frontier, so those jumps are scattered and unrelated to one
/// another — forty-eight strokes of debris that flicker rather than move,
/// because there is no motion between them to draw. Quantised to the same
/// cells, the edge holds still while the search works a region and steps when
/// it moves to the next, which is the thing worth seeing.
///
/// None of the three is thinned. An earlier cut of this kept one cumulative
/// sample and halved it whenever it filled, which on a search of half a
/// million expansions fires eight times: what that draws is a haze that fills
/// and abruptly thins, over and over, and it reads as the search starting
/// again rather than as the search going on. A bound the picture never
/// notices is worth more than a bound that shows.
///
/// It carries one thing that is not a picture: whether the search has been
/// told to give up. A click on the plot button over a route that is still
/// being searched takes it back ([`Self::abandon`]), and the search reads
/// this often enough to stop — a galactic crossing is minutes of a pool
/// thread, and dropping the task does not interrupt a body that is already
/// running.
#[derive(Default)]
pub(crate) struct Frontier {
    /// The picture, under the lock.
    reached: Mutex<Reached>,
    /// Where the route is going, in light years
    ///
    /// Fixed for the whole search and outside the lock, so every phase of a
    /// route measures how close it has come against the same place. A plan
    /// flown leg by leg searches toward one waypoint after another, and a
    /// chain kept for whichever tip was closest to *the leg's* goal is a
    /// picture that starts again at every waypoint.
    goal: DVec3,
    /// Whether the search has been told to give up
    ///
    /// Outside the lock and read on every expansion, which is why it is an
    /// atomic and not a field of [`Reached`]: the picture is copied out
    /// under a lock once a batch, and this is asked half a million times a
    /// route.
    stopped: AtomicBool,
}

/// What has been reached, under the lock.
#[derive(Default)]
struct Reached {
    /// Where the search set out from, in light years, once it is known
    ///
    /// [`None`] until the search starts, which is a search whose ends could
    /// not be resolved: there is nothing to draw and nowhere to draw it.
    from: Option<DVec3>,
    /// How wide a cell of the closed set is, in light years
    across: f64,
    /// The cells the search has expanded in, by their corner in cell counts
    cells: HashSet<[i32; 3]>,
    /// The last cells worked in, oldest first
    edge: VecDeque<[i32; 3]>,
    /// The coarse plan, where the route was planned before it was flown
    ///
    /// Its own layer and its own colour, drawn under the branches: a
    /// refinement that shared the plan's line read as the same line, most
    /// legs being one jump and drawing exactly over the hop they refine.
    plan: Vec<DVec3>,
    /// What the searching has drawn: a branch per leg flown, and the leg
    /// under way
    ///
    /// One strand for a flat search, which is the chain to the closest
    /// system it has reached. Several for a plan flown leg by leg; see
    /// [`Sampler::strands`].
    reaching: Vec<Vec<DVec3>>,
    /// How far that system is from the goal, in light years
    closest: f64,
    /// How many systems have been expanded, every one of them counted
    expanded: u64,
    /// Whether the search has stopped, whatever it found
    finished: bool,
    /// How many times anything here has moved
    ///
    /// What the map rebuilds its meshes against. Counting what is held cannot
    /// answer it: the edge is a window that turns over at a fixed length and
    /// the chain moves without changing how many links it has, so two frames
    /// of entirely different pictures count the same and the drawing stands
    /// still until something else disturbs it — which is what happened, and
    /// what read as a picture that only updated when the camera moved.
    ///
    /// Read on its own by [`super::frontier::draw`] before it takes a copy,
    /// so a frame where nothing has moved costs one lock and one compare
    /// rather than a walk of the whole closed set.
    revision: u64,
    /// How many times the closed set has grown, or been drawn coarser
    cells_at: u64,
    /// How many times the leading edge has moved
    edge_at: u64,
    /// How many times the chain to the closest reached has moved
    ///
    /// The three of these are what let a flush that only added a cell leave
    /// the edge's and the chain's meshes alone. [`Self::revision`] is their
    /// disjunction, and is what says whether to look at them at all.
    reaching_at: u64,
}

/// What the map draws of a search, taken in one lock.
pub(crate) struct Drawn {
    /// Where the search set out from
    pub(crate) from: DVec3,
    /// How wide a cell of the closed set is, in light years
    pub(crate) across: f64,
    /// The middle of every cell expanded in
    pub(crate) cells: Vec<DVec3>,
    /// The middle of the last few cells worked in, newest last
    pub(crate) edge: Vec<DVec3>,
    /// Where the route is going
    ///
    /// Drawn to, not just measured against: a search's chain stops at the
    /// closest thing it has reached, which on a galactic plot is hundreds
    /// of light years short of the goal and *looks arrived* — the gap is
    /// sub-pixel at that zoom. See [`super::frontier::left_color`].
    pub(crate) goal: DVec3,
    /// The coarse plan, empty where there was none
    pub(crate) plan: Vec<DVec3>,
    /// A branch per leg flown, and the leg under way
    pub(crate) reaching: Vec<Vec<DVec3>>,
    /// How far that system is from the goal, in light years
    pub(crate) closest: f64,
    /// How many times anything here has moved; see [`Reached::revision`]
    pub(crate) revision: u64,
    /// How many times the closed set has grown or coarsened
    pub(crate) cells_at: u64,
    /// How many times the leading edge has moved
    pub(crate) edge_at: u64,
    /// How many times the chain has moved
    pub(crate) reaching_at: u64,
}

impl Frontier {
    /// A frontier for a search from `from` to `goal`
    ///
    /// The cell of the closed set is [`super::frontier::CELLS`]-th of the way
    /// between them, so the picture is about that many cells along the route
    /// whether that is two hundred light years or twenty-two thousand. Which
    /// is what bounds the set by the geometry rather than by a count: the
    /// cells the search touches are the corridor it searched, and a corridor
    /// is not much wider than the line through it.
    ///
    /// A corridor is what a search that has a route to find walks. One that
    /// has not expands in every direction until the reachable component runs
    /// out, and then the geometry bounds nothing — so the cell is widened
    /// again past [`super::frontier::CELL_CEILING`] of them; see
    /// [`Sampler::flush`].
    pub(crate) fn between(from: DVec3, goal: DVec3) -> Arc<Frontier> {
        Arc::new(Frontier {
            reached: Mutex::new(Reached {
                from: Some(from),
                across: (from.distance(goal) / super::frontier::CELLS).max(1.),
                closest: f64::INFINITY,
                ..Reached::default()
            }),
            goal,
            stopped: AtomicBool::new(false),
        })
    }

    /// A sampler that feeds this, for a search to carry.
    pub(crate) fn sampler(self: &Arc<Frontier>) -> Sampler {
        let reached = self.reached.lock().expect("the frontier lock");
        Sampler {
            goal: self.goal,
            into: Arc::clone(self),
            expanded: 0,
            across: reached.across,
            cells: HashSet::new(),
            edge: VecDeque::with_capacity(super::frontier::EDGE),
            worked: None,
            stepped: false,
            plan: Vec::new(),
            flown: Vec::new(),
            reaching: Vec::new(),
            closest: f64::INFINITY,
            settled: true,
            flushed: 0,
            quiet: false,
        }
    }

    /// What there is to draw, or [`None`] where the search has reached
    /// nothing yet.
    pub(crate) fn drawn(&self) -> Option<Drawn> {
        let reached = self.reached.lock().expect("the frontier lock");
        Some(Drawn {
            from: reached.from?,
            across: reached.across,
            cells: reached
                .cells
                .iter()
                .map(|cell| middle(*cell, reached.across))
                .collect(),
            edge: reached
                .edge
                .iter()
                .map(|cell| middle(*cell, reached.across))
                .collect(),
            goal: self.goal,
            plan: reached.plan.clone(),
            reaching: reached.reaching.clone(),
            closest: reached.closest,
            revision: reached.revision,
            cells_at: reached.cells_at,
            edge_at: reached.edge_at,
            reaching_at: reached.reaching_at,
        })
    }

    /// Where the search set out from, once its ends have resolved
    ///
    /// Fixed for the whole of a search, so [`super::frontier::draw`] takes it
    /// once and keeps it: the depth a mark is sized at is measured from here,
    /// and that has to be known before the revision can be weighed against a
    /// zoom that has moved.
    pub(crate) fn from(&self) -> Option<DVec3> {
        self.reached.lock().expect("the frontier lock").from
    }

    /// How many times the picture has moved; see [`Reached::revision`]
    ///
    /// The whole of what [`super::frontier::draw`] needs to know whether to
    /// take a copy at all. Asked first and on its own, since [`Self::drawn`]
    /// walks every cell of the closed set and clones the chain, under the lock
    /// the search flushes through — work worth nothing on a frame where the
    /// picture has not changed, which is most of them.
    pub(crate) fn revision(&self) -> u64 {
        self.reached.lock().expect("the frontier lock").revision
    }

    /// How many systems the search has expanded.
    pub(crate) fn expanded(&self) -> u64 {
        self.reached.lock().expect("the frontier lock").expanded
    }

    /// Whether nothing more will come of this search
    ///
    /// Set when the search stops of its own accord ([`Sampler::done`]) and
    /// when its leg is given up on ([`Self::abandon`]), the map having the
    /// same thing to do either way: take the layers down.
    pub(crate) fn finished(&self) -> bool {
        self.reached.lock().expect("the frontier lock").finished
    }

    /// Give up on this search, its leg having been cancelled
    ///
    /// Two things at once, because a cancelled leg needs both.
    ///
    /// **The picture comes down.** A route task dropped before the pool has
    /// begun polling it never runs, so nothing calls [`Sampler::done`] and
    /// the frontier would sit unfinished for the rest of the session — three
    /// layer entities apiece, re-uploaded on every zoom, and counted by
    /// [`Frontiers::expanded`] and [`Frontiers::closest`] that the form
    /// reads.
    ///
    /// **And the search stops.** Dropping the task does *not* interrupt a
    /// body the pool is already running: a route walk is one long stretch of
    /// arithmetic with nothing to await, so a cancelled galactic crossing
    /// would go on burning a pool thread for its full ten minutes with
    /// nobody left to read the answer. The search reads
    /// [`Self::stopped`] once per expansion and gives up, which is what
    /// makes a click on the plot button over a route being searched mean
    /// something.
    ///
    /// [`Frontiers::expanded`]: super::frontier::Frontiers::expanded
    /// [`Frontiers::closest`]: super::frontier::Frontiers::closest
    pub(crate) fn abandon(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.reached.lock().expect("the frontier lock").finished = true;
    }

    /// Whether the search has been told to give up.
    ///
    /// Relaxed: the flag is a one-way switch and the only thing that turns
    /// on it is whether the search stops this expansion or the next.
    pub(crate) fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }
}

/// Which cell of a grid `across` light years wide a place falls in.
fn cell_of(at: DVec3, across: f64) -> [i32; 3] {
    [
        (at.x / across).floor() as i32,
        (at.y / across).floor() as i32,
        (at.z / across).floor() as i32,
    ]
}

/// The middle of such a cell.
fn middle(cell: [i32; 3], across: f64) -> DVec3 {
    DVec3::new(
        (cell[0] as f64 + 0.5) * across,
        (cell[1] as f64 + 0.5) * across,
        (cell[2] as f64 + 0.5) * across,
    )
}

/// The cell a place falls in once the grid is twice as wide
///
/// Euclidean division, which is the whole of why it is exact: the cell of a
/// grid `2w` wide is `floor(x / 2w)`, and that is `floor(floor(x / w) / 2)`
/// for every sign of `x`. So a set of cells can be widened without going back
/// to the places that filled it.
fn coarser(cell: [i32; 3]) -> [i32; 3] {
    [cell[0].div_euclid(2), cell[1].div_euclid(2), cell[2].div_euclid(2)]
}

/// A search's own tally, flushed into a [`Frontier`] in batches
///
/// What every expansion pays: a counter, a compare and a distance to the
/// goal. What one in [`super::frontier::STRIDE`] pays on top: a cell insert,
/// and a ring push where that cell is a new one. What the closest system reached moving pays: a walk back
/// up the search's parent map, hundreds of links at the worst. The lock and
/// the copies happen once a batch.
pub(crate) struct Sampler {
    into: Arc<Frontier>,
    /// How many expansions have been seen
    expanded: u64,
    /// How wide a cell of the closed set is, in light years
    across: f64,
    /// The cells expanded in since the last flush
    cells: HashSet<[i32; 3]>,
    /// The last cells worked in, oldest first
    edge: VecDeque<[i32; 3]>,
    /// The cell the last sample fell in, so a run of them in one cell is one
    /// step of the edge rather than a dozen
    worked: Option<[i32; 3]>,
    /// Whether the edge has moved since the last flush, so a flush that only
    /// added a cell leaves the edge's mesh where it is
    stepped: bool,
    /// Where the route is going, which is what `closest` is measured to
    goal: DVec3,
    /// The coarse plan, waypoint to waypoint, once there is one
    ///
    /// A galactic route is planned over the jet cones and then flown leg by
    /// leg ([`JumpGraph::flown`]), and **each leg is its own search with
    /// its own parent map**: the chain a leg can hand over is the leg's,
    /// two to six jumps of a hundred and forty. Measured, the drawn chain
    /// went from 116 links to 2 the moment the first leg began — the plan
    /// thrown away and replaced by a stub for the two seconds the legs
    /// take.
    ///
    /// So the plan is kept whole here and the legs are drawn as branches
    /// off it; see [`Self::strands`].
    plan: Vec<DVec3>,
    /// The legs already flown, each the jumps that leg actually took
    ///
    /// One strand apiece rather than spliced into the plan, because that is
    /// what a refinement *is*: the plan promised a straight hop between two
    /// cones and the leg found the way the ship can really fly it. Drawn
    /// side by side, the branch is the difference between the two.
    flown: Vec<Vec<DVec3>>,
    /// The chain the search now running has reached, its own branch
    reaching: Vec<DVec3>,
    /// How far the closest system reached is from the goal, in light years
    closest: f64,
    /// Whether the chain in hand is the one for the closest system reached
    settled: bool,
    /// How many expansions have already been handed over
    ///
    /// The frontier's tally is added to rather than assigned, so several
    /// samplers can feed one search's picture — which is what a plan's legs
    /// refined side by side are. See [`Self::beside`].
    flushed: u64,
    /// Whether this one counts and never draws
    ///
    /// A [`Self::beside`] sibling, carried by a leg being refined in
    /// parallel with its neighbours. The picture is one thing and the legs
    /// are many: the chain, the closed set and the working edge all
    /// *replace* what the frontier holds, so two samplers drawing at once
    /// would each rub out the other. A quiet one keeps the expansion tally
    /// honest and the cancellation flag readable, and its leg is drawn by
    /// the sampler that owns the picture once the leg has landed
    /// ([`Self::flew`]).
    quiet: bool,
}

impl Sampler {
    /// A sibling that counts into the same frontier and draws nothing
    ///
    /// What a leg refined beside its neighbours carries. It shares the
    /// frontier, so an expansion of any leg shows in the readout and a
    /// route taken back stops every leg at once ([`Self::stopped`]); it
    /// draws nothing, because the chain, the closed set and the edge each
    /// replace what the frontier holds and two of these drawing at once
    /// would rub each other out. The leg's own way is drawn by the owner
    /// when it lands.
    pub(crate) fn beside(&self) -> Sampler {
        Sampler {
            into: Arc::clone(&self.into),
            goal: self.goal,
            across: self.across,
            expanded: 0,
            cells: HashSet::new(),
            edge: VecDeque::new(),
            worked: None,
            stepped: false,
            plan: Vec::new(),
            flown: Vec::new(),
            reaching: Vec::new(),
            closest: f64::INFINITY,
            settled: false,
            flushed: 0,
            quiet: true,
        }
    }

    /// Note the expansion of `node`, which sits at `at`
    ///
    /// `came` is the search's own record of where each system was reached
    /// from — the thing A* keeps in order to give an answer at all — so the
    /// jump drawn is the jump the search took to get here and the chain is
    /// the one it would hand back if this were the goal. `place` says where
    /// a system sits, and is asked only where something is drawn or the
    /// record moves.
    pub(crate) fn expanded(
        &mut self,
        node: Node,
        at: DVec3,
        came: &FxHashMap<Node, Node>,
        place: impl Fn(Node) -> DVec3,
    ) {
        self.reached(at, || {
            let mut walk = node;
            let mut chain = vec![place(walk)];
            while let Some(&before) = came.get(&walk) {
                walk = before;
                chain.push(place(walk));
                // A walk that will not end is a link cycle rather than a
                // route, and one drawn forever would be a hang.
                if chain.len() > came.len() + 1 {
                    break;
                }
            }
            chain.reverse();
            chain
        });
    }

    /// Note an expansion at `at`, with `chain` the hops back to the start
    ///
    /// The half of [`Self::expanded`] that is about the picture rather than
    /// about a jump graph, so the coarse plan over the boost stars
    /// ([`super::highway::Highway::plan`]) draws itself the same way — its
    /// nodes are cones and its parents an array, and there is no galaxy
    /// node to hand over. `chain` is asked for only where this is the
    /// closest anything has come, which is the only time it is drawn.
    ///
    /// Which matters more than it sounds: the coarse plan is most of the
    /// wait on a galactic route (4.4–4.8 s of 6.5 s at 50 ly), and until
    /// this it drew nothing at all — a click that sat there for seconds
    /// with an empty sky before the legs began.
    pub(crate) fn reached(
        &mut self,
        at: DVec3,
        chain: impl FnOnce() -> Vec<DVec3>,
    ) {
        self.expanded += 1;

        // A quiet sibling counts and nothing else: no chain walked back, no
        // cell kept, and the tally handed over on the same beat. See
        // [`Self::beside`].
        if self.quiet {
            if self.expanded % super::frontier::STRIDE == 0 {
                self.flush();
            }
            return;
        }

        // How close the search has got, which is what the chain is drawn to.
        // Against the route's own goal and not the leg's, so the legs of one
        // plan are comparable and the form's readout counts down once.
        let away = at.distance(self.goal);
        if away < self.closest {
            self.closest = away;
            self.reaching = chain();
            self.settled = true;
        }

        if self.expanded % super::frontier::STRIDE != 0 {
            return;
        }
        let cell = cell_of(at, self.across);
        self.cells.insert(cell);
        // Where the work is now. Only where it has moved to a cell it was not
        // in: the search grinds through hundreds of systems in one cell, and
        // an edge that re-listed the same cell every sample would be a window
        // holding one place a dozen times over.
        if self.worked != Some(cell) {
            self.worked = Some(cell);
            self.stepped = true;
            self.edge.push_back(cell);
            if self.edge.len() > super::frontier::EDGE {
                self.edge.pop_front();
            }
        }
        if self.expanded
            % (super::frontier::STRIDE * super::frontier::BATCH as u64)
            == 0
        {
            self.flush();
        }
    }

    /// Say what was planned, before any of it is flown
    ///
    /// The waypoints of the coarse plan, which is the route as it stands
    /// until the legs refine it. Drawn from here on, so the picture does
    /// not fall back to whichever leg is under way.
    pub(crate) fn planned(&mut self, plan: Vec<DVec3>) {
        self.plan = plan;
        self.reaching.clear();
        self.settled = true;
    }

    /// Say a leg was flown, `jumps` being the way it went
    ///
    /// Kept as its own branch and the live chain cleared, so the next leg
    /// starts from nothing drawn rather than from the last one's tip.
    ///
    /// Redraws at once rather than waiting for the next leg to reach
    /// anything: a leg that is a single supercharged jump never samples at
    /// all, and most of a plan's legs are exactly that.
    pub(crate) fn flew(&mut self, jumps: Vec<DVec3>) {
        self.flown.push(jumps);
        self.reaching.clear();
        self.settled = true;
    }

    /// Every branch there is to draw: the legs flown, and the leg under way
    ///
    /// The plan is not among them — it has its own layer, drawn under these
    /// — and empty strands are dropped, a search that has reached nothing
    /// being a line of no length. A flat search hands over the one chain it
    /// has, which is what every route drew before the two-level planner.
    fn strands(&self) -> Vec<Vec<DVec3>> {
        let mut strands = Vec::with_capacity(self.flown.len() + 1);
        for strand in self.flown.iter().chain(std::iter::once(&self.reaching)) {
            if strand.len() > 1 {
                strands.push(strand.clone());
            }
        }
        strands
    }

    /// Hand what is held to the frontier
    ///
    /// The cells are merged in, the edge replaces whatever was there, and the
    /// chain is handed over where it has moved. Nothing is thrown away: the
    /// edge is bounded by its own length, and the cells by the corridor
    /// searched — or, where the search is not walking a corridor, by being
    /// drawn coarser.
    ///
    /// That is the loop at the end. Past [`super::frontier::CELL_CEILING`]
    /// cells the grid doubles and every cell held is mapped onto the wider
    /// one, which is exact ([`coarser`]) and needs none of the places back.
    /// The sampler's own width goes with it, so what it counts next lands on
    /// the same grid; so does its edge, which is copied over whole every
    /// flush and would otherwise put cells of the old width back. A doubling
    /// takes about eight cells to one, so the loop runs once in practice and
    /// terminates in any case.
    ///
    /// Each layer's own revision moves only where that layer did, so a flush
    /// that added a cell and nothing else leaves the edge's and the chain's
    /// meshes alone. [`Reached::revision`] moves where any of them did, and is
    /// what the map reads first: unchanged, it never asks for the copy.
    fn flush(&mut self) {
        let mut reached = self.into.reached.lock().expect("the frontier lock");
        // What this one has counted since it last said so, added rather than
        // assigned: a plan's legs are refined side by side and each carries
        // its own sampler ([`Self::beside`]), so the frontier's tally is the
        // sum of theirs and the owner's.
        reached.expanded += self.expanded - self.flushed;
        self.flushed = self.expanded;

        // A quiet sibling draws nothing, so there is nothing else to hand
        // over and no revision to move.
        if self.quiet {
            return;
        }

        let grew = !self.cells.is_empty();
        reached.cells.extend(self.cells.drain());
        if grew {
            reached.cells_at += 1;
        }

        let stepped = std::mem::take(&mut self.stepped);
        if stepped {
            reached.edge.clear();
            reached.edge.extend(self.edge.iter().copied());
            reached.edge_at += 1;
        }

        // Taken rather than read, so the chain is handed over on the flush
        // after it moved and not on every flush thereafter.
        let settled = std::mem::take(&mut self.settled);
        if settled {
            reached.closest = self.closest;
            reached.plan.clone_from(&self.plan);
            reached.reaching = self.strands();
            reached.reaching_at += 1;
        }

        let mut coarsened = false;
        while reached.cells.len() > super::frontier::CELL_CEILING {
            reached.across *= 2.;
            reached.cells =
                reached.cells.iter().copied().map(coarser).collect();
            for cell in reached.edge.iter_mut() {
                *cell = coarser(*cell);
            }
            self.across = reached.across;
            for cell in self.edge.iter_mut() {
                *cell = coarser(*cell);
            }
            self.worked = self.worked.map(coarser);
            coarsened = true;
        }
        // A wider cell moves every mark of both sampled layers, whatever else
        // happened this flush.
        if coarsened {
            reached.cells_at += 1;
            reached.edge_at += 1;
        }

        if grew || stepped || settled || coarsened {
            reached.revision += 1;
        }
    }

    /// Say the search has stopped, and hand over whatever is left.
    pub(crate) fn done(&mut self) {
        self.flush();
        self.into.reached.lock().expect("the frontier lock").finished = true;
    }

    /// Whether the search this feeds has been told to give up
    ///
    /// Asked once per expansion, which is why it is one relaxed atomic load
    /// and touches nothing under the lock. See [`Frontier::abandon`].
    pub(crate) fn stopped(&self) -> bool {
        self.into.stopped()
    }
}

/// The router's galaxy, and its graph once something has asked for a route.
///
/// The graph is [`None`] until then, and it costs almost nothing to make:
/// the galaxy's places are the cell payloads, mapped as a query reaches
/// them, so a graph is a handle on the index and not a structure over it. It
/// used to be a grid of its own — 32 bytes a system of points, an address
/// map beside them and a bucket per occupied cell, 13.7 GB and 32 s at
/// 200 M, paid on the click that asked for a route.
#[derive(Resource, Clone, Default)]
pub struct Jumps {
    /// The graph, once a route has asked for one.
    pub graph: Option<Arc<JumpGraph>>,
    /// The galaxy it reads, opened where the index is a directory this
    /// process can map. [`None`] over a transport that cannot be mapped,
    /// and then nothing routes.
    pub sky: Option<Arc<Sky>>,
}

impl Jumps {
    /// The galaxy as the router reads it, opened once for the session.
    pub fn over(sky: Arc<Sky>) -> Jumps {
        Jumps { graph: None, sky: Some(sky) }
    }

    /// The graph, opening it if this is the first route of the session.
    pub fn built(&mut self, boosts: &Boosts) -> Option<Arc<JumpGraph>> {
        let sky = self.sky.clone()?;
        let held = self
            .graph
            .get_or_insert_with(|| Arc::new(JumpGraph::over(&sky, boosts)));
        Some(Arc::clone(held))
    }
}

/// The galaxy a route is searched over: the mapped cell payloads, and what
/// can supercharge a drive.
///
/// A route in flight holds the graph it started on and finishes against
/// that. It is the right answer as well as the cheap one: a search half-run
/// against a galaxy that grew underneath it has been searching two
/// different skies. The payloads make that hold for free — a cell the feed
/// republishes is renamed into place, so a mapping this holds keeps reading
/// what it was given ([`galos_index::store`]).
#[derive(Clone)]
pub struct JumpGraph {
    /// The galaxy's places, read where they lie.
    sky: Arc<Sky>,
    /// Which systems can supercharge a drive, as published.
    ///
    /// Held beside the places rather than folded into them, though a boost
    /// is a fact about a place. The table moves on nearly every publish —
    /// four systems in a hundred can supercharge and the feed names eighty
    /// a minute — and it is read per expansion, which is one lookup for the
    /// system being left rather than one for each of the thousands it can
    /// see.
    boosts: Boosts,
    /// The boost stars as a graph of their own, once a long supercharged
    /// route has asked for one.
    ///
    /// Behind a [`OnceLock`] rather than built with the graph: it is a
    /// sort of four million rows into cells (213 ms at 200 M), which a
    /// route that is short, unaided, or proven has no use for. Shared
    /// rather than per-graph so the legs of a trip, which run at once on
    /// the compute pool, build it once between them.
    highway: Arc<OnceLock<Option<Arc<Highway>>>>,
}

/// How many of an expansion's neighbours are relaxed.
///
/// The fix for a search whose work is quadratic in stellar density. A
/// boosted jump in the core reaches a sphere holding thousands of systems,
/// and relaxing every one costs milliseconds *per expansion* — the same
/// place EDDA landed, whose `LEG_FANOUT` is this number and whose audit
/// found the result tracks corridor density at 0.66–1.03× rather than its
/// square.
///
/// The ones kept are the cheapest per light year of ground closed, priced
/// by whatever the route is weighed by, so what is thinned is the half of
/// the sphere that ask was never going to step into. See [`thinned`].
///
/// **One number at every setting, and it is [`Routing::approximates`] that
/// decides whether it applies at all.** A weight on the estimate bounds
/// the answer — weighted A\* proves a route inside `1 + over/100` of the
/// fewest — and a cap bounds nothing whatever: what it drops, the search
/// cannot learn, and no theorem says how much that costs. So it is on or
/// off with the promise, rather than scaled along with a percentage that
/// means something it cannot deliver.
///
/// Which leaves the question of whether it *should* scale, and the
/// measurement says the cap's currency is time rather than the answer.
/// Over `.index/full`, the cap varied by hand at three sizes:
///
/// | route | 64 | 512 | 4096 |
/// |---|---|---|---|
/// | Sol → Col 285 ZQ-K C9-12, 100 ly, least fuel | 0.193 fuel in **0.70 s** | 0.191 in 1.05 s | 0.191 in **21.8 s** |
/// | the same, fewest jumps, charged | 3 stops, 3.0 ms | 3 stops, 1.4 ms | 3 stops, 1.4 ms |
/// | Sol → 3 kly, 50 ly, fewest jumps, charged | 37 stops, **320 ms** | 37 stops, **143 ms** | 37 stops, 177 ms |
///
/// Thirty times the wall clock across that range for **one percent** of
/// the fuel, and not a jump anywhere else — and the 3 kly row is the
/// reason a dial would not be the free win it looks: a cap of 64 is
/// *twice as slow* there, the search having lost the neighbours it needed
/// and wandered for want of them. So the size that would be dialled does
/// not move monotonically with time either, and 512 stays borrowed until
/// something measures the curve properly.
const FANOUT: usize = 512;

/// The squared distance between two points, the distance itself wanted for
/// nothing here but comparing.
fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dx * dx + dy * dy + dz * dz
}

/// Move the `keep` cheapest-scoring neighbours to the front of `found`
///
/// Only far enough into the order to find the boundary — the rest are left
/// unsorted, which is all a cap needs: what it keeps is a set and not a
/// ranking. `keep` is asked of a slice longer than itself, the caller
/// having established there is something to drop.
fn cheapest_first(
    found: &mut [(Node, [f64; 3], f64)],
    keep: usize,
    score: impl Fn(&(Node, [f64; 3], f64)) -> (f64, f64),
) {
    found.select_nth_unstable_by(keep, |a, b| {
        let (a, b) = (score(a), score(b));
        a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
    });
}

/// Thin `found` to `cap` neighbours, keeping every one `cone` admits
///
/// **What a cap drops decides what the search can never learn.** Two rules
/// decide what is kept, and the second of them is the ask's own.
///
/// **A jet cone is never what the cap drops.** Nearness is the right
/// measure for an ordinary system, whose only use is where it stands. It is
/// the wrong one for a system that can supercharge: a cone's use is the
/// reach of the jump *out* of it, six hundred light years at a fifty light
/// year range, and a cone standing off to the side of the heading is worth
/// more than a system a little nearer the goal. Dropping one is not a few
/// percent off a route, it is a leg the search never hears about. So the
/// cones go to the front and the cap is spent on the rest. They are rare
/// enough to cost nothing — four systems in a hundred can supercharge,
/// against a [`FANOUT`] of five hundred and twelve — and where a sphere
/// holds more cones than the cap allows, the cones are what the cap is
/// spent on.
///
/// Measured over `.index/full` on the charged routes where the cap actually
/// bites, which is dense space at a boosted range:
///
/// | route | old jumps | keeping cones |
/// |---|---|---|
/// | 1 kly, 100 ly range | 9 | **7** |
/// | 300 ly, SCO at 50 ly | 5 | **4** |
/// | 1 kly, 50 ly | 19 | **18** |
///
/// For 15–35% more time, the cap's own currency: 13.5 s against 17.3 s on
/// the first of them. And the cap was dropping **half the cones it saw** —
/// 3,605 of 7,376 on one 300 ly crossing, 464 of 545 on the 1 kly one.
///
/// **And what is kept is what the route is weighed by**, which `score`
/// carries: the cheapest candidates per light year of ground closed on the
/// goal. For a route counted in jumps that is the nearest the goal, a jump
/// costing one wherever it lands — the same set the cap always kept, since
/// every candidate that closes ground is nearer than every candidate that
/// does not. For one weighed by fuel it is *not*: fuel goes as the square
/// of a jump, so the dearest candidate in the sphere is the one furthest
/// out, and "nearest the goal" kept precisely those. Reported as a
/// least-fuel route that "jumps a lot too far in the beginning", and it
/// only showed at the start because the cap bites where the sky is dense.
/// Measured, Sol to Col 285 Sector ZQ-K C9-12 at a 100 ly range, least
/// fuel over unpriced hops:
///
/// | | stops | flown | longest | fuel | found in |
/// |---|---|---|---|---|---|
/// | optimal | 35 | 235.2 ly | 16.3 ly | 0.192 | 19.3 s |
/// | 95%, nearest the goal | 15 | 217.5 ly | **50.9 ly** | **0.611** | 0.10 s |
/// | 95%, uncapped | 35 | 235.2 ly | 16.3 ly | 0.192 | 18.8 s |
/// | **95%, cheapest per light year** | 35 | 235.2 ly | 16.3 ly | **0.192** | **1.05 s** |
///
/// Three times the fuel for a setting that promised five percent, and the
/// cap was the whole of the difference — the uncapped row is the proof of
/// that, an unpriced fuel estimate being nothing to lean on, so the 95%
/// ordering and the exact one are the same one. Scored by what the metric
/// charges, the cap comes back with the *optimal* route in a twentieth of
/// the exact search's time: what it now throws away is the expensive half
/// of the sphere, which is the half the answer was never in.
///
/// Nothing changes where the cap does not bite. `found` holds the
/// neighbours *worth relaxing* rather than every system in the sphere, so
/// a route across the galaxy on the coarse plan never reaches the cap at
/// all: measured at zero bites over the whole of Sol to Colonia.
fn thinned(
    found: &mut Vec<(Node, [f64; 3], f64)>,
    cap: usize,
    score: impl Fn(&(Node, [f64; 3], f64)) -> (f64, f64),
    mut cone: impl FnMut(Node) -> bool,
) {
    if found.len() <= cap {
        return;
    }

    let mut cones = 0;
    for i in 0..found.len() {
        if cone(found[i].0) {
            found.swap(i, cones);
            cones += 1;
        }
    }

    match cones >= cap {
        true => cheapest_first(found, cap, score),
        false => cheapest_first(&mut found[cones..], cap - cones, score),
    }
    found.truncate(cap);
}

/// The search's ledger: what it has reached, a page per cell
///
/// **The structure the exact settings live or die by.** Measured over the
/// 200 M-system index, an exact charged search three thousand light years
/// long: 2,635,464 expansions, 45.4 *billion* systems measured, 2.04
/// billion of them inside the reach — and 3.9 million relaxations. Every
/// reached system was measured some seven hundred times over, and each of
/// those measurements asked a hash map whether it already held something
/// cheaper.
///
/// So what a system has been reached for is kept in an array over the cell
/// it belongs to, allocated the first time the search touches that cell.
/// Two things come of it:
///
/// - **A slot instead of a hash.** The cell's costs are one contiguous run
///   and the offset is the node's own, so the test is a load.
/// - **A cell can be skipped whole.** Once every system in a cell has been
///   reached in no more jumps than the expansion standing on it, nothing
///   in that cell can be improved by one more jump — under either
///   ordering, a jump more is dearer. The payload is then never read and
///   its systems are never measured, which is what takes the settled
///   interior of a search out of the cost. See [`Self::settled`].
///
/// Bounded by what the search touched, as the map it replaced was: a
/// corridor's cells and not the galaxy's.
struct Ledger<C> {
    /// What each system of a touched cell has been reached for, in payload
    /// order.
    cells: FxHashMap<CellId, Page<C>>,
}

/// One cell's page of it.
struct Page<C> {
    /// [`Metric::UNREACHED`] until something reaches the system.
    cost: Vec<C>,
    /// How many of them have been reached, so a cell that is not full yet
    /// is never skipped.
    recorded: u32,
    /// The most jumps any of them was reached in, which is what the
    /// saturation test compares against.
    worst: u32,
}

impl<C: Metric> Ledger<C> {
    /// A ledger that has reached nothing.
    fn new() -> Ledger<C> {
        Ledger { cells: FxHashMap::default() }
    }

    /// What `node` has been reached for, or [`Metric::UNREACHED`].
    fn cost(&self, node: Node) -> C {
        self.cells
            .get(&node.cell)
            .and_then(|held| held.cost.get(node.at as usize).copied())
            .unwrap_or(C::UNREACHED)
    }

    /// Reach `node` for `cost`, its cell holding `len` systems.
    fn record(&mut self, node: Node, len: usize, cost: C) {
        let held = self.cells.entry(node.cell).or_insert_with(|| Page {
            cost: vec![C::UNREACHED; len],
            recorded: 0,
            worst: 0,
        });
        let Some(slot) = held.cost.get_mut(node.at as usize) else { return };
        if *slot == C::UNREACHED {
            held.recorded += 1;
        }
        *slot = cost;
        held.worst = held.worst.max(cost.spent());
    }

    /// Whether nothing in `cell` can be bettered by a chain that is already
    /// `jumps` long
    ///
    /// Every system in it reached, and none of them in more jumps than the
    /// chain standing outside: a candidate there would be offered one jump
    /// more, which is dearer whether the cost counts jumps alone or jumps
    /// and then light years.
    fn settled(&self, cell: CellId, len: usize, spent: u32) -> bool {
        self.cells.get(&cell).is_some_and(|held| {
            held.recorded as usize == len.min(held.cost.len())
                && held.worst <= spent
        })
    }
    /// What the search holds about one cell, if it has touched it.
    fn page(&self, cell: CellId) -> Option<&Page<C>> {
        self.cells.get(&cell)
    }
}

impl<C: Metric> Page<C> {
    /// Whether the `at`th system of this cell can be bettered by a chain
    /// that is already `jumps` long
    ///
    /// The per-record half of [`Ledger::settled`], and the one that fires:
    /// a cell is rarely reached *whole*, and a search in a dense corridor
    /// measures the same systems hundreds of times over. Four bytes read
    /// out of the cell's own run, against a position read off a
    /// forty-one-byte record and a distance measured to it.
    fn worth_measuring(&self, at: u32, spent: u32) -> bool {
        self.cost.get(at as usize).is_none_or(|held| held.spent() > spent)
    }
}

impl JumpGraph {
    /// The graph over a galaxy and the supercharge table beside it.
    ///
    /// Nothing is built. The places are the cell payloads, read where they
    /// lie, which is why this is instant where the grid it replaced was 32
    /// seconds.
    pub fn over(sky: &Arc<Sky>, boosts: &Boosts) -> JumpGraph {
        JumpGraph {
            sky: Arc::clone(sky),
            boosts: boosts.clone(),
            highway: Arc::new(OnceLock::new()),
        }
    }

    /// The boost stars as a graph of their own, placed on the first ask.
    ///
    /// [`None`] where the index publishes no supercharge table, or where
    /// none of its rows is a system the names table places: there is then
    /// no chain of cones to plan on and a long supercharged route is
    /// searched flat, as it was.
    ///
    /// Said aloud, because it happens once a session and is worth seeing
    /// in a log beside the route that paid for it: 213 ms to sort the
    /// 3,846,802 published rows into cells, over the 200 M-system index.
    fn highway(&self) -> Option<Arc<Highway>> {
        self.highway
            .get_or_init(|| {
                let clock = std::time::Instant::now();
                let placed = Highway::over(&self.boosts);
                match &placed {
                    Some(highway) => info!(
                        "the highway: {} boost stars placed in {:.2?}",
                        highway.len(),
                        clock.elapsed(),
                    ),
                    None => info!(
                        "no highway: the index publishes no supercharge \
                         table, so a long supercharged route is searched flat"
                    ),
                }
                placed.map(Arc::new)
            })
            .clone()
    }

    /// How many systems the graph can route between.
    pub fn len(&self) -> usize {
        self.sky.len() as usize
    }

    /// Whether it holds no places at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Where a node sits. A node a mapping no longer holds reads as the
    /// origin rather than panicking a route task.
    fn place(&self, node: Node) -> [f64; 3] {
        self.sky.place(node).unwrap_or_default()
    }

    /// What a node's system is called.
    fn address(&self, node: Node) -> i64 {
        self.sky.address(node).unwrap_or_default()
    }

    /// How many systems the cell a node belongs to holds, which is how long
    /// the search's record of that cell has to be.
    fn held(&self, node: Node) -> usize {
        self.sky.payload(node.cell).map_or(0, |payload| payload.len())
    }

    /// What the system at a node can supercharge, if anything.
    fn boost(&self, node: Node) -> Option<Boost> {
        self.boosts.get(self.address(node))
    }

    /// Which node holds `address`, given where the names table says it sits.
    fn node_of(&self, address: i64, near: [f64; 3]) -> Option<Node> {
        self.sky.node_of(address, near)
    }

    /// The systems a node can jump to, with their places.
    ///
    /// `range` is what the ship reaches unaided, and the jump is scaled by
    /// whatever the system being left can supercharge into the `drive`
    /// fitted: a neutron star is four times as far, or six. Which is why a
    /// boost is read off the system a jump leaves rather than the one it
    /// lands in — the charge is taken in the jet cone and spent on the jump
    /// out.
    ///
    /// **Cells nothing can come of are never read.** `reached` says what the
    /// search holds and `jumps` how long the chain standing here is, and a
    /// cell whose every system was reached in no more jumps than that is
    /// skipped without its payload being touched: one jump more is dearer
    /// under either ordering, so there is nothing in there to improve. This
    /// is what takes the settled interior of a search out of the cost — an
    /// exact charged search measured 45.4 billion systems to relax 3.9
    /// million. See [`Ledger`].
    ///
    /// **Capped where `fanout` says so**, keeping the neighbours that get
    /// nearest `goal`. A boosted jump in the core sees thousands of systems
    /// and relaxing every one is what makes the search quadratic in
    /// density — but a cap can drop the very neighbour a fewest-jumps chain
    /// went through, so only a setting that has not promised the fewest
    /// carries one. See [`Routing::fanout`].
    ///
    /// **A jet cone is never what the cap drops.** Nearest the goal is the
    /// right order for an ordinary system, whose only use is where it
    /// stands; it is the wrong order for one that can supercharge, whose
    /// use is what the jump *out* of it reaches. Dropping a cone is not a
    /// few percent off a route, it is a leg of six hundred light years the
    /// search never learns about, and a cone stands behind the goal as
    /// readily as in front of it. So the cones a sphere holds are kept
    /// first and the ordinary systems fill what is left. They are rare
    /// enough for that to cost nothing — four systems in a hundred can
    /// supercharge, against a cap of [`FANOUT`] out of the thousand a
    /// fifty light year sphere holds — and where there are more cones than
    /// the cap allows, it is the cones that are taken nearest the goal
    /// first.
    ///
    /// Asked only of a drive that can use one: an unaided route is thinned
    /// as it always was, and pays no lookup for an answer that cannot
    /// matter to it.
    ///
    /// `out` is the caller's buffer, cleared here: there are half a million
    /// expansions in a route across the galaxy and an allocation apiece is
    /// not worth paying.
    #[allow(clippy::too_many_arguments)]
    fn neighbors<C: Metric>(
        &self,
        node: Node,
        at: [f64; 3],
        range: f64,
        drive: Drive,
        goal: [f64; 3],
        fanout: Option<usize>,
        step: &impl Fn(&JumpGraph, Node, f64) -> C,
        reached: &Ledger<C>,
        spent: u32,
        cells: &mut Vec<CellId>,
        out: &mut Vec<(Node, [f64; 3], f64)>,
    ) {
        out.clear();
        cells.clear();
        let range = range * drive.factor(self.boost(node));
        let reach = range * range;
        // The cells the sphere touches, off the resident tree by descent,
        // and then each one's systems measured out of its mapping — which
        // is [`Sky::each_near`] with the skip in the middle of it.
        self.sky.index().each_near(at, range, |cell| cells.push(cell));
        for &cell in cells.iter() {
            let Some(payload) = self.sky.payload(cell) else { continue };
            if reached.settled(cell, payload.len(), spent) {
                continue;
            }
            let page = reached.page(cell);
            for i in 0..payload.len() {
                // Reached already, and in no more jumps than the chain
                // standing here: one jump more is dearer under either
                // ordering, so there is nothing to measure. Four bytes out
                // of the cell's own run against a position off a
                // forty-one-byte record, and it is the test that empties
                // the settled middle of a search.
                if page
                    .is_some_and(|page| !page.worth_measuring(i as u32, spent))
                {
                    continue;
                }
                let place = payload.position_at(i);
                let away = dist2(at, place);
                if away > reach {
                    continue;
                }
                let found = Node { cell, at: i as u32 };
                if found != node {
                    out.push((found, place, away.sqrt()));
                }
            }
        }
        if let Some(cap) = fanout {
            // What the cap keeps is what this route is weighed by: the
            // candidates that cost least per light year of ground closed on
            // the goal, which for a jump-counted route is the nearest the
            // goal and for a fuel-weighed one is emphatically not. The
            // price comes off `step`, so there is one rule here and the
            // orderings are the metrics' own. See [`thinned`].
            let here = dist2(at, goal).sqrt();
            let score = |cand: &(Node, [f64; 3], f64)| {
                let there = dist2(cand.1, goal).sqrt();
                let closed = here - there;
                let spent = step(self, node, cand.2).spent() as f64;
                let per = match closed > 0. {
                    true => spent / closed,
                    // Ground it does not close is ground no amount of
                    // cheapness buys, and these then fall in behind every
                    // candidate that closes any — in their own order of
                    // nearness, which is where the cap always left them.
                    false => f64::INFINITY,
                };
                (per, there)
            };
            // A cone is worth keeping for the reach of the jump *out* of
            // it, which is a thing no score of where it lands can see — and
            // worth nothing at all to a drive that cannot charge off one,
            // which then pays no lookup for the answer.
            match drive.widest() > 1. {
                true => {
                    thinned(out, cap, score, |node| self.boost(node).is_some())
                }
                false => thinned(out, cap, score, |_| false),
            }
        }
    }

    /// A route between two systems by address, at a ship's jump `range` and
    /// with `drive` fitted, as the hops it passes through. [`None`] where
    /// either end is unknown or no chain of jumps that long connects them.
    ///
    /// All three cost one per jump and estimate what is left as the
    /// straight-line distance in whole jumps. [`Routing::FEWEST`] and
    /// [`Routing::SHORTEST`] leave that estimate alone, so it can never
    /// overstate the jumps remaining and both come back with a genuine
    /// fewest-jumps route; what they choose between is the enormous number of
    /// chains that are all exactly that long. [`Routing::QUICK`] multiplies it
    /// instead, and comes back with a route inside a twentieth of the fewest
    /// in a hundredth of the time.
    ///
    /// The estimate divides by the widest jump the drive could make and not by
    /// the range asked for, or a boost would let the route beat the estimate
    /// and the search would stop settling for the fewest jumps. See
    /// [`Drive::widest`].
    /// `watching` is filled in as the search runs, for the map to draw what it
    /// has reached; see [`super::frontier`]. [`None`] where nothing is
    /// watching, and the search then records nothing at all.
    pub(crate) fn route(
        &self,
        start: (i64, [f64; 3]),
        end: (i64, [f64; 3]),
        range: f64,
        how: Routing,
        drive: Drive,
        tune: Tuning,
        watching: Option<&Arc<Frontier>>,
    ) -> Option<Vec<(i64, [f64; 3])>> {
        // The ends are the one thing a route knows by *name*: a commander
        // picked them, so the names table says where they are and this finds
        // the records. Twice a leg, against the millions of places the
        // search itself reads straight out of the payloads.
        let from = self.node_of(start.0, start.1)?;
        let to = self.node_of(end.0, end.1)?;
        let goal = self.place(to);
        let mut sampled = watching.map(Frontier::sampler);
        // A long supercharged route is planned on the chain of jet cones
        // first, where the setting allows an approximation: the flat search
        // is minutes across the galaxy and this is seconds. It answers
        // [`None`] where it does not apply or cannot finish, and the flat
        // search then runs exactly as it did. See [`Self::charged`].
        let planned = how
            .highway()
            .then(|| {
                self.charged(
                    from,
                    to,
                    goal,
                    range,
                    drive,
                    how,
                    tune,
                    &mut sampled,
                )
            })
            .flatten();
        let path = match planned {
            Some(path) => Some(path),
            None => {
                self.walked(from, to, goal, range, drive, how, &mut sampled)
            }
        };
        if let Some(sampled) = &mut sampled {
            sampled.done();
        }
        Some(
            path?
                .into_iter()
                .map(|node| (self.address(node), self.place(node)))
                .collect(),
        )
    }

    /// A long supercharged route, planned on the boost stars and flown leg
    /// by leg
    ///
    /// Two levels. The chain of jet cones worth taking comes off
    /// [`super::highway`], which is a graph of the two systems in a
    /// hundred that can supercharge a drive and nothing else — 3.8 M nodes
    /// of 200 M — and then each hop of that chain is flown by the same
    /// fewest-jumps search the flat router uses, over a few hundred light
    /// years rather than twenty-two thousand.
    ///
    /// Measured over the real index, Sol to Colonia at 50 ly with a
    /// standard drive: the plan and its legs come to 140 jumps in 2.0 s,
    /// where the flat charged search over the same two ends is 137 jumps
    /// in **610 s**. At 80 ly it is 81 jumps in 0.8 s. Most of what is
    /// left is the plan; see [`super::highway::Highway::plan`] for what
    /// its leaning costs and what it saves.
    ///
    /// Four things make it not apply, and each answers [`None`] so the
    /// caller searches flat:
    ///
    /// - **No drive to charge.** The highway is jet cones; unaided there is
    ///   nothing on it to use.
    /// - **No published supercharge table**, which is not the same as a
    ///   galaxy with no cones in it. See [`Boosts::published`].
    /// - **A short route.** Under [`LONG_ROUTE_LY`] the flat search is
    ///   already quick and it is exact; a coarse plan could only make the
    ///   answer worse.
    /// - **No chain.** The highway does not reach everywhere — 2.4 M of
    ///   its 3.8 M stars are reachable from the bubble — and a leg the
    ///   plan promised may have no chain of systems to fly it. Either way
    ///   the flat search is what stands behind this.
    ///
    /// The legs are searched with the *exact* estimate rather than with
    /// the leaning this setting carries: what is approximate is the chain,
    /// and a leg of a few hundred light years is small enough to settle
    /// honestly. The per-expansion fanout cap the setting carries does
    /// still apply, this being that setting's search.
    #[allow(clippy::too_many_arguments)]
    fn charged(
        &self,
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        tune: Tuning,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        if drive == Drive::Unaided || !self.boosts.published() {
            return None;
        }
        let at = self.place(start);
        if dist2(at, goal).sqrt() <= LONG_ROUTE_LY {
            return None;
        }
        let highway = self.highway()?;
        let chain = highway.plan(
            (at, self.boost(start)),
            goal,
            range,
            drive,
            how,
            tune,
            sampled.as_mut(),
        )?;

        // The waypoints as records. A cone the plan named that the galaxy
        // no longer places is dropped rather than failing the plan: the
        // hop either side of it is a longer leg, which the leg search is
        // free to fly its own way.
        let mut hops = Vec::with_capacity(chain.len() + 2);
        hops.push(start);
        for (address, place) in chain {
            if let Some(node) = self.node_of(address, place)
                && node != *hops.last().expect("the start")
            {
                hops.push(node);
            }
        }
        hops.push(end);
        // The plan, drawn whole from here: the legs refine it one at a time
        // and each is its own short search, so without this the picture
        // would fall back to whichever leg is under way.
        if let Some(sampled) = sampled.as_mut() {
            sampled.planned(self.places(&hops));
        }
        self.flown(&hops, range, drive, how, tune, sampled)
    }

    /// Where a run of nodes sits, for the map to draw it.
    fn places(&self, nodes: &[Node]) -> Vec<DVec3> {
        nodes.iter().map(|node| DVec3::from(self.place(*node))).collect()
    }

    /// Fly a chain of waypoints: one search a leg, stitched.
    ///
    /// **The legs are independent searches and are run side by side.** A
    /// leg is `hops[i]` to `hops[i + 1]` and it ends *at* `hops[i + 1]`, so
    /// the leg after it starts where the plan said and not where the last
    /// one happened to land — which is what makes them independent, and
    /// holds through a merge too: a merged leg lands on `hops[i + 2]`,
    /// which is where the precomputed leg from there begins. Measured over
    /// `.index/full`, Sol → Colonia with every gap searched
    /// ([`Crossing::Searched`]): the legs are **5.93 s of the 6.32 s**
    /// crossing at 50 ly and 8.10 s of 9.20 s at 25 ly, the coarse plan
    /// being the rest. Under the default [`Crossing::Stepped`] a leg is
    /// arithmetic rather than a search and the same phase is 8.9 ms, so
    /// what this is for is the setting that asks for the gaps to be proven.
    ///
    /// A leg with no chain of systems to fly it is merged into the next
    /// one and the pair tried again, which is what EDDA's refinement does
    /// with the same failure (`long_range.rs:3421-3441`): a coarse hop is
    /// a promise about a gap, and where the promise is wrong the way
    /// across is usually to skip the cone it was made about. A second
    /// failure abandons the plan. That retry is searched here, in the
    /// stitch, because a merge is the one leg whose ends the plan did not
    /// name.
    fn flown(
        &self,
        hops: &[Node],
        range: f64,
        drive: Drive,
        how: Routing,
        tune: Tuning,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        let refined = self.refined(hops, range, drive, how, tune, sampled);

        let mut path = vec![*hops.first()?];
        let mut leg = 0;
        while leg + 1 < hops.len() {
            let from = *path.last().expect("the leg it flew to");
            let (flown, next) = match refined
                .get(leg)
                .and_then(|held| held.clone())
                .filter(|_| from == hops[leg])
            {
                Some(flown) => (flown, leg + 1),
                // The leg the plan promised has no chain of systems to fly
                // it. Merged with the next and searched here: its ends are
                // the only pair nothing precomputed.
                None => (
                    self.leg(
                        from,
                        *hops.get(leg + 2)?,
                        range,
                        drive,
                        how,
                        tune,
                        sampled,
                    )?,
                    leg + 2,
                ),
            };
            // The way this leg went, drawn as its own branch off the plan:
            // a coarse hop is a promise about a gap, and this is how the
            // ship can really fly it. Drawn in the order flown, whatever
            // order the legs were searched in.
            if let Some(sampled) = sampled.as_mut() {
                sampled.flew(self.places(&flown));
            }
            // The leg's own start is the hop already stood on.
            path.extend(flown.into_iter().skip(1));
            leg = next;
        }
        Some(path)
    }

    /// Every leg of `hops` flown, searched side by side
    ///
    /// One entry per leg, in the plan's order; [`None`] where that leg has
    /// no chain of systems to fly it, which the stitch answers by merging
    /// it into the next. Each worker carries a quiet sampler
    /// ([`Sampler::beside`]) so the expansion readout keeps counting and a
    /// route taken back stops every leg at once, and the index is taken off
    /// one atomic rather than handed out in blocks: a plan's legs are
    /// nothing alike in cost — one measured 17.93 s of an 18.97 s route —
    /// so a static split would leave most threads idle behind the worst leg.
    #[allow(clippy::too_many_arguments)]
    fn refined(
        &self,
        hops: &[Node],
        range: f64,
        drive: Drive,
        how: Routing,
        tune: Tuning,
        sampled: &Option<Sampler>,
    ) -> Vec<Option<Vec<Node>>> {
        let legs = hops.len().saturating_sub(1);
        let mut refined: Vec<Option<Vec<Node>>> = vec![None; legs];
        // One leg is the search itself, and a thread to hand it to costs
        // more than the hand-off saves.
        if legs <= 1 {
            if legs == 1 {
                refined[0] =
                    self.leg(hops[0], hops[1], range, drive, how, tune, &mut {
                        sampled.as_ref().map(Sampler::beside)
                    });
            }
            return refined;
        }

        let next = AtomicUsize::new(0);
        let done: Mutex<Vec<(usize, Vec<Node>)>> = Mutex::new(Vec::new());
        let hands = std::thread::available_parallelism()
            .map_or(4, std::num::NonZeroUsize::get)
            .min(legs);
        std::thread::scope(|scope| {
            for _ in 0..hands {
                let (next, done) = (&next, &done);
                let mut beside = sampled.as_ref().map(Sampler::beside);
                scope.spawn(move || {
                    loop {
                        let leg = next.fetch_add(1, Ordering::Relaxed);
                        if leg >= legs {
                            break;
                        }
                        // A route taken back mid-refinement: the legs left
                        // are not started, and the one running reads the
                        // same flag itself.
                        if beside.as_ref().is_some_and(Sampler::stopped) {
                            break;
                        }
                        if let Some(flown) = self.leg(
                            hops[leg],
                            hops[leg + 1],
                            range,
                            drive,
                            how,
                            tune,
                            &mut beside,
                        ) {
                            done.lock().expect("the legs").push((leg, flown));
                        }
                    }
                    // What this hand counted, handed over once rather than
                    // per expansion.
                    if let Some(beside) = beside.as_mut() {
                        beside.flush();
                    }
                });
            }
        });

        for (leg, flown) in done.into_inner().expect("the legs") {
            refined[leg] = Some(flown);
        }
        refined
    }

    /// One leg of a plan: the fewest jumps between two of its waypoints.
    ///
    /// A leg the ship can fly in one jump is answered rather than
    /// searched: one jump between two systems is trivially the fewest, so
    /// nothing is given up, and most of a plan's legs are exactly that —
    /// the chain is cones one supercharged hop apart.
    ///
    /// **Nothing is given up on fuel either, and the cone is why.**
    /// [`burned`] caps a jump at one tankful however far the jet throws
    /// the ship, so a leg answered in one *boosted* jump — which is every
    /// leg but the first, a plan's hops standing on cones — beats any
    /// chain of ordinary jumps across the same gap: four jumps at a 45 Ly
    /// range to cross 180 Ly is four tankfuls against the one the cone
    /// costs. Only the first leg can be answered by an unboosted jump, and
    /// what taking it whole overpays is bounded by that single jump.
    #[allow(clippy::too_many_arguments)]
    fn leg(
        &self,
        from: Node,
        to: Node,
        range: f64,
        drive: Drive,
        how: Routing,
        tune: Tuning,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        let goal = self.place(to);
        let reach = range * drive.factor(self.boost(from));
        if dist2(self.place(from), goal).sqrt() <= reach {
            return Some(vec![from, to]);
        }
        // Proven or leaned, as the settings say — which is the same walk
        // with the weight taken off. Proven is the odd one: the plan this
        // refines is an approximation, so an exact leg is the one exact
        // thing in an inexact answer, and at 50 ly the eight legs that need
        // searching are 2.0 s of the 2.5 s.
        // Stepping first where it is asked for, and the search where the
        // walk cannot close: a gap handed to the search is a gap the walk
        // spent microseconds failing at. Either way the gap is crossed at
        // the route's own optimality and never better — the plan it belongs
        // to is a guess, so an exact gap inside one buys nothing anybody
        // asked for. See [`Crossing::Searched`].
        if tune.crossing == Crossing::Stepped
            && let Some(stepped) = self.stepped(from, to, range, drive, how)
        {
            return Some(stepped);
        }
        self.walked(from, to, goal, range, drive, how, sampled)
    }

    /// How many steps a walk across one gap may take, where the route is
    /// counted in jumps
    ///
    /// A gap is the reach of one supercharged jump and a few ordinary ones
    /// by construction ([`Tuning::reach`]), so a walk that has taken this
    /// many is not crossing a gap — it is wandering, and the search can
    /// have it.
    const STEPS: usize = 32;

    /// And how many where it is weighed by fuel
    ///
    /// **A backstop and not a bound anybody sets.** A fuel-weighed walk
    /// takes the cheapest step per light year of ground closed, which at
    /// an unpriced hop is about the shortest jump the sky offers — that
    /// being the ask ([`floor`]) — so the count is whatever the star field
    /// gives and thirty-two is a walk cut off for obeying its own price.
    ///
    /// Measured over `.index/full` at 45 ly with a standard drive, the
    /// longest walk any gap of a planned crossing took:
    ///
    /// | hop | Sol → Colonia | Sol → 16 kly out |
    /// |---|---|---|
    /// | 50% | 11 steps | 12 steps |
    /// | 25% | 19 steps | 19 steps |
    /// | unpriced | 44 steps | **61 steps** |
    ///
    /// Which is the price's own arithmetic showing: a step costs least at
    /// a jump of `hop` percent of the range, so a gap takes about the
    /// reciprocal of that many, and unpriced it is the star field that
    /// decides. Eight times the worst of them, the thing it bounds being a
    /// pathological chain of micro-jumps at a sphere scan apiece — and
    /// the walk hands the gap to the search where it runs out, exactly as
    /// the jump-counted one does.
    const HOPS: usize = 512;

    /// Walk across a gap, jumping to whatever step the route's own
    /// weighing prices cheapest
    ///
    /// `to` is the boost star on the other side of the gap — the next one
    /// the plan named — and each step is one scan of what a jump from here
    /// reaches. It answers [`None`] rather than wandering: a step that
    /// lands no closer than the one before is a dead end, and the caller
    /// hands the gap to the search. See [`Crossing::Stepped`].
    ///
    /// **What a step is scored by is what the route is weighed by**, which
    /// is the same rule [`thinned`] follows one level down: the cheapest
    /// candidate per light year of ground closed on the far cone, priced
    /// by the metric itself.
    ///
    /// - Counted in jumps, that is the candidate *nearest* the far cone. A
    ///   jump costs one wherever it lands, so the most ground closed is
    ///   the cheapest per light year of it, and every candidate that
    ///   closes ground is nearer than every candidate that does not. The
    ///   walk this always did, at the squared distance it always compared.
    /// - Weighed by fuel, it is emphatically not. Fuel goes as the square
    ///   of a jump ([`burned`]) so the dearest candidate in the sphere is
    ///   the furthest out, and "nearest the far cone" is precisely the set
    ///   of those: a supercharged least-fuel route's gaps were crossed
    ///   with no regard for fuel at all, which is the same defect the
    ///   coarse plan had until it read [`Weigh`]. The price the route was
    ///   asked for rides along, [`floor`] being a toll on every jump, so
    ///   the rail means one thing here and in the flat search: a walk at a
    ///   priced hop takes jumps of about that fraction of the range, and
    ///   an unpriced one takes the shortest the sky offers.
    ///
    /// A boosted jump is still what it is: [`burned`] caps a jump at one
    /// tankful however far the cone throws the ship, so 180 light years
    /// out of a cone is 556 units a light year against a 1 ly jump's 49 at
    /// an unpriced hop and 694 against 2,222 at a 50% one. The walk keeps
    /// reaching for cones where the price makes them cheap and splits its
    /// jumps where it does not.
    ///
    /// **What that is worth, measured over `.index/full`** at 45 ly with a
    /// standard drive, the whole planned crossing's own fuel — the hop
    /// rail moved from one end to the other, scored the old way and the
    /// new:
    ///
    /// | crossing | hop | nearest | on fuel |
    /// |---|---|---|---|
    /// | Sol → Colonia | 50% | 166 stops, 157.41 tanks | 185 stops, **148.28** |
    /// | | 25% | 166, 157.41 | 218, **143.76** |
    /// | | unpriced | 166, 157.41 | 336, **142.21** |
    /// | Sol → 16 kly out | 50% | 137 stops, 127.65 tanks | 155 stops, **119.11** |
    /// | | 25% | 137, 127.65 | 187, **114.73** |
    /// | | unpriced | 137, 127.65 | 298, **113.54** |
    ///
    /// **The nearest column is one number three times over, which is the
    /// defect.** Every position of the rail came back with the same route
    /// and the same fuel: on a planned crossing every gap was walked and
    /// the rail was inert. Scored on fuel it moves — 6 to 11 percent of
    /// the tank for 11 to 118 percent more stops, which is the trade the
    /// rail is for — and what it costs in time is a few percent: 76–113 ms
    /// against 99, and 325–335 ms against 304, a longer chain of shorter
    /// jumps being what there is more of to build.
    ///
    /// A third of that saving is the arrival: 146.77 tanks against 142.21
    /// on the unpriced Colonia row is the difference between taking the
    /// far cone from the edge of the reach and closing in on it first.
    ///
    /// Six to eleven percent and not more, because most of what such a
    /// crossing burns is the cones themselves: a hundred and thirty
    /// boosted jumps at a tankful apiece is the floor under any chain of
    /// them, and the gaps are the only place fuel is there to be saved.
    fn stepped(
        &self,
        from: Node,
        to: Node,
        range: f64,
        drive: Drive,
        how: Routing,
    ) -> Option<Vec<Node>> {
        let goal = self.place(to);
        let mut path = vec![from];
        let mut at = from;
        let mut left = dist2(self.place(from), goal).sqrt();
        let mut cells = Vec::new();
        // What every jump is charged before it burns anything, or [`None`]
        // for a route counted in jumps — which is the whole of the
        // difference between the two scores, read once rather than matched
        // per candidate.
        let toll = match how.weigh {
            Weigh::Jumps | Weigh::Shortest => None,
            Weigh::Fuel { hop, .. } => Some(floor(hop)),
        };
        let steps = match toll {
            None => Self::STEPS,
            Some(_) => Self::HOPS,
        };

        while path.len() <= steps {
            let here = self.place(at);
            let reach = range * drive.factor(self.boost(at));
            // Arrived: the boost star on the other side is one jump off,
            // which is the hop the plan promised in the first place — and
            // for a route counted in jumps nothing in the sphere can
            // better it, a jump costing one wherever it lands.
            //
            // **Weighed by fuel it can be bettered, and by a whole
            // tank.** Arriving from the edge of the reach is a jump at
            // full range, which is the most expensive jump there is, where
            // closing in first and arriving from a quarter of it costs a
            // sixteenth. So the arrival is priced like any other step
            // below: the far cone is a candidate in the sphere the moment
            // it is in reach, and landing on it is what ends the walk.
            if toll.is_none() && dist2(here, goal).sqrt() <= reach {
                path.push(to);
                return Some(path);
            }

            // The best step out of here, measured to where the gap ends.
            // One pass over the sphere with no ledger and no heap: this is
            // the whole of what makes it cheap where a search is not. The
            // score and the distance both, the second being what the
            // progress rule is measured in and the first being what the
            // ask prices.
            let mut best: Option<(f64, f64, Node)> = None;
            cells.clear();
            self.sky.index().each_near(here, reach, |cell| cells.push(cell));
            for &cell in cells.iter() {
                let Some(payload) = self.sky.payload(cell) else { continue };
                for i in 0..payload.len() {
                    let place = payload.position_at(i);
                    let jump = dist2(here, place);
                    if jump > reach * reach {
                        continue;
                    }
                    let away = dist2(place, goal);
                    let score = match toll {
                        // The root is not taken: what is compared is an
                        // order, and the square root is monotone in it.
                        None => away,
                        Some(toll) => {
                            let closed = left - away.sqrt();
                            // A candidate that closes nothing is priced at
                            // no ground closed, which is not a step at
                            // all. The progress rule below throws such a
                            // walk away; here it is simply not a
                            // candidate.
                            if closed <= 0. {
                                continue;
                            }
                            (toll + burned(jump.sqrt(), range)) as f64 / closed
                        }
                    };
                    if best.is_none_or(|(held, ..)| score < held) {
                        best = Some((score, away, Node { cell, at: i as u32 }));
                    }
                }
            }

            // Every step lands closer than the last, which is what
            // terminates this and what stops it stepping back where it
            // came from.
            let (_, away, next) = best?;
            let away = away.sqrt();
            if away >= left {
                return None;
            }
            left = away;
            at = next;
            path.push(at);
            // The far cone was the cheapest step out of there, which is
            // the fuel-weighed walk's way of arriving: it closes in on the
            // cone and takes it when taking it is cheapest per light year
            // left. A jump-counted walk never gets here, having arrived
            // above the moment the cone was in reach.
            if next == to {
                return Some(path);
            }
        }
        None
    }

    /// Whichever search the ask calls for
    ///
    /// The one place the three are chosen between, so a leg of a coarse plan
    /// is walked the same way the whole route would have been.
    #[allow(clippy::too_many_arguments)]
    fn walked(
        &self,
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        match how.weigh {
            Weigh::Jumps => {
                self.flat(start, end, goal, range, drive, how, sampled)
            }
            Weigh::Shortest => {
                self.shortest(start, end, goal, range, drive, how, sampled)
            }
            Weigh::Fuel { hop, .. } => self.economical(
                start,
                end,
                goal,
                range,
                drive,
                how,
                floor(hop),
                sampled,
            ),
        }
    }

    /// The flat search, weighted by what was asked for
    ///
    /// One walk for every `over`: the same jump costs, the same tie-break,
    /// and at nothing over an estimate that never overstates — which is the
    /// proof. The two used to be separate functions called `direct` and
    /// `quick`, differing in one multiply.
    ///
    /// Distance is not in the cost at all, so nothing here decides between
    /// two chains of the same length — the tie-break does. Nearest the goal
    /// first is what makes a route look like one, and it is a preference
    /// rather than a bound: no claim is made that the chain it finds is the
    /// shortest of them, only that each step reaches as far toward the goal
    /// as the ones beside it. [`Weigh::Shortest`] is what settles that.
    fn flat(
        &self,
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        let widest = range * drive.widest();
        self.search(
            start,
            end,
            goal,
            range,
            drive,
            how,
            // A jump costs [`WHOLE`], so leaning on the estimate is an exact
            // multiple of this rather than a float rounded twice. The
            // leaning is [`Self::walk`]'s, which wants the estimate
            // admissible: it is what the ceiling is measured against.
            |_, _, _| WHOLE,
            // Saturating, because the range is whatever was typed into the
            // form and a small enough one puts more jumps between two systems
            // than a `u32` holds: the cast pins at the top and the scaling
            // would then overflow. A saturated estimate is still an
            // overstatement of a distance nothing can cross, which is what
            // the search does with it.
            |at| {
                let left = dist2(at, goal).sqrt();
                let jumps = (left / widest).ceil() as u32;
                jumps.saturating_mul(WHOLE)
            },
            sampled,
        )
    }

    /// Fewest jumps, and provably the shortest of them
    ///
    /// Distance goes into the cost under the jump count, so the search settles
    /// the tie itself instead of leaving it to the tie-break. See [`Cost`] for
    /// the ordering and for why the rounding leans as it does.
    fn shortest(
        &self,
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        let widest = range * drive.widest();
        self.search(
            start,
            end,
            goal,
            range,
            drive,
            how,
            // The leg's length comes off the neighbour query, which measured
            // it to decide the system was in range at all.
            |_, _, leg| Cost { jumps: WHOLE, light_years: leg.ceil() as u32 },
            |at| {
                let left = dist2(at, goal).sqrt();
                let jumps = (left / widest).ceil() as u32;
                Cost {
                    jumps: jumps.saturating_mul(WHOLE),
                    light_years: left.floor() as u32,
                }
            },
            sampled,
        )
    }

    /// The least fuel, ties broken by the fewest jumps
    ///
    /// **The one search here whose answer is not measured in jumps.** What a
    /// step costs is what the jump burns ([`burned`]) plus [`FLOOR`], and
    /// the two together are the whole of what makes an economical route a
    /// route: fuel alone is minimised by never going anywhere much, and
    /// pricing the jump itself is what keeps the answer to jumps in the
    /// upper half of the ship's range.
    ///
    /// The estimate is **the cheapest a light year can possibly be**, times
    /// the light years left. Which needs working out rather than assuming,
    /// and the first cut assumed: a floor per jump and `left / widest`
    /// jumps to go. Admissible, four times too slack, and it cost two
    /// orders of magnitude — measured over `.index/full`, the same two
    /// routes before and after tightening it:
    ///
    /// | route | slack estimate | this one |
    /// |---|---|---|
    /// | 300 ly | 4.39 s | **25 ms** |
    /// | 1 kly | 102.97 s | **1.34 s** |
    ///
    /// Same answers, 77 times over. An estimate is not free to be lazy
    /// because it is honest.
    ///
    /// Crossing `L` light years in `k` equal jumps is priced at
    /// `k x FLOOR + TANKFUL x L^2 / (k x range^2)`, least at
    /// `k = (L / range) x sqrt(TANKFUL / FLOOR)`, where it comes to
    ///
    /// ```text
    /// 2 x (L / range) x sqrt(FLOOR x TANKFUL)
    /// ```
    ///
    /// and unequal jumps only cost more, the price being convex in the
    /// length. So that is a true lower bound and it is four times the first
    /// cut. **Unless the drive can supercharge**, and then it is not: a jet
    /// cone throws the ship four times its range for one jump's fuel, so a
    /// light year can cost as little as `(FLOOR + TANKFUL) / widest`, and
    /// an estimate that ignored cones would overstate what is left and send
    /// A\* home with the wrong answer. The cheaper of the two it is.
    ///
    /// The fuel left to burn has no *positive* bound below, a jump being
    /// able to be arbitrarily short — which is the whole reason [`FLOOR`]
    /// is load-bearing twice. Without it there is nothing to bound, A\* has
    /// no heading, and an economical route becomes Dijkstra over the
    /// galaxy: every direction at once, for as long as it takes.
    ///
    /// What it comes to, measured over `.index/full` from Sol at a fifty
    /// light year range, unaided, against the same route weighed by jumps:
    ///
    /// | route | weighed by | jumps | fuel | found in |
    /// |---|---|---|---|---|
    /// | 300 ly | jumps | 6 | 5.47 | 2.2 ms |
    /// | 300 ly | **fuel** | 11 | **2.99** | 25 ms |
    /// | 300 ly, 95% | fuel | 10 | 3.26 | 1.1 ms |
    /// | 1 kly | jumps | 21 | 19.47 | 64 ms |
    /// | 1 kly | **fuel** | 39 | **10.16** | 1.34 s |
    /// | 1 kly, 95% | fuel | 36 | 11.00 | 4.6 ms |
    ///
    /// The fuel column is jumps' worth at the square, which is the ordering
    /// this minimises. **Half the fuel for one jump under twice as many**,
    /// which is what the default hop of half the range asks for, and the
    /// trade landing where [`floor`]'s arithmetic says it should. Proving
    /// it costs ten to twenty times what the jump count costs, the estimate
    /// being the weaker of the two; at 95% it is *cheaper* than the jump
    /// count at 95%, and gives up a twentieth of the saving.
    ///
    /// `floor` is what a jump is priced at before it burns anything, which
    /// the ask carries and the reader sets: it decides how far the route is
    /// willing to split, and unpriced this walk is Dijkstra's.
    ///
    /// **It used to change nothing on a planned supercharged route**, and
    /// that was the defect rather than the design: a plan's gaps are
    /// walked rather than searched, and the walk scored its steps by
    /// nearness alone, so Sol to Colonia came back as the same route and
    /// the same 157.41 tanks wherever the hop rail stood. The walk is
    /// scored by the metric now ([`JumpGraph::stepped`]) and the rail
    /// moves a planned route by 6 to 11 percent of the tank. What this
    /// search still owns is every gap the walk cannot close
    /// ([`Crossing::Searched`]) and the whole of a flat route, which is
    /// what an unaided one is at any distance.
    #[allow(clippy::too_many_arguments)]
    fn economical(
        &self,
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        floor: u32,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        let widest = range * drive.widest();
        // The cheapest a light year can be bought for, in the units
        // [`TANKFUL`] counts: by splitting jumps to the price's own optimum,
        // or by taking a jet cone as far as it throws, whichever is less.
        let cheapest = {
            let split = 2. * ((floor as f64) * (TANKFUL as f64)).sqrt() / range;
            let boosted = (floor + TANKFUL) as f64 / widest;
            split.min(boosted)
        };
        self.search(
            start,
            end,
            goal,
            range,
            drive,
            how,
            // The leg's length comes off the neighbour query, which measured
            // it to decide the system was in range at all. A jump costs one
            // jump and what it burns, and the burn is measured against what
            // the ship does *unaided*: a jet cone multiplies the reach and
            // not the tank, so a boosted jump of four hundred light years
            // burns what a jump at the range burns and no more.
            |_, _, leg| Burn { fuel: floor + burned(leg, range), jumps: WHOLE },
            |at| {
                let left = dist2(at, goal).sqrt();
                Burn {
                    // Floored, so the rounding cannot turn a bound into an
                    // overstatement. Admissible, the leaning being
                    // [`Self::walk`]'s: the optimality setting then means
                    // the same thing to fuel as it does to jumps, and the
                    // ceiling has an honest estimate to measure against.
                    fuel: (left * cheapest).floor() as u32,
                    jumps: ((left / widest).ceil() as u32)
                        .saturating_mul(WHOLE),
                }
            },
            sampled,
        )
    }

    /// A* over the jump graph, keeping where each system was reached from
    ///
    /// Its own loop rather than the crate's, for the parent map. A* keeps one
    /// as a matter of course — it is what the answer is reconstructed from —
    /// and a library that hands back only the path leaves the map to guess at
    /// it: an earlier cut recorded the most promising few steps of every
    /// expansion instead, and a supercharged route across the galaxy came back
    /// with a chain two links long, a jump out of a neutron star landing
    /// somewhere no expansion had thought its own best step. Here the chain to
    /// the closest system reached is exact and costs nothing, being the same
    /// map the route itself comes out of.
    ///
    /// Ties are broken by distance to the goal. Where the cost carries
    /// distance ([`Cost`]) it settles its own ties and this does nothing; where
    /// it counts jumps alone, every chain of the same length costs the same
    /// and this is what picks the one that heads at the goal instead of the
    /// one that happened to be reached first.
    ///
    /// **What a system is remembered by is an array over its own cell**,
    /// allocated the first time the search touches that cell — not an array
    /// over the galaxy, which is 1.6 GB a leg at 200 M, and no longer a hash
    /// map over what was reached, which cost a hash per candidate and could
    /// not answer the question that matters: whether a whole cell is settled
    /// and can be skipped unread. See [`Ledger`].
    /// **The optimality setting is the greediness, and there is nothing to
    /// prove after the fact.** Weighted A\* multiplies the estimate by
    /// `1 + over/100`, which drives the search harder at the goal, and the
    /// theorem says the *first* route it arrives at already costs no more
    /// than that multiple of the fewest. So the walk returns on arrival and
    /// the promise is kept by the arithmetic rather than by a search for
    /// something better.
    ///
    /// A cut of this tried three passes — a greedy chain for an upper
    /// bound, a hurried walk pruned by it, then the asked-for walk pruned
    /// at `U x WHOLE / (WHOLE + over)`. Every part of it was sound and the
    /// whole of it was pointless: a route under 95% does not need proving,
    /// and what the third pass measured was **52 ms spent establishing that
    /// no twenty-jump route exists** after the answer — twenty-one jumps,
    /// inside the five percent asked for — was already in hand at 2 ms.
    /// That was the reported "sticks for a moment on the last jump": the
    /// route was found and the map sat there while a proof nobody wanted
    /// finished behind it.
    ///
    /// What the same measurement did say is worth keeping: leaning at
    /// [`HURRIED`] found **the same twenty-one jumps twenty times faster**
    /// than leaning at five percent. The slider's useful range is not near
    /// the top — a weight of 1.05 is A\* with a nudge — and that is a thing
    /// to say in the form rather than to work around in here.
    #[allow(clippy::too_many_arguments)]
    fn search<C: Metric>(
        &self,
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        step: impl Fn(&JumpGraph, Node, f64) -> C,
        estimate: impl Fn([f64; 3]) -> C,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        self.walk(
            start, end, goal, range, drive, how, &step, &estimate, sampled,
        )
    }

    /// The A\* walk itself, returning on arrival
    #[allow(clippy::too_many_arguments)]
    fn walk<C: Metric>(
        &self,
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        step: &impl Fn(&JumpGraph, Node, f64) -> C,
        estimate: &impl Fn([f64; 3]) -> C,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        if start == end {
            return Some(vec![start]);
        }
        // What each system has been reached for, and where it was reached
        // from. A cheaper way to a system that turns up later replaces the
        // entry it had, and the entry it replaced is recognised on the way
        // out of the heap by the cost it was pushed with.
        //
        // **Whether it is then expanded again is the setting's business.**
        // A proven route has to reopen and it never has to in practice: an
        // exact estimate never reaches a system for less after settling it,
        // so the test costs a comparison and nothing else. A leaned one
        // does reopen, the weighting being inconsistent by up to a jump —
        // and that reopening is a search going back over ground near the
        // start long after the route past it stopped changing, which is
        // what it looks like as well as what it is. Reported that way.
        //
        // So a route that has not promised the fewest expands each system
        // once. The bound is unharmed: weighted A* without re-expansion
        // answers inside the same `1 + over/100` given a heuristic
        // consistent before the weighting, which this one is — a jump
        // closes at most the widest jump's worth of what is left, so
        // `h(n) <= c(n,n') + h(n')` for every jump. It is `ARA*`'s own
        // argument. What stood here before claimed the opposite, that the
        // bound needed reopening; measured over `.index/full`, Sol to a
        // system three thousand light years out, shortest of the fewest
        // jumps at 95%:
        //
        // |  | pops | of those re-expansions | found in |
        // |---|---|---|---|
        // | reopening | 34,138 | **25,258** | 2.04 s |
        // | expanded once | 20,402 | 0 | **0.57 s** |
        //
        // The same 45 stops, and 2,132.6 light years against 2,135.0 — a
        // shorter route, not a worse one, for a quarter of the wait. Three
        // quarters of that search was re-expansion. Nothing moves on the
        // asks weighed by jumps or by fuel: measured at **zero**
        // re-expansions on both, the distance component of `Cost` being
        // what the weighting makes inconsistent.
        let mut best: Ledger<C> = Ledger::new();
        // And which of them have been expanded, where the answer is not
        // being proven. See the note above: this is [`None`] for a search
        // that has promised the fewest, which needs no such set and would
        // pay a hash an expansion for it.
        let mut settled: Option<FxHashSet<Node>> =
            how.approximates().then(FxHashSet::default);
        // Where each was reached from, which is asked once per relaxation
        // rather than once per candidate — three million times against a
        // billion — so it stays a map and pays no bytes for a system the
        // search only looked at.
        let mut came: FxHashMap<Node, Node> = FxHashMap::default();
        // Nearest the goal is the smallest number and a heap pops the
        // greatest, so the whole key is reversed: the ordering is "cheapest
        // first, and of those the one nearest the goal".
        let mut open = BinaryHeap::new();
        let mut near: Vec<(Node, [f64; 3], f64)> = Vec::new();
        let mut cells: Vec<CellId> = Vec::new();
        // Read once, here, so every setting's promise is kept by the one
        // place that could break it. See [`Routing::fanout`].
        let fanout = how.fanout();
        // How hard this pass leans on the estimate, which is where the
        // leaning belongs rather than inside the estimate itself: `ceiling`
        // then has an admissible estimate to measure a candidate against.
        let weight = how.weight();

        // Squared, and as an integer: the key is only ever compared, and
        // squaring is monotone over distances that are never negative, so
        // the ordering is the same one a square root would give and the
        // root itself is forty million calls nobody reads.
        let away = |at: [f64; 3]| dist2(at, goal) as u64;

        let from = self.place(start);
        best.record(start, self.held(start), C::ZERO);
        open.push(Reverse((
            estimate(from).share(weight, WHOLE),
            away(from),
            C::ZERO,
            start,
        )));

        while let Some(Reverse((_, _, was, node))) = open.pop() {
            // Told to give up: the leg was cancelled, and nothing is left to
            // read the answer. One relaxed load an expansion against a
            // search that is otherwise minutes of a pool thread nobody is
            // waiting on. See [`Frontier::abandon`].
            if sampled.as_ref().is_some_and(Sampler::stopped) {
                return None;
            }
            // A system can be pushed more than once, a cheaper way to it
            // having been found after the first; the dearer entries are still
            // in the heap and are nothing to expand again.
            if was > best.cost(node) {
                continue;
            }
            // Expanded once, where the setting allows it. A system popped
            // a second time is one a cheaper chain reached after it was
            // settled, and expanding it again is the work that shows as a
            // search going back over ground near the start long after the
            // route past it stopped changing.
            if let Some(settled) = settled.as_mut()
                && !settled.insert(node)
            {
                continue;
            }
            let at = self.place(node);
            if node == end {
                let mut path = vec![end];
                while let Some(&before) = came.get(path.last().expect("a step"))
                {
                    path.push(before);
                }
                path.reverse();
                return Some(path);
            }
            if let Some(sampled) = sampled.as_mut() {
                sampled.expanded(node, DVec3::from(at), &came, |node| {
                    DVec3::from(self.place(node))
                });
            }
            self.neighbors(
                node,
                at,
                range,
                drive,
                goal,
                fanout,
                step,
                &best,
                was.spent(),
                &mut cells,
                &mut near,
            );

            for &(next, place, leg) in &near {
                let cost = was + step(self, node, leg);
                if cost >= best.cost(next) {
                    continue;
                }
                let left = estimate(place);
                best.record(next, self.held(next), cost);
                came.insert(next, node);
                open.push(Reverse((
                    cost + left.share(weight, WHOLE),
                    away(place),
                    cost,
                    next,
                )));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, sky_apart, sky_of};
    use galos_index::CellId;
    use galos_index::meta::NameEntry;

    /// A system named for its address, at `at`.
    fn at(address: i64, at: [f32; 3]) -> NameEntry {
        NameEntry { address, name: format!("S{address}").into(), position: at }
    }

    /// A route's end, as the router takes one
    ///
    /// The address and where the names table says it sits: the pair
    /// [`JumpGraph::route`] wants, because the record itself is found by
    /// descending to that place ([`galos_index::Sky::node_of`]). A test
    /// knows both, having put the system there.
    fn end(entries: &[NameEntry], address: i64) -> (i64, [f64; 3]) {
        let found = entries
            .iter()
            .find(|entry| entry.address == address)
            .expect("a system the test placed");
        (
            found.address,
            [
                found.position[0] as f64,
                found.position[1] as f64,
                found.position[2] as f64,
            ],
        )
    }

    /// A built galaxy holding `entries`, and the directory it lives in
    ///
    /// The directory comes back with it and must be held for as long as the
    /// galaxy is read: the places are the cell payloads, mapped where they
    /// lie, so a route over a directory that has been removed is a route
    /// over nothing.
    fn galaxy(what: &str, entries: &[NameEntry]) -> (Scratch, Arc<Sky>) {
        let dir = Scratch::new(what);
        let sky = sky_of(dir.path(), entries);
        (dir, sky)
    }

    /// The graph over such a galaxy.
    fn over(sky: &Arc<Sky>, boosts: &Boosts) -> JumpGraph {
        JumpGraph::over(sky, boosts)
    }

    /// A published supercharge table naming `cones`, placed where the test
    /// put them
    ///
    /// The published row carries the place ([`galos_index::SystemBoost`]),
    /// which is what takes the names table out of routing — so a test's
    /// cones have to be systems the test actually placed.
    fn cones(entries: &[NameEntry], cones: &[(i64, Boost)]) -> Boosts {
        Boosts::of(
            cones
                .iter()
                .map(|&(address, boost)| {
                    let placed = entries
                        .iter()
                        .find(|entry| entry.address == address)
                        .expect("a cone the test placed");
                    galos_index::SystemBoost {
                        address,
                        boost,
                        position: placed.position,
                    }
                })
                .collect(),
        )
    }

    /// How far a route runs, following its legs.
    fn run(path: &[(i64, [f64; 3])]) -> f64 {
        path.windows(2).map(|w| dist2(w[0].1, w[1].1).sqrt()).sum()
    }

    /// Both proven settings, since every claim below holds of both.
    const BOTH: [Routing; 2] = [Routing::FEWEST, Routing::SHORTEST];

    /// A range small enough to make the estimate overflow is answered, not
    /// panicked on
    ///
    /// The form takes any range over nothing, and a small enough one puts more
    /// jumps between two systems than a `u32` holds: the cast pins at the top
    /// and scaling it by [`LEANING`] wrapped — a panic on the compute pool in
    /// a debug build. It fires on the first push, before a neighbour is looked
    /// at, so having no reachable neighbours is no protection.
    #[test]
    fn a_range_too_small_to_estimate_does_not_overflow() {
        let entries = vec![at(0, [0., 0., 0.]), at(1, [100., 0., 0.])];
        let (_dir, sky) = galaxy("overflow", &entries);
        let graph = over(&sky, &Boosts::default());

        assert!(
            graph
                .route(
                    end(&entries, 0),
                    end(&entries, 1),
                    1e-9,
                    Routing::QUICK,
                    Drive::Standard,
                    Tuning::default(),
                    None,
                )
                .is_none(),
            "nothing is reachable at that range"
        );
    }

    /// A search with no corridor to walk is drawn coarser rather than without
    /// bound
    ///
    /// The cell of the closed set is sized off how far there is to go, on the
    /// argument that the cells touched are the corridor searched. A leg with
    /// no route expands in every direction instead, and the layer grew with
    /// the search: every cell copied out under the lock each frame and turned
    /// into four vertices. Held at [`super::frontier::CELL_CEILING`] now, by
    /// widening the cell.
    #[test]
    fn a_search_that_spreads_is_held_at_the_cell_ceiling() {
        let goal = DVec3::new(100., 0., 0.);
        let frontier = Frontier::between(DVec3::ZERO, goal);
        let first = frontier.drawn().expect("a frontier").across;
        let mut sampler = frontier.sampler();
        // One node, reached from nowhere: the sampler hashes what it is
        // given and asks the closure where it sits, so a search's worth of
        // expansions needs no galaxy behind it.
        let node = Node {
            cell: CellId { level: 13, x: 4096, y: 4096, z: 4096 },
            at: 0,
        };
        let came: FxHashMap<Node, Node> = FxHashMap::default();

        // Expansions marching away in a straight line, one cell apiece: what
        // a search with nowhere to go looks like to the sampler.
        let ceiling = super::super::frontier::CELL_CEILING;
        let stride = super::super::frontier::STRIDE;
        let step = first * 1.5;
        for n in 0..(ceiling as u64 * 4 * stride) {
            let at = DVec3::new(0., 0., (n / stride) as f64 * step);
            sampler.expanded(node, at, &came, |_| DVec3::ZERO);
        }
        sampler.done();

        let drawn = frontier.drawn().expect("a frontier");
        assert!(
            drawn.cells.len() <= ceiling,
            "the closed set held {} cells, over the ceiling of {ceiling}",
            drawn.cells.len()
        );
        assert!(
            drawn.across > first,
            "the cell never widened: still {} light years",
            drawn.across
        );
    }

    /// Of two chains the same number of jumps long, the route follows the
    /// straighter
    ///
    /// The reported trouble, in miniature: counting jumps alone makes every
    /// chain of five equally good, so a detour off the line and back is free
    /// and the line drawn wanders. The detour is offered to the search first,
    /// by being built first, so a router that takes whatever it reaches first
    /// takes the detour.
    ///
    /// [`Routing::FEWEST`] gets here by preferring the neighbour nearest the
    /// goal and [`Routing::SHORTEST`] by costing the distance, so the two
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
        let (_dir, sky) = galaxy("straighter", &entries);
        let graph = over(&sky, &Boosts::default());

        for how in BOTH {
            let path = graph
                .route(
                    end(&entries, 0),
                    end(&entries, 9),
                    500.,
                    how,
                    Drive::Unaided,
                    Tuning::default(),
                    None,
                )
                .expect("a route");

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
        let (_dir, sky) = galaxy("fewest", &entries);
        let graph = over(&sky, &Boosts::default());

        for how in BOTH {
            let path = graph
                .route(
                    end(&entries, 0),
                    end(&entries, 9),
                    500.,
                    how,
                    Drive::Unaided,
                    Tuning::default(),
                    None,
                )
                .expect("a route");

            assert_eq!(path.len() - 1, 2, "{how:?} took the short hops");
            assert_eq!(path[1].0, 50, "{how:?} missed the far waypoint");
        }
    }

    /// A quick route is a route: real jumps, and none of them cheating
    ///
    /// [`Routing::QUICK`] leans on the estimate until it overstates, which is
    /// what stops A* expanding the plateau of systems that could have been on
    /// an equally short chain — and a bound on how much worse the answer may
    /// be is all that weighting an estimate gives back. What it must not touch
    /// is what a jump is: every leg inside the range asked for, and never
    /// fewer jumps than the fewest, which would mean the search had found a
    /// chain the proof says does not exist.
    ///
    /// A corridor with two lanes down it is that plateau in miniature: every
    /// crossing between them makes another chain of the same length, and the
    /// chains number two to the thirtieth.
    #[test]
    fn a_quick_route_is_made_of_real_jumps() {
        let mut entries = vec![at(0, [0., 0., 0.])];
        for k in 1..=30 {
            entries.push(at(k, [90.0 * k as f32, 0., 0.]));
            entries.push(at(100 + k, [90.0 * k as f32, 40., 0.]));
        }
        // Named outside the lanes, which hold 1..=30 and 101..=130.
        entries.push(at(999, [2790., 0., 0.]));
        // One neutron star in the corridor, so the range a jump may take and
        // the range the estimate divides by are different numbers — which is
        // what a leg has to be measured against, and what a search reaching
        // by the wrong one of the two would be caught by below.
        let boosts = cones(&entries, &[(15, Boost::Neutron)]);
        let (_dir, sky) = galaxy("quick", &entries);
        let graph = over(&sky, &boosts);
        let drive = Drive::Standard;
        let (start, goal) = (end(&entries, 0), end(&entries, 999));

        let fewest = graph
            .route(
                start,
                goal,
                100.,
                Routing::FEWEST,
                drive,
                Tuning::default(),
                None,
            )
            .expect("a route")
            .len()
            - 1;
        let quick = graph
            .route(
                start,
                goal,
                100.,
                Routing::QUICK,
                drive,
                Tuning::default(),
                None,
            )
            .expect("a quick route");

        assert_eq!(quick.first().expect("a start").0, 0, "started elsewhere");
        assert_eq!(quick.last().expect("an end").0, 999, "ended elsewhere");
        assert!(
            quick.len() - 1 >= fewest,
            "{} jumps beats the fewest there are, {fewest}",
            quick.len() - 1
        );
        for leg in quick.windows(2) {
            let flown = dist2(leg[0].1, leg[1].1).sqrt();
            // What the system being left could charge the drive to, which is
            // the only thing that says how far a jump out of it may go. Its
            // record is found from the place the route itself came back with,
            // that being where the payload has it.
            let reach = 100.
                * drive.factor(
                    graph
                        .node_of(leg[0].0, leg[0].1)
                        .and_then(|node| graph.boost(node)),
                );
            assert!(
                flown <= reach,
                "a leg of {flown:.0} ly out of {} reaching {reach:.0}",
                leg[0].0
            );
        }
    }

    /// What a chain burns and what it is priced at, as the search weighs it
    ///
    /// Fuel first, since that is the claim; the floors beside it, since
    /// that is what the search actually minimised.
    fn burn(path: &[(i64, [f64; 3])], range: f64) -> (f64, f64) {
        let fuel: u32 = path
            .windows(2)
            .map(|w| burned(dist2(w[0].1, w[1].1).sqrt(), range))
            .sum();
        let jumps = path.len().saturating_sub(1) as u32;
        (
            fuel as f64 / TANKFUL as f64,
            (fuel + jumps * floor(HOP)) as f64 / TANKFUL as f64,
        )
    }

    /// The economical route burns less by flying more, shorter jumps
    ///
    /// The two asks are opposed and this is where they part. The fewest
    /// jumps means every jump pushed as near the range as the sky allows,
    /// and a jump at full range costs the drive's whole maximum fuel — so
    /// the fewest-jumps route is the thirstiest there is. A thousand light
    /// years down a line of systems fifty apart, at a five hundred light
    /// year range: two jumps of five hundred against four of two hundred
    /// and fifty.
    #[test]
    fn the_economical_route_burns_less_than_the_fewest_jumps() {
        let entries: Vec<NameEntry> = (0..=20)
            .map(|step| at(step, [step as f32 * 50., 0., 0.]))
            .collect();
        let (_dir, sky) = galaxy("economical", &entries);
        let graph = over(&sky, &Boosts::default());

        let plotted = |how: Routing| {
            graph
                .route(
                    end(&entries, 0),
                    end(&entries, 20),
                    500.,
                    how,
                    Drive::Unaided,
                    Tuning::default(),
                    None,
                )
                .expect("a route down a line of systems")
        };

        let fewest = plotted(Routing::FEWEST);
        let thrifty = plotted(Routing::ECONOMICAL);

        assert_eq!(fewest.len() - 1, 2, "the fewest jumps moved");
        assert_eq!(
            thrifty.len() - 1,
            4,
            "the economical route flew {} jumps, not the four a quarter \
             floor asks for",
            thrifty.len() - 1,
        );

        // Half the fuel, which is what a floor of a quarter buys.
        let (burned_fewest, _) = burn(&fewest, 500.);
        let (burned_thrifty, _) = burn(&thrifty, 500.);
        assert_eq!(burned_fewest, 2., "two jumps at full range burn two");
        assert_eq!(burned_thrifty, 1., "four at half the range burn one");

        // And it is the least the search could have been priced at, which
        // is the claim `Weigh::Fuel` makes: nothing on this line is cheaper
        // under what it weighs.
        let priced = burn(&thrifty, 500.).1;
        for k in 1..=20u32 {
            let jump = 1000. / k as f64;
            if jump > 500. {
                continue;
            }
            let other =
                (k * (floor(HOP) + burned(jump, 500.))) as f64 / TANKFUL as f64;
            assert!(
                priced <= other + 1e-9,
                "{k} jumps of {jump:.0} ly price at {other}, under {priced}",
            );
        }
    }

    /// No jump is free, however short
    ///
    /// **The price used to round some jumps to nothing.** At the old
    /// resolution a light year at a 45 light year range priced at
    /// `(1/45)^2 x 1000 = 0.49`, which rounded to zero — and a graph with
    /// free edges has no positive lower bound on the fuel left to burn, so
    /// the ordering is degenerate and nothing the exact setting answers is
    /// proven. It showed as a 95% route coming back **cheaper than the
    /// proven one**: 2.144 against 2.148 of a tank on a 702 light year
    /// crossing, measured over `.index/full`. Repriced, the same crossing
    /// is 2.1375 proven against 2.1387 at 95%, which is the way round it
    /// has to be.
    ///
    /// Two rules, and the floor is the one that cannot be had by
    /// resolution alone: any leg costs at least one unit, and a leg of
    /// nothing costs nothing — a system already stood on is not a jump.
    #[test]
    fn a_jump_is_never_free() {
        // The leg that used to round away, at the range it did it at.
        assert!(burned(1., 45.) > 0, "a light year came to nothing");
        // And everything under it, down to where a position is exact.
        for leg in [0.5, 0.1, 0.03125, 1e-6] {
            assert!(burned(leg, 45.) > 0, "{leg} ly came to nothing");
            assert!(
                burned(leg, 1_000.) > 0,
                "{leg} ly at 1 kly came to nothing"
            );
        }
        // Standing still is not a jump.
        assert_eq!(burned(0., 45.), 0, "a leg of nothing was charged");
        // And the price still rises with the leg and caps at a tankful.
        assert!(
            burned(10., 45.) < burned(20., 45.),
            "the price stopped rising"
        );
        assert_eq!(burned(45., 45.), TANKFUL, "a jump at range is a tankful");
        assert_eq!(burned(900., 45.), TANKFUL, "a boosted jump burns more");
    }

    /// The hop is the trade, and it is a straight line
    ///
    /// **The least fuel there is, is as many hops as the sky offers.**
    /// Splitting any jump saves fuel and nothing in the arithmetic stops
    /// it, so an unpriced fuel route takes every system it can reach —
    /// measured against the real index, ninety-four jumps of a third of a
    /// light year to cross three hundred, against six weighed by jumps. So
    /// where to stop splitting is the reader's to say, and this is the
    /// setting that says it: a shorter hop is more jumps and less fuel,
    /// every time, and a hop of the whole range asks for no splitting at
    /// all.
    #[test]
    fn a_shorter_hop_buys_fuel_with_jumps() {
        let entries: Vec<NameEntry> = (0..=20)
            .map(|step| at(step, [step as f32 * 50., 0., 0.]))
            .collect();
        let (_dir, sky) = galaxy("hops", &entries);
        let graph = over(&sky, &Boosts::default());

        let flown = |hop: u32| {
            let path = graph
                .route(
                    end(&entries, 0),
                    end(&entries, 20),
                    500.,
                    Routing { over: 0, weigh: Weigh::Fuel { hop, expand: 0 } },
                    Drive::Unaided,
                    Tuning::default(),
                    None,
                )
                .expect("a route down a line of systems");
            (path.len() - 1, burn(&path, 500.).0)
        };

        let (whole, burned_whole) = flown(100);
        let (half, burned_half) = flown(50);
        let (quarter, burned_quarter) = flown(25);

        // More jumps every step down, and less fuel every step down.
        assert!(
            whole < half && half < quarter,
            "the jumps did not grow: {whole}, {half}, {quarter}",
        );
        assert!(
            burned_whole > burned_half && burned_half > burned_quarter,
            "the fuel did not fall: {burned_whole}, {burned_half}, \
             {burned_quarter}",
        );

        // And the whole range asks for no splitting: the same two jumps the
        // fewest-jumps route flies.
        assert_eq!(whole, 2, "a hop of the whole range still split");
    }

    /// And it does not dissolve into a thousand tiny hops
    ///
    /// Fuel alone is least where the ship barely moves: every split of a
    /// jump saves fuel, so a route weighed on fuel and nothing else would
    /// take every system in the line. What stops it is that a jump is
    /// priced whatever it burns ([`floor`]) — and this is the test that
    /// says so, the line below offering twenty hops it could have taken.
    #[test]
    fn the_economical_route_does_not_dissolve_into_hops() {
        let entries: Vec<NameEntry> = (0..=20)
            .map(|step| at(step, [step as f32 * 50., 0., 0.]))
            .collect();
        let (_dir, sky) = galaxy("dissolve", &entries);
        let graph = over(&sky, &Boosts::default());

        let path = graph
            .route(
                end(&entries, 0),
                end(&entries, 20),
                500.,
                Routing::ECONOMICAL,
                Drive::Unaided,
                Tuning::default(),
                None,
            )
            .expect("a route");

        assert!(
            path.len() - 1 < 10,
            "the route took {} of the twenty hops on offer",
            path.len() - 1,
        );
    }

    /// Where the two part: only one of them proves the shortest chain
    ///
    /// A detour that is *not* offered first and is not the nearest step at any
    /// point — its first leg goes almost sideways — so preferring the nearest
    /// neighbour walks straight past it. Both find four jumps; the point is
    /// that [`Routing::SHORTEST`] is the one that has proved no shorter four
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
        let (_dir, sky) = galaxy("proves", &entries);
        let graph = over(&sky, &Boosts::default());

        for how in BOTH {
            let path = graph
                .route(
                    end(&entries, 0),
                    end(&entries, 9),
                    500.,
                    how,
                    Drive::Unaided,
                    Tuning::default(),
                    None,
                )
                .expect("a route");
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
        let (_dir, sky) = galaxy("gap", &entries);
        let graph = over(&sky, &Boosts::default());
        let (start, goal) = (end(&entries, 0), end(&entries, 1));

        for how in BOTH {
            assert!(
                graph
                    .route(
                        start,
                        goal,
                        500.,
                        how,
                        Drive::Unaided,
                        Tuning::default(),
                        None
                    )
                    .is_none(),
                "{how:?} jumped 600 at 500"
            );
            assert!(
                graph
                    .route(
                        start,
                        goal,
                        700.,
                        how,
                        Drive::Unaided,
                        Tuning::default(),
                        None
                    )
                    .is_some(),
                "{how:?} refused 600 inside 700"
            );
        }
    }

    /// A neighbour search reads cells other than the one the jump leaves,
    /// and offers each system once
    ///
    /// The places are the cell payloads now, so what a jump can reach has
    /// nothing to do with where a cell boundary fell: a search bounded by
    /// the payload it starts in would only ever step to whatever happens to
    /// share that payload, which is a route through one cell of the galaxy
    /// and nothing else. The whole of the neighbour query is the sphere.
    ///
    /// Cut to a system a cell so there is something to cross ([`sky_apart`]:
    /// the build divides on count, so a handful of systems under the
    /// published caps is one root payload however far apart they lie), and
    /// the two systems 384 light years apart land the two ways they can. One
    /// is in an *internal* node's slice and the other below it — a cell
    /// keeps the brightest of what fell in it and pushes the rest down, so a
    /// system in an internal node is ordinary and not an edge case — and the
    /// third is off in a sibling subtree the search's own cell does not
    /// contain at all.
    ///
    /// Once each, and never itself. A system is in exactly one cell's
    /// payload, so a doubled neighbour would mean a cell of the descent had
    /// been scanned twice; and a system offered as its own neighbour is a
    /// jump of no distance, which every setting would take for free forever.
    #[test]
    fn a_neighbour_search_reads_past_the_cell_it_starts_in() {
        // Wider than any jump, and the same either side, so the one range
        // reaches both of the systems it should and the boundary is one
        // number.
        let far = 384.;
        let here = [0., 0., 0.];
        let dir = Scratch::new("across-cells");
        // The brightest is the system the root keeps, and it is parked far
        // enough off that no range below reaches it: what it is for is to
        // leave the three the assertions are about under the root rather
        // than in it.
        let sky = sky_apart(
            dir.path(),
            &[
                (0, [-5_000., 0., 0.]),
                (1, here),
                (2, [far, 0., 0.]),
                (3, [-far, 0., 0.]),
            ],
        );
        let graph = over(&sky, &Boosts::default());
        let from = graph.node_of(1, here).expect("the system it maps");
        for address in [2, 3] {
            let cell = graph
                .node_of(address, [0., 0., 0.])
                .expect("the others too")
                .cell;
            assert_ne!(
                from.cell, cell,
                "S{address} shares a payload, so there is nothing to cross"
            );
        }

        let found = |range: f64| {
            let mut out = Vec::new();
            graph.neighbors(
                from,
                here,
                range,
                Drive::Unaided,
                [far, 0., 0.],
                // Uncapped: what this pins is which systems are in reach,
                // not which of them a setting would bother to relax.
                None,
                // What a jump costs, which an uncapped call never scores
                // anything with.
                &|_: &JumpGraph, _, _| 0u32,
                // A ledger that has reached nothing, so no cell is settled
                // and every one of them is read.
                &Ledger::<u32>::new(),
                0,
                &mut Vec::new(),
                &mut out,
            );
            let mut addresses: Vec<i64> =
                out.iter().map(|&(node, ..)| graph.address(node)).collect();
            addresses.sort();
            addresses
        };

        assert_eq!(
            found(far + 1.),
            vec![2, 3],
            "the systems a cell away, each once and itself never"
        );
        assert!(found(far - 1.).is_empty(), "and nothing inside that");
    }

    /// Only the setting that has not promised the fewest jumps thins an
    /// expansion.
    ///
    /// The cap is the fix for a search whose work is quadratic in stellar
    /// density, and it is also the one thing here that can lose the right
    /// answer: the neighbour a fewest-jumps chain went through may be the
    /// one thinned away. So [`Routing::FEWEST`] and [`Routing::SHORTEST`],
    /// which both claim the fewest, must carry no cap at all — and a
    /// setting added later must decide which it is rather than inherit
    /// whatever the match arm above it said.
    #[test]
    fn only_a_quick_route_thins_an_expansion() {
        assert_eq!(Routing::QUICK.fanout(), Some(FANOUT));
        for how in BOTH {
            assert_eq!(
                how.fanout(),
                None,
                "{how:?} promises the fewest jumps and may not thin",
            );
        }
    }

    /// A neighbour standing where the test wants it, cone or not
    ///
    /// The place is what the thinning reads; the node is only an identity,
    /// so it counts the neighbours off and says which of them can charge a
    /// drive by whether the count is in `cones`.
    fn near(at: u32, away: f64) -> (Node, [f64; 3], f64) {
        let cell = galos_index::CellId { level: 1, x: 0, y: 0, z: 0 };
        (Node { cell, at }, [away, 0., 0.], away)
    }

    /// The score a jump-counted route thins by
    ///
    /// A jump costs one wherever it lands, so the cheapest candidate per
    /// light year closed is simply the nearest the goal. The fixtures here
    /// put the goal at the origin and every candidate out along x, so the
    /// distance to the goal is the place itself.
    fn nearness(cand: &(Node, [f64; 3], f64)) -> (f64, f64) {
        (cand.1[0], cand.1[0])
    }

    /// And the score a fuel-weighed route thins by
    ///
    /// Fuel goes as the square of a jump, so a candidate's price is
    /// `(leg / range) ^ 2` of a tank and what the cap should keep is the
    /// cheapest per light year of ground closed. Here every candidate lies
    /// between the ship and the goal, so the ground it closes *is* its
    /// leg — and the score comes to the leg over ten, which is to say the
    /// short jumps win.
    fn per_fuel(cand: &(Node, [f64; 3], f64)) -> (f64, f64) {
        let closed = cand.2;
        let burn = (cand.2 / 100.).powi(2) * TANKFUL as f64;
        (burn / closed, cand.1[0])
    }

    /// The cap never drops a jet cone to keep a system nearer the goal
    ///
    /// The reason the cap can lose a route outright rather than a few
    /// percent of one. A cone's worth is the reach of the jump out of it —
    /// six hundred light years at a fifty light year range — so a cone off
    /// to the side beats a system a little further along the heading, and
    /// nearness to the goal is an order that cannot see the difference.
    /// Measured against the real index: the old rule dropped 3,605 of the
    /// 7,376 cones it saw on one 300 ly crossing, and the route came back
    /// nine jumps where keeping them gives seven.
    #[test]
    fn the_cap_keeps_the_cones() {
        // The goal is at the origin, so a larger `away` is further from it.
        // Two cones out at the far end, and four ordinary systems nearer
        // than either of them, thinned to three.
        let mut found = vec![
            near(0, 10.),
            near(1, 900.),
            near(2, 20.),
            near(3, 950.),
            near(4, 30.),
            near(5, 40.),
        ];
        let cones = [1u32, 3];

        thinned(&mut found, 3, nearness, |node| cones.contains(&node.at));

        let kept: std::collections::HashSet<u32> =
            found.iter().map(|(node, ..)| node.at).collect();
        assert_eq!(found.len(), 3, "the cap was not spent: {kept:?}");
        for cone in cones {
            assert!(kept.contains(&cone), "a cone was dropped: {kept:?}");
        }
        // And the one place left went to the nearest of the rest.
        assert!(
            kept.contains(&0),
            "the nearest ordinary system went: {kept:?}"
        );
    }

    /// More cones than the cap allows: the cap is spent on cones
    ///
    /// Which happens at a boosted range in the core — a 200 ly sphere holds
    /// tens of thousands of systems, so its cones alone can outrun the cap.
    /// Taken nearest the goal there, since something has to choose and that
    /// is the only reading left.
    #[test]
    fn more_cones_than_room_are_taken_nearest_the_goal() {
        let mut found =
            vec![near(0, 40.), near(1, 10.), near(2, 30.), near(3, 20.)];

        thinned(&mut found, 2, nearness, |_| true);

        let kept: std::collections::HashSet<u32> =
            found.iter().map(|(node, ..)| node.at).collect();
        assert_eq!(kept, [1, 3].into_iter().collect(), "{kept:?}");
    }

    /// And a drive that cannot charge pays nothing for the question
    ///
    /// An unaided route is thinned as it always was: every neighbour is an
    /// ordinary system to it, whatever the galaxy publishes about the star.
    #[test]
    fn an_unaided_expansion_is_thinned_by_nearness_alone() {
        let mut found =
            vec![near(0, 900.), near(1, 10.), near(2, 950.), near(3, 20.)];

        thinned(&mut found, 2, nearness, |_| false);

        let kept: std::collections::HashSet<u32> =
            found.iter().map(|(node, ..)| node.at).collect();
        assert_eq!(kept, [1, 3].into_iter().collect(), "{kept:?}");
    }

    /// Nothing is thinned where the cap is not reached
    ///
    /// Which is most expansions: what is collected is the neighbours worth
    /// *relaxing* rather than every system in the sphere, and a route
    /// across the galaxy on the coarse plan never reaches the cap at all.
    #[test]
    fn an_expansion_inside_the_cap_is_left_whole() {
        let mut found = vec![near(0, 900.), near(1, 10.)];

        thinned(&mut found, 512, nearness, |_| true);

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0.at, 0, "the order moved under a cap that idled");
    }

    /// A fuel-weighed route keeps the cheap jumps, not the far ones
    ///
    /// **The reported trouble.** Sol to Col 285 Sector ZQ-K C9-12 at a 100
    /// ly range, least fuel over unpriced hops: the optimal answer is 35
    /// stops, 235.2 ly flown, no jump over 16.3 ly and 0.192 of a tank —
    /// and at 95% optimality it came back 15 stops, longest jump **50.9
    /// ly**, **0.611 of a tank**. Three times the fuel from a setting that
    /// said five percent, and it "jumps a lot too far in the beginning"
    /// because the cap only bites where the sky is dense, which is the
    /// bubble it sets out from. With no cap at all the 95% walk answers the
    /// optimal route exactly, so the cap was the whole of it: the 512 it
    /// kept were the ones nearest the goal, which in a 100 ly sphere are
    /// the longest jumps in it and so the dearest fuel there is.
    ///
    /// Four candidates here, all of them between the ship and the goal, so
    /// each closes exactly its own leg of ground. Nearness keeps the two
    /// longest jumps; the fuel score keeps the two shortest, which is the
    /// answer to what a least-fuel route is asking for.
    #[test]
    fn a_fuel_ask_keeps_the_cheap_jumps() {
        let legs = [5., 20., 50., 90.];
        let mut found: Vec<(Node, [f64; 3], f64)> = legs
            .iter()
            .enumerate()
            .map(|(at, &leg)| {
                let cell = galos_index::CellId { level: 1, x: 0, y: 0, z: 0 };
                (Node { cell, at: at as u32 }, [100. - leg, 0., 0.], leg)
            })
            .collect();

        let mut by_fuel = found.clone();
        thinned(&mut by_fuel, 2, per_fuel, |_| false);
        let kept: std::collections::HashSet<u32> =
            by_fuel.iter().map(|(node, ..)| node.at).collect();
        assert_eq!(
            kept,
            [0, 1].into_iter().collect(),
            "the cap spent a fuel route's places on long jumps: {kept:?}",
        );

        // And the same four thinned for a route counted in jumps, which
        // wants the ground closed and does not care what it costs.
        thinned(&mut found, 2, nearness, |_| false);
        let kept: std::collections::HashSet<u32> =
            found.iter().map(|(node, ..)| node.at).collect();
        assert_eq!(
            kept,
            [2, 3].into_iter().collect(),
            "a jump-counted route stopped keeping the ground closers: \
             {kept:?}",
        );
    }

    /// Every approximation turns on the one question the reader answered
    ///
    /// The rule used to be "an approximation belongs to `Routing::Quick`",
    /// a named mode to be remembered. It is now the answer to *how many
    /// jumps over the fewest*: nothing over is a promise, and a promise
    /// admits no thinning and no coarse plan. Which also settles the pair
    /// the three modes could not express — shortest distance *and* a
    /// percent over — in the only way that is sound.
    #[test]
    fn nothing_over_the_fewest_approximates_nothing() {
        for weigh in [
            Weigh::Jumps,
            Weigh::Shortest,
            Weigh::Fuel { hop: HOP, expand: EXPAND },
        ] {
            let proven = Routing { over: 0, weigh };
            assert_eq!(proven.fanout(), None, "{proven:?} thinned");
            assert!(!proven.highway(), "{proven:?} planned coarsely");
            assert_eq!(proven.weight(), WHOLE, "{proven:?} weighted");

            let over = Routing { over: 5, weigh };
            assert!(over.highway(), "{over:?} would not plan coarsely");
            assert_eq!(over.weight(), WHOLE + 5);
            // And the cap is the weighing's own where the form offers it,
            // the fixed valve everywhere else — including at a *priced*
            // hop, where the rail is not drawn and the count it last held
            // used to go on biting unseen. See [`Routing::fanout`].
            let kept = match weigh {
                Weigh::Fuel { hop: 0, expand } => expand as usize,
                Weigh::Fuel { .. } | Weigh::Jumps | Weigh::Shortest => FANOUT,
            };
            assert_eq!(over.fanout(), Some(kept), "{over:?} did not thin");
        }

        // Nothing asked of an *unpriced* fuel route is every system in
        // range, which is the graph a proven route searches — so the
        // rail's top stop and the proven ask meet rather than leaving a
        // gap between them. A priced hop keeps the valve, the rail not
        // being drawn there to ask for anything else.
        let all = Routing { over: 5, weigh: Weigh::Fuel { hop: 0, expand: 0 } };
        assert_eq!(all.fanout(), None, "the rail's top stop still capped");
        let priced =
            Routing { over: 5, weigh: Weigh::Fuel { hop: HOP, expand: 0 } };
        assert_eq!(
            priced.fanout(),
            Some(FANOUT),
            "a priced hop took a cap nobody could see",
        );

        // **And at an unpriced hop the cap is the whole of the promise.**
        // The percent cannot approximate anything there — the estimate is
        // zero, so weighting it is zero — which is what lets the cap
        // rail's top stop be the proven ask and what let the form drop its
        // `Optimal` tick. See [`Routing::approximates`].
        let unpriced = Weigh::Fuel { hop: 0, expand: 0 };
        for over in [0, 5, 95] {
            let proven = Routing { over, weigh: unpriced };
            assert!(!proven.approximates(), "{proven:?} approximated");
            assert!(!proven.highway(), "{proven:?} planned coarsely");
            assert_eq!(proven.fanout(), None, "{proven:?} thinned");
        }
        // And a cap is an approximation whatever the percent says, the
        // slack being the one thing that cannot make up for what a search
        // never looked at.
        let capped =
            Routing { over: 0, weigh: Weigh::Fuel { hop: 0, expand: 64 } };
        assert!(capped.approximates(), "a capped search claimed a proof");
        assert!(capped.highway(), "{capped:?} would not plan coarsely");
        assert_eq!(capped.fanout(), Some(64), "{capped:?} did not thin");
    }

    /// And what a route says it was asked for
    ///
    /// Read in a line of prose under the panel's title, so it says what the
    /// route is rather than which of three buttons was pressed. In the
    /// reader's own terms — an *optimality*, where the search wants the
    /// slack — so the two ways round have to agree.
    ///
    /// **A route that was capped says so**, which is the half a reader
    /// could not otherwise discover: what a cap drops the search never
    /// learns, and the only sign of it is a route that went the long way.
    /// A proven route is capped at nothing and says nothing.
    #[test]
    fn a_route_says_what_it_was_asked_for() {
        assert_eq!(Routing::FEWEST.named(), "optimal, fewest jumps");
        assert_eq!(Routing::SHORTEST.named(), "optimal, shortest");
        assert_eq!(
            Routing::QUICK.named(),
            "95% optimality, fewest jumps, the nearest 512 expanded"
        );
        assert_eq!(
            Routing { over: 20, weigh: Weigh::Shortest }.named(),
            "80% optimality, shortest, the nearest 512 expanded"
        );
        // The fuel weighing says its own two settings, and the cap it says
        // is the one the reader set.
        assert_eq!(
            Routing { over: 5, weigh: Weigh::Fuel { hop: 0, expand: 8 } }
                .named(),
            "95% optimality, the least fuel there is, the nearest 8 expanded"
        );
        assert_eq!(
            Routing::ECONOMICAL.named(),
            "optimal, fuel over jumps at 50% hops"
        );

        // The word `optimal` follows the promise and not the percent. At
        // an unpriced hop the cap is the promise, so every system in range
        // is optimal however much slack was left standing —
        assert_eq!(
            Routing { over: 95, weigh: Weigh::Fuel { hop: 0, expand: 0 } }
                .named(),
            "optimal, the least fuel there is"
        );
        // — and a cap is not optimal however little was.
        assert_eq!(
            Routing { over: 0, weigh: Weigh::Fuel { hop: 0, expand: 64 } }
                .named(),
            "bounded by what it expanded, the least fuel there is, \
             the nearest 64 expanded"
        );

        // And the two ways round agree: what a reader asks for is what the
        // search is set to, at either end and in between.
        for optimality in [0, 5, 75, 95, 100] {
            let asked = Routing::at(optimality, Weigh::Jumps);
            assert_eq!(
                asked.optimality(),
                optimality,
                "{optimality}% came back as something else"
            );
        }
        assert_eq!(Routing::at(100, Weigh::Jumps), Routing::FEWEST);
        assert_eq!(Routing::at(95, Weigh::Jumps), Routing::QUICK);
    }

    /// A cap keeps the neighbours that get nearest the goal, and uncapped
    /// keeps them all.
    ///
    /// Which is the whole of what the cap does, and what makes it a
    /// defensible approximation rather than an arbitrary one: what it drops
    /// is the far side of the sphere, away from where the route is going.
    #[test]
    fn a_cap_keeps_the_neighbours_nearest_the_goal() {
        let here = [0., 0., 0.];
        let goal = [1_000., 0., 0.];
        // Nine systems in a line, the goal off one end, so which of them is
        // nearest it is unambiguous and the order is the line's own.
        let places: Vec<(i64, [f64; 3])> =
            (1..=9).map(|n| (n, [n as f64 * 10., 0., 0.])).collect();
        let dir = Scratch::new("capped");
        let sky = crate::testing::sky(dir.path(), &places);
        let graph = over(&sky, &Boosts::default());
        let from = graph.node_of(1, places[0].1).expect("the first system");

        let reached = |cap: Option<usize>| {
            let mut out = Vec::new();
            graph.neighbors(
                from,
                here,
                500.,
                Drive::Unaided,
                goal,
                cap,
                // Counted in jumps, a jump costing the same wherever it
                // lands — which is what makes the cap's own order nearness.
                &|_: &JumpGraph, _, _| WHOLE,
                &Ledger::<u32>::new(),
                0,
                &mut Vec::new(),
                &mut out,
            );
            let mut found: Vec<i64> =
                out.iter().map(|&(node, ..)| graph.address(node)).collect();
            found.sort();
            found
        };

        // Every other system is in range, and itself is never among them.
        assert_eq!(reached(None), vec![2, 3, 4, 5, 6, 7, 8, 9]);
        // Capped, the ones kept are the far end of the line: nearest the
        // goal, which sits past S9.
        assert_eq!(reached(Some(3)), vec![7, 8, 9]);
        assert_eq!(reached(Some(1)), vec![9]);
    }

    /// And a fuel-weighed expansion keeps the cheap jumps out of the same
    /// sphere
    ///
    /// The half of the reported trouble that lives in [`JumpGraph`] rather
    /// than in [`thinned`]: the cap's order comes off the metric's own
    /// price, so the same nine systems thin one way for a route counted in
    /// jumps and the other way for one weighed by fuel. Scored by nearness
    /// alone — which is what it used to be, and what a wiring mistake here
    /// would quietly go back to — this keeps S9 and S8, the two longest
    /// jumps in the sphere and the dearest fuel in it.
    #[test]
    fn a_capped_fuel_expansion_keeps_the_short_jumps() {
        let here = [0., 0., 0.];
        let goal = [1_000., 0., 0.];
        let places: Vec<(i64, [f64; 3])> =
            (1..=9).map(|n| (n, [n as f64 * 10., 0., 0.])).collect();
        let dir = Scratch::new("capped-fuel");
        let sky = crate::testing::sky(dir.path(), &places);
        let graph = over(&sky, &Boosts::default());
        let from = graph.node_of(1, places[0].1).expect("the first system");

        let mut out = Vec::new();
        graph.neighbors(
            from,
            here,
            500.,
            Drive::Unaided,
            goal,
            Some(2),
            // What a least-fuel route is charged for a jump: the square of
            // it, and nothing before that at an unpriced hop.
            &|_: &JumpGraph, _, leg: f64| Burn {
                fuel: burned(leg, 100.),
                jumps: WHOLE,
            },
            &Ledger::<Burn>::new(),
            0,
            &mut Vec::new(),
            &mut out,
        );
        let mut found: Vec<i64> =
            out.iter().map(|&(node, ..)| graph.address(node)).collect();
        found.sort();
        assert_eq!(
            found,
            vec![2, 3],
            "a fuel-weighed expansion kept the long jumps",
        );
    }

    /// A boost is worth what the drive fitted makes of it, and nothing
    /// unaided
    ///
    /// The multipliers the game gives: four times off a neutron star and half
    /// again off a white dwarf, six and three off the drive built for it. A
    /// scale of one where there is no boost to be had or no drive to take it,
    /// so a caller can scale by this without asking first.
    #[test]
    fn a_boost_is_worth_what_the_drive_makes_of_it() {
        for drive in [Drive::Unaided, Drive::Standard, Drive::Optimised] {
            assert_eq!(drive.factor(None), 1., "no jet cone, {drive:?}");
        }
        for boost in [Boost::Neutron, Boost::WhiteDwarf] {
            assert_eq!(
                Drive::Unaided.factor(Some(boost)),
                1.,
                "no drive to take it, {boost:?}"
            );
        }

        assert_eq!(Drive::Standard.factor(Some(Boost::Neutron)), 4.);
        assert_eq!(Drive::Standard.factor(Some(Boost::WhiteDwarf)), 1.5);
        assert_eq!(Drive::Optimised.factor(Some(Boost::Neutron)), 6.);
        assert_eq!(Drive::Optimised.factor(Some(Boost::WhiteDwarf)), 3.);

        // What the estimate of the jumps left has to divide by: the widest
        // any one of them could be.
        assert_eq!(Drive::Unaided.widest(), 1.);
        assert_eq!(Drive::Standard.widest(), 4.);
        assert_eq!(Drive::Optimised.widest(), 6.);
    }

    /// A neutron star carries a jump no unboosted ship could make
    ///
    /// Which is the whole of what supercharging is for. The gap out of the
    /// neutron star is three and a half times the ship's own reach, so unaided
    /// there is no route at all; with a standard drive it is one jump of the
    /// four the star is worth.
    #[test]
    fn a_neutron_star_carries_a_jump_the_ship_could_not_make() {
        let entries = vec![
            at(1, [0., 0., 0.]),
            // The neutron star, one ordinary jump along.
            at(2, [90., 0., 0.]),
            // And the far side of a gap only a boost crosses.
            at(9, [440., 0., 0.]),
        ];
        let boosts = cones(&entries, &[(2, Boost::Neutron)]);
        let (_dir, sky) = galaxy("neutron", &entries);
        let graph = over(&sky, &boosts);
        let (start, goal) = (end(&entries, 1), end(&entries, 9));

        for how in BOTH {
            assert!(
                graph
                    .route(
                        start,
                        goal,
                        100.,
                        how,
                        Drive::Unaided,
                        Tuning::default(),
                        None
                    )
                    .is_none(),
                "{how:?} crossed 350 ly at a 100 ly range unaided"
            );

            let path = graph
                .route(
                    start,
                    goal,
                    100.,
                    how,
                    Drive::Standard,
                    Tuning::default(),
                    None,
                )
                .expect("a supercharged route");
            assert_eq!(
                path.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
                vec![1, 2, 9],
                "{how:?} did not go by way of the neutron star"
            );
        }
    }

    /// The boost is spent on the jump out, not on the one that arrives
    ///
    /// A charge is taken in the jet cone and held until a jump uses it, so
    /// what a jump may reach is the star of the system it leaves. Reading it
    /// off the system landed in instead would let a ship cross the gap first
    /// and pick up the boost afterwards, which is a route it cannot fly.
    #[test]
    fn the_boost_belongs_to_the_system_a_jump_leaves() {
        // The neutron star is at the far side of the gap this time.
        let entries = vec![
            at(1, [0., 0., 0.]),
            at(2, [90., 0., 0.]),
            at(9, [440., 0., 0.]),
        ];
        let boosts = cones(&entries, &[(9, Boost::Neutron)]);
        let (_dir, sky) = galaxy("leaves", &entries);
        let graph = over(&sky, &boosts);

        for how in BOTH {
            assert!(
                graph
                    .route(
                        end(&entries, 1),
                        end(&entries, 9),
                        100.,
                        how,
                        Drive::Standard,
                        Tuning::default(),
                        None,
                    )
                    .is_none(),
                "{how:?} flew a gap on a boost it had not collected yet"
            );
        }
    }

    /// White dwarfs are worth half again, and only the drive says how much
    ///
    /// The same gap, at a range only the optimised drive's threefold boost
    /// crosses: a standard drive's half again is not enough, so the two
    /// settings differ on the same sky rather than on the same ship.
    #[test]
    fn a_white_dwarf_carries_what_the_drive_allows() {
        let entries = vec![at(1, [0., 0., 0.]), at(9, [250., 0., 0.])];
        let boosts = cones(&entries, &[(1, Boost::WhiteDwarf)]);
        let (_dir, sky) = galaxy("dwarf", &entries);
        let graph = over(&sky, &boosts);
        let (start, goal) = (end(&entries, 1), end(&entries, 9));

        for how in BOTH {
            assert!(
                graph
                    .route(
                        start,
                        goal,
                        100.,
                        how,
                        Drive::Standard,
                        Tuning::default(),
                        None
                    )
                    .is_none(),
                "{how:?} made 250 ly of a 150 ly boosted jump"
            );
            assert!(
                graph
                    .route(
                        start,
                        goal,
                        100.,
                        how,
                        Drive::Optimised,
                        Tuning::default(),
                        None
                    )
                    .is_some(),
                "{how:?} refused 250 ly of a 300 ly boosted jump"
            );
        }
    }

    /// Only a quick route plans on the highway
    ///
    /// The same rule as the fanout cap, and the same reason it is a method
    /// on the setting: a chain of jet cones ([`super::highway`]) is a
    /// guess, and its edges say how few jumps *could* cross a gap rather
    /// than that a chain of systems crosses it that way. A setting that
    /// claims the fewest jumps cannot be answered off one.
    #[test]
    fn only_a_quick_route_plans_on_the_highway() {
        assert!(Routing::QUICK.highway());
        for how in BOTH {
            assert!(
                !how.highway(),
                "{how:?} would answer a fewest-jumps claim off a coarse plan"
            );
        }
    }

    /// The walk crosses a gap, and gives up rather than wandering
    ///
    /// Two halves of one rule. Stepping jumps to whatever lands closest to
    /// the boost star the gap ends at, which crosses an ordinary gap in a
    /// scan per jump — and is wrong on its own, because the way across is
    /// sometimes sideways, around a pocket with nothing in it. So a step
    /// that lands no closer is [`None`], which is the caller's cue to
    /// search the gap instead ([`Crossing::Searched`]).
    #[test]
    fn a_walk_crosses_a_gap_or_gives_it_up() {
        // A line to walk, and a decoy: 2 is nearer the goal than anything
        // else in reach of the start, and nothing in reach of *it* lands
        // closer still. The way there is the chain that sets out away from
        // the goal.
        let places: Vec<(i64, [f64; 3])> = vec![
            (1, [0., 0., 0.]),
            (2, [90., 0., 0.]),
            (3, [40., 80., 0.]),
            (4, [120., 120., 0.]),
            (5, [200., 90., 0.]),
            (6, [270., 60., 0.]),
            (7, [300., 0., 0.]),
        ];
        let dir = Scratch::new("stepping");
        let sky = crate::testing::sky(dir.path(), &places);
        let graph = over(&sky, &Boosts::default());
        let node = |address: i64| {
            let (_, place) = places[address as usize - 1];
            graph.node_of(address, place).expect("a placed system")
        };

        // Along the line, every step closing: 1 to 2 is one jump of a
        // hundred light years, and the walk arrives.
        let walked = graph
            .stepped(node(1), node(2), 100., Drive::Unaided, Routing::FEWEST)
            .expect("a gap one jump wide");
        assert_eq!(walked, vec![node(1), node(2)], "the walk wandered");

        // And into the decoy: the best step out of 1 is 2, which is nearer
        // the goal than anything else in reach, and out of 2 nothing closes
        // further — 3 is in reach and points away. Given up, not wandered.
        assert!(
            graph
                .stepped(
                    node(1),
                    node(7),
                    100.,
                    Drive::Unaided,
                    Routing::FEWEST,
                )
                .is_none(),
            "the walk claimed a gap it cannot step across",
        );
    }

    /// A gap is walked on what the route is weighed by
    ///
    /// The defect this fixes: the walk took whatever landed nearest the
    /// far cone, which inside the sphere a jump reaches is the *longest*
    /// jump in it — and fuel goes as the square of a jump, so a
    /// supercharged least-fuel crossing's gaps were flown at full range
    /// with no regard for fuel whatever. Measured over `.index/full`, Sol
    /// to Colonia came back with the same 166 stops and the same 157.41
    /// tanks at every position of the hop rail; scored on fuel it spends
    /// 142.21 to 148.28. See [`JumpGraph::stepped`].
    ///
    /// A line of stepping stones twenty light years apart, and a ship that
    /// jumps a hundred: counted in jumps the walk takes the full hundred
    /// every time, and weighed by fuel it takes the stones. **And the top
    /// of the hop rail is the fewest-jumps walk again**, which is the same
    /// thing that rail's two ends mean everywhere else: a hop of the whole
    /// range prices a jump at a tankful, so splitting stops paying.
    #[test]
    fn a_gap_is_walked_on_what_the_route_is_weighed_by() {
        let places: Vec<(i64, [f64; 3])> =
            (0..=15).map(|k| (k + 1, [k as f64 * 20., 0., 0.])).collect();
        let dir = Scratch::new("stepping-fuel");
        let sky = crate::testing::sky(dir.path(), &places);
        let graph = over(&sky, &Boosts::default());
        let node = |address: i64| {
            let (_, place) = places[address as usize - 1];
            graph.node_of(address, place).expect("a placed system")
        };
        // The far cone is the end of the line, 300 light years out.
        let walked = |weigh: Weigh| {
            graph
                .stepped(
                    node(1),
                    node(16),
                    100.,
                    Drive::Unaided,
                    Routing::at(95, weigh),
                )
                .expect("a gap of stepping stones")
                .iter()
                .map(|&hop| graph.address(hop))
                .collect::<Vec<_>>()
        };

        // Nearest the far cone, which is a hundred light years at a time.
        assert_eq!(
            walked(Weigh::Jumps),
            vec![1, 6, 11, 16],
            "the jump-counted walk stopped short of its range",
        );
        // And the price at the top of the rail is that same walk: a jump
        // at the range costs a tankful, so a shorter one saves nothing
        // worth the extra jump.
        assert_eq!(
            walked(Weigh::Fuel { hop: 100, expand: 0 }),
            vec![1, 6, 11, 16],
            "a tankful-priced hop split its jumps anyway",
        );
        // Unpriced, every stone: fuel goes as the square of a jump, so
        // five twenties cost a fifth of what one hundred costs.
        assert_eq!(
            walked(Weigh::Fuel { hop: 0, expand: 0 }),
            (1..=16).collect::<Vec<_>>(),
            "the fuel-weighed walk jumped further than it had to",
        );
    }

    /// A gap is never crossed better than the route asked for
    ///
    /// The rule the settings rest on, and the reported confusion behind it:
    /// an optimality is a claim about *the route*, so paying to prove one
    /// gap exact inside a plan that bounds nothing buys nothing anybody
    /// asked for. Measured, it never bought a jump either. So both ways of
    /// crossing a gap search at the route's own optimality — the walk falls
    /// back to exactly what the other setting does.
    ///
    /// Which is also why there is nothing to plan at 100%: an optimality
    /// that has to be *true* leaves only the flat search, and
    /// [`Routing::approximates`] is the one gate that says so.
    #[test]
    fn a_gap_is_crossed_at_the_routes_own_optimality() {
        for crossing in [Crossing::Stepped, Crossing::Searched] {
            let tune = Tuning { crossing, ..Tuning::default() };
            // Nothing in the settings can turn an approximation on where
            // the route did not ask for one.
            assert!(
                !Routing::FEWEST.approximates(),
                "{tune:?} would approximate a proven route",
            );
            assert!(!Routing::FEWEST.highway(), "a proven route was planned");
            assert_eq!(
                Routing::FEWEST.weight(),
                WHOLE,
                "a proven route was weighted"
            );
        }
    }

    /// A search told to give up stops where it is, route or no route
    ///
    /// What makes a click on the plot button over a route being searched
    /// mean anything. The task is dropped when its leg is cancelled, and a
    /// dropped task whose body the pool has already begun goes on running:
    /// a route walk is one long stretch of arithmetic with nothing to
    /// await, so a cancelled galactic crossing would burn a pool thread for
    /// its full ten minutes with nobody left to read the answer.
    ///
    /// The same sky either way, and a route there plainly is: the one that
    /// was told to give up comes back with nothing, and having expanded
    /// nothing.
    #[test]
    fn a_search_told_to_give_up_stops_where_it_is() {
        let entries: Vec<NameEntry> =
            (0..=50).map(|k| at(k, [50. * k as f32, 0., 0.])).collect();
        let (_dir, sky) = galaxy("stopped", &entries);
        let graph = over(&sky, &Boosts::default());
        let (start, goal) = (end(&entries, 0), end(&entries, 50));
        let watched =
            || Frontier::between(DVec3::ZERO, DVec3::new(2_500., 0., 0.));

        let flown = watched();
        assert!(
            graph
                .route(
                    start,
                    goal,
                    50.,
                    Routing::FEWEST,
                    Drive::Unaided,
                    Tuning::default(),
                    Some(&flown),
                )
                .is_some(),
            "there is a route to find over this sky"
        );

        let taken_back = watched();
        taken_back.abandon();
        assert!(
            graph
                .route(
                    start,
                    goal,
                    50.,
                    Routing::FEWEST,
                    Drive::Unaided,
                    Tuning::default(),
                    Some(&taken_back),
                )
                .is_none(),
            "a search that was taken back answered anyway"
        );
        assert_eq!(
            taken_back.expanded(),
            0,
            "it expanded systems after being told to stop"
        );
    }

    /// A long supercharged route is flown from cone to cone
    ///
    /// Fifty systems in a line, every fourth of them a neutron star, and
    /// two and a half thousand light years end to end — past
    /// [`LONG_ROUTE_LY`], so the route is planned on the cones and each hop
    /// of the plan flown by the ordinary search.
    ///
    /// What it must come back with is a route and not a plan: the ends the
    /// caller asked for, every leg inside the reach of the system it leaves
    /// — a boosted jump only out of a system with a cone — and the hops in
    /// between the cones it planned through. The unaided answer over the
    /// same sky is the ship's own fifty jumps, which is what the cones save.
    #[test]
    fn a_long_supercharged_route_is_flown_from_cone_to_cone() {
        let entries: Vec<NameEntry> =
            (0..=50).map(|k| at(k, [50. * k as f32, 0., 0.])).collect();
        let chain: Vec<(i64, Boost)> =
            (0..=48).step_by(4).map(|k| (k, Boost::Neutron)).collect();
        let boosts = cones(&entries, &chain);
        let (_dir, sky) = galaxy("cones", &entries);
        let graph = over(&sky, &boosts);
        let (start, goal) = (end(&entries, 0), end(&entries, 50));
        let drive = Drive::Standard;

        let charged = graph
            .route(
                start,
                goal,
                50.,
                Routing::QUICK,
                drive,
                Tuning::default(),
                None,
            )
            .expect("a supercharged route");
        let unaided = graph
            .route(
                start,
                goal,
                50.,
                Routing::QUICK,
                Drive::Unaided,
                Tuning::default(),
                None,
            )
            .expect("an unaided route");

        assert_eq!(charged.first().expect("a start").0, 0, "started elsewhere");
        assert_eq!(charged.last().expect("an end").0, 50, "ended elsewhere");
        assert_eq!(unaided.len() - 1, 50, "the ship's own fifty jumps");
        assert_eq!(
            charged.len() - 1,
            13,
            "twelve hops between the cones and the approach off the last",
        );
        for hop in &charged[1..charged.len() - 1] {
            assert_eq!(
                hop.0 % 4,
                0,
                "S{} is no jet cone, so the route stopped for nothing",
                hop.0
            );
        }
        for leg in charged.windows(2) {
            let flown = dist2(leg[0].1, leg[1].1).sqrt();
            let reach = 50.
                * drive.factor(
                    graph
                        .node_of(leg[0].0, leg[0].1)
                        .and_then(|node| graph.boost(node)),
                );
            assert!(
                flown <= reach,
                "a leg of {flown:.0} ly out of S{} reaching {reach:.0}",
                leg[0].0
            );
        }
    }

    /// A short supercharged route is searched flat, plan or no plan
    ///
    /// Under [`LONG_ROUTE_LY`] the flat search is quick *and* exact, so
    /// there is nothing a coarse plan could win and a proof to lose. The
    /// gap here is one only the cone crosses, and the route across it is
    /// the same three systems whatever setting asks for it — which is the
    /// answer a plan over one cone could not have bettered.
    #[test]
    fn a_short_supercharged_route_is_searched_flat() {
        let entries = vec![
            at(1, [0., 0., 0.]),
            at(2, [90., 0., 0.]),
            at(9, [440., 0., 0.]),
        ];
        let boosts = cones(&entries, &[(2, Boost::Neutron)]);
        let (_dir, sky) = galaxy("flat", &entries);
        let graph = over(&sky, &boosts);
        let (start, goal) = (end(&entries, 1), end(&entries, 9));

        for how in [Routing::QUICK, Routing::FEWEST, Routing::SHORTEST] {
            let path = graph
                .route(
                    start,
                    goal,
                    100.,
                    how,
                    Drive::Standard,
                    Tuning::default(),
                    None,
                )
                .expect("a supercharged route");
            assert_eq!(
                path.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
                vec![1, 2, 9],
                "{how:?} did not go by way of the neutron star"
            );
        }
    }
}
