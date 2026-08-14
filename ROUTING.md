# Routing

A design note for plotting long routes, from reading [astronav][] against
what is here. **Nothing in it is built.** It is written down so the thinking
survives, and the open questions at the end are the point of it as much as
the rest.

[astronav]: https://github.com/earthnuker/astronav

## Where it stands

`System::route_to` in `galos_db/src/systems/nav.rs` is A\* from the
`pathfinding` crate. It works, and it does not scale, for one reason: every
node the search expands is a live PostGIS round trip.

- `neighbors()` runs `ST_3DDWithin` per expanded system, and returns whole
  `System` rows — including an `array_agg` over `system_factions` that
  routing never looks at.
- The heuristic is `(dist(s, end) / range).ceil()`, with a comment above it
  admitting the constant is chosen for speed over optimality.
- The CLI already takes `--range`, `--total-mass`, `--optimized-mass`,
  `--size` and `--class`. None of them reach the router. `_fuel_cost` beside
  it has the hyperspace equation and no callers.
- `galos_map/src/search.rs` already models a route being plotted, failing,
  and being drawn.

So the shape is there and the engine is the part to replace.

## Beam search

Beam search is best-first search with a bounded frontier: expand everything
at the current depth, rank it, keep the best N, throw the rest away. Cost is
`N × depth` — linear and predictable — where A\*'s frontier grows roughly
exponentially with depth in a dense star field.

astronav ranks nodes with

```
max(dist(node, goal) − mult(node) · range, 0)
```

which reads as *the distance still left if we jumped from here straight at
the goal*. `mult` is where the interesting part is: **1.0** for an ordinary
star, **1.5** for a white dwarf, **4.0** for a neutron star. That single
factor is what makes the router discover the neutron highway on its own,
rather than having it hand-coded as a special case.

Its measured results, from ~100 random routes on a 5950X against ~140M
systems:

| Beam width | Speedup vs full BFS | Jumps worse than optimal |
|---|---|---|
| 64 | 353× | 33.6 |
| 1024 | 200× | 13.4 |
| 8192 | 61× | 6.1 |
| ∞ (BFS) | 1× | 0 |

Sol → Colonia, 22,000 Ly: about 8 seconds at width 8192, 130 jumps, against
2 minutes and 122 jumps exhaustively.

Two things follow from the width being bounded. Beam search is **not
optimal**, which the table quantifies. It is also **not complete** — in a
sparse region it can prune the one bridge star and report failure where a
route exists. A beam failure must never be shown as "no route"; it means
widen and try again.

## Doing it in Postgres

The reason beam search suits this database is that it expands a whole
frontier at once, and a frontier is a batch. One query per jump-depth rather
than one per node:

```sql
SELECT DISTINCT ON (s.address)
       s.address, s.position, s.primary_star_class, f.parent
FROM unnest(:positions, :addresses, :mults) AS f(pos, parent, mult)
CROSS JOIN LATERAL (
    SELECT * FROM systems
    WHERE ST_3DDWithin(position, f.pos, :range * f.mult)
) s
WHERE NOT EXISTS (SELECT 1 FROM visited v WHERE v.address = s.address)
ORDER BY s.address,
         GREATEST(ST_3DDistance(s.position, :goal)
                  - :range * star_mult(s.primary_star_class), 0)
LIMIT :beam_width;
```

with an outer sort on the heuristic before the `LIMIT`, each depth's
survivors written into a `TEMP TABLE visited`, and Rust holding only the
parent pointers for reconstructing the path.

The arithmetic: a Colonia-scale route is ~130 jumps, so ~130 indexed queries
against `systems_position_idx`. The current design issues one for every node
A\* pops, which is tens of thousands. Postgres stays the source of truth,
which means no cache to rebuild when EDDN writes a new system — a real
advantage over astronav's frozen snapshot.

What it does not fix: latency is `jumps × round trip` however good each query
is, so a 500-jump route is still hundreds of round trips, and the map cannot
replot interactively. That is the wall an in-memory index exists to get past.

## Choosing between A\* and beam

Both are the same search with a different width — astronav literally has a
`BeamWidth::Infinite`. So this is one engine with a policy in front of it,
not two implementations.

`dist(start, end) / range` is a free lower bound on jump count before any
search happens, and it is enough to choose:

- **Short routes** (say under 25 jumps) — A\*. It returns the provably
  fewest jumps and the frontier never gets big enough to hurt. Beam saves
  nothing and gives up the guarantee for free.
- **A fuel objective** — A\* or Dijkstra. Beam's pruning ranks by geometric
  progress toward the goal, which does not respect fuel cost; pruning on it
  can throw away the cheap route early.
- **Long routes, or a latency budget** — beam, starting narrow and widening
  on failure. astronav's `incremental_broadening` is exactly this, and it
  removes the need for a precise threshold.

One trap. As a *ranking* function the boosted heuristic is fine, because beam
does not care about admissibility. A\*'s optimality guarantee does: with
boosted jumps in play, the admissible bound is `dist / (4 · range)` — assume
every remaining jump is boosted. Using the aggressive form in A\* quietly
turns it into "fast, slightly wrong", which is sometimes the right trade but
should be a choice.

## Beam informing A\*

The two compose better than either alone, which is where this should
probably end up.

**Incumbent pruning.** Run beam first and keep its route as an upper bound of
L jumps. Then run A\* discarding any node whose `f = g + h` reaches L — it
cannot beat what we already hold. That is branch-and-bound with a warm start,
and it clips the search hard exactly where A\* normally drowns. If A\*
exhausts the unpruned space, the incumbent is *proved* optimal.

It is also naturally anytime, which is what `galos_map` wants: draw the beam
route immediately, refine in the background, swap if something better
arrives.

**Corridor restriction.** The optimal route is nearly always a local shuffle
of the beam route, not a different path. Build a `LINESTRING Z` from the beam
route and add `ST_3DDWithin(position, :route_line, :corridor)` to the
neighbour query — the branching factor collapses and it is one more predicate
on an index we already have. It forfeits the optimality proof, since the true
optimum could leave the tube, so it is a refinement pass rather than an
answer.

**Beam-stack search** (Zhou & Hansen) is the published version of this
family: beam search that remembers what it pruned and backtracks into it,
converging on optimal while producing good routes early. astronav has
`beam_stack.rs` and `incremental_beam_search.rs` as sketches, so its author
arrived at the same place.

## Where the multiplier's data comes from

The boost is the whole point and the data for it is thin. `mult` needs the
class of the star you arrive at, and there are three sources here, none
complete:

- **`systems.primary_star_class`** exists, and is written by exactly one path:
  EDDN `NavRoute` events, at `eddn.rs:270`. That is the in-game route
  planner's output, so coverage is biased toward space people travel through
  and absent from most of the galaxy. `System::from_journal` — the scan path —
  does not set it at all.
- **`stars.star_class`** is populated from EDDN `Scan` events and carries
  `distance_from_arrival_ls` and `parent_id`, so a system's arrival star can
  be derived. Better coverage, but a join or a denormalisation.
- **The Spansh `galaxy.json` dump** has it for everything, from `bodies[]`
  where `mainStar` is true. `DATABASE.md` has that import; astronav's
  `get_sys_flags` is the reference for reading the field.

Until one of those is settled, a boosted router will quietly behave like an
unboosted one across most of the galaxy.

## Open questions

### Blocked on measurement

1. **What is a neighbour lookup actually worth at scale?** Everything below
   turns on `ST_3DDWithin` at 140M rows. If it is 0.5 ms the Postgres design
   is enough for a long time; if it is 50 ms the map needs an in-memory
   index. This is the reason `DATABASE.md`'s import exists and should be
   answered before anything is built.
2. **How much of the galaxy does a route actually touch?** A Colonia route
   never needs systems 20,000 Ly off-axis. Loading only a cylinder around the
   straight line might make an in-memory index cost a fraction of astronav's
   140M nodes — a third option between "all in Postgres" and "all in RAM"
   that nobody has costed.

### To decide

3. **Where does the search run?** Postgres batched frontier, a resident
   KD-tree, or the per-route corridor above. (2) informs this and (1) decides
   it.
4. **Which source fills `mult`, and do we denormalise it?** Deriving from
   `stars` per query is a join in the hot path; denormalising onto `systems`
   is a column that has to be kept true.
5. **Do we model fuel at all?** astronav does — range as a function of
   current fuel, boost multipliers, refuel stops. The equation is already
   here and dead. Jumps-only is a smaller thing that is useful sooner.
6. **What is a ship?** A `--range` number, or a loadout parsed from the
   journal. `elite_journal` can already read `Loadout`, so plotting for the
   ship you are actually flying is close.
7. **One engine or two?** The map wants an answer inside 100 ms and can show
   a worse route; the CLI can take ten seconds and should not. Same code with
   different width and time budget, presumably — but that needs the anytime
   shape from the start.
8. **Where does the code live?** `nav.rs` is fine for a query-per-expansion
   router and wrong for one holding an index. A `galos_route` crate used by
   both the CLI and the map is the obvious answer and has not been argued.
9. **Do we want provable optimality at all,** or is "within a few jumps"
   always fine? It decides whether the A\* half is worth keeping once beam
   works.

### Deliberately not thought about yet

10. **Supercharging costs something.** A boosted jump means flying into the
    jet cone, which takes time and damages the hull. A route that says "boost
    here" 40 times is not free in the way the graph says it is.
11. **Waypoints.** astronav plots through a list of hops. `route.rs` has a
    commented-out `Route` enum — `Path`, `Both`, `Either` — sketching
    something richer than that. Unclear whether it is still wanted.
12. **What the map draws while it is thinking.** `search.rs` models a route
    being plotted and failing, but not a route that is provisional and about
    to improve, which is what anytime search produces.

## References

In astronav: `src/common.rs` (the heuristic, `SystemFlags`, `TreeNode`),
`src/route/algorithms/beam.rs`, `src/route/algorithms/incremental_broadening.rs`,
`src/ship.rs`, `src/galaxy.rs` (`get_sys_flags`).

Here: `galos_db/src/systems/nav.rs`, `src/bin/galos/route.rs`,
`galos_map/src/search.rs`, and the `systems_position_idx` migration.
