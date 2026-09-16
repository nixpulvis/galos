# The routing settings, and what is left of them

What the route form asks for, what each control measurably buys, and the
order the remaining work is worth doing in. Measured on 2026-09-15 and
2026-09-16 against `.index/full` — **200,071,629 systems**, 3,846,802
published jet cones — on an Apple M5 Pro, release.

Everything below is either landed and measured, or named as a next step with
the number that argues for it. Nothing here is a guess dressed as a plan.

## What the form is now

```
Range          [ 45 ] Ly
least fuel  [====22.5 Ly====]  fewest jumps      the trade
[ ] Shortest                                     at the fewest-jumps end only
Expand nearest [8 ──●── all]                     at a shortest hop of 0 only
Within         [ 0% ──●── optimal ]              everywhere else
Supercharging  [ Standard ▾ ]
    Plan           [optimal ──── 50%]            its own weight, not the route's
    Crossing a gap [ Stepped ▾ ]
```

Two rules throughout. **A control that does not apply is not drawn** — the
trade's ends are the two asks that used to be separate modes, a hop of the
whole range pricing a jump at a tankful, which is the fewest-jumps question
arrived at from the other side (3 stops either way, measured). And **the
proven ask is the top stop of whichever rail applies**, rather than a tick
standing over them: `Within` at `optimal` where the percent bites, and
`Expand nearest` at `all` where it does not, an unpriced hop having no
bound for a percent to tighten.

And one that is new: **a control whose wrong settings are a cliff is not a
control.** `Gap allowed` was the last of those — measured, one notch of it
was worth 32 stops in 6.2 ms against 60 in 32.3 s — so the plan climbs the
reach itself and nobody is asked.

## What each control is worth, measured

| control | what it moves | measured |
|---|---|---|
| the trade | fuel against jumps | 0.191 → 0.876 tanks, 36 → 5 stops, Sol to Col 285 at 100 Ly |
| `Within` | the bound on the answer | 89 expansions at 75% against 5,876 at 95%, same 45 stops |
| `Expand nearest` | what a fuel search looks at | 8 → all: 168 ms → 3.06 s at 45 Ly; **1.0× from a quarter of the range up** |
| `Plan` | how hard the cone chain is worked | exact: 156 stops against 166 on Colonia for 19×; free where the plateau is small |
| `Crossing a gap` | how a gap's jumps are found | stepped 141 stops in 0.30 s against searched 140 in 2.60 s |
| `Shortest` | the tie among equal-jump chains | 621 → 543 Ly of wander for one stop |

## Landed this pass

- **The gap width climbs itself, and the rail is gone.** Asked why the
  reach is never under 400 Ly even at a 10 Ly range: because the cliff is
  a distance, and that much held up — re-measured on whole plans at 10,
  25 and 50 Ly, **350 Ly strings nothing and 380 plans at every range**.
  What did not hold up is the *single* number. Flooding measured
  connectivity from Sol; a plan needs the cones to connect along a
  corridor, and Sol to a system 2 kly out plots **60 stops in 32.3 s at
  405 Ly against 32 stops in 6.2 ms at 495** — at 10 Ly, 495 stops in
  896 s against 430 in 15.5 s. Stops are **monotone** in the reach on
  every corridor sampled, a wider one only adding edges to the cone
  graph, so the floor is 500 Ly where the curve flattens and
  `Highway::plan` climbs from there: a chain that does not close on the
  goal is planned again a jump wider, up to `RUNGS` = 8. A rung is only
  ever paid where the narrower reach had already failed. The wider floor
  costs the guard's own corridors 30–60% of their milliseconds and buys
  2–3% of the jumps flown: 141 stops in 341 ms became 138 in 442 ms at
  50 Ly, and 334 in 882 ms became 328 in 1.31 s at 25.
- **The `Optimal` tick is gone into the rails.** It was a second control
  over one number: ticking it hid `Within` and `Expand nearest`,
  unticking it had to remember where they had stood, and the same state
  was reachable two ways. `Within` runs to 100 and reads `optimal` there,
  as `Plan` does. What made one control enough is that
  `Routing::approximates` reads the *ask* rather than the percent alone:
  at an unpriced hop the estimate is zero, so the percent cannot
  approximate anything and the cap is the whole of the promise — `all` is
  every system in reach, which is the graph a proven route searches. Of
  the four things the gate switches, three were already off there (the
  weight is inert, the cap is off, and expanding once is exact under a
  zero heuristic); the fourth was the **coarse plan**, which a proven
  route cannot have, its edges being lower bounds on gaps. Two write-
  throughs keep the pair honest: the cap's top stop clears the slack, and
  a proven ask run down to the fuel end lands on `all` rather than on the
  count the rail remembered.
- **The plan has its own weight.** `Tuning::planning` carries the coarse
  leaning, `Highway::plan` reads it instead of `Routing::over`, and the
  rail's top stop is the exact plan and the default. Measured end to end
  at 45 Ly, exact against the plan leaned by the route's own percent:
  **156 stops against 166** on Colonia at 80% and **170 against 175** 22
  kly out, for 1.90 s against 98 ms and 2.00 s against 292 ms — and 137
  either way 16 kly out. So it is *tried* rather than promised:
  `Tuning::allowance` drops an exact pass that has spent **2,048
  expansions**, some 40 ms, and the plan is then worked leaned exactly as
  before. Where the cone plateau is small the exact pass lands inside
  that in 512 expansions — 3 kly out at a 495 Ly gap, 40 stops in 4.27 ms
  against 4.02 ms warm.
- **A gap is crossed on what the route is weighed by.** `Crossing::Nearest`
  stepped to whatever landed closest to the next cone, so a planned
  least-fuel crossing came back with **the same 166 stops and the same
  157.41 tanks at every position of the hop rail** — the trade was inert
  on any supercharged route long enough to be planned. Scored by the
  metric, as `thinned` already is, the rail moves: 148.28 tanks at a 50%
  hop, 143.76 at 25%, **142.21 unpriced**, for 11–118% more stops and a
  few percent of the clock. A third of that is the *arrival* — taking the
  far cone the moment it comes into reach is a jump at full range, the
  dearest there is. Renamed `Crossing::Stepped`, "nearest" having stopped
  being true of it.
- **The price has no free jumps.** `TANKFUL` went to hundred-thousandths
  and `burned` floors at one unit: a 1 Ly jump at a 45 Ly range used to
  round to **zero**, so the graph held free edges and a 95% route came back
  *cheaper* than the proven one (2.144 against 2.148). Now 2.1375 proven
  against 2.1387 at 95%, which is the way round it has to be.
- **The cap follows the ask.** `thinned` keeps the cheapest candidates per
  light year of ground closed rather than the nearest the goal. A
  least-fuel plot at 95% was burning **0.611 of a tank against 0.192**
  because the cap kept the longest jumps in the sphere; it now answers the
  optimal route in 1.05 s against 19.3 s to prove it.
- **An approximate search expands each system once.** Weighted A\* without
  re-expansion keeps the same `1 + over/100` bound, which is `ARA*`'s own
  argument; the claim that the bound *needed* reopening was wrong. Three
  quarters of a shortest search was re-expansion: 34,138 pops to 20,402,
  2.04 s to 0.57 s, and a shorter route.
- **The coarse plan reads `Weigh`.** It ignored it, so every planned route
  was the fewest-jumps chain whatever the form asked. Least fuel is
  deliberately *not* on the distance ordering — a boosted jump costs a
  whole tank however far the cone throws the ship, so fuel on a chain of
  cones is the hop count.
- **The gap rail was floored, and then it was deleted.** It opened at one
  hop (225 Ly for a 45 Ly ship) and every notch under the default could
  only break the plot, so it was moved to open at the first quantum ≥
  `GAPS_LY` — which also fixed a hard panic, the old ceiling having been
  counted off the boosted jump so that a 10 Ly ship got `min > max. min =
  400, max = 200`. Superseded by the ladder above: there is no rail left
  to floor or to panic in.
- **A route landing no longer freezes the map.** A cut used to throw every
  cell's filter verdicts away and the next frame re-walked 152 M resident
  points — **3.4 s** in one frame. Stale verdicts are kept and brought
  forward at `VERDICT_BUDGET` = 200,000 points a frame.
- **A route filter costs what any other filter costs.** `admits` walked the
  stop list per point: 27.5 ns against 3.8 for a faction, 4.18 s over the
  resident set. `Filters::prepared` gathers the named addresses once a
  pass: **3.9 ns, 0.59 s**, and a long route costs what a short one does.
- **Zooming into a system on a route no longer eats memory.** A dash is a
  share of the view and the leg is not, so the count was unbounded: 15.5 M
  points and **414 MB** at a ten-thousandth of a light year, rebuilt every
  1.33× step of the wheel. `DASH_CAP` bounds it at 229 KB a leg by
  lengthening the dash rather than truncating the run.

## What the last pass got wrong, so it is not re-derived

The table §1 was argued from read **exact as 2–5× faster for the same
answer** on the two 3 kly rows — 271 → 149 ms and 50 → 9 ms. Re-measured,
that is the *cold read*, not the weight: the leaned row was taken first in
each pair, and the same corridor measured four times running gives 44.40
ms, 4.17, 4.27, 4.02 — a tenfold first row whichever weight it carries.
Warm, exact and leaned are within six percent of each other there.

What survives is the other half: where the cone plateau is large, exact is
2–19× the wait for nothing to six percent of the stops, and no distance
predicts which. The allowance is therefore doing a different job than the
one the table imagined — it is not choosing the faster plan, it is buying
the exact chain wherever that is nearly free and refusing to pay for it
anywhere else.

## The next steps, in order

### 1. `JumpGraph::STEPS` is the last constant tuned against a 50 Ly ship

A gap walk gives up after **32 steps**, on the reasoning that "a gap is one
supercharged jump and a few ordinary ones by construction, so a walk that
has taken this many is wandering". That reasoning is the reach divided by
the range, and the reach is a distance: at 10 Ly a 500 Ly hop is **47
jumps**, so *every* wide gap exceeds the cap, the walk gives up, and the
gap goes to the search — which is the expensive path stepping exists to
avoid (0.30 s stepped against 2.60 s searched on one Colonia crossing,
17.9 s for a single gap near the core).

What is measured is the symptom: Sol to a system 2 kly out at 10 Ly plots
495 stops in **896 s** where its coarse plan takes 1.31 s, so the legs are
essentially all of it. That the step cap is the cause is arithmetic rather
than an A/B — nothing has yet run the same plot with a scaled cap.

The fix is the shape [`Tuning::reach`] already took: derive the cap from
the reach in jumps — about `reach / range`, with slack — rather than from
a constant. The fuel-weighed walk already has its own bound for the same
reason (`HOPS` = 512, measured against walks of 11–61 steps).

### 2. The screen walk is 22–29 ms a frame

`Index::needed` marks 151,619 cells at a wide zoom and costs 22–29 ms every
frame at 200 M, which is a hard ~35 fps ceiling independent of everything
above. Not a hitch — a floor. Untouched, and the biggest remaining cost in
the map.

### 3. What the ladder costs where no rung works

A rung that does not close on the goal pays its stall allowance before the
next is tried — measured at 1.0–1.3 s on the 2 kly corridor at 405 Ly — so
a corridor no reach can plan now pays up to nine of those before the flat
galaxy-wide fallback it used to reach immediately. Unmeasured, and the far
rim is where to measure it: the nearest cone to `[0, 0, -20,000]` is 4,378
Ly out and joins no chain at any reach.

`Tuning::stall` is the number that bounds it, and it was measured against
a single pass rather than against nine.

### 4. The exact chain is not reachable from the form

`Tuning::allowance` drops an exact coarse plan that has spent 2,048
expansions, and on Colonia the plan it refuses to pay for is **156 stops
against 166** — six percent of the jumps flown, for 1.90 s against 98 ms.
Nothing on the form asks for it: the `Plan` rail only leans *harder* than
exact, and the allowance's unit is expansions, which is not a thing to put
in front of a reader. Either it wants a stop past the rail's top that
means "and pay for it", or the answer is that a reader who wants the best
chain there is ticks nothing and waits for the flat search.

### 5. Two smaller things, both measured

- **`Expand nearest` is worth 1.0× above a quarter of the range**, so it is
  drawn only at an unpriced hop. It is worth 18× at the bottom of the trade
  on a 45 Ly ship and 1.3× on a 25 Ly one, where the sphere barely holds
  more than the cap. Consider dropping the rail's `8` stop: it buys 2.3× on
  a long route for +2.6% fuel, and 64 already gets within a fiftieth. And
  it has a default nobody sees — every other position of the trade carries
  `EXPAND` = 64 invisibly, which measures 1.0× there and so is harmless,
  but it is a number the form does not admit to holding.
- **The far rim has cones that connect to nothing** — the nearest cone to
  `[0, 0, -20,000]` is 4,378 Ly out and joins no chain at any reach. No gap
  setting helps there; `START_BRIDGE` is what carries such a start, and
  whether it does was not measured.

## What the theory says, so nobody re-derives it

**The most efficient coarse edge is exactly one supercharged jump.**
`hops(d)` charges one jump inside the boosted reach and one more per range
beyond it, so distance bought per jump falls monotonically from 4× the
range to 1× as the edge lengthens. At 45 Ly:

| edge | jumps | Ly per jump | beats a detour only over |
|---|---|---|---|
| 180 Ly | 1 | 180.0 | 180 Ly |
| 405 Ly | 6 | 67.5 | 1,080 Ly |
| 765 Ly | 14 | 54.6 | 2,520 Ly |

So a reach beyond the boosted jump **cannot improve the best possible
plan** — it buys *connectivity* and *shortcuts across voids*, nothing else,
and a direct edge costing `k` jumps only beats going round when the detour
would exceed `k × 180` Ly. That is why the gains above the fence measured
1–5%.

**And the fence is not one bottleneck after all.** Flooding the cone graph
said it joins up between **360 and 380 Ly** out of the bubble and at **225
Ly or less** everywhere else sampled, which read as Sol's own cone cluster
setting the height of the only fence there is — so a single galaxy-wide
constant looked like the right shape. Re-measured against whole routes, it
is not. Sol to a system 2 kly out at `[1400, 0, 1400]`, 45 Ly, standard
drive, at the rail's own stops:

| reach | stops | found in |
|---|---|---|
| 400 Ly, the default | 60 | **30.97 s** |
| 450 Ly | 47 | 55.6 ms |
| 495 Ly | **32** | **7.54 ms** |

**Four thousand times the speed and twenty-eight fewer stops for one
notch of a setting nobody knows to move**, and the same corridor at 10 Ly
goes from 495 stops in 896 s to 430 in 15.5 s. What happens at 400 is not
a slower plan: the coarse search never reaches the goal, spends its stall
allowance, hands over the 7 cones it did reach, and the stretch left over
becomes one enormous gap for the legs. Flooding measured *connectivity
from Sol*; a plan needs the cones to connect **along a corridor**, and
that is a different question with a different answer per route.

So the constant was a floor that happens to work out of the bubble, not
the height of the only fence, and what the shape argued for is a
**ladder** — plan, and where the chain does not close on the goal, widen
the reach a notch and plan again, which is what `Highway::coarse` already
did with the *start* bridge (`[bridge, bridge * 3, START_BRIDGE]`) when
its first scan found nothing. **Built**: the floor is 500 Ly, where the
curve flattens, and `Highway::plan` climbs eight rungs from there. Stops
are monotone in the reach on every corridor sampled — a wider one only
adds edges — so a rung can never answer worse than the one below it, and
it is only ever paid where the narrower reach had already failed.

## Where the numbers live

- `galos_map/src/perf.rs` — the guard and the measurement log, section by
  section in the order they were taken.
- `galos_map/src/systems/route/graph.rs` — `EXPAND`, `TANKFUL`, `burned`,
  `thinned`, `JumpGraph::walk`, `JumpGraph::stepped`, `Tuning::planning`
  and `Tuning::allowance` carry their own measurements.
- `galos_map/src/systems/route/highway.rs` — `GAPS_LY`, `ALLOWANCE`,
  `Coarse`, `Highway::plan` and its two passes.
- `galos_map/src/ui.rs` — `trading`, `approximating`, `planning` carry why
  each control is drawn where it is, and what it was before.
