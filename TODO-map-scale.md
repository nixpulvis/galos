# The map at two hundred million systems

Why loading, searching and routing fell over between 2.6 M systems and
200 M, what the numbers actually are, and the order the work is worth doing
in. Measured on 2026-09-13 against `.index/full`, which the 200 M import
finished the same afternoon: **200,071,629 systems**, 204,466 cells, levels
0..13.

## The one-line answer

**The map's resident set was larger than the machine.** It read three
whole-galaxy tables at startup and built three whole-galaxy hash structures
out of them — the user measured **45 GB against 24 GiB of RAM** — so every
later operation ran against swap. Nothing about the LOD path is wrong, which
is exactly why LOD fetch still felt fine: it is the only path whose work is
proportional to what is on screen.

**The names table is packed now** (`galos_map/src/names.rs`): measured over
the real 200,071,629-name table, **7.2 GB peak resident, read and packed in
33 s**, against the ~47 GB the old form would want at 235 B an entry. The
router's bucketing is built on the first route rather than at startup, so a
session that only looks at the sky never pays it. What is left is in item 0
and item 1 below.

| | measured |
|---|---|
| machine | **24 GiB RAM**, Apple M5 Pro |
| `.index/full` | **200,071,629 systems**, 204,466 cells, `index.bin` 38 MB |
| names on disk | 5.66 GiB over 2,004 chunks at 131 M; ~8.6 GiB at 200 M |
| `reaches.bin` | 751 MiB at 131 M; **76,044,388 rows** at 200 M |
| `boosts.bin` / `populated.bin` | 39 MiB / 11 MiB |
| cell payloads | read per cell on demand, which is why they cost nothing |

## What was held, and what is held now

`loading::read` reads, sequentially, on one task: the stamps, the index,
`populated`, the names, `reaches`, `boosts`, `factions`.

| structure | was, at 200 M | is now |
|---|---|---|
| the names table | `Vec<NameEntry>`: 48 B a struct plus a heap block a name, ~235 B an entry measured → **~47 GB** | four arrays and a blob, **7.2 GB measured**, `galos_map/src/names.rs` |
| address → entry | `HashMap<i64, usize>`, ~24 B an entry → **~4.8 GB** | gone: the addresses are sorted, so it is a binary search |
| the reaches | `HashMap<i64, f32>` over 76 M rows → **~2 GB** | two sorted arrays, 12 B a row → **912 MB** |
| the read itself | the whole table decoded into one `Vec` before packing anything → the peak again | a chunk in, packed, dropped: 64 Ki entries at a time |
| `Places` (the router's grid) | built at startup whether or not anything routed: 32 B a point, an address map beside it, a bucket per occupied cell → **~16 GB and ~72 M allocations at 200 M** | built on the first route asked for, from `Names::points` |
| `best` + `came`, per route leg | 1.6 GB a leg at 200 M, allocated before the first expansion | unchanged — item 2 |

The old doc comments were all written against 2.6 M — "a hundred megabytes
and two and a half million entries", "six megabytes", "a hundred and fifty
megabytes". Every one of them was off by nearly a hundred, and they are
rewritten with the arithmetic.

**Measured, on the real 200,071,629-system table** (a throwaway harness
reading `.index/full` through `FsSource`, peak RSS off `getrusage`):

```
packed: 200071629 names read in 17.0s, table 200071629 entries,
        76044388 reaches, peak RSS 7.2 GB, total 32.7s
1001 of 1001 addresses found in 322.7ms (322.3µs each)
search: 1 hits in 11.3s
```

Two things to read out of the last two lines. The address lookups are ~28
random touches into a 1.6 GB array, and 322 µs each says those touches are
faulting rather than hitting — memory pressure on a machine holding 7 GB of
fresh arrays with a 610 GB import still running, not the arithmetic. And
**a search is still a scan of the whole blob**: 11.3 s at 200 M, against
minutes when it allocated a lowercased `String` per entry, but still O(N).
That is what item 1's sorted by-name part is for.

## Search: one scan, 131 M allocations

- Fires on Enter only, not per keystroke (`ui.rs:2092-2097`, `:6289-6291`) —
  so the typing lag is not search.
- `Names::find` (`lib.rs:241-246`) walks every entry and calls
  `e.name.to_lowercase()` on each: **131.29 M String allocations** and
  131.29 M substring searches, uncapped `Vec` of matches, sorted whole, then
  truncated to 25 (`search.rs:323-341`, `RESULTS = 25` at `:27`).
- It runs **on the main thread**: the `AsyncComputeTaskPool::spawn` at
  `search.rs:289-293` wraps a value that has already been computed.
- Plotting a route resolves its endpoints through `Names::address`
  (`lib.rs:257-261`) — another full scan each, two per leg at
  `route/fetch.rs:100` plus one per stop in `route/mod.rs:539`, all on the
  main thread before any search starts. **Four to six full scans of 131 M
  entries to begin a route.**
- Labels do not use any of this: they go through `by_address`
  (`systems/spawn.rs:912`). That path is fine.

## Routing: the work is quadratic in density

`ROUTING-INDEX.md` measured charged Sol → Colonia at 2,635,093 systems:
588,261 expansions, 1.59 s, 4,273 candidates tested per expansion.

Fifty times the systems in the same volume means **fifty times the nodes in
the corridor and fifty times the candidates tested at each one**. The search
is ρ² in density, so the same route is ~2,500× the work: **~2.9e7
expansions, tens of minutes** `[INFERENCE, from the measured rate and the
measured per-expansion sweep]`. That is before swap.

Nothing bounds it. No node budget, no time budget, no beam, no corridor, no
hierarchy — `CELL_CEILING` (`route/frontier.rs`) bounds the *drawing* of the
frontier, not the search. A leg cannot be cancelled once running; dropping
the task only stops the reader.

`ROUTING-INDEX.md` §5 already said the conclusion out loud: *"No index makes
a full-graph A\* over 200 M subsecond. The search space must be smaller, not
the index faster."*

## The plan

Ordered by measured leverage, and by what unblocks what.

### 0. Stop paying for what nobody asked — a day, no format change

- **Do not build `Places` at startup.** Build it on the first route, behind
  a `OnceCell`. Loading and browsing stop paying 11 GB and ~47 M
  allocations. (`loading.rs:307`)
- **~~`find` without the allocation~~ — done.** A system's name is
  `galos_index::SystemName`, upper case by construction, so the comparison
  is bytes against bytes and the fold is one, of the query. It used to
  lowercase *both sides of every comparison*: 131 M `String` allocations to
  answer one search. Same change removed the `to_uppercase` a label used to
  pay per name on screen and the `UPPER($2)` Postgres used to pay twice per
  system write. **What is left of this bullet is the cap:** `find` still
  collects every match and sorts before truncating to 25.
- **Resolve a name once.** Route endpoints and `names_exactly` are exact
  byte comparisons now, but still four to six full scans per plot. Interim:
  a `HashMap<&SystemName, i64>` built beside `by_address`; properly, item
  1's sorted file — which the invariant is what makes possible, a
  case-insensitive comparison having no order to binary-search.
- **A budget on every route.** Expansions and wall clock, with a partial
  answer and an explicit "not proven minimal" flag in the UI. A route that
  cannot finish must say so in a second, not in an hour.

Nothing here is the fix; all of it is the difference between unusable and
usable while the fix lands.

### 1. Names off the heap — the load and the search

The names table is 5.66 GiB on disk and ~31 GB resident. It should be
**mapped, fixed-width and sorted**, and then it is neither.

- `names.by_address`: `[i64 address][f32 x 3][u32 blob offset][u16 len]`,
  26 B a row, address-sorted → **3.4 GB mapped, nothing resident**. Address
  lookup is a binary search; `Names::by_address`'s 3 GB hash map goes away.
- `names.blob`: the name bytes, once.
- `names.by_name`: `[u32 blob offset][i64 address]`, sorted by a normalised
  (case-folded, trimmed) name → **1.6 GB mapped**. Prefix search is a binary
  search; exact resolution for a route endpoint is O(log N); substring
  search becomes a scan of mapped bytes with no allocation and no decode.
- **Reaches into the payload record.** A reach is per scanned system and the
  payload is already read per cell; carrying it there removes a 751 MB
  decode at startup and ~1.4 GB resident. Otherwise the same fixed-width
  mapped treatment (12 B a row).

This is `galos_index` work — new parts beside the chunks, written by the
same builder, with the chunk format kept for the feed's incremental writes.
It is also the part that most wants to be decided *with* the EDDA formats
rather than before them: these are exactly the "parts" a served index hands
over.

### 2. Routing: a smaller graph, not a faster index

- **Positions from the payloads, cell-sorted.** The router's second copy of
  every position is unnecessary: the payloads hold `[f64; 3]` per system and
  are already paged per cell. A cell-sorted CSR directory over mapped
  payloads is the layout `ROUTING-INDEX.md` §3 measured at **1.6× the
  neighbour query and −62 MB** at 2.6 M; at 131 M it is the difference
  between an 11 GB resident grid and a corridor-bounded read.
- **Contract the graph.** §6.1 named this the only path to parity and §5
  measured why it did not work then: boost-to-boost at 200 ly reached 127 of
  106,642 nodes because 2.6 M systems were bubble-concentrated. *That was a
  coverage finding, and the coverage has arrived.* First measurement to
  make: rebuild the highway over `.index/full`'s boosts and flood it. If it
  connects, query cost scales with boost count (~4 % of systems) instead of
  system count.
- **Hierarchy over the cells we already have.** `index.bin` is 25 MB and
  134,185 cells, resident for free. Route cell-to-cell on the aggregates,
  then refine inside the corridor. This is the structural answer for unaided
  long-range routes, where no contraction exists.
- **`Quick` by default at long range.** Measured 18× cheaper unaided
  (72 ms against 1.58 s) for a route that is not proven minimal. Proven
  minimality over a galaxy is a thing to ask for, not a default.

### 3. Where this meets EDDA

There is no EDDA document in this tree — I grepped; `TODO-postgres.md`,
`TODO-source-sink.md`, `TODO-scale.md`, `TODO-scale-regions.md` and
`ROUTING-INDEX.md` are all of it. So this plan is written against the
`Source` seam as it stands, and two decisions in it should be taken with the
EDDA formats in hand rather than ahead of them:

1. **The served parts.** Items 1 and 2 turn names, reaches and positions
   into mapped fixed-width files. Over a transport those are range requests
   against the same files — which is what `Source`'s `Part`/`Stamp` seam
   (`galos_index/src/source.rs`) was built for. The formats should be one
   set, not one for the filesystem and another for the wire.
2. **The contraction as a published part.** A boost highway or a cell-level
   coarse graph is derived once and read by every client. If EDDA publishes
   it, no client builds it.

`TODO-scale-regions.md` item 5 is the same change seen from the writer's
side — "the cell is the unit of storage, of transport and of edit" — and its
`NameTable` line (47 GB at 200 M) is the number this file just met from the
reader's side.

## Meanwhile

Do not open the map on `.index/full` on a 24 GiB machine until item 1 lands;
`.index/7day` is 730,544 systems and behaves as it always did. The perf
guard (`galos_map/src/perf.rs`, `GALOS_PERF_DIR`) only ever ran against a
seven-day directory, which is why none of this showed up in it — a guard at
131 M is the first thing item 0 should add.
