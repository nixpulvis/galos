// TODO: reorganize. This measures a local `GALOS_PERF_DIR` and passes
// silently without one. Revisit alongside galos_index's perf tests when
// setting up proper criterion benchmarks.

//! What a plotted route must not get slower at, measured against a real
//! directory.
//!
//! The router reads the galaxy's places out of the cell payloads and plans
//! over the boost stars, so anything that changes how either is held lands
//! on it first.
//!
//! A unit-test module rather than a file in `tests/`: it measures
//! [`Routing`], [`Tuning`], [`Weigh`] and [`Drive`], every one of them
//! `pub`, which an integration test could reach only by widening the
//! router's API for the sake of a test's file location. The zoom half of
//! what used to be one guard is an integration test of `galos_index`
//! (`galos_index/tests/zooming.rs`), everything it touches being that
//! crate's public surface.
//!
//! Stands down without `GALOS_PERF_DIR` naming a built index directory:
//!
//! ```sh
//! GALOS_PERF_DIR=.index/full cargo test -p galos_route --lib perf -- --nocapture
//! ```
//!
//! The clocks are loose, this running on whatever machine is to hand, so
//! what they catch is a change of *shape*. The stop counts are not loose,
//! and [`plotted`] says why.
//!
//! ## Measured 2026-09-13, the supercharged crossing
//!
//! Against `.index/full` — 200,071,629 systems — on an Apple M5 Pro,
//! release: opening 33 ms, the guard's own thousand light year route 50 ms
//! either setting, and Sol to Colonia at 50 ly with a standard drive
//!
//! | | jumps | found in |
//! |---|---|---|
//! | `Quick`, on the highway | 139 | 6.5 s warm, 7.6 s cold |
//! | `Direct`, searched flat | 137 | **610 s** |
//!
//! The cold figure is the first charged route of a session, which pays for
//! placing the 3,846,802 boost stars. Of the warm 6.5 s, the coarse plan
//! is 4.4–4.8 s (116 waypoints, 178,910 expansions) and its 117 legs are
//! 2.0–2.3 s. See [`crate::highway`].
//!
//! ## Measured again, the place published beside the cone
//!
//! The first charged route of a session used to pay 4.9–7.9 s before its
//! first expansion: the supercharge table was addresses, and finding where
//! four million cones sat meant walking the names table's address column.
//! The place is in the published row now
//! ([`galos_index::SystemBoost`]) and the highway is a 213 ms sort, so a
//! cold click costs what a warm one does:
//!
//! | | jumps | before | after |
//! |---|---|---|---|
//! | first charged crossing, 50 ly | 139 → 140 | 9.0–12.3 s | **2.2 s** |
//! | second, 50 ly | 139 → 140 | 6.2–6.5 s | 2.0 s |
//! | first charged crossing, 80 ly | 80 → 81 | 8.6 s | **0.8 s** |
//!
//! The second half of that is the plan's own estimate, leaned on by the
//! twentieth the setting already leans its flat one by: one jump in a
//! hundred and forty, and 3.3× at 50 ly against 13× at 80. And the plan is
//! drawn while it runs now
//! ([`crate::graph::Sampler::reached`]) — it used to be
//! seconds of empty sky before the legs began. See
//! [`crate::highway::Highway::plan`].
//!
//! ## Measured again, what a ship that jumps half as far costs
//!
//! The reported trouble: Sol to Colonia at **25 ly** took 3 min 57 s for
//! 316 jumps, where EDDA plots the same crossing in **284 ms** for 331.
//! The cause was not a slow plan — it was no plan at all. A coarse hop was
//! one supercharged jump plus four ordinary ones, which is 400 ly at 50 ly
//! of range and **200 ly at 25**, and at 200 ly the boost stars are not a
//! connected graph: the plan died in 0.3 ms and the caller fell back to the
//! flat galaxy-wide search. So the hop's reach is a distance now
//! ([`crate::graph::Tuning::reach`], default 400 ly, the
//! measured threshold — 350 fails at *both* ranges), the leaning is a
//! setting, and a leg is refined leaned rather than proven by default
//! (measured: the same jumps, and faster).
//!
//! | crossing | before | after |
//! |---|---|---|
//! | 25 ly | 316–317 jumps in **233 s** | 332 jumps in **4.8 s** |
//! | 50 ly | 140 jumps in 2.2 s | 140 jumps in 2.0 s |
//! | 80 ly | 81 jumps in 0.69–0.78 s | 81 jumps in **0.18 s** |
//!
//! Which is EDDA's own answer to a jump — 332 against 331 — at seventeen
//! times its wall clock rather than eight hundred and thirty. What is left
//! of the gap is theirs by design and measured in their own comments: a
//! coarse weight of 1.5 against our 1.05 (`long_range.rs:52-57`), bucket
//! thinning, a goal cone, a hard 30,000-expansion cap, leg refinement that
//! is arithmetic rather than a search, and a rayon portfolio of eleven to
//! twenty-three variants raced under a grace clock — so their wall clock is
//! the *fastest* variant and their answer the *best* of them.
//!
//! ## Measured again, a gap stepped across rather than searched
//!
//! Reported: EDDA plots Colonia to Sgr A\* in **50 s** where the same
//! corridor cost us minutes. Instrumented, the wait was not spread over the
//! plan's hundred-odd gaps — it was **one** of them: 17.93 s of an 18.97 s
//! route, and the expensive ones are all near the core, where a
//! supercharged jump sees thousands of systems and an A\* frontier is
//! quadratic in that.
//!
//! So a gap — the stretch between the boost star the ship stands on and the
//! next one the plan named — is now crossed by jumping to whatever lands
//! closest to that next star, again and again, and searched only where no
//! jump lands closer. EDDA's `bridge_leg` (`long_range.rs:299-410`). What
//! that costs and buys:
//!
//! | crossing | stepped | searched | jumps |
//! |---|---|---|---|
//! | Sol → Colonia, 50 ly, 5% | 141 in **0.30 s** | 140 in 2.60 s | +1 |
//! | Sol → Colonia, 25 ly, 5% | 334 in **0.84 s** | 332 in 6.19 s | +2 |
//! | Sol → Colonia, 80 ly, 5% | 82 in 0.12 s | 81 in 0.14 s | +1 |
//! | Colonia → Sgr A\*, 45 ly, 25% | 81 in **0.97 s** | 80 in 19.84 s | +1 |
//!
//! One jump in a hundred and forty for eight to twenty times the speed, so
//! it is the default ([`crate::graph::Crossing::Stepped`]).
//! The guard's own rows moved with it: the 50 ly crossing is **0.33 s warm
//! and 0.58 s cold** against 2.0–2.2 s, and the 25 ly crossing **0.89 s**
//! against 4.8 s — where before any of this it was 233 s.
//!
//! ## Measured again, a stalled plan handed over rather than dropped
//!
//! Reported: Colonia to Sgr A\* at 45 ly came back in **776 ms at 25% over**
//! and in **4 min 05 s at 5%** — a cliff where the setting is meant to be a
//! dial. Instrumented, the five percent plot spent *no time planning at
//! all*: the coarse search hit its stall allowance, answered nothing, and
//! the whole wait was the flat galaxy-wide fallback. A nearly admissible
//! coarse estimate expands the plateau of boost stars, and near the core
//! that plateau is enormous.
//!
//! So a stalled plan hands over the chain to the closest cone it reached
//! and the stretch left over becomes one more gap to cross — EDDA's rule
//! (`long_range.rs:3273-3281`). A search that closed on *nothing* still
//! answers nothing, which is what the flat fallback is for.
//!
//! | | before | after |
//! |---|---|---|
//! | Colonia → Sgr A\*, 45 ly, 5% | 74 jumps in **226.89 s** | 73 jumps in **8.53 s** |
//! | Colonia → Sgr A\*, 45 ly, 25% | 81 jumps in 0.97 s | 81 jumps in 0.96 s |
//!
//! Twenty-seven times faster *and* a jump better, and the setting is a dial
//! again: eight jumps between the two ends of it for nine times the wait.
//! What the five percent plot now pays is the stall allowance itself — 8.5 s
//! of coarse search before it gives up and flies what it has, which is
//! [`crate::graph::Tuning::stall`] and the next thing to
//! measure.
//!
//! ## Measured once, what the drawing does through a plan's legs
//!
//! The chain drawn, polled every 20 ms through one 50 ly crossing, before
//! the legs were told what they are legs of:
//!
//! ```text
//!  266 ms  chain 116  cells 85  closest 396   the plan, drawn whole
//!  338 ms  chain   2  cells 85  closest 148   the first leg, alone
//!  1.68 s  chain   6  cells 85  closest   9
//!  2.22 s  FINISHED; answer landed at 2.20 s
//! ```
//!
//! A leg is its own search with its own parent map, so the chain it can
//! hand over is its own two to six jumps — the crossing replaced by a stub
//! for the two seconds the legs take, while the closed set stayed up. It
//! read as the picture being taken down before the answer arrived, and the
//! handoff itself is not the fault: `finished` and the answer are 20 ms
//! apart, inside a frame.
//!
//! The plan is drawn whole now and each leg is a strand beside it
//! ([`crate::graph::Sampler::planned`],
//! [`crate::graph::Sampler::flew`]). The same crossing,
//! measured again:
//!
//! ```text
//!  288 ms  strands  1  links 116  cells 85   the plan's own search
//!  338 ms  strands  1  links 118  cells 85   the plan, resolved
//!  485 ms  strands  2  links 123  cells 85   the first leg refined off it
//!  1.71 s  strands  3  links 130
//!  2.12 s  strands 10  links 150
//!  2.24 s  FINISHED; answer landed at 2.24 s
//! ```
//!
//! Also why the two sampled layers stand still through all of it: the
//! closed-set grid is sized once from the route's own length
//! ([`crate::graph::CELLS`]), which over 22 kly is
//! 1,100 ly a cell — and a refinement leg is 200–600 ly, so a whole leg
//! search falls inside one cell. `cells 85` from 239 ms to the end is not a
//! search standing still; it is a picture drawn at the wrong scale to see
//! one.
//!
//! ## Measured again, what the exact settings cost
//!
//! Sol to a system three thousand light years out, `Direct`, over the same
//! directory:
//!
//! | | jumps | before the cell ledger | after |
//! |---|---|---|---|
//! | unaided | 62 | 832 ms | 600–680 ms |
//! | supercharged | 32 | 124.0 s | **72 s** |
//!
//! Same expansions and same relaxations either way — 2,635,464 and
//! 3,893,601 for the charged one — so what the ledger took out is work
//! that changed no answer. What is left is the expansion count itself: two
//! hundred times the unaided search's for a shorter route, which is the
//! estimate having to divide by the widest jump the drive could make. A
//! goal field that knows where the cones are not is what attacks that.
//!
//! ## Measured over the columnar directory, and what the guard had missed
//!
//! Against `.index/full` rewritten columnar — 200,071,629 systems, 24 bytes
//! a row against 41, payloads 8.2 GB against 4.6 — the charged rows came
//! back with **451 stops at 50 ly and 919 at 25**, against the 141 and 334
//! recorded. It read as the coarse plan having stopped engaging under the
//! new layout. It had not: the rows themselves asked for `Drive::Unaided`,
//! a drive with no cone to charge off, so
//! [`crate::graph::JumpGraph::route`] never reached the
//! highway at all and the flat galaxy-wide search answered. 451 and 919 are
//! that search's own counts. Asked with `Drive::Standard`, over the same
//! columnar directory:
//!
//! | crossing | stops | found in |
//! |---|---|---|
//! | Sol → Colonia, 50 ly, 95% | 141 | 452 ms cold, 291 ms warm |
//! | Sol → Colonia, 80 ly, 95% | 82 | 180 ms |
//! | Sol → Colonia, 25 ly, 95% | 334 | 824 ms |
//! | Sol → 3 kly, optimal, unaided | 62 | 702 ms |
//! | Sol → 3 kly, optimal, charged | 32 | 82.8 s |
//!
//! Which is every row unmoved by the columnar rewrite, and the reading the
//! guard should have taken. What let it pass was the assertion: the rows
//! measured `plotted < 120 s` and the flat fallback is seconds, so tripling
//! the answer moved nothing the guard looked at. The count is asserted now
//! ([`plotted`]) — a route that comes back three times as long, or does not
//! come back at all, is the failure whatever the clock says.
//!
//! ## Measured once, a route reported as going off at a bad angle
//!
//! Reported over Sagittarius A\* to Quemaae JN-K D8-3 — 7,201 light years
//! — plotted for least fuel over 50% hops: the line runs straight for most
//! of the way, wanders wide, and closes back on the goal at the end.
//!
//! **The wandering is paid for, which is the answer to the report.**
//! Measured against the coarse graph's own prices, over every cone inside
//! a corridor of the straight line — and Sagittarius A\* has no jet cone
//! within 392 light years, so every chain pays the same eight-jump start
//! bridge:
//!
//! | chain | jumps | strays |
//! |---|---|---|
//! | the plan | 47 | 621 ly |
//! | best inside ±100 ly | 59 | ≤100 ly |
//! | best inside ±200 ly | 51 | ≤200 ly |
//! | best inside ±800 ly | 47 | ≤800 ly |
//!
//! So the bow is where the boost stars are. Confining the route to a
//! corridor a fifth as wide costs four jumps, and what the picture shows
//! is the chain of cones rather than a search that lost its heading.
//!
//! **What the same probe did find is that the plan never read the ask.** A
//! least-fuel plot and a fewest-jumps plot came back byte for byte the
//! same route — 48 stops, the same 621 ly — because the coarse search
//! did not look at [`crate::graph::Weigh`] at all. For
//! least fuel that is the *right* answer and the cap is why: a boosted
//! jump costs a whole tank however far the cone throws the ship, so on a
//! chain of cones the fuel is the hop count. For the shortest of the
//! fewest jumps it was not an answer at all — the chain came back as
//! whichever of that many the boost table's ordering reached first. The
//! coarse cost carries the distance flown for that one ask now
//! (`highway::Coarse`). Measured on this corridor, all three at 95%:
//!
//! | ask | stops | flown | strays | planned in |
//! |---|---|---|---|---|
//! | fewest jumps | 48 | 7,749 ly | 621 ly | 0.31 s |
//! | least fuel over 50% hops | 48 | 7,749 ly | 621 ly | 0.18 s |
//! | **shortest** | 49 | **7,593 ly** | **543 ly** | 0.50 s |
//!
//! A hundred and fifty-six light years shorter for one jump more, which is
//! the leaning's own slack rather than the ordering's doing: at 95% neither
//! 48 nor 49 is proven, and the distance key changes which of the two the
//! coarse search pops first. The other two asks are byte for byte the
//! routes they were.
//!
//! ## Measured again, a least-fuel route that jumped too far to begin with
//!
//! Reported: Sol to Col 285 Sector ZQ-K C9-12, 186 light years apart, at a
//! 100 ly range and least fuel over *unpriced* hops. Optimal gives 35
//! stops and no jump over 16.3 ly; 95% optimality gave 15 stops with a
//! 50.9 ly jump out of the door, and the long ones were all at the
//! beginning.
//!
//! The weighted estimate was not the cause and could not have been: at a
//! hop of nothing the fuel left to burn has no positive lower bound, so
//! the estimate is zero, and weighting zero is zero. Confirmed by removing
//! the fanout cap, which left 95% answering the optimal route *exactly*.
//! The cap was the whole of it — it kept the 512 candidates nearest the
//! goal, which inside a 100 ly sphere are the longest jumps in it, and fuel
//! goes as the square of a jump. It showed only at the start because the
//! cap bites where the sky is dense, which is the bubble the route sets out
//! from.
//!
//! What the cap keeps now is the cheapest candidates per light year of
//! ground closed, taken off the metric's own price
//! ([`crate::graph::thinned`]) — which for a route counted
//! in jumps is the nearest the goal, the same set as before, and for one
//! weighed by fuel is emphatically not:
//!
//! | ask | stops | flown | longest | fuel | found in |
//! |---|---|---|---|---|---|
//! | optimal, unpriced hops | 35 | 235.2 ly | 16.3 ly | 0.192 | 19.3 s |
//! | 95%, before | 15 | 217.5 ly | 50.9 ly | 0.611 | 0.10 s |
//! | **95%, after** | 35 | 235.2 ly | 16.3 ly | **0.192** | **1.05 s** |
//! | optimal, 50% hops | 5 | 186.9 ly | 49.1 ly | 0.876 | 6.5 ms |
//! | 95%, 50% hops | 5 | 186.3 ly | 55.3 ly | 0.887 | 1.9 ms |
//!
//! So the setting is honest again and it is *eighteen times* faster than
//! proving the same answer: what the cap throws away is now the expensive
//! half of the sphere, which is the half a least-fuel route was never in.
//! The guard's own jump-counted rows are unmoved — 141, 82, 334, 62 and 32
//! stops — a jump costing the same wherever it lands, which is what makes
//! nearness the right order for them.
//!
//! ## Measured again, a search going back over ground near the start
//!
//! Reported from watching the layers: a route whose far end has long since
//! stopped changing, still expanding near where it set out. Some of that
//! is inherent — A\* pops from all over its frontier, and the closed-set
//! layer only ever grows, so old ground stays lit whether or not anything
//! is happening in it. The rest was **re-expansion**, and it was most of
//! one search. Instrumented over `.index/full`, Sol to a system three
//! thousand light years out, unaided, at 95% optimality:
//!
//! | ask | pops | of those re-expansions | found in |
//! |---|---|---|---|
//! | fewest jumps | 5,876 | 0 | 0.42 s |
//! | least fuel, unpriced hops | 5,156 | 0 | 0.99 s |
//! | shortest of the fewest | 34,138 | **25,258** | **2.04 s** |
//! | shortest, expanded once | 20,402 | 0 | **0.57 s** |
//!
//! Three quarters of the shortest search was ground it had already
//! covered, and expanding each system once gives the same 45 stops over
//! 2,132.6 light years against 2,135.0 — a shorter route for a quarter of
//! the wait. Only that ask reopens at all: it is the distance component of
//! `Cost` that the weighting makes inconsistent, where a jump count
//! weighted stays consistent by a jump. So a route that has not promised
//! the fewest now expands each system once
//! ([`crate::graph::JumpGraph::walk`]), which keeps the
//! same `1 + over/100` bound — weighted A\* without re-expansion is
//! `ARA*`'s own argument, and the claim that the bound *needed* reopening
//! was wrong.
//!
//! And what the dial itself is worth on the same route, which is the other
//! half of the answer: **89 pops at 75% against 5,876 at 95%**, for the
//! same 45 stops in 6.1 ms against 0.42 s. A weight of 1.05 is A\* with a
//! nudge; the setting's useful range is nowhere near the top of it.
//!
//! ## Measured once, why two exact least-fuel plots are not comparable
//!
//! Asked because a 45 ly least-fuel plot came back in 4.8 s and read as
//! the exact setting having got faster. It has not: nothing above touches
//! it, every approximation being gated on
//! [`crate::graph::Routing::approximates`], and the
//! guard's own exact rows sat at 0.66–0.81 s and 81–88 s throughout, from
//! before the first of these changes to after the last.
//!
//! What varies is the corridor. The same 702 light years out of Sol
//! towards the core, least fuel over unpriced hops:
//!
//! | range | optimal | 95% |
//! |---|---|---|
//! | 45 ly | 243 stops, 941.9 ly, **59.1 s** | 228 stops, 916.4 ly, 57.5 s |
//! | 100 ly | 222 stops, 918.5 ly, **156.9 s** | 223 stops, 922.0 ly, 61.1 s |
//!
//! Two and a half times the wall clock for twice the range, the sphere a
//! jump reaches holding eleven times the systems — and twelve times the
//! wall clock of the reported 4.8 s plot at the same range and the same
//! distance, because that corridor is sparser: 91 jumps averaging 7.7
//! light years against 242 averaging 3.9. An exact least-fuel search
//! splits every jump the star field lets it, so what it costs is a fact
//! about how many stars are in the way.
//!
//! The 45 ly rows are also what a cap that no longer misleads looks like:
//! 57.5 s against 59.1 s for an answer within a jump of the proven one,
//! where at 100 ly it is 61.1 s against 156.9 s.
//!
//! ## Measured again, a price with no free jumps in it
//!
//! The fuel figures above were all taken against a price that rounded to
//! thousandths of a tank, which at a 45 ly range charges **nothing** for
//! any jump under about 1.07 light years. A graph with free edges has no
//! positive lower bound on the fuel left to burn, so the ordering is
//! degenerate and "optimal" is optimal with respect to a cost function
//! that gives some jumps away. It showed: the 702 ly row above has a 95%
//! route at 2.144 of a tank against the proven route's 2.148.
//!
//! Priced in hundred-thousandths, with any jump charged at least one unit
//! ([`crate::graph::burned`]), the ordering comes right and
//! the proven routes come out *cheaper* than they did:
//!
//! | corridor | proven, before | proven, after | 95% after, all in range |
//! |---|---|---|---|
//! | Col 285, 100 ly | 35 stops, 0.1910 | 36 stops, **0.1908** | 0.1908 |
//! | 186 ly, 45 ly | 54 stops, 0.7510 | 55 stops, **0.7506** | 0.7506 |
//! | 702 ly, 45 ly | 243 stops, 2.1480 | 231 stops, **2.1375** | 2.1387 |
//!
//! Proven is now at or under every approximate row on every corridor,
//! which is the property the old price could not hold. It costs nothing in
//! time: 18.1 s, 2.66 s and 54.0 s against 18.9, 2.77 and 56.6.
//!
//! And the expand-nearest curve re-measured against it, at 95%, which is
//! the one the slider would be geared to. Unchanged in shape:
//!
//! | neighbours expanded | Col 285, 100 ly | 186 ly, 45 ly | 702 ly, 45 ly |
//! |---|---|---|---|
//! | nearest 8 | +0.2%, **0.25 s** | +4.9%, **0.15 s** | +2.6%, **6.9 s** |
//! | nearest 64 | +0.2%, 0.66 s | +1.8%, 0.36 s | +1.2%, 14.4 s |
//! | all in range | +0.0%, 0.99 s | +0.0%, 2.68 s | +0.1%, 52.6 s |
//! | proven | — 18.1 s | — 2.66 s | — 54.0 s |
//!
//! So 64 is the default the measurement supports: within 1.8% of the fuel
//! at three to twenty-seven times the speed of proving it, where the
//! present 512 buys the last two percent of a percent for all of the wait.
//!
//! ## Measured 2026-09-16, the plan's weight taken off the route's
//!
//! The coarse plan multiplied its estimate by
//! [`crate::graph::Routing::over`], so a reader asking for
//! a route within five percent was also asking for a plan leaned by five
//! and could not ask for the exact plan at all. It is
//! [`crate::graph::Tuning::planning`] now, with a rail of
//! its own in the planning fold and exact at the top of it.
//!
//! What exact is worth, end to end from Sol at 45 ly with a standard
//! drive, the plan leaned by the route's own percent against the plan
//! exact with no allowance on it:
//!
//! | corridor | 95%, leaned | 95%, exact | 80%, leaned | 80%, exact |
//! |---|---|---|---|---|
//! | Colonia | 158 stops, 563 ms | **156**, 2.23 s | 166 stops, 98 ms | **156**, 1.90 s |
//! | 16 kly out | 137, 697 ms | 137, 1.42 s | 137, 306 ms | 137, 1.06 s |
//! | 22 kly out | 172, 1.31 s | **170**, 2.99 s | 175, 292 ms | **170**, 2.00 s |
//!
//! So **nothing to six percent of the stops for two to nineteen times the
//! wait**, and the coarse search is where all of it goes: at 80% the plan
//! alone is 2,048 expansions in 54 ms leaned against 97,792 in 1.92 s
//! exact on Colonia, 37,376/300 ms against 140,800/794 ms at 16 kly, and
//! 33,280/278 ms against 276,480/1.78 s at 22 kly.
//!
//! And the case the exact plan is *for*: a corridor whose cone plateau is
//! small settles it in 512 expansions, where the whole route comes back in
//! 4.27 ms exact against 4.02 ms leaned — 3 kly out at a 495 Ly gap, warm,
//! the same 40 stops. Every corridor sampled is a factor of forty-eight
//! either side of that, so exact is *tried* rather than promised:
//! [`crate::graph::Tuning::allowance`] drops an exact pass
//! that has spent 2,048 expansions — 40 ms at some 20 µs apiece — and the
//! plan is then worked leaned exactly as it was.
//!
//! A plan that stalls is not re-planned either way: 3 kly out at a 405 Ly
//! gap stalls at 200,192 expansions and hands over the same 8 cones
//! whatever the weight, and the stall allowance is a hundred times the
//! exact pass's.
//!
//! ## Measured again, a gap crossed on what the route is weighed by
//!
//! `Crossing::Nearest` stepped to whatever landed closest to the next cone
//! and never looked at fuel — so on a *planned* crossing the hop rail was
//! inert, every position of it coming back with the same route. Measured
//! at 45 ly with a standard drive, the crossing's own fuel:
//!
//! | crossing | hop | nearest | on fuel |
//! |---|---|---|---|
//! | Sol → Colonia | 50% | 166 stops, 157.41 tanks | 185 stops, **148.28** |
//! | | 25% | 166, 157.41 | 218, **143.76** |
//! | | unpriced | 166, 157.41 | 336, **142.21** |
//! | Sol → 16 kly out | 50% | 137 stops, 127.65 tanks | 155 stops, **119.11** |
//! | | 25% | 137, 127.65 | 187, **114.73** |
//! | | unpriced | 137, 127.65 | 298, **113.54** |
//!
//! One number three times over in the third column, which is the defect,
//! against 6 to 11 percent of the tank once the step is priced by the
//! metric the route is weighed by — the same rule
//! [`crate::graph::thinned`] follows one level down. It
//! costs a few percent of the clock (76–113 ms against 99, 325–335 against
//! 304) and 11 to 118 percent more stops, which is the trade the rail is
//! for.
//!
//! A third of the saving is the *arrival*: taking the far cone the moment
//! it came into reach is a jump at full range, which is the dearest jump
//! there is, and closing in on it first is 142.21 tanks against 146.77 on
//! the unpriced Colonia row. The walk is called
//! [`crate::graph::Crossing::Stepped`] now, "nearest"
//! having stopped being true of it.
//!
//! The longest walk any gap took, which is what
//! `JumpGraph::HOPS` is sized against: 11–12 steps at a 50% hop, 19 at
//! 25%, and 44–61 unpriced, against the 32 a jump-counted walk is allowed.
//!
//! ## Measured again, the gap width that was a trap
//!
//! Asked why the gap a plan may string together is never under 400 Ly
//! even for a ship that jumps 10. It is a distance because the cliff is
//! one: flooding the cones said the graph joins up between 360 and 380 Ly
//! out of the bubble, and re-measured on whole plans at 10, 25 and 50 Ly,
//! **350 Ly strings nothing and 380 plans at every range**. The reach is
//! a fact about where the cones are; what the ship changes is only what
//! one hop costs it — a 400 Ly hop is 6 jumps at 45 Ly and **37 at 10**.
//!
//! What that measurement missed is that it flooded *from Sol*, and a plan
//! needs the cones to connect **along a corridor**. Whole routes at 45 Ly
//! with a standard drive:
//!
//! | reach | Sol → 2 kly | Sol → 3 kly | Sol → Colonia | Sol → 16 kly |
//! |---|---|---|---|---|
//! | 405 Ly | 60 stops, **32.3 s** | 67, **50.3 s** | 166, 214 ms | 137, 418 ms |
//! | 450 Ly | 47, 33.6 ms | 54, 8.1 ms | 166, 97 ms | 137, 423 ms |
//! | **495 Ly** | **32, 6.2 ms** | **40, 4.8 ms** | **164, 101 ms** | **136, 473 ms** |
//! | 585 Ly | 32, 5.8 ms | 40, 5.6 ms | 164, 116 ms | 136, 594 ms |
//! | 765 Ly | 31, 6.9 ms | 39, 5.5 ms | 164, 175 ms | 136, 913 ms |
//!
//! **Monotone in stops on every corridor**, which is the shape of the
//! thing: a wider reach only *adds* edges to the cone graph, so it can
//! never cost a route stops — it costs the scan width of one coarse
//! expansion. And what 400 Ly did on the two short corridors was not plan
//! slowly: the search never closed on the goal, spent its stall
//! allowance, handed over the 7 cones it had, and the stretch left over
//! became one enormous gap for the legs. At 10 Ly the same corridor went
//! 495 stops in **896 s** at 400 Ly to 430 in 15.5 s at 450.
//!
//! So the floor is 500 Ly, where the curve flattens, and the plan climbs
//! from there: a chain that does not close on the goal is planned again
//! one ordinary jump wider, up to `RUNGS` = 8 rungs
//! ([`crate::highway::Highway::plan`]). A rung is only
//! paid where the narrower reach had already failed. **And the rail is
//! gone** — no reader can be expected to know which corridor wants which
//! number, and every wrong answer was a cliff.
//!
//! What the wider floor costs the guard's own rows, which are corridors
//! that never needed it: 141 stops in 341 ms warm becomes **138 in
//! 442 ms** at 50 Ly, 334 in 882 ms becomes **328 in 1.31 s** at 25 Ly,
//! and the 45 Ly least-fuel crossing 175 in 407 ms becomes **170 in
//! 645 ms**. Thirty to sixty percent of a plot's milliseconds for two to
//! three percent of the jumps a commander actually flies, which is the
//! trade in the right direction.
//!
//! ## Measured again, the exact plan the allowance refused to pay for
//!
//! `Tuning::allowance` drops an exact coarse plan that has spent 2,048
//! expansions, which is what lets the `Plan` rail open at exact for 40 ms
//! of risk — and on the corridors where it drops one, the chain it drops
//! is a real answer nothing on the form could ask for. Measured at 45 Ly
//! and 80% optimality, the plan leaned by the route's own percent against
//! the bounded try against the same plan paid for:
//!
//! | corridor | leaned | `optimal if cheap` | `optimal` |
//! |---|---|---|---|
//! | Sol → 2 kly | 33 stops, 4.2 ms | 32, 4.2 ms | 32, 4.3 ms |
//! | Sol → Colonia | 164, 187 ms | 164, 98 ms | **154, 2.83 s** |
//! | Sol → 22 kly out | 174, 482 ms | 174, 467 ms | **168, 4.42 s** |
//!
//! Three to six percent of the jumps flown for nine to twenty-nine times
//! the wait, and nothing where the exact plan lands inside the allowance
//! anyway. At 95% optimality the same three corridors read 32/32/32 stops,
//! 156/156/**154**, and 171/171/**168** — the paid column is the same
//! chain either way, the route's own percent having nothing to do with the
//! plan's since `Tuning::planning` split off.
//!
//! So `allowance` is an `Option` and the rail has two exact stops rather
//! than one word for both: `optimal if cheap` tries and may end up leaned,
//! `optimal` pays. The route panel says which it was — an exact plan
//! "where it was cheap" is not the same claim as an exact plan — which is
//! the honesty the single setting could not offer.
//!
//! ## Measured again, what the reach ladder cost where no rung works
//!
//! The ladder tries a wider gap where a plan does not close on the goal,
//! up to eight rungs, and it was unbounded on the corridors where none of
//! them can plan: each rung pays its own stall allowance first. Measured
//! at 45 Ly, the coarse plan alone, rung by rung:
//!
//! ```text
//! far rim [0, 0, -20,000]   0: stalled 5,460 Ly, 6.19 s
//!                           1: stalled 4,574 Ly, 7.74 s
//!                         2-8: stalled 4,574 Ly, 48 s for nothing
//! under the disc [0, -8,000, 0]
//!                           0: stalled 6,090 Ly, 1.40 s
//!                         1-8: stalled 6,090 Ly, 20 s for nothing
//! Sol → Colonia             0: chain of 131, 89.7 ms
//! ```
//!
//! Seven rungs of the far rim and eight of the corridor under the disc
//! came back at **exactly the same distance** as the rung below them —
//! wider hops over a chain of cones that does not reach the goal at any
//! width. So a rung that comes no closer ends the climb, which is the
//! same progress rule `Tuning::stall` is one level down:
//!
//! | corridor | before | after | chain |
//! |---|---|---|---|
//! | far rim | 63.1 s | **20.9 s** | 84 cones, unchanged |
//! | under the disc | 21.9 s | **3.04 s** | 19 cones, unchanged |
//! | Sol → Colonia | 89.4 ms | 90.8 ms | 131 cones, off rung zero |
//!
//! Three to seven times less waiting for the same answer, and nothing at
//! all where the plan closes — which is every corridor a commander
//! actually plots.
//!
//! ## Measured again, a cap that bit where it was not drawn
//!
//! Two questions about `Expand nearest`: whether its `8` stop is worth
//! keeping, and what the count it leaves behind does at the positions of
//! the trade where the rail is not drawn. Least fuel at 45 Ly, unaided,
//! 95% optimality:
//!
//! | corridor | nearest 8 | nearest 64 | nearest 1024 | all in range |
//! |---|---|---|---|---|
//! | 186 ly out | 51 stops, 0.743 t, 121 ms | 53, 0.727, 326 ms | 56, 0.709, 2.27 s | 56, 0.709, 2.24 s |
//! | 700 ly out | 216, 2.221, 6.87 s | 233, 2.184, 14.7 s | 241, 2.164, 55.6 s | 241, 2.164, 55.1 s |
//!
//! **The `8` stop stays.** Two percent of the tank for two to three times
//! the speed is the same shape as the rest of the rail, and 1024 against
//! `all` is the last hundredth of a percent for nothing — which is what
//! the rail's last two stops have always said.
//!
//! **And the count it leaves behind was not inert.** `ui::traded` carries
//! the cap along so a trip down the rail and back does not lose it, and
//! `Routing::fanout` was reading it for *every* fuel-weighed ask — at hops
//! where the form does not draw the control:
//!
//! | ask | the nearest 64 | the `FANOUT` valve |
//! |---|---|---|
//! | 186 ly, hop 5% | 47 stops, 0.733 t, 206 ms | 49, **0.716**, 487 ms |
//! | 186 ly, hop 25% | 15, 1.251, 17.8 ms | 15, 1.251, 19.0 ms |
//! | 186 ly, hop 50% | 9, 2.019, 0.59 ms | 9, 2.019, 0.69 ms |
//! | 700 ly, hop 5% | 197, 2.225, 6.76 s | 199, **2.206**, 11.6 s |
//!
//! From a quarter of the range up it is inert, which is what the form was
//! told when it stopped drawing the rail there and what made the leak so
//! quiet — but under that it decided **two percent of the tank** with
//! nothing on screen to say so. The cap is the reader's where the rail is
//! drawn and the fixed valve everywhere else now, so what the number means
//! is what the form shows.

#![cfg(test)]

use crate::Boosts;
use crate::graph::{
    Drive, EXPAND, Frontier, JumpGraph, Routing, Tuning, Weigh,
};
use galos_index::{FsSource, Source as _};
use glam::DVec3;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The directory to measure against, or [`None`] to stand down.
fn measured() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("GALOS_PERF_DIR").ok()?);
    if !dir.join("index.bin").exists() {
        eprintln!("{}: not a built index; standing down", dir.display());
        return None;
    }
    Some(dir)
}

/// What a plotted row must come back with: a route, no longer than the one
/// it was measured at, inside its clock.
///
/// **The count is the assertion a clock cannot make.** Every charged row
/// below was written with `Drive::Unaided` — a drive with no cone to charge
/// off, which [`crate::graph::JumpGraph`] answers no plan
/// for — so the coarse plan never ran and the flat galaxy-wide search
/// answered in its place: 451 stops where the plan finds 141, and 919 where
/// it finds 334. Both were still seconds, so the clock saw nothing and the
/// guard stayed green over a session of measuring the wrong thing. A route
/// three times the recorded length is the failure; how long it took to find
/// is only what it cost.
///
/// A fifth over the recorded count: loose enough that a rebuilt directory
/// shifting a route by a jump or two is not a failure, tight enough that
/// the flat fallback — three times over at every range measured — has
/// nowhere to hide. A row that answers *nothing* fails outright, which the
/// old `map_or(0, ..)` print reported as zero jumps and passed.
///
/// Stops and not jumps: the systems the route runs through, the ship's own
/// jumps being one fewer. It is the count every table in this file and in
/// [`crate::graph`] was recorded in.
fn plotted(
    what: &str,
    route: Option<Vec<(i64, [f64; 3])>>,
    took: Duration,
    stops: usize,
    clock: u64,
) {
    let found =
        route.unwrap_or_else(|| panic!("{what}: no route at all")).len();
    println!("route {what}: {found} stops in {took:.2?}");
    assert!(
        found <= stops + stops / 5,
        "{what}: {found} stops against the {stops} recorded — the coarse \
         plan is not being used",
    );
    assert!(
        took < Duration::from_secs(clock),
        "{what}: {took:?}, over its {clock} s",
    );
}

/// Routing: opening the galaxy, and crossing it.
///
/// The two halves are timed apart because they fail differently. Opening
/// used to be *building* — a grid over every place in the galaxy, 32 s and
/// 13.7 GB, paid on the click that asked for a route — and is now reading
/// the index file and nothing else. The search is what a commander waits
/// on.
#[test]
fn routing_stays_quick() {
    let Some(dir) = measured() else { return };
    let source = FsSource::new(&dir);
    let boosts = match pollster::block_on(source.boosts())
        .expect("the boosts should read")
    {
        Some(rows) => Boosts::of(rows),
        None => Boosts::default(),
    };

    // The two ends, chosen from the data rather than named: the system
    // nearest the origin, and the one nearest a point a thousand light
    // years off it. Both ends are then in the part of the galaxy anybody
    // has actually visited, so the search crosses inhabited space and comes
    // back with a route — where the two extremes of the table are as likely
    // to be an isolated pair with no chain between them at all, which times
    // an exhausted search rather than a real one.
    //
    // Opened first and picked through the mapping, because picking them any
    // other way is what this test is *for*: finding two systems through the
    // names table faults every byte of `addr.bin` — 1.6 GB at 200 M — and
    // the peak resident set then measures the harness rather than the
    // router. A sphere query touches the two neighbourhoods and nothing
    // else.
    let at = Instant::now();
    let sky = std::sync::Arc::new(
        galos_index::Sky::open(&dir).expect("the galaxy maps"),
    );
    let graph = JumpGraph::over(&sky, &boosts);
    let opened = at.elapsed();
    println!("route graph: {} systems in {opened:.2?}", graph.len());
    assert!(
        opened < Duration::from_secs(1),
        "opening the galaxy for routing took {opened:?}",
    );

    // Widening until something is in reach: the origin is in the bubble and
    // finds a system at once, and a point out in the dark would otherwise
    // be an empty answer rather than a slow one.
    let nearest = |to: [f64; 3]| {
        let mut radius = 50.0;
        loop {
            let mut best: Option<(f64, i64, [f64; 3])> = None;
            sky.each_near(to, radius, |node, place, away| {
                if best.is_none_or(|(held, ..)| away < held) {
                    best = Some((
                        away,
                        sky.address(node).expect("a named system"),
                        place,
                    ));
                }
            });
            if let Some((_, address, place)) = best {
                return (address, place);
            }
            radius *= 4.0;
            assert!(radius < 1.0e6, "{to:?} has nothing near it");
        }
    };
    let start = nearest([0.0, 0.0, 0.0]);
    let end = nearest([700.0, 0.0, 700.0]);

    for (how, stops) in [(Routing::QUICK, 22), (Routing::FEWEST, 22)] {
        let at = Instant::now();
        let route = graph.route(
            start,
            end,
            50.0,
            how,
            Drive::Unaided,
            Tuning::default(),
            None,
        );
        plotted(
            &format!("{} 1 kly", how.named()),
            route,
            at.elapsed(),
            stops,
            120,
        );
    }

    // What the exact settings cost at three thousand light years, which is
    // the distance at which they are still worth asking for. Both drives:
    // the charged one is two hundred times the work of the unaided one over
    // the same two ends, all of it the admissibility tax — the estimate has
    // to divide by the widest jump the drive could make, and two systems in
    // a hundred can actually make it.
    for (drive, stops, ceiling) in
        [(Drive::Unaided, 62, 30), (Drive::Standard, 32, 240)]
    {
        let far = nearest([2000.0, 0.0, 2200.0]);
        let at = Instant::now();
        let route = graph.route(
            start,
            far,
            50.0,
            Routing::FEWEST,
            drive,
            Tuning::default(),
            None,
        );
        plotted(
            &format!("optimal {drive:?} 3 kly"),
            route,
            at.elapsed(),
            stops,
            ceiling,
        );
    }

    // And the crossing, supercharged, which is what the boost stars are
    // for. Colonia's own coordinates: twenty-two thousand light years of
    // it, a hundred and forty jumps off the jet cones against the 610 s the
    // flat charged search takes to prove its hundred and thirty-seven.
    //
    // Timed twice because the first charged route of a session is the one
    // that sorts the boost stars into cells (213 ms, and it used to be a
    // 7.9 s join before the place was published); the second is the plan
    // and its legs alone. Then again at 80 ly, where the same crossing is
    // fewer and longer jumps and a wider coarse query.
    let colonia = nearest([-9530.5, -910.28, 19808.12]);
    for (round, range, stops) in
        [("cold", 50.0, 138), ("warm", 50.0, 138), ("80 ly", 80.0, 82)]
    {
        let at = Instant::now();
        let route = graph.route(
            start,
            colonia,
            range,
            Routing::QUICK,
            Drive::Standard,
            Tuning::default(),
            None,
        );
        plotted(
            &format!("charged crossing ({round}, {range} ly)"),
            route,
            at.elapsed(),
            stops,
            120,
        );
    }

    // And the same crossing for a ship that jumps half as far, which is
    // what the settings are for: at four bridging jumps — a 200 ly reach at
    // this range — the boost stars are not connected, the plan answered
    // nothing in 0.3 ms, and the flat search spent 233 s to come back with
    // 317 jumps. The reach is a distance now (`Tuning::reach`), so the same
    // 400 ly that a 50 ly ship always had makes this a plan.
    {
        let at = Instant::now();
        let route = graph.route(
            start,
            colonia,
            25.0,
            Routing::QUICK,
            Drive::Standard,
            Tuning::default(),
            None,
        );
        plotted("charged crossing (25 ly)", route, at.elapsed(), 328, 60);
    }

    // And the same crossing weighed by *fuel*, which is the walk across
    // each gap doing the work: every step is priced per light year of
    // ground it closes, so a fuel-weighed walk takes tens of short jumps
    // where a jump-counted one takes eight — 44 steps at the worst,
    // against the 32 a jump-counted walk is allowed. The failure this
    // catches is the walk giving up and handing its gaps to the
    // `economical` search, which near the core is seconds apiece: 175
    // stops in 423 ms measured, where a hundred-odd searched gaps would be
    // minutes.
    {
        let least_fuel =
            Routing::at(95, Weigh::Fuel { hop: 50, expand: EXPAND });
        let at = Instant::now();
        let route = graph.route(
            start,
            colonia,
            45.0,
            least_fuel,
            Drive::Standard,
            Tuning::default(),
            None,
        );
        plotted(
            "charged crossing (45 ly, least fuel over 50% hops)",
            route,
            at.elapsed(),
            170,
            30,
        );
    }

    // And what a click on the plot button over a route still being searched
    // costs: the flat charged crossing, which is ten minutes of work, told
    // to give up once it is under way. Dropping the task does not interrupt
    // a body the pool has begun, so the search reads the flag itself — and
    // what this measures is how long it takes to notice, which is one
    // expansion's worth of work.
    let taken_back =
        Frontier::between(DVec3::from(start.1), DVec3::from(colonia.1));
    std::thread::scope(|scope| {
        let searching = scope.spawn(|| {
            graph.route(
                start,
                colonia,
                50.0,
                Routing::FEWEST,
                Drive::Standard,
                Tuning::default(),
                Some(&taken_back),
            )
        });
        while taken_back.expanded() == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        let at = Instant::now();
        taken_back.abandon();
        let answer = searching.join().expect("the search thread");
        let stopping = at.elapsed();
        println!("route Direct charged: taken back in {stopping:.2?}");
        assert!(answer.is_none(), "a search taken back answered anyway");
        assert!(
            stopping < Duration::from_secs(5),
            "a cancelled crossing took {stopping:?} to stop",
        );
    });
}
