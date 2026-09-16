# The routing index

What kind of index the router uses, why it is not the tree, what a Morton index
would and would not buy, and what a competing implementation's subsecond
supercharged route across the galaxy actually implies. Measured on the real
`.galos_index` (2.8 GB, 2,635,093 positioned systems) on an Apple M5 Pro,
`--release`, 2026-09-09.

Everything below with a number beside it was run. The throwaway examples that
produced the numbers were deleted after; they are described well enough to
rebuild. Anything not measured is marked `[INFERENCE]`.

## 1. What the router uses today

A **uniform spatial hash grid over 64 ly cubic cells**. Not a k-d tree, not a
BVH, not an octree, and not Morton-keyed.

`galos_map/src/systems/route/graph.rs`:

- `Places` (`:731`) is the whole index: `points: Vec<(i64, [f64; 3])>`,
  `by_address: HashMap<i64, usize>`, `buckets: HashMap<[i32; 3], Vec<usize>>`.
- The cell key is `bucket_of(p)` (`:783`), componentwise `floor(x / BUCKET_LY)`
  with `BUCKET_LY = 64.0` (`:25`) — sized against the jump range, so an unaided
  jump sweeps 3³ cells.
- `neighbors_each` (`:910`) sweeps `±ceil(range / 64)` around the home cell,
  drops cells whose nearest corner is out of range (`bucket_away`, `:771`),
  then tests `dist2` on the survivors and hands each hit to a closure. No
  allocation per expansion.
- Two instances per `JumpGraph`: `base` (the resident names table) and `fresh`
  (systems the feed has named this session), both `Arc<Places>`, both
  immutable. A refresh clones the base handle and rebuilds only the small one;
  an in-flight route keeps the graph it started on (`:707-710`).
- `boosts` sits beside the cells rather than in a point's payload, because the
  boost table churns on nearly every publish (`:717-726`).

Historically this was a database question: `ST_3DDWithin` against
`systems_position_idx`, a GiST index with `gist_geometry_ops_nd`
(`galos_db/migrations/20210321042750_index_systems_position.up.sql`). That path
still exists for `galos_db::System::route_to` and the `galos` and
`galos_server` binaries. The map no longer links `galos_db`, so
the grid replaced it there.

## 2. Why not the existing Morton cell tree

`galos_index` already has a Morton-keyed sparse adaptive octree
(`geometry.rs`), and it is the wrong shape for routing for five separate
reasons.

**It is a magnitude-ordered LOD index, not a containment index.**
`assign_slices` (`tree.rs:985-1009`) walks systems brightest first and settles
each at the *shallowest cell on its path with room* — `INTERNAL_SLICE = 512`,
`LEAF_CAP = 4096`. Containment holds one way only: a system in cell C is inside
C's box, but the systems inside C's box are scattered up C's entire ancestor
chain. So "every system within 50 ly of here" is not a subtree query; it is
every intersecting leaf plus all thirteen ancestors of every one of them, each
holding up to 512 records that mostly sit thousands of light years away.
`[INFERENCE]` ~6.6 k ancestor records scanned and discarded per expansion,
against the grid's 27 cells holding a few hundred points. And the ancestors
cannot be skipped: `Routing::Direct` and `Routing::Shortest` claim a genuine
fewest-jumps route (`graph.rs:963-970`), which one dropped edge voids.

**The data is on disk.** `index.bin` is aggregates and rank ranges only, a few
megabytes (`tree.rs:736-739`); the systems live in `cells/LL-<morton>.bin`,
41 bytes a record, fetched per cell on demand (`ARCHITECTURE.md:165-166`). A
galaxy route is ~500 k expansions — effectively the whole payload corpus
streamed off disk mid-search. The names table is already resident and carries
exactly address plus position.

**Adaptive depth is tuned to density; the jump query wants a fixed radius.**
`BUCKET_LY` is chosen against the query. Tree cells subdivide on count, so cell
size tracks stellar density and varies independently of jump range.

**No address index.** `route` takes two `i64` addresses and resolves them via
`index_of` → `HashMap<i64, usize>` (`graph.rs:873-877`). The tree is keyed by
position.

**Snapshot semantics.** `Tree` is mutable with eviction chains
(`tree.rs:19-27`); the router needs an immutable snapshot per in-flight search.

## 3. Would a separate Morton index help?

Four layouts, same neighbour query, same corner pruning, results asserted
identical. 20,111 probes, stable to ±3% over three runs.

What the grid costs today:

```
937,581 occupied cells over 2.63 M systems
per cell: mean 2.8, median 1, p90 4, p99 27, max 1013
bytes: points 84.3 MB | index-vecs 65.3 MB (937,581 allocations)
       | keys 11.3 MB | by_address 42.2 MB
build: 231 ms
```

The median cell holds **one** system, so `HashMap<[i32;3], Vec<usize>>` pays a
three-word header plus its own heap block to store, typically, a single
`usize`: 65 MB of container to index 84 MB of points, and nearly a million
allocations. Every candidate is then a random 32-byte-strided read across
84 MB.

| layout | unaided 50 ly | neutron 200 ly | build |
|---|---|---|---|
| grid — `HashMap<[i32;3], Vec<usize>>` | 3.66 µs | 34.2 µs | 231 ms |
| CSR — `u64` dir, contiguous *indices*, points in place | 3.2 µs (1.2×) | 27.1 µs (1.3×) | 88 ms |
| **Morton-sorted** — points cell-contiguous, `HashMap<u64,(u32,u32)>` | 2.40 µs (1.5×) | 21.4 µs (1.6×) | 150 ms |
| row-major-sorted — identical layout, `x<<26\|y<<13\|z` | 2.54 µs (1.45×) | 20.6 µs (1.7×) | 151 ms |

Per-probe geometry: 27 cells swept / 14 corner-pruned / 5 empty / 404
candidates / 82.5 hits unaided; 729 / 489 / 126 / 4273 / 2657 boosted.

**Read the middle two rows against each other.** CSR keeps the `u64` key and
kills the million allocations but leaves points scattered → 1.2×. Sorting the
points into cell order → 1.5-1.6×. The last row is the control: the same layout
with a row-major key is as fast or faster than Morton, every run.

So a separate index is worth ~1.6×, and **Z-order contributes none of it**. The
win is (a) points contiguous per cell, (b) one `u64` key instead of SipHash
over a 12-byte triple, (c) one allocation instead of 937,581. Morton's locality
is a property of key *ordering*, and the router never scans key order —
`neighbors_each` enumerates its cells explicitly and does one directory lookup
each.

Morton would earn its keep for key-interval range scans (LITMAX/BIGMIN), so
empty cells cost nothing rather than a hash miss. Measured, that is worth
almost nothing here: of 729 cells swept on a boosted jump, 489 are
corner-pruned before any lookup and only 126 of the remaining 240 are empty.

Secondary win, which is real: resident bytes 84.3 + 65.3 + 11.3 + 42.2 =
**203 MB** today against 84.3 + 15.0 + 42.2 = **141 MB** cell-sorted. −62 MB,
and it makes `graph.rs:704-705`'s "a hundred and fifty megabytes" honest again.
Build drops 231 ms → 150 ms, sorting 2.6 M `u64`s beating 2.6 M hash inserts
with a million allocations.

If this is taken: replace `Places::of` (`graph.rs:742-751`), keep
`by_address`, leave the `base`/`fresh` `Arc` split alone. Key it with
`galos_index::geometry::morton_encode` for the reuse — it needs an origin bias,
`BIAS = 1 << 10` was verified collision-free at 64 ly over the real table — or
row-major to avoid coupling the router to the tree's coordinate frame. The
measurement says that choice is free.

## 4. What a supercharged route actually costs

Replicating the router's walk over the real index. 2,635,093 systems, 106,639
boost systems positioned (99,080 neutron, 7,559 white dwarf — 4.0%),
Sol → Colonia 22,000 ly, range 50 ly, `Drive::Standard` (4×).

| | jumps | expansions | time |
|---|---|---|---|
| unaided `Direct` | 458 | 519,623 | 1.58 s |
| unaided `Quick` | 458 | 28,160 | 72 ms |
| charged `Direct` | **141** | 588,261 | 1.59 s |
| charged `Quick` | 142 | 527,564 | 1.51 s |

The unaided rows reproduce the table in `Routing`'s header. The new fact is the
fourth row: **`Quick` saves 18× unaided and 1.1× supercharged.** The weighting
cannot bite, because `Drive::widest()` (`graph.rs:175-177`) must divide the
estimate by 200 ly to stay admissible while only 4% of systems can boost — the
estimate already carries ~4× slack nearly everywhere, and `LEANING`'s 21/20 on
top of that is noise.

Tightening the estimate is not the fix either. Inadmissible, no fewest-jumps
claim survives these:

| charged, estimate divided by | jumps | expansions | time |
|---|---|---|---|
| 200 (admissible, today) | 141 | 588,261 | 1.59 s |
| 155 | 157 | 298,287 | 1.22 s |
| 100 | 196 | 146,826 | 954 ms |
| 75 | 200 | 101,840 | 705 ms |

Still 705 ms, and the route rots from 141 jumps to 200. The other half of the
cost is branching: a boosted expansion sweeps 9³ cells and tests 4,273
candidates, 2,657 of them in range, against 404/82 unaided.

## 5. The 200 M claim

Context: another implementation reports subsecond supercharged Sol → Colonia
over a 200 M-system dataset with a Morton index.

**The index cannot be what does it.** Charged `Direct` expands 588 k of 2.63 M
systems — 22% of the galaxy. Holding the corridor fraction constant, 200 M
systems is ~45 M expansions. We sustain 370 k expansions/s; grant the
Morton layout its measured 1.6× and call it 600 k/s → **75 seconds**
`[INFERENCE, from the measured rate]`. No index makes a full-graph A\* over
200 M subsecond. The search space must be smaller, not the index faster.

**The plausible mechanism is a contracted graph**: nodes are boost stars, edges
are precomputed supercharged hops. Query cost then scales with boost count
rather than system count, and the estimate is tight because every node can
boost.

**The naive contraction fails on our data, which is itself the finding.** Boost
nodes only, direct boost→boost edges at 200 ly:

```
highway: 106,641 boost nodes, built in 6.0 ms
supercharged flood from the bubble: 127 of 106,642 nodes reached,
                                    farthest 522 ly out
```

Direct neutron-to-neutron chains do not exist in our table. 2.6 M positioned
systems are bubble-concentrated, so the highway is disconnected at 200 ly hops;
a real contraction needs edge cost = unaided jumps between boost stars, which
needs the systems in between to exist. At full-galaxy coverage they do. **The
larger dataset buys feasibility of the formulation, not merely speed.**

> **Settled, 2026-09-13, at 200,071,629 systems.** Both halves of that
> paragraph held. The highway is 3,846,802 boost stars; flooded from Sol at
> one supercharged hop it still reaches only **135** of them, and with four
> ordinary jumps of bridging after each hop it reaches **2,419,398**, out
> past 61,413 ly. The contraction is built and shipped —
> `galos_map/src/systems/route/highway.rs`, planned coarse and flown leg by
> leg — and Sol → Colonia charged is **139 jumps in 6.5 s** against the flat
> search's **137 in 610 s**. See `TODO-map-scale.md` 2e.

Four questions that would make the comparison mean something:

1. Proven fewest jumps, or greedy/beam? Our `Quick` is already 72 ms unaided.
   Seconds without the guarantee attached are not comparable.
2. Jump count for Sol → Colonia at 50 ly with a 4× drive. Ours is 141 and
   provably minimal. That is the comparable number.
3. Full 200 M graph, or a boost subgraph / precomputed hop table?
4. Resident or mmapped? 200 M × 32 B = 6.4 GB will not sit in RAM, so it is
   paged — and that is where Morton genuinely earns its keep, key-interval
   scans over a paged file, which is the same argument as the cell tree's file
   layout. The opposite of our resident 2.6 M case, where Morton measured equal
   to row-major.

## 6. Leverage, ordered by measured size

1. ~~**Contracted boost graph.**~~ Built; see the note in §5. Orders of
   magnitude, and it was indeed the only path to parity on supercharged
   routes.
2. **Coverage.** 2.6 M positioned systems against the ~10² M known. An ingest
   problem, and it caps both route quality and any contraction.
3. **Cell-sorted layout.** 1.6× on the neighbour query, −62 MB resident, ~40
   lines replacing `Places::of`.

The index is last, which is the answer to the question that started this.

## 7. Incidental

The names table is upper-cased: the only entry at the origin is `"SOL"`, and
zero of 2.6 M names contain `"Sol"`. Search still works — `Names::find`
lowercases both sides (`galos_map/src/lib.rs:239-242`) — but every row renders
shouty, and route endpoints resolve only by case-folding. Worth chasing in the
builder's name derivation if it is not deliberate.

## How to rebuild the measurements

Two throwaway examples under `galos_index/examples/`, run
`--release` against a built index directory:

- **Layout comparison.** Load `source.names()`, build four indices over the
  same points — the current `HashMap<[i32;3], Vec<usize>>`; a CSR pair of a
  `HashMap<u64,(u32,u32)>` directory over a cell-sorted index array with points
  left in place; points sorted by `morton_encode(cell + BIAS)` with a run
  directory; the same sorted by `x<<26|y<<13|z` — then replay
  `neighbors_each`'s sweep over every ~130th system at 50 ly and 200 ly,
  asserting the four agree on the hit count.
- **Route cost.** Load `names()` and `boosts()`, build the grid, and run the
  `direct`/`quick` A\* from `graph.rs:1060-1241` verbatim: cost one per jump,
  ties broken by squared distance to the goal, `best`/`came` as flat arrays,
  reopening allowed. Endpoints by name, falling back to the nearest system to
  Sol's and Colonia's coordinates, which is what turned up §7. The highway
  variant rebuilds `Places` over the boost systems alone plus the two
  endpoints, then floods it at 200 ly to test connectivity.
