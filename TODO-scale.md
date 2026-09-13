# What breaks as the galaxy grows

The index is built and served for 2.7 M systems today. The galaxy is a
hundred and twenty-nine million, and the question asked was 200 M+. This is
what gives way on the road there, in the order it gives way, with the
numbers it was measured from so that nobody has to measure them again.

Four walls. **Three of them are down**; what each cost and what it now
costs is below, measured the same way twice. The fourth is a design change
and is still standing, on purpose.

## Measured, 2026-09-11

From the live `.galos_index` and `.galos_index.checkpoint`, over
`galos_postimport_backup`: **2,730,800 positioned systems**, 527,077 of them
with anything scanned.

| part | size | files | per system |
|---|---|---|---|
| `bodies/` | 2.7 GB | 527,077, one directory | 5.4 KB per *scanned* system |
| checkpoint | 162 MiB | 1 | 62 B |
| `names/` | 121 MB | 42 chunks | 44 B |
| `cells/` | 108 MB | 3,355 | 40 B |
| whole directory | 2.9 GB | ~531 k | ~1.06 KB |

Resident shapes, for the memory arithmetic below: `galos_index::System` is
56 B (`tree.rs`, and fixed by the resume point's format), `meta::NameEntry`
about 64 B with its heap, a `NameTable::slot` entry about 24 B before the
map's own overhead.

Every projection here is a linear extrapolation from that one point, and it
holds two rates fixed: **19 %** of systems have a scan, and **1 in 40** is
populated. The scan rate is the one most likely to move — it is a function
of how much of the galaxy anyone has honked — and it drives the largest
number on the list.

A cold build over that database, before any of this and after all of it,
both on the same machine against the same live feed:

| | before | after |
|---|---|---|
| peak resident | **5.19 GB** | **1.14 GB** |
| peak footprint | 9.68 GB | 1.32 GB |
| wall clock | 162 s | 98 s |
| systems | 2,733,797 | 2,736,908 |

## 1. `bodies/` was one flat directory — fixed

`galos_index::source::bodies_path` was `dir/bodies/{address}.bin` and
sharded nothing. 527,077 files then, 2.7 GB of the directory's 2.9; **37 M
files and 39 GB** at 200 M. APFS will store them. `readdir` will not enjoy
it — the listing that produced the table above took most of nine seconds —
and neither will a backup, an `rsync`, or `write_bodies`' own sweep for the
files of systems that no longer have a scan.

It is `bodies/{shard:03x}/{address}.bin` over 4,096 shards now, and the
shard is **not** `address % 4096`. An Elite `id64` packs a mass code and
boxel coordinates into its low bits, so the obvious modulo leaves 1,200
shards empty and piles 3,290 files into one. Measured over the 527,396 real
addresses on this disk:

| shard function | shards used | median | p99 | max |
|---|---|---|---|---|
| `address % 4096` | 2,896 | 112 | 1,853 | 3,290 |
| `(address * 0x9E37_79B9_7F4A_7C15) >> 52` | 4,096 | 129 | 157 | 170 |

So the multiply. At 200 M that is about 9 k files a shard, evenly.

`reshard_bodies` moves a pre-sharding directory's loose files down on the
first open — measured at **527,530 files in 85 s**, once — and it is
idempotent, so every open after that is one `readdir` of 4,096 entries.
Until it has run, `read_bodies` falls back to the flat path, and
`remove_bodies` clears both, so a withdrawn scan cannot leave a stale flat
file to be read instead.

## 2. The full build read the galaxy in one `fetch_all` — fixed

`galos_db::index::read_galaxy` was one query materialised whole, and it
built *both* the tree inputs and the names table out of those rows while
holding both. The projection was 12.8 GB of `System` and another ~13 GB of
`NameEntry` at 200 M, plus sqlx's row buffer for 200 M rows.

The estimate was low, because the largest thing a cold build held was not in
it: `bodies_of(db, None)` read every star, body and barycenter into three
`Vec`s and grouped them into a `HashMap<i64, SystemBodies>` — the whole of
`bodies/`, 2.7 GB on disk and more in memory, to write each file once and
measure each reach once. That is most of the 5.19 GB in the table above.

Every read a cold build makes is a cursor now, and nothing it reads is held:

- **`systems` and `stars` are merged.** Both are `fetch`, both ordered by
  address — `systems` by its primary key, `stars` by the leading column of
  its own — and the merge holds one star, not a map of every scanned
  system. The ordered scan of `systems` costs 4.4 s over 2.73 M rows.
- **The inputs are never a `Vec`.** They go straight into the resume
  point's base through `Compaction`, and `Tree::build` is handed a
  `&[System]` pointing into the mapping of what that wrote. The build's
  inputs and the resume point it owes are the same bytes.
- **The names are never a `Vec` either.** They go into the `NameTable` the
  metadata is published from as the rows arrive.
- **The scanned rows are four more merged cursors** — stars, bodies with
  their materials, barycenters, and the systems a supercharge could be
  published for — handed to `each_scanned` a system at a time. The body
  file is written, the reach measured and the arrival star classified while
  one system's rows are in hand.

What is left resident is what the run *holds open*, which is wall 4: the
tree, the names table and the sidecars. A watch pass is unchanged and still
reads its chunk with `= ANY($1)`; it is bounded by `CHANGED_CHUNK`, and it
needs the whole group to say which addresses came back with nothing.

Checked against the old build over the same database: 400 sampled body
files are byte-identical, and `tests/derivations_agree.rs` still passes.

## 3. The checkpoint became unwritable — fixed

62 B × 200 M = **12.4 GB**, written whole and atomically on a
`CHECKPOINT_EVERY` timer, and read whole at startup to rebuild the editable
tree. The timer existed because "a checkpoint is every system at full
precision, and writing one each pass costs more than everything else a pass
does put together" — at 162 MB. At 12.4 GB the timer means nothing and a
restart is a 12 GB read before the first publish.

`Pending` was the right primitive pointing the wrong way: a delta log that a
whole checkpoint *cleared*. The two have changed places.

- **The log is what a publish writes.** One frame: how many systems it
  moved, the cursor that holds once they are applied, and the systems
  themselves at full precision. Measured on the real database: a pass that
  published 84 systems wrote **4,720 bytes**, where the timer's answer was
  153 MB.
- **The base is the compaction**, and happens when the log has grown past a
  sixteenth of it, never below 4 MiB and never past 256 MiB — the ceiling
  being what keeps a restart's replay to a few million systems however
  large the galaxy gets.
- **Every pass records.** There is no state in which the directory is ahead
  of the resume point beside it, so `CHECKPOINT_EVERY`, `Level::checkpointed`
  and `Level::unrecorded` are all gone, on both sides.

The base is a 64-byte header and then nothing but `System` records, 56 bytes
each, exactly as the machine holds one: `Checkpoint::read` maps it and hands
`Tree::build` a slice into the mapping. That is what makes wall 2's spill
free, and it is why `System` is `repr(C)` with a `u32` age bucket and a
compile-time assertion on its width. The file is this machine's — native
order, native layout, a magic read as a native `u64`, the record width and a
format version in the header — which is affordable because it is private,
never served, and a refused one costs a rebuild.

A file in the old MessagePack form is **upgraded the first time it is
read**, log and all. Measured: a 169.5 MB old-form checkpoint over the
flat-`bodies` directory it was written for came back as a 153.1 MB base,
resumed 2,734,116 systems from its cursor without a rebuild, and the run
after it resumed from the new form and replayed the log the first one left.
The read writes, which is a surprise worth stating and is stated where it
happens: a log half in one framing and half in the other reads as the older
one and stops at the join, silently dropping everything published since.

What this does *not* do is make a restart cheap. It no longer decodes a
galaxy — the base is mapped, not parsed — but the tree it builds is still
every system, which is wall 4 and not this one.

## 4. The resident tables end the single-process design

**Which process?** Three run, and only two of them have this problem:

- **Ingestion into Postgres** (`--from … --db`) is per message and holds
  nothing that grows with the galaxy. It is not on this list at all.
- **The builder** (`--index`, either derivation) holds the editable index:
  ~105 GB held at 200 M. A cold build no longer does — its peak is the
  region budget's, see step 1 — so what holds it is the live tree a watch
  resumes into. That is 4a below.
- **The client** (`galos_map`) holds the tables it reads whole: ~35 GB at
  200 M, before it draws a single cell. That is 4b, and it is not fixed by
  fixing the builder.

The cell payloads — the bulk of the directory — are on neither list. The
builder writes them and forgets them; the client holds the ones in view and
drops them when they leave. That part of the design already works at any
size, which is the reason to make the rest look like it.

### 4a. The builder

**Measured, 2026-09-11, not projected from a shape.** Two sizes each, peak
resident under `/usr/bin/time -l`, with the 56 B of inputs held in every
case, so the slope is what one more system costs:

Two kinds of number, and this file confused them twice before getting it
right. **Peak** is the process high-water mark under `/usr/bin/time -l`;
**held** is live heap under a counting allocator, which is what a run
actually carries. `Tree::build` runs `Snapshot::build` first, so its peak
includes a batch build's intermediates and whatever the allocator kept.

| | peak B/system | held B/system | held at 200 M |
|---|---|---|---|
| `Snapshot::build` — a batch build, no live tree | 298 | 56–72 | — |
| `Tree` — the live tree a watch edits | 504 | **273** | **~55 GB** |
| `NameTable`, over this disk's real entries | 235 | — | ~47 GB |
| `Sidecars`: `populated`, `reaches`, `boosts` | — | — | ~3 GB |

Held is flat across 500 k, 2 M and 8 M systems, and the inputs measure at
exactly 56 B a system, which is what says the counter is honest.

So **~105 GB** for a process holding 200 M in a live tree, against the
40 GB this file first guessed from struct widths and the 151 GB it then
read off peaks.

### The container slack is not worth chasing — withdrawn

This file used to say that collapsing `records`, `owner` and `leaf` into
one table and reserving capacity was worth ~3× and should be done first.
Measured, it is not. The tree's analytic floor is about 166 B a system
(`records` 64, `owner` and `leaf` 24 each, a `slice` entry 16, `physical`
8) and it holds 273, so the slack is 107 B — **39 %, not two thirds**. Of
that, merging the three maps saves perhaps 30 B, a denser slice and
physical list perhaps 20 more.

Best case is ~200 B a system, which is 40 GB at 200 M against a 2 GB
budget. It buys nothing that matters, costs a churn of the most delicate
code in the crate, and would have to be undone by 4c anyway, which holds no
per-system state at all. Do not do it.

### 4b. The client

Measured the same way, one table per process run against this disk's
directory, with the 6.18 MB floor of an empty run subtracted:

| table | entries here | resident | per entry | at 200 M |
|---|---|---|---|---|
| `names/` | 2,778,879 | 441 MB | 159 B | **31.7 GB** |
| `populated.bin` | 67,690 | 20.4 MB | 301 B | 1.5 GB |
| `reaches.bin` | 534,322 | 18.9 MB | 35 B | 1.3 GB |
| `boosts.bin` | 133,136 | 6.9 MB | 52 B | 0.5 GB |
| `index.bin` | 3,432 cells | 2.5 MB | 740 B a cell | 0.3 GB |
| `factions.bin` | 70,927 | 8.8 MB | 124 B | ~0.01 GB |

**~35 GB before the map draws anything**, and 90 % of it is the names
table. A builder that no longer holds the galaxy does not fix one byte of
this: the client is a different process reading the same directory, and it
reads these whole because the format gives it no way to read a part.

The fix is the same shape for both halves, which is why they are one wall.
The names table is a flat list with no order — "the client reads the whole
table and indexes it by address itself" — read entire because a search
reaches any name and a route steps between any two places. Keyed by cell
instead, the way the payloads already are, the client fetches the names of
what it is looking at and the *builder* stops holding a names table at all,
because a changed system's name lands in the file its payload is already
being written to. One format change, both halves.

`factions.bin` does not scale with the galaxy — a faction count is a
political fact, not a spatial one — and `index.bin` at 0.3 GB is the
aggregates, which is the one table that should be resident.

### One smaller thing in the same family

On a path that runs at the end of every run: `Tables::named()` collects
every address into a `HashSet<i64>`, which is 5–8 GB transient at 200 M.
`Index::publish_whole`'s copy of the same trick is gone — the trim it fed
now runs over the tree itself, through `Tree::forget`, rather than over a
`HashMap` of the galaxy rebuilt beside it.

**This one is a design change rather than a fix**, and it is where the
builder stops being one process. Nothing above needed it decided first.

### 4c. The budget, and what it forces

Stated 2026-09-11, and it is the number the design is against rather than a
target to approach: **the builder gets 2 GB and the client 2–4 GB**, at any
galaxy size, with a builder given more machine being allowed to go faster
rather than being required to. At 200 M that is 75× under what the builder
holds today and 17× under the client.

No amount of paging the *inputs* reaches it, which is what walls 1 to 3
did. What reaches it is one rule:

> **Nothing per system is resident anywhere. The cell is the unit of
> storage, of transport, and of edit.**

Everything else follows from it rather than being decided separately:

- **The tree stops being a structure in memory.** An edit descends by
  *position* — which the event carries and the resident aggregates index —
  reads the dozen payload files on that path, re-settles ownership among
  them, and writes them back. The `records`, `owner` and `leaf` maps, 55 GB
  of the 105, exist only because the builder chose to remember what the
  path already says. Measured cost of an edit this way: ~360 KB read and
  ~64 KB written, so thirty systems a second is ~12 MB/s.
- **The names go per cell**, which empties 47 GB from the builder and
  31.7 GB from the client at once — see 4b.
- **The sidecars go per cell too.** `populated`, `reaches` and `boosts` are
  ~3 GB at 200 M on *both* sides, which is over budget on its own. Each is
  per system and each belongs beside the payload of the cell that owns that
  system, where the writer is already writing and the reader is already
  reading.
- **The aggregates stay resident**, and are the only thing that does:
  740 B a cell over ~370 k cells is ~274 MB at 200 M. They are what a walk
  plans on, they are what an edit descends by, and they are the one table
  whose size is the *tree's* and not the galaxy's.

What is resident then, at 200 M: the aggregates (~274 MB), a bounded cache
of hot cells (a flag, not a consequence), and the feed's working set. Both
processes land in the low hundreds of megabytes, and the way to spend a
bigger machine is to make the cache bigger.

### The level of detail *is* the memory mechanism

Not a way to keep Bevy's entity count down — that is a side effect. The
budget is enforced by the cut the walk makes and by nothing else, and the
machinery already exists: `cache::Resident` and the three set operations
against a `Needed` — drawing takes what is needed and resident, loading
fetches what is needed and absent, eviction drops what is resident and no
longer needed.

Two things follow, and they decide the shape of everything below.

**Per-system data hangs off the cell or it is a leak.** A `HashMap` keyed
by address that fills lazily as cells are drawn is not streaming; it is the
same gigabyte arriving more slowly, because nothing ever takes anything out
of it. Anything per system must sit *in* the resident cell beside the
points, so that one eviction drops both. That is the strongest argument for
reach and boost being on the payload record: they inherit eviction by being
the payload. The builder wants the same structure under a different policy
— hot by recent edit rather than by view — and should share it rather than
grow a second one.

**The walk's refusal of a budget becomes a memory policy.** `walk_screen`
says outright that "nothing here bounds how many marks come back … a
frame-cost ceiling is a drawing concern that belongs at draw time". That
was free when the whole galaxy's payloads were 98 MB: measured, every zoom
marks 1,641–3,168 cells and 55–98 MB, so unbounded and all-of-it were the
same number. At 200 M they part company — marks are bounded by screen
separation, but the *cells* they live in grow with the tree and each holds
up to `LEAF_CAP` × 43 B = 176 KB. Whether eviction alone holds the budget
or the walk needs a ceiling is a measurement, not an argument, and
`galos_map::perf` already prints the two numbers that settle it.

So the client's budget decomposes as `index.bin` — ~274 MB at 200 M, the
only thing resident that is sized by the galaxy, and per cell so it is
really sized by the tree — plus whatever the level of detail is holding.
Nothing else may be resident at all.

### The four sidecars, and why only one of them stays a table

`names/` is 4b's problem and is dealt with there. Of the other four, ~3.3 GB
at 200 M on each side:

- **reach** is four bytes and is wanted for *every drawn system, every
  frame* — the map sizes the sky by it. It belongs on the payload record,
  which goes 39 B to 43 B: +10 % on a fetch already being made, against a
  1.3 GB table and a second read per cell.
- **boost** is two bits, and there are five spare in the record's
  `temp_bucket` byte, `TEMP_BUCKETS` being 6. It costs nothing at all — and
  it carries the router: `id64`, position and boost are the whole of what
  `JumpGraph` needs, so the payload *is* the jump graph and routing stops
  wanting a resident table. 0.5 GB to nothing.
- **populated** is ~300 B and belongs to one system in forty, so putting it
  on every record would serve 2.5 % of them. It stays a file and is
  **re-sorted into cell order** with a per-cell offset table — 370 k
  offsets, 3 MB resident — so a drawn cell range-reads its ~20 KB and
  nothing else is ever in memory. One file rather than a file per cell:
  fewer inodes, and one `Range:` request over the transport to come.
- **factions** is 12 MB at 200 M and is political rather than spatial. It
  does not grow with the galaxy. Leave it whole.

The ordering is the part that matters, not the file count. As written today
these are address-ordered MessagePack: variable-width, so there is nothing
to seek to, and address order scatters a cell's systems across the whole
file — mmapped, drawing one cell would touch four thousand separate pages
to read sixteen kilobytes. Cell-ordered and fixed-width where it can be,
the same bytes are one contiguous read.

### A filter at distance is a column, not a table

`Filter::Faction` walks the whole resident `Populated` map today
(`galos_map/src/systems/filter.rs:485`), which is the only reason that
table has to be whole. But a filter is never a galaxy-wide enumeration: it
is the systems in view when they are drawn, and an aggregate when they are
not. The format already knows this — `Aggregate::aged` is "a column of the
record so a Recency span can be answered by prefix sum off the aggregates
alone" — and the political axes simply have no such column yet.

Adding them costs a rounding error: allegiance is ~6 values, so `[u32; 6]`
is 24 B a cell and **8.9 MB** over 370 k cells, against 1.5 GB of resident
rows. Government, security and economy are the same argument at 19, 6 and
18 MB. It is also strictly more than the table can do: "where in the galaxy
is the Empire" becomes a glow at any distance rather than nothing until the
systems are individually drawn.

Faction is the exception and the reason is cardinality, not design: ~100 k
ids cannot be a column on every cell. Faction filtering is what is in view,
or later an inverted index of faction to the cells it appears in, which is
small because a faction sits in a handful of systems in one neighbourhood.

### What 4c costs, stated before it is started

- **Search.** "A search reaches any name" is why the names table is read
  whole. Per cell, a name lookup needs an index that is not the table: a
  prefix index, sharded by prefix, fetched on demand. Elite's names are
  strongly structured (`Col 285 Sector AB-C d1-23`), so prefix sharding is
  unusually well suited, but it is a new artifact and the client's search
  path has to learn it.
- **Routing.** "A route steps between any two places" — the router reads
  positions and boosts along a corridor, which per cell is a spatial fetch
  and arguably more natural than a resident table. It is still a rewrite of
  how the router asks.
- **A system that moves.** Descending by position finds where a system *is*
  now, not where it was. A corrected position would leave the old record
  behind as a duplicate. Positions are corrected "about never", but never is
  not a guarantee: it wants either a repair sweep at compaction or a small
  record of moves, and it is the one correctness question 4c opens.
- **`cells/` becomes wall 1 again.** 370 k payload files at 200 M, plus a
  names file and a sidecar file each, is over a million in one flat
  directory. It shards the way `bodies/` now does, with the same function.

## 5. Every publish rolled up the whole galaxy — fixed

Found while measuring wall 4's shapes, and it is not a memory problem, so
nothing above would have caught it. `Tree::publish` goes through
`to_snapshot`, and the tree deliberately does not maintain its aggregates:
they "compose exactly but drift under repeated floating-point addition and
subtraction, and `m_min` cannot be recovered from a summed flux at all", so
they are recomputed **from every record in the galaxy** on every publish.

Measured, with the tree already standing in the directory so the publish is
the incremental one a watch pass makes:

| systems | edits | `apply` | `to_snapshot` | whole publish | payload cells rewritten |
|---|---|---|---|---|---|
| 2 M | 100 | 0.40 ms | **477 ms** | 484 ms | 214 (9.1 MB) |
| 2 M | 10,000 | 25 ms | **495 ms** | 609 ms | 2,070 (77.7 MB) |
| 8 M | 100 | 0.87 ms | **2.55 s** | 2.61 s | 238 (6.4 MB) |
| 8 M | 10,000 | 45 ms | **2.54 s** | 3.03 s | 11,161 (243 MB) |

So the roll-up is ~0.3 µs a system and everything else is noise beside it.
At 200 M that is **about a minute per publish, for one changed system**, on
a `--publish 5` beat. The edits themselves are 2.5–8.7 µs each and grow
like the depth of the tree; a feed at thirty systems a second is not the
problem and never was.

The fix was not a fix to the arithmetic, and it needed no format change
either: the aggregates a sibling contributes are already in `index.bin`,
which the builder holds. `Tree` maintains them now, in `agg` and
`owned_below`, and `Tree::settle` works out the cells whose subtrees moved
— what `bump_count` marked along the path, what a slice change dirtied, and
their ancestors — deepest first. A leaf is re-summed from its own members,
at most `LEAF_CAP` of them; an internal cell is the merge of its children.
Nothing is subtracted, so `m_min` stays recoverable and nothing drifts with
the number of edits.

`Tree::publish` then assembles the index off those totals in one pass over
the cells and builds a payload only for a cell it is about to write. It
used to build every payload in the galaxy to write a dozen files.

**Measured after, against the table above:**

| systems | edits | publish before | after |
|---|---|---|---|
| 2 M | 100 | 484 ms | **52.9 ms** |
| 8 M | 100 | 2.61 s | **52.4 ms** |
| 2 M | 10,000 | 609 ms | 419 ms |
| 8 M | 10,000 | 3.03 s | 2.05 s |

A hundred changed systems costs the same at 8 M as at 2 M, which is the
claim. The ten-thousand rows are still large and are *supposed* to be: 10 k
random edits dirty about 11 k cells and 243 MB of payloads at 8 M, which is
what moved rather than what exists.

Held to it by the oracle — `every_edit_stays_equal_to_a_rebuild` compares
the live tree against a fresh build after every single operation, with
`rank_lo`, `rank_hi`, the count and `m_min` exact and the flux to one part
in a million.

What is left of a publish that is not proportional to the change is
`index.bin`, written whole: ~73 MB at 200 M. That is the item below and
this one no longer hides it.

### The age bucket cannot be re-binned at publish after all

Item 10 of `TODO-source-sink.md` wants `age_bucket` worked out at publish
from `updated_at` rather than frozen at read time. That is incompatible
with a maintained roll-up: every cell's age column would change as the
clock moved, whether or not anything in it did, and the settle would be the
galaxy again.

The way out is to stop binning by *age* at all and bin by **absolute
date**: a column counting systems per calendar bucket is time-invariant, so
it is maintained incrementally, and the client — which knows what time it
is — maps a Recency span onto the buckets it wants. That is a change to
what the aggregate *means* to a reader, so it belongs with the other
served-format changes in 4c rather than here.

### And a region cannot be a level

The obvious way to bound a build is to take one top-level cell at a time.
Measured over this disk's 2,771,734-system index, the worst cell's share of
the galaxy by level:

| level | cells | median | max | max as a share | × the mean |
|---|---|---|---|---|---|
| 1 | 8 | 153,156 | 1,401,727 | 50.6 % | 4× |
| 2 | 20 | 8,220 | 1,391,689 | 50.2 % | 10× |
| 3 | 43 | 7,761 | 682,856 | 24.6 % | 11× |
| 4 | 104 | 4,243 | 462,143 | 16.7 % | 17× |
| 5 | 230 | 2,091 | 264,644 | 9.5 % | 22× |

Halving the worst share costs about a level and a half, so no fixed level
is a bound — and this directory's skew is the feed's (the bubble), where an
import's is the galaxy's (the core), so the shape cannot be assumed either.
A region has to be **split by count, not by level**: a cheap streaming pass
over positions alone, counted into a grid a few levels down — 2 M buckets
is about a hundred megabytes — and then those buckets packed greedily into
regions under a memory budget. One extra read of one column, and the split
is measured on the dataset in hand rather than guessed from this one.

### `index.bin` is rewritten whole, per publish

675,586 B for 3,412 cells, so 198 B a cell: ~73 MB at 200 M, written
entire every time anything is published. It is the one file `Snapshot::write`
and `write_diff` both replace wholesale, on the grounds that it is "a few
megabytes over a galaxy and cheap to replace". At a five-second beat that
is 15 MB/s to say that one system moved, and it wants the same treatment
the payloads already have: per-region files, or a delta the client folds in.

### The payload is very nearly the record already

Wall 3 says the tree cannot be rebuilt from the served directory, because a
`Point` "carries a downcast `f32` magnitude, a bucketed temperature and no
age bucket". Read field by field against what the tree actually consumes,
that is one loss and two misreadings:

| field | on the wire | what a rebuild needs |
|---|---|---|
| `pos` | 3 × `f64` | **exact**, carried through unchanged |
| `temp_bucket` | `u8` | **exact** — `Record.temperature` reaches only `Aggregate::of_system` and `Point::new`, and both call `temp_bucket` on it at once (`tree.rs:603`, `:644`). The value beyond the bucket has no consumer. |
| `updated_at` | `u32` | **exact** |
| `age_bucket` | absent | **derived**: `derive::updated(updated_at, now)` |
| `magnitude` | `f32` | **exact** |

Nothing on the wire is lossy now. The magnitude was an `i16` centimag —
0.01 mag, a 0.92 % flux error a system — and is an `f32`, the width
`stars.absolute_magnitude` and the dump both hold. The record went 39 B to
41 B and `INDEX_VERSION` to 2, which refuses a directory written by the
older codec.

So the payload can be canonical: the tree holding what the payload carries,
and the age bucket recomputed from `updated_at` at publish. That makes the
served directory the durable record, and a cell's aggregate re-derived from
its own payload exactly what any other derivation computes — which is half
of `TODO-source-sink.md` item 5's complaint that the aggregates cannot
cheaply be made deterministic.

### The age bucket is frozen at read time, which is a bug

Found while checking the above. `age_bucket` is computed against `now` when
a row is read and then kept in the tree; a system nobody reports again
keeps the bucket it was built with. Over a week's watch a cell's Recency
histogram rots while the `updated_at` on the payload beside it stays exact,
so the far view and the near view of the same filter disagree about a
system — which is the one thing `System`'s own doc says cannot happen.
Recomputing the bucket from `updated_at` at publish costs one subtraction
per system and is required anyway if the payload becomes canonical.

## What does not get worse

Worth writing down so that nobody optimises the wrong thing:

- The event side's `galos_index::galaxy::Galaxy` scales with feed traffic,
  not with galaxy size. It has its own unbounded-growth problem — see
  `TODO-source-sink.md` — and that problem is independent of this one.
- `names/` reaches 3,052 chunks, which is fine. `cells/` reaching ~370 k
  files in one flat directory is **not** fine and this file used to say it
  was — it is wall 1 again, with the same fix. See 4c.
- A watch pass's *reads* are bounded by `CHANGED_CHUNK` whatever the galaxy
  holds. What it then publishes is not — see wall 5.
- The merge rules, the report fan-out and the sidecar tables are per system
  and asymptotically neutral. Measured: 100,001 events through the event
  path took 13.6–14.0 s before the unification and 13.6–14.0 s after, at
  604.0 MB and 598.3 MB peak RSS.
- The manifest proposed for `TODO-source-sink.md` item 1 is O(parts): one
  small file, unaffected by galaxy size.

## Order

1. ~~**Shard `bodies/`.**~~ Done. A published directory holds a median of
   129 entries per bodies subdirectory and a maximum of 170, and
   `FsSource::bodies` reads a system written by the old layout or the new
   one.
2. ~~**Page or stream `read_galaxy`.**~~ Done, and the body rows with it. A
   full build's peak is what the run holds open rather than what it read:
   5.19 GB to 1.14 GB, 162 s to 98 s. The two derivations still agree —
   `tests/derivations_agree.rs` is what says so.
3. ~~**Invert the checkpoint and `Pending`.**~~ Done. A publish writes what
   moved — 4,720 bytes for 84 systems — and a restart resumes from a mapped
   base and a log rather than a 153 MB decode.
4. **The resident tables**, against a stated budget: 2 GB for the builder,
   2–4 GB for the client, at any galaxy size. ~105 GB held by a watch's
   tree and ~35 GB measured for the client today at 200 M. The budget
   forces 4c — nothing per system resident, the cell as the unit of
   storage, transport and edit — so the shape is no
   longer the open question; the order of arriving at it is.
5. **The publish's roll-up.** A minute per publish at 200 M for one changed
   system, and the cheapest of the five to state: roll up the changed paths,
   not the galaxy. It shares a format question with wall 4's third shape, so
   it is decided with it and not before it.

### Where step 1 stands

`galos_index` has its half, in this tree. A region is a cell and the
systems in it; `Snapshot::of_region` builds one, and
`Snapshot::build` is now that function with the root cell and nothing
claimed, so there is one build and not two. `region::Offer` is what a
region streams out of itself — its brightest `level × internal_slice`
systems, which is all the cells above it could ever take, and its total —
and `region::Crown` settles those cells and answers what it claimed.
`region::joined` makes the index out of the crown's cells and the regions'.
`a_regional_build_is_the_whole_build` holds the whole of it to equality
with a whole build, cell for cell, payload for payload, over a deliberately
lumpy galaxy at two depths of cut.

The caller is in too. `galos_db::index::build_to_dir` is the cold build now
and it is regional, and the counting pass this item first described is gone
with it: nothing is counted before the galaxy is read. `build_cells` merges
the ordered `systems` and `stars` cursors and pushes each row into
`galos_index::Build` under `GALOS_REGION_BUDGET` systems (7 M by default,
about 2 GB at the measured 298 B). Each system goes straight to the spill of
the fixed level-4 `galos_index::bucket` its position falls in and each name
into a `galos_index::Chunks` that goes to disk as it fills, so the counts
fall out of the writing. `Build::finish` forms the cut from those counts,
grouping the sparse buckets into regions and dividing a bucket over the
budget by re-reading that bucket alone one level deeper; then it offers,
crowns, and builds and writes one region at a time off its spill, appending
each spill to the resume point's base and deleting it. Nothing holds a
`Tree`: `bring_level` gets one by *resuming* from what the build just wrote,
which is where a live tree's cost belongs.

**Measured over 2.79 M systems, `--only cells,names`:**

| budget | regions | peak resident |
|---|---|---|
| 7 M — the whole galaxy in one region | 1 | 677 MB |
| 200 k | 62 | **149 MB** |

Same 3,432 cells either way, and `placed == systems` both times. The
build's peak is the budget's now, not the galaxy's. A whole run — build,
then resume into a watch — peaks at 1.5 GB, and all of that is the live
tree the resume raises: wall 4a, untouched and next.

What is left of step 1: the sidecars still go through maps
(`populated`/`reaches`/`boosts`, ~3 GB at 200 M), which is 4c's work and
wants the cell-ordered format rather than a streaming writer for the
current one. `cells/` is sharded and migrated.

