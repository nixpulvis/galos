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

**The names table is mapped now** (`galos_index/src/names.rs`, item 1): an
address-sorted fixed-width base plus an append-only log, opened with five
`mmap` calls and nothing resident. The step before it — packing the decoded
table into arrays in `galos_map` — measured **7.2 GB peak resident and 33 s**
over the real 200,071,629-name table, against the ~47 GB the original form
wanted at 235 B an entry; that was the same shape of answer, and the
residency was the bug rather than the layout of the residency. The router's
bucketing is built on the first route rather than at startup, so a session
that only looks at the sky never pays it. What is left is in items 1a–1d and
item 2 below.

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
| the names table | `Vec<NameEntry>`: 48 B a struct plus a heap block a name, ~235 B an entry measured → **~47 GB**; then four arrays and a blob, **7.2 GB measured** | a mapped file, **~5.8 GB on disk and nothing resident**, `galos_index/src/names.rs` |
| address → entry | `HashMap<i64, usize>`, ~24 B an entry → **~4.8 GB** | gone: the addresses are the index, so it is a binary search of a mapping |
| exact name → address | a scan of every entry, **11.3 s measured**, 4–6 per route plot | a binary search of `byname.bin` |
| the reaches | `HashMap<i64, f32>` over 76 M rows → **~2 GB** | two sorted arrays, 12 B a row → **912 MB** — still decoded at startup, item 1a |
| the read itself | the whole table decoded into one `Vec` before packing anything → the peak again | not decoded at all |
| `Places` (the router's grid) | built at startup whether or not anything routed: 32 B a point, an address map beside it, a bucket per occupied cell → **~16 GB and ~72 M allocations at 200 M** | built on the first route asked for, from `Names::points` — still the worst structure left, item 2 |
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

## Search: one scan, 131 M allocations — what it was

Kept as the record of what item 1 was answering; the first three bullets are
now a binary search of `byname.bin` and a capped `match_indices` scan of
mapped bytes.

- Fires on Enter only, not per keystroke (`ui.rs:2092-2097`, `:6289-6291`) —
  so the typing lag was never search.
- `Names::find` walked every entry and called `e.name.to_lowercase()` on
  each: **131.29 M String allocations** and 131.29 M substring searches,
  uncapped `Vec` of matches, sorted whole, then truncated to 25
  (`search.rs:323-341`, `RESULTS = 25` at `:27`). The fold went first, with
  `SystemName`; the cap and the scan went with item 1.
- It ran **on the main thread**: the `AsyncComputeTaskPool::spawn` at
  `search.rs:289-293` wrapped a value that had already been computed. **Still
  true** — the work is now `O(log N)` plus a capped scan, so it matters far
  less, but the task is still a lie and should either do the work or go.
- Plotting a route resolved its endpoints through `Names::address` — another
  full scan each, two per leg at `route/fetch.rs:100` plus one per stop in
  `route/mod.rs:539`. **Four to six full scans of 131 M entries to begin a
  route**, and the single biggest thing `byname.bin` deleted.
- Labels never used any of this: they go through the address path
  (`systems/spawn.rs:912`), which is fine — and item 1c is how it gets
  cheaper still.

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
  system write. **~~What is left of this bullet is the cap~~ — also done:**
  the cap is in the index now, so nothing collects the galaxy to truncate
  it to 25.
- **~~Resolve a name once~~ — done, and better than the interim.** Route
  endpoints and `names_exactly` were four to six full scans per plot. The
  `HashMap<&SystemName, i64>` this bullet proposed is unnecessary: item 1's
  `byname.bin` resolves a name in `O(log N)` off a mapping, with nothing
  resident to build. The invariant that made it possible is
  `SystemName`'s — a case-insensitive comparison has no order to
  binary-search.
- **A budget on every route.** Expansions and wall clock, with a partial
  answer and an explicit "not proven minimal" flag in the UI. A route that
  cannot finish must say so in a second, not in an hour.

Nothing here is the fix; all of it is the difference between unusable and
usable while the fix lands.

### 1. Names off the heap — built, and what it left

**Landed as a mapped, address-sorted format with an append-only log**
(`galos_index/src/names.rs`). The chunked MessagePack table is gone: not
held differently, not decoded faster — not decoded. Opening is five `mmap`
calls and six length checks, and what a session touches is what the kernel
pages in.

```text
names/head.bin          64 B: magic, version, live generation, count, name bytes
names/<gen>/addr.bin    N x i64        strictly ascending
names/<gen>/pos.bin     N x [f32; 3]
names/<gen>/byname.bin  N x u32        row numbers, sorted by name bytes
names/<gen>/span.bin    (N+1) x u40    offsets into text.bin
names/<gen>/text.bin    the name bytes, in address order
names/delta.bin         append-only log of changed rows
```

| | was | is |
|---|---|---|
| on disk | 8.7 GB over 3,053 chunks | 29 B a row + ~25 B a name → ~5.8 GB |
| resident | 7.9 GB packed (47 GB unpacked) | **nothing** |
| address → row | `HashMap<i64, usize>`, 4.8 GB | the addresses are the index |
| exact name → address | a scan of the galaxy, 11.3 s, 4–6 per route plot | a binary search of `byname.bin` |
| a publish | a 3 MB chunk rewritten | the changed rows appended |
| a refresh | the moved chunk re-read and diffed per entry | the log's bytes past the client's offset |

Four things are worth carrying forward as *decisions*, because each one was
a fork:

- **Structure of arrays, not records.** An address lookup walks `addr.bin`
  alone, 8 bytes a step, and never faults a position or a name it will not
  answer with. This is where the format diverges from EDDA's EDGX, which
  packs 29-byte records — correctly, for a reader whose hot path is a
  spatial scan reading position and class together. Ours starts from an
  address.
- **Sorted by name, not hashed.** A hash index would have been far cheaper
  to build (a numeric sort, no name reads) and would answer an exact name
  just as fast, but it cannot answer a prefix. `SystemName` being upper
  case by construction is what makes a byte-sorted index possible at all
  (`galos_index/src/name.rs:16-18`), and that promise is now cashed.
- **A generation, swapped by one rename.** A build reads the galaxy for as
  long as that takes and the table beneath it is served the whole time,
  which `cold.rs::a_stopped_build_leaves_the_directory_alone` pins.
  Sections are written into `names/<gen+1>/` and `head.bin` is renamed over
  last; the old generation is then unlinked, and a reader that mapped it
  keeps reading, a mapping outliving the directory entry.
- **The by-name order is a radix over the name bytes**, not a sort with a
  comparator that reads the mapping: that would be 200 M random reads into
  5 GB and hours of page faults. Bucket by first byte, re-bucket an
  oversized bucket by the next, sort a bucket that fits. Every pass is
  sequential and the working set is bounded at 256 MB whatever the galaxy's
  size. EDDA pays the same cost in RAM instead (`import.rs:417-446`, and
  its note that materialising a lowercase `String` per system wanted 8–10
  GB).

**Measured**, on the real 200,071,629-system table (`.index/full`, rebuilt
2026-09-13; `cargo run --release --example names_bench -p galos_index --
.index/full`, peak resident off `/usr/bin/time -l`):

```
open      200071629 systems in 646.8µs        (cold), 103.2µs (warm)
address   1001 of 1001 found, 5.4µs each
by name   1001 of 1001 resolved, 25.1µs each
search    5 hits for "SOL" in 3.0ms
search    1 hits for "SOLATI" in 10.1µs
search   25 hits for "COL 285" in 1.9ms
peak RSS  745 MB opening, looking up and searching
```

| | was | is |
|---|---|---|
| the open | **33 s** | **0.6 ms** |
| peak resident, a session that draws, looks up and searches | **7.2 GB of heap** | **745 MB of evictable file pages** |
| an address | 322 µs (faulting a 1.6 GB array) | 5.4 µs |
| an exact name | **11.3 s** | **25 µs** |
| a search for `SOL` | a scan of the galaxy — it hung the map | **3.0 ms** (item 1f) |

Two things to read out of it. **The open is five `mmap` calls and six
length checks, and that is the whole of it** — 0.6 ms against 33 s is not a
faster read, it is the absence of one. And the resident figure changed in
kind as well as size: 745 MB of *file-backed, clean, evictable* pages that
the kernel drops under pressure, against 7.2 GB of anonymous heap that it
could only swap.

The sections, for the record: `addr.bin` 1.60 GB, `pos.bin` 2.40 GB,
`span.bin` 1.00 GB, `byname.bin` 800 MB, `text.bin` 3.94 GB — 9.1 GB in
all, against 8.7 GB of chunks. **The file got bigger and the open got
33,000× cheaper**, which is the trade the whole item is: `byname.bin` and
`span.bin` are 1.8 GB that the chunks did not carry, and they are what
deleted the 11.3 s scan.

#### 1f. ~~`matching` falls through to a scan~~ — cut; search is a prefix

**The scan is gone.** `Table::matching` is `rows_starting` and nothing
else.

It was worse than the measurement above made it look. `matching` took
prefix hits off `byname.bin` — 13 µs — and then, *whenever it had not
filled its limit of 25*, scanned all 3.94 GB of `text.bin`: 564 ms warm,
5.7 s cold, on the main thread, for most queries. But the wait was the
lesser half. **Faulting the whole names blob in evicted the cell payloads
the map draws from** — a zoom pages in 6.1 GB of them (item 2's
measurement) — so searching `SOL` stalled the galaxy's reads as well as
the frame. Reported from the map as "searching for SOL hangs and is
killing the systems IO", which is exactly what it was: not a slow search,
a search that evicted the working set.

Measured after, over the real 200,071,629-name table:

```
search    5 hits for "SOL" in 3.0ms
search    1 hits for "SOLATI" in 10.1µs
search   25 hits for "COL 285" in 1.9ms
search   25 hits for "PRAEA EUQ" in 606.3µs
```

**What was given up: a name with the query in the middle.** `SOL` no
longer answers with `NEW SOL`. That is a real loss of search reach and the
way to have it back is an index, not a scan:

- **A word index.** `byname.bin` keys each name by its first byte; a
  second section keyed by *every word start* would answer `SOL` with `NEW
  SOL` in the same binary search. Elite names average ~4 words, so it is
  ~800 M entries — at 8 B a row (u32 row, u32 offset into the name) about
  6.4 GB mapped, which is the biggest section by a distance and wants
  measuring against how much anyone actually searches mid-name.
- **Parallelising the scan was the other candidate and is the wrong
  answer.** It divides the 564 ms by cores, but it still reads 3.94 GB and
  still evicts the payloads — it makes the symptom faster and the cause
  worse.

Also still true, and now the remaining wart in search: it runs **on the
main thread**. The `AsyncComputeTaskPool::spawn` at `search.rs:289-293`
wraps a value already computed. At 3 ms that is survivable where 5.7 s was
not, so it is no longer urgent — but the task is still a lie and should
either do the work or go.

**EDDA does the same thing, and has no substring search either.** Checked
against the implementation rather than the docs. `Galaxy::find`
(`ed-galaxy/src/format.rs:950-970`) is a binary search of the same
`byname.bin` shape; `Galaxy::complete` (`:976-1013`) is a lower-bound
search then a walk forward while the names still `starts_with`, breaking
at the first that does not. A grep for substring or `contains` across
`ed-galaxy` finds nothing, and station search is no exception — it is
Postgres, but still a prefix: `lower(st.name) LIKE lower($1) || '%'`
(`ed-api/src/names.rs:61-78`) over a `text_pattern_ops` index, with `%`,
`_` and `\` escaped so a user's wildcard stays literal. So the reach item
1f gave up is reach the reference implementation never offered.

**Ours is a probe cheaper.** Theirs calls `to_lowercase()` *inside* the
loop — `format.rs:962` in `find`, `:992` and `:1006` in `complete` — so an
exact lookup is ~27 heap `String`s and a prefix walk one more per hit.
`SystemName` makes ours bytes against bytes with nothing allocated. Their
*builder* already knows this: `import.rs:159-172` has an ASCII
`lowercase_cmp` and the byname sort works on bare `u32` keys because
"materialising a lowercase String per system took roughly 8-10 GB at
full-galaxy scale" (`import.rs:417-446`). The writer got the treatment and
the reader never did, which is the price of normalising at read time
instead of making it an invariant of the type.

Two things of theirs to take:

1. **A minimum prefix.** `MIN_PREFIX = 2` (`ed-api/src/names.rs:29`), with
   the reason put well: "one character of a galaxy-wide index is every
   system starting with that letter." `usable_prefix` answers too-short
   and too-long as an empty list — a one-character query is not a
   question. Ours answers `S` with the first 25 systems in name order,
   which is noise dressed as an answer. They also clamp `DEFAULT_LIMIT =
   12`, `MAX_LIMIT = 25`, `MAX_PREFIX = 64` at the edge.
2. **The sparse-install guard**, which matters the moment item 3's range
   requests exist. On a partly downloaded index a hole reads as *zeros*,
   and `format.rs:944-949` names the failure exactly: "comparing against a
   zeroed name would silently corrupt the binary search". So `find` and
   `complete` bail to `None` the moment they would touch a record whose
   bytes have not landed — fewer suggestions, no wrong ones. The price is
   that `cell_of_record` is itself a binary search, so a sparse install
   makes a lookup `O(log² N)`.

#### 1a. What is still MessagePack, and still decoded at startup

Of the 32.7 s measured open, ~17 s was the names and the rest was the other
sidecars. Mapping the names does not touch that:

- **`reaches.bin`** is the one that matters: 751 MiB at 131 M, **76,044,388
  rows** at 200 M, decoded whole into `Vec<SystemReach>` and packed into
  `galos_map::names::Reaches` (~912 MB resident). It wants exactly the
  treatment the names just got and it is a much smaller change — 12 B a
  row, `[i64 address][f32 reach]`, already written address-sorted
  (`sidecars.rs:363-374`), so the writer needs no sort at all. **Do this
  next; it is the rest of the open.**
- `populated.bin` (11 MiB) and `boosts.bin` (39 MiB) are small enough to
  leave decoded, and saying so is a decision rather than an oversight.

#### 1b. `pos.bin` is a duplicate, and item 2 is what makes it removable

Positions are on disk twice, and were before this change: the cell payloads
hold `[f64; 3]` per system in a 41 B `Point` (8.2 GB of payloads), and
`names/<gen>/pos.bin` holds `[f32; 3]` address-sorted — **2.40 GB, 41 % of
the names base**. Every byte of it is derivable.

It is there because **the names table is the only address-ordered structure
in the index**. The payloads are keyed by space and there is no
address→cell index anywhere in the tree, so "where is system X" cannot be
answered from payloads at all today. Removing `pos.bin` now would cost:

- an address-sorted `cell.bin` of u32 cell ordinals, **+800 MB**;
- one position lookup going from a single page touch to decoding a whole
  cell — `Index::read_payload` deserialises a `Vec<Point>`
  (`store.rs:208-223`), and a leaf at 200 M/204,466 cells averages ~978
  systems ≈ 40 KB. Fine per route stop; 12 MB of decode for the filter
  panel's 314 members;
- `Table::positions()` ceasing to be a zero-copy `&[[f32; 3]]` slice, so
  the router's bulk read walks 8.2 GB of 41 B records to extract 24 B
  fields it narrows to f32.

Net **−2.4 GB, +0.8 GB, and the two hottest position paths get worse**.
The trade only turns once item 2 deletes `Places` — it is the *only*
consumer of bulk positions — after which nothing asks for more than a
handful of positions at a time and paging one cell is fine. **So: delete
`pos.bin` as part of item 2, not before it.**

One economic note, because the rewrite changed it: duplication in a mapped
file costs disk and cold page faults, not RSS. 2.4 GB of positions used to
be 2.4 GB of resident heap on a 24 GiB machine, which was unaffordable; it
is now 2.4 GB of file touched only where a session touches it. The urgency
to deduplicate dropped by most of its reason.

#### 1c. A payload record that points into the names

The stronger version of the same observation, and a separate change: give
the payload `Point` a `name_row: u32` (+800 MB, 41 → 45 B a record). That
removes the address binary search from the **draw** path — `build_system`
does one `Names::get(address)` per system drawn (`spawn.rs:857`), ~28 page
touches into `addr.bin` each, for every newly drawn point every frame — and
turns it into a direct index.

Blocked on build order, not on the format: payloads are written per region
as the read goes (`cold.rs:434-443`), while name rows are only assigned by
the external sort at `Writer::finish`. The row does not exist when the
payload is written, so this needs a second pass stamping 200 M records,
~9 GB rewritten per build. EDDA hits the identical problem and pays for it
— it assigns name offsets after its spatial sort, "one extra ~4 GB copy"
(`import.rs:352-357`). Worth doing; note that it *adds* a field rather than
removing `pos.bin`, so 1b and 1c are not substitutes.

#### 1d. The log has to be collapsed, and the EDDN feed never ends

The delta log is append-only, so its bound is entirely in when it is
folded. What is built:

- An unchanged report appends **nothing** — `Names::name` compares against
  the mapped base row first — so the log tracks the systems that are new or
  changed, not the messages that arrive. That is what makes a firehose
  affordable.
- `Delta::worth_folding` trips on **either** 256 Ki addresses mentioned
  *or* 64 MB of file, and `Sidecars::compact_names` folds base+log into a
  new generation and re-opens. Two thresholds because the count and the
  file do not move together: an address the log already mentions that
  changes again appends a row and leaves the count where it was. Renames
  are near-never, but "near-never" is not a bound.
- At ~60 k newly-named systems a week the count threshold is reached about
  monthly.

What is **not** built, and should be considered before this runs for a
year:

- **The fold is synchronous and on the publish path.** It rewrites the
  whole base — an external sort over ~5.8 GB plus the by-name radix, call
  it ~25 GB of I/O — so the feed stalls for the duration, monthly. The
  sink's resume cursor absorbs it, so this is a latency spike and not lost
  data, but it has not been measured and it is the obvious thing to dislike.
- **The fix, if the stall bites: numbered log segments.**
  `names/delta/NNNNN.bin`, rolled at a size cap, exactly as the old chunk
  table was numbered from zero with no manifest. A fold then consumes
  segments `[0..k)` while the feed keeps appending to `k`, and dropping the
  folded prefix is an unlink rather than a rewrite — which is what makes a
  *background* fold possible at all. A client's cursor becomes
  `(segment, offset)` instead of one offset.
- **Write amplification is fine and should not be the reason to act**:
  ~5.8 GB rewritten monthly is ~70 GB/year. The stall is the problem, not
  the bytes.

#### 1e. Reindexing without reimporting — three tiers, and one gap

A 610 GB dump import is an afternoon, so no format change may require one.
What a change costs depends on where the content already is.

1. **Derivable from the directory itself — no source at all.** The names
   are the case: `galos-index fold-names <dir>` reads the chunks a row at a
   time, external-sorts, writes a generation and swaps it in;
   `names::compact` → `Writer::onto` is the general form, seeding from the
   table *as it answers* (base under log). Minutes, and generations make it
   safe against a live reader — the new sections land in `<gen+1>`,
   `head.bin` is one rename, a reader that mapped the old one keeps it, and
   a fold that dies leaves the table that stood.
2. **The tree, payloads and aggregates — from the resume point, not the
   dump.** `.index/full.checkpoint` is 10.4 GB of mapped fixed-width
   `System` records (56 B, `checkpoint.rs`) plus a `Pending` log, and
   `Build::begin(Start::Resuming)` already pushes every one of them back
   into the buckets — `cold.rs:265-269` makes the argument: 11 GB read and
   written against the 610 GB the systems came out of. Push nothing from
   the source and `finish()`, and that is a complete rebuild.
   **What is missing is only the front door: `galos-index rebuild <dir>
   <checkpoint>`**, which is begin-resuming, push nothing, finish. Small,
   and it covers the names too, a cold build's `Writer::onto` reading the
   published table.
3. **A genuinely new per-system measurement** needs the source. Note that
   1c's `name_row` is *not* in this tier — it is derivable, so a stamping
   pass over the payloads is enough.

**The gap, and it wants fixing at v1 while it is free.** `Table::open`
refuses `version != VERSION` outright. Right for a client; wrong for tier
1, because a future v2 reader then cannot read a v1 directory in order to
re-derive it, and tier 1 quietly stops working at the first bump. The chunk
case only worked because `fold_chunks` carries its own legacy reader, and
that is a treadmill if every bump needs one.

EDDA's answer is the better one and is normative there: `record_len` is a
*field* rather than a constant and the record accessor branches on the
header version (`format.rs:562-570`), so a v3 reader must also read v2 and
v1 while a v2-only reader rejects v3 by the version field alone. Do the
same — make the version a runtime field with per-version section
accessors — so `Writer::onto` can always read whatever is on disk. It costs
nothing with exactly one version in existence.

This is `galos_index` work, and it is the part that most wants to be decided
*with* the EDDA formats rather than before them: these are exactly the
"parts" a served index hands over.

### 2. Routing: a smaller graph, not a faster index

**Measured for the first time at 200 M**, the perf guard having only ever
been pointed at a seven-day directory before
(`GALOS_PERF_DIR="$PWD/.index/full" cargo test -p galos_map --release --lib
perf -- --nocapture`, 2026-09-13):

```
zoom      10 ly: walk 25.96ms  marks 151619  read 24.10s  points 152693955 (6.1 GB)
zoom    1000 ly: walk 55.62ms  marks 150841  read 23.72s  points 151982317 (6.1 GB)
zoom   25000 ly: walk 36.59ms  marks  97891  read 13.86s  points  98225419 (3.9 GB)
zoom  100000 ly: walk 25.47ms  marks  12503  read 227.66ms points 10453036 (0.4 GB)
route graph: 200071629 systems in 34.23s     FAILED its ceiling
```

Three readings. **The LOD walk is fine and always was** — 26–56 ms to plan
what to draw, which is the only path whose work is proportional to what is
on screen. **The payload read is the next thing after item 1a**: 24 s and
6.1 GB to page in 152 M points for one zoom, which is what the first
bullet below is about. And **the jump graph is the failure**: 32 s to
bucket every system in the galaxy, before a single expansion.

The guard failing here is the guard working. It is an opt-in test — it
stands down without `GALOS_PERF_DIR` — so the default suite stays green
while it keeps saying that routing at 200 M is not done.

#### 2a. ~~`Places`~~ — deleted; the router reads the payloads

**Done.** Reported from the map as "clicking Plot Route starts blowing up
memory a lot… from like 7 GB to 20", and then as the click hanging before
anything started. Both were the same structure: `Places` held three copies
of what the index already maps, and built them on the click.

| | at 200,071,629 |
|---|---|
| `points: Vec<(i64, [f64; 3])>` | 32 B a system = **6.40 GB** |
| `by_address: HashMap<i64, usize>` | 268 M slots × 17 B = **4.56 GB** |
| `buckets: HashMap<_, Vec<usize>>` | 8 B a system plus a heap block per occupied bucket = **~2.7 GB**, ~72 M allocations |
| transient: `collect()` doubling, the `filter` having killed the size hint | up to **+3.2 GB** |

≈13.7 GB, which is the +13 that was reported, and 32 s to build it.

All of it is gone. The galaxy's places are the cell payloads
(`galos_index::Sky`, `galos_index::store::Payload`), mapped as a query
reaches them; the cells come off `index.bin` by descent
(`Index::each_near`) so the work is the sphere's and not the galaxy's; and
the search state is `FxHashMap` over what was reached rather than dense
arrays over every system there is — which were 1.6 GB a leg, allocated
before the first expansion.

Measured on `.index/full`, the whole test process:

```
route graph: 200071629 systems in 28.57ms   (was 32.11s)
route Quick: 22 jumps in 75.31ms
route Direct: 22 jumps in 43.28ms
peak RSS 1.81 GB                            (was 4.90 GB)
```

**Opening is 1,124× faster than building was**, and it is not a faster
build: there is no build. Three notes on the rest.

- The 22-jump route is the guard's own pair of ends, ~1,000 ly apart. It
  says the machinery works and is fast; it does **not** say a galactic
  crossing is, which is what 2b's two-level plan is for.
- The peak used to be 4.90 GB and 4.37 GB of a later run was the *harness*,
  not the router: picking two ends by walking `Names::points` faults every
  byte of `addr.bin` and `pos.bin`. The guard picks them with a sphere
  query now, so the number measures what it claims to.
- Payload writes go beside-and-rename (`store.rs`), which mapping them
  required: `fs::write` truncates in place, and a truncation under a
  reader's mapping is `SIGBUS`. A cell the feed republishes is a new inode,
  so a route in flight keeps reading the galaxy it started on.

**The fanout cap is behind `Quick` and nothing else.** A cap that thins an
expansion's neighbours can drop the one a fewest-jumps chain went through,
so `Direct` and `Shortest` carry none — `Routing::fanout` is the single
place that decides, and
`graph::tests::only_a_quick_route_thins_an_expansion` is what stops a
setting added later from inheriting an approximation by accident.

#### 2b. How EDDA does it, and the order to port it in

Read against the implementation. EDDA plots Sol → Colonia over
**199,636,869 systems in 1.14 s, 141 jumps, 1,597 expansions**
(`docs/benches/2026-09-09-galos-index-spike.csv`, prod API). The same file
is their measurement of **our** router at `a8fc2fc` on the same data:
**137 jumps, 10,092,328 expansions, 130 s, graph built in 57.1 s, 7.98 GB
peak**. Six thousand times the expansions for four fewer jumps. Their
verdict on our router as a server-side plotter was "BURIED"; it is worth
reading their numbers rather than re-deriving them.

The two complaints are separate and have separate answers.

**The click that hangs before anything starts.** EDDA builds *nothing*.
`cells.bin` is a sorted array of `(morton key, start, count)` beside a
spatially-ordered record array, both mapped, and that *is* the spatial
index: a radius query is a lower-bound search plus a walk that skips
out-of-box runs with Morton BIGMIN, touching only occupied cells
(`format.rs:662-736`, entry point `for_each_within_toward` at `:878`).
Zero build, zero resident, zero allocation per route. Their measured
crossover is worth keeping: probe cells individually for tiny boxes (≤216
cells), walk for everything else (`format.rs:671-697`).

**The search that takes minutes.** Three mechanisms, in the order they
matter:

1. **A fanout cap per expansion** — the actual fix for ρ² in density. A
   boosted sphere in the core holds thousands of stars; they relax the
   best 512 by progress-to-goal (`router.rs:383`, `:459-474`) with
   scoopables ranked a full jump ahead so a fuel stop is never thinned
   away, plus a goal-directed cell prune that discards the half of the
   sphere pointing away (`router.rs:528`). Their audit says the result is
   **density-proportional, not density-quadratic**: expansions track
   corridor density at 0.66–1.03×.
2. **Search state as a hash map keyed by record index**, never dense
   arrays: `FxHashMap<u64, …>` with `key = (idx << 8) | fuel_or_dry`
   (`router.rs:443-454`). Two traps they hit and documented: keying on a
   counter that constrains nothing minted a state per system per counter
   value (**7.25 M expansions / 161 s** to refuse one impossible plot,
   `router.rs:436-441`), and quantised fuel multiplies states per system
   unless a Pareto frontier of (jumps, fuel) is kept per system
   (`:450-466`). This is what our `best` + `came` want to be.
3. **Two levels above ~1,500 ly**: coarse weighted A* over a boost
   sub-index — 1.8 % of systems, 3,487,192 records, on its own 250 ly grid
   because the queries are ~500 ly — then each coarse hop refined by the
   exact planner, legs in parallel, then fuel settled in rounds
   (`long_range.rs:24`, `:59-61`, `:2522`, `:3292-3455`). Weighted at both
   levels, 1.5 coarse and 1.3 per leg, and their own measurement says the
   lower weight's optimality was noise: **1.3 took 3,120 expansions / 6 s
   for 72 jumps; 1.5 took 73 expansions / 71 ms for 66**.

Everything above a client can derive locally. The one thing that needs a
*published* artifact is void crossing: `graph250.bin`'s cell graph and its
per-plot Dijkstra goal field took Colonia → Spase from **284 jumps / 24 s
to 91 jumps / ~1 s**. Note that nothing in their production builds it, and
that they gate their ALT landmark oracle behind a per-plot uniformity
check because consulting it unconditionally cost **+25 % wall for
identical expansions** — an unguarded landmark oracle is a net loss on a
healthy corridor.

**Worth not porting:** the `agg250.bin` prefix-aggregate oracle (fast and
correct — 1.5–2.4 µs against 19–140 µs walked — but it has no production
consumer in their router, because the density audit found nothing for it
to fix), and the nineteen-variant portfolio with its grace clock and
prize-aware waves, which is the residue of an empirical campaign rather
than the mechanism.

**Where we are better, and it is in their record as a kept weakness:** an
unreachable goal costs their planner **161 s** weighted and over 240 s
exact on a 145 k fixture, where ours answers in about a second.

**The one real difference for us.** Their record array is *spatially*
ordered, so a cell's members are a contiguous row range and a cell query
needs no lookup at all. Ours is *address* ordered — which is what makes
`addr.bin` the address index — so a cell's members are not contiguous in
it. Our spatially ordered array is the cell payloads, and that is where
routing belongs.

**The fork, corrected.** An earlier draft of this section proposed adding a
Morton permutation to the *names* table so a cell's rows would be
contiguous in it. That was wrong twice over, and the question that broke it
was "why are names involved in routing at all?"

They should not be. `NameEntry` carries a position, so the names table was
the only address-keyed table with positions in it, which made
`Names::points` the path of least resistance — and item 2a's `Places::over`
deepened that by making the router read `pos.bin` in bulk. The router needs
exactly two things and neither is a name: bulk "what is near `p`", which is
the spatial index's job, and address → node twice a leg, at the endpoints.
Where the names table *does* earn a position is a search result — you type
a name and it hands back somewhere to fly to — which is dozens of rows, not
200 M. So `Places::over` is a stopgap and not the destination.

It was also wrong on fact: `rank_lo`/`rank_hi` are ranks in the subtree's
**magnitude order**, for the LOD slices (`aggregate.rs:290-303`), not
payload offsets. There is no rank space to route in.

**The move that is right needs no format change: map the payloads.**
`Index::read_payload` does `fs::read` plus `Vec::<Point>::from_bytes`
(`store.rs:211-226`) — it *decodes* a cell every time it is asked. Mapped
instead:

- the router reads positions out of the mapping, already cell-ordered, with
  no derived structure, no build and no 32 s — which is EDDA's design
  exactly;
- names leave the routing path entirely;
- and it fixes the number sitting beside the 32 s in the guard, which is
  **24 s and 6.1 GB to page in one zoom**: 152 M points decoded into
  `Vec<Point>`s. Same defect, bigger blast radius.

41 B records against `pos.bin`'s 12 is ~3.4× the page traffic on a spatial
scan. EDDA runs 29 B records and treats it as fine.

#### 2e. The highway — landed, and what it measured

**Done**, and it is 2b's item 3 without the fuel model: the two-level plan.
`galos_map/src/systems/route/highway.rs` holds the boost stars as a graph of
their own and `JumpGraph::charged` flies each hop of its plan with the
search the flat router already has. Measured on `.index/full`
(200,071,629 systems), Sol → Colonia at 50 ly with a standard drive, release,
Apple M5 Pro, 2026-09-13:

| | jumps | found in |
|---|---|---|
| `Quick`, planned on the cones | 139 | **6.5 s** warm, 7.6 s cold |
| `Direct`, searched flat | 137 | **610–644 s** |

Eighty-six times the speed for two jumps in a hundred and thirty-nine. The
guard measures both halves of the warm figure: the plan is 116 waypoints and
178,910 coarse expansions in 4.4–4.8 s, and its 117 legs come to 2.0–2.3 s.
The cold figure is the first charged route of a session, which pays for
placing the boost stars.

**The supercharge table is 3,846,802 rows of 200,071,629 — 1.9 %** (3,480,954
neutron, 365,848 white dwarf), against EDDA's 3,853,782 over 199 M on the
same 250 ly grid, and 475,068 occupied cells against their reported 475 k.
Two independent readings of the same galaxy agreeing is worth as much as
either number.

**§5's disconnection finding was a coverage finding, and it survives the
coverage.** Flooding the real table from Sol with supercharged hops *alone*
(200 ly) reaches **135** of 3,846,802 stars and stops 196 ly out. Allowing
four ordinary jumps after each hop reaches **2,419,398** of them, out past
61,413 ly. So the bridging edges are not a refinement, they are what makes
the graph connected at all — and EDDA's `MAX_BRIDGE_JUMPS = 4` is the same
four, arrived at independently.

**Making the coarse search cheaper: where the knee is.** Leaning on the
coarse estimate was first measured only at 1.5, where it is plainly a loss,
and written up here as "a small graph is worth searching properly". That was
one point on a curve, and the curve has a knee one step past exact. Sol to
Colonia, jumps *flown* after the legs, warm, over `.index/full`:

| the coarse estimate times | 50 ly | 80 ly |
|---|---|---|
| 1 — exact in the coarse graph | 139 in 6.7 s | 80 in 9.4 s |
| **21/20 — shipped** | **140 in 2.0 s** | **81 in 0.8 s** |
| 11/10 | 142 in 1.8 s | 83 in 0.6 s |
| 6/5 | 148 in 1.8 s | 87 in 0.7 s |
| 3/2 | 163 in 1.7 s | 95 in 1.4 s |

A twentieth buys **3.3× at 50 ly and 13× at 80** for one jump, and
everything past it buys nothing: a fifth costs eight more jumps than a
twentieth and saves fifty milliseconds. So the fraction to use is the one
`Routing::Quick` already leans its *flat* estimate by — `LEANING`, 21/20 —
and the word means the same thing at both levels. Above the knee the
original objection stands: twenty-three extra jumps at 3/2 is some
twenty-five minutes of flying at the game's cadence, to save three hundred
milliseconds.

Two other cheapenings are still losses, and both for the same reason —
what they prune, the search pays back in longer chains:

| | coarse | flown |
|---|---|---|
| fanout 64 nearest the goal | 461,462 expansions, no faster | 160 jumps |
| a goal-direction prune | 6.5 → 7.6 s at 50 ly, 8.7 → 10.4 at 80 | unchanged |
| 400 ly cells | same expansions, 6.0 s | 139 jumps |

**~~What the join costs~~ — deleted: the place is published.** The
supercharge table used to be addresses and classes, so finding where four
million cones sat meant walking the names table's address column: **4 GB of
mapping faulted and 4.9–7.9 s before the first coarse expansion**, once a
session, on the click. Reported from the map as a plot that hung with an
empty sky, and it was the whole of the difference between a cold click and
a warm one.

`meta::SystemBoost` carries `position: [f32; 3]` now — 12 bytes a row, 70 MB
to 132 MB over the table — and the highway is a sort of the published rows
into cells, 213 ms. Measured on `.index/full`: the first charged crossing
**9.0–12.3 s → 6.5 s**, and a cold click now costs what a warm one does
(6.46 s against 6.29 s). Both derivations publish it, `Galaxy::boost_of`
answering the whole row and the database reading `ST_X/ST_Y/ST_Z` beside the
class; `source::place_boosts` brings a directory written before it forward
in one pass (3,846,802 rows in 513 ms over `.index/full`) and `migrate` runs
it, so an older directory reads as *absent* until a sync passes over it
rather than reading wrong. And routing no longer touches the names table at
all, which is 2b's "names leave the routing path" finished rather than
argued.

**Drawn while it runs.** The coarse plan is most of the wait and used to
draw nothing — seconds of empty sky, then the legs flashing past. The
sampler's picture half is now `Sampler::reached`, which takes a place and a
chain rather than a galaxy node and a parent map, so the plan over the cones
feeds it exactly as the flat search does.

**And the legs drawn as branches off it.** Reported as the prospective
line disappearing before the route was ready, and it was neither the
despawn nor the handoff: polled every 20 ms over a 50 ly crossing, the
chain went **116 links at 266 ms to 2 at 338 ms** and stayed a stub until
`finished` at 2.22 s, while the closed set stayed up — and `finished` and
the answer landing are 20 ms apart, inside a frame. A leg is its own search
with its own parent map, so the only chain it can hand over is its own two
to six jumps of the hundred and forty.

So the picture is the split now, rather than whichever leg is inside it.
`Sampler::planned` takes the coarse plan whole and `Sampler::flew` takes
each leg's real jumps as its own strand, and what the frontier hands over
is the plan plus a set of branches rather than one chain — which the mesh
already allowed, the layer being built as line segments and not a strip.
Measured: 1 strand/116 links at 288 ms, 2/123 at 485 ms, 3/130 at 1.71 s,
10/150 at 2.12 s, to the handoff at 2.24 s. A strand of one place is
dropped.

**And the plan is its own layer.** One colour for both was a picture of
nothing, which is exactly how it read: most of a plan's legs are a single
supercharged jump, so their branch lies *along* the hop it refines, and the
legs that do deviate are a few hundred light years out of twenty-two
thousand. Same line, same colour, no fork to see. So there are four layers
now — the closed set, the leading edge, the plan (`planned_color`, deep and
faint) and the branches over it (`reaching_color`, bright) — and a
refinement reads as the fork it is.

**What the wait says while it happens, and what it cost after.** Two
clocks, because they answer different questions. The form's readout is the
search's own: `Frontiers::asked_for` is the longest-running live leg,
counted from the click rather than from the first expansion, and it stands
beside the count and the distance left — which matters most exactly when
the other two say nothing, a leg between two cones expanding a few hundred
systems nobody can see. The panel's is the total: `Entry::took` holds what
the plot cost, wall time from the click to the answer landing, said as
`plotted in 2.2 s` under the range and the search mode. Beside the row
rather than in the filter, since the filter is what dedupes the row and
finds the line to take off the map — a route that took 2.2 s is not a
different route from one that took 2.3. A trip's panel is not a row of its
own, so it takes the longest of its legs: they are searched at once.

`closest` goes with it: measured against the route's own goal now rather
than each leg's, which is what makes the legs of one plan comparable at
all. It stops counting down once the plan has reached the goal (199 ly at
337 ms, then flat while the legs are flown), so the form's readout is *how
close anything has come* and not *how much is left to fly* — the second
wants the flown stretch's own distance, and is not written yet.

**What the two sampled layers do through all this: nothing.** The
closed-set grid is sized once from the route's length (`CELLS` = 20, so
1,100 ly a cell over 22 kly) and a refinement leg is 200–600 ly, so a whole
leg search falls inside a single cell — `cells 85` from 239 ms to the end
in both traces. The haze and the leading edge are the *plan's* corridor,
held still, and there is no live search picture during the leg phase at
all. Drawing one means a second pair of layers on a grid sized to the leg
and cleared at each `flew`, which is the honest version and not written;
the branches are what says where the work is for now.

**A ship that jumps half as far — and why it took four minutes.** Reported:
Sol to Colonia at **25 ly** came back with 316 jumps in **3 min 57 s**,
against EDDA's 331 in **284 ms**. The cause was not a slow plan. It was *no
plan*: a coarse hop was one supercharged jump plus four ordinary ones, which
is 400 ly at 50 ly of range and **200 ly at 25**, and at 200 ly the boost
stars are not a connected graph. The coarse search expanded into a dead end,
answered nothing in **0.3 ms**, and `charged` fell back to the flat
galaxy-wide search — silently, which is why it read as slowness rather than
as a failure.

Measured on `.index/full`, the threshold is a *distance* and the same one at
both ranges:

| hop reach | 25 ly | 50 ly |
|---|---|---|
| 325–350 ly | **no plan** | **no plan** |
| 400 ly | 243 hops in 0.74 s | 116 hops in 0.28 s |
| 450–600 ly | 243 hops in 0.87 s | 116 hops in 0.33–0.42 s |

So the knobs are settings now — `graph::Tuning`, a resource read where the
route is asked for, turned in a `Planning` fold **in the form**, shut until
it is asked for and offered only where it runs at all: nothing over the
fewest is searched flat, and an unaided ship has no cones to plan on.

They went to a route's info panel first, on the argument that the panel is
where the question comes up — you have just been told what the plot cost.
Wrong: a panel *describes a route already flown*, so controls for the next
one standing under it read as nonsense, and did. The ask belongs with the
ask. What the panel should hold instead is the **record** — what this route
was planned with, beside the range and the drive it already says — and that
needs the tuning carried through `FetchIndex::Route` and `PlottedRoute` to
the row, as `took` is. Not written, and deliberately not faked by reading
the live setting: that would say the current knobs on a route plotted under
the old ones.

- **`reach`** — how large a gap the plan may string together, in light
  years, asked as **Gap allowed**. Default **400**,
  the measured threshold, derived into a bridging allowance off the *widest*
  hop the drive could make: four jumps at 50 ly, which is what it always
  was, and twelve at 25. Derived per node instead — off each system's own
  cone — a white dwarf or an unboosted start gets a far wider scan for the
  same distance, and the 50 ly crossing measured 4.4 s against 2.2 s for one
  jump in a hundred and forty.

  **Floored at the supercharged jump plus one**, because a hop is those:
  asked for less, the allowance clamps at one jump and the reach is quietly
  more than the setting said — the knob read as doing nothing below the
  boosted jump, which is a control that lies. The panel's rail starts there
  and steps by whole jumps, both measured off the ship the route it stands
  under was plotted for, so at 25 ly with a standard drive it runs
  125–525 Ly in 25 Ly notches rather than a fixed 200–800.
- **`leaning`** — the coarse estimate's weight, in twentieths. Default 21
  (×1.05) still. At 25 ly: ×1.05 is 327 jumps in 7.8 s, ×1.20 is 338 in
  6.7 s, ×1.50 is 355 in 5.4 s. The knee is at the bottom, as it was at
  50 ly, which is why the default did not move.

  Offered as the *bound* rather than the weight, because "leaning" is the
  algorithm's word and no reader's: a weighted A\* comes back with a route
  no worse than `w` times the fewest jumps, so the panel asks **jumps over
  fewest** as a percentage and 21/20 reads as 5%. One twentieth is exactly
  five percent, so `info::over_fewest` multiplies rather than dividing —
  `(21 / 20 - 1) * 100` is 5.000000000000004, and a reading that will not
  round-trip is a control lying about what it did.
- **`crossing`** — *Nearest first* or *Searched*
  (`Crossing::{Nearest, Searched}`), asked as **Crossing a gap**. Default
  *Nearest first*; see the stepping measurements below.

  **Both cross the gap at the route's own optimality, and neither better.**
  Proving a gap was offered for one round and taken out on the argument
  that settles it: an optimality is a claim about *the route*, so an exact
  gap inside a plan that bounds nothing buys nothing anybody asked for.
  Measured, it never bought a jump either — Colonia to Sgr A\* came to 53
  jumps proven and 53 at the route's optimality (14.64 s against 4.94 s),
  and Sol to Colonia at 25 ly came to 355 either way. So what is left is a
  choice of *method* at a fixed bargain: walk the gap, or search it.
  `a_gap_is_crossed_at_the_routes_own_optimality` pins that nothing in the
  settings can approximate where the route did not ask for it.

  **The words took four tries, and one of them is now a vocabulary rule.**
  The setting was `Legs` offering `Leaned`/`Proven`, and the other control
  was `Hop reach`. Both were wrong, because the map already spends both
  words on other things: a **leg** is a part of a multi-stop trip (the bar
  says "3 Leg Route" over its legs' rows) and a **hop** is one jump of a
  route (`hops_said`: "how many jumps a row says its route is flown in").
  What the plan deals in is neither — it is the **gap** between two boost
  stars, which is what the code always called it (`GAPS_LY`). The router's
  internal prose still says "leg", which is EDDA's word for it too; the rule
  is about what the user is shown.
- **`stall`** — expansions spent without closing on the goal. Unchanged at
  200,000, and it turns out never to have been the binding constraint.

| crossing | before | after |
|---|---|---|
| 25 ly | 316–317 jumps in **233 s** | 332 jumps in **4.8 s** |
| 50 ly | 140 jumps in 2.2 s | 140 jumps in 2.0 s |
| 80 ly | 81 jumps in 0.69–0.78 s | 81 jumps in **0.18 s** |

EDDA's answer is 331 jumps; ours is 332. The gap is seventeen times its wall
clock now rather than eight hundred and thirty, and what is left is theirs by
design: coarse weight 1.5 against our 1.05 (their own A/B, `long_range.rs:52-57`:
weight 1.3 took 3,120 expansions and 6 s for 72 jumps, weight 1.5 took **73
expansions and 71 ms** for 66), bucket thinning to one-forward-one-near per
cube, a goal-direction cone in the index query itself, a hard
30,000-expansion cap with a 2,000-expansion goal settle, leg refinement that
is *arithmetic* (`direct_leg`) with a full search as the last resort, and a
rayon portfolio of 11–23 variants raced under a grace clock — so their wall
clock is the fastest variant and their answer the best of them.

**Three modes were two numbers.** Reading the three settings against each
other once the percentage existed: `Direct` is an admissible estimate with
`jumps: 1`, `Quick` is the same walk with the estimate weighted, and
`Shortest` is the admissible estimate with distance in the cost. So the
choice was never three-way — it was *how many jumps over the fewest* and
*are ties broken by distance*, and `Direct` is exactly 0%. `Routing` is
those two fields now:

- The form asks them: a `Jumps over fewest` slider (0–100%, in fives) and a
  `Shortest distance` checkbox, replacing the mode combo. The plane holds
  the pairs the enum could not say, a quick route whose ties break by
  distance among them.
- **The approximation gate became self-evident.** It read "an approximation
  belongs to `Routing::Quick`" — a named mode to be remembered — and is now
  `over > 0`: nothing over is a promise, and a promise admits neither the
  fanout cap nor the coarse plan. `nothing_over_the_fewest_approximates_nothing`
  pins it across both values of `shortest`.
- `direct` and `quick` were one function differing in a multiply, so they
  are one: `flat`, weighted by the ask, with `walked` the single place the
  flat and shortest searches are chosen between — which is also what makes
  a leg of a coarse plan walk the way the whole route would have.
- The weight is exact integers still: a jump costs `WHOLE` (100) and the
  estimate is multiplied by `WHOLE + over`, so every whole percent lands on
  its own integer. It was 21/20 with a jump at 20.
- A route's panel says what it was asked for in its own words — "fewest
  jumps", "within 5% of fewest", "fewest jumps, shortest" — instead of
  naming which of three buttons was pressed.

**And the identity is now the whole ask.** `Filter::Route` carries
`Routing` *and* `Tuning`, so the optimality, the shortest flag, the gap
allowed and how a gap is crossed are all part of what a route **is**. Two
plots between the same ends planned differently are two rows and two lines
rather than one that overwrote the other, `FetchIndex::Route` keys on the
same thing so they are two tasks rather than one cancelling the other, and
`Section::Trip` keys on it so two trips through the same stops keep their
own legs. `a_plot_asked_for_differently_is_its_own_route` pins it across
all three axes.

Which also makes the panel able to *say* it: a route's panel carries a
second line — "planned over 450 Ly gaps, nearest first" — read off the
route rather than off the live settings, so a plot from ten minutes ago
still reports what it was actually plotted with
(`info::planned_with`). Nothing for a route that was never planned: 100%
optimality is searched system by system, and an unaided ship has no boost
stars, and a line about the plan would be describing machinery that never
ran.

`Tuning::reach` became whole light years to get there. An identity wants
`Hash` and `Eq`, which a float does not have and should not — and a light
year is finer than any gap worth asking about, the rail stepping by whole
jumps of tens of light years.

**The picture said "arrived" while it was still hundreds of light years
out.** Reported, and it took a zoom to see: the orange chain ended short of
Sol with a gap that is *sub-pixel* at galaxy zoom, so a plot still being
worked out read as finished and the minutes after it read as the map doing
nothing. A search's chain ends at the closest thing it has reached, and a
plan's chain at the last cone — up to `GOAL_BRIDGE` jumps from the goal by
construction, further while it is still searching. So there is a fifth
layer now (`frontier::left_color`): the straight line from the furthest
reached to the goal, cold and faint, drawn only while there is no coarse
plan — once there is one it spans the whole route itself. It shrinks as the
search closes, which is the picture answering "is this getting anywhere" at
every zoom rather than only when zoomed in.

**And what the phase after the plan actually costs: one leg.** Measured on
the reported trip, 45 ly at 25% over with a standard drive:

| leg | plan | legs | route |
|---|---|---|---|
| Sol → Colonia | 130 waypoints in 44 ms | 20 of 129 searched in 2.15 s, worst 800 ms | 169 jumps in 2.20 s |
| Colonia → Sgr A\* | 66 waypoints in 1.02 s | 8 of 65 searched in **17.95 s, of which one leg is 17.93 s** (8 jumps) | 80 jumps in 18.97 s |
| Sgr A\* → Sol | — | — | **did not finish in a 50-minute run** |

So the wait is not the plan and not the legs in general: it is a *single*
leg, and the ones near the core are where the time goes — a boosted jump in
the core sees thousands of systems, and the neighbour scan is quadratic in
density (9,526 systems scanned per expansion for 145 in range, item 2i).
Two things follow, and the second is the serious one:

- **Legs are serial.** Parallelising them (2i lever #1) would fix a route
  whose cost is spread over many legs, and would do nothing at all for this
  one: 17.93 s of 17.95 s is one leg, and a barrier cannot be faster than
  its slowest member.
- **A leg's search is unbounded.** Nothing caps it, so a core leg can grind
  for tens of minutes — which is what the third leg did. EDDA caps every leg
  (200,000 expansions on a bridge tail, 5,000,000 on a refinement,
  `long_range.rs:378`, `3318`) and reaches the search last, after a ladder
  of cheaper constructors. **Not yet written**, and the first thing to write
  after this.

  **What a cap would do to the slider**, since that is the question it
  raises. Legs exist only where the highway ran, and the highway runs only
  past 0%, so *nothing proven can be capped*: at 0% there are no legs. What
  a cap can muddy is a claim that is already only measured — `highway`'s own
  doc says nothing about a planned route is proven. The evidence is a
  comparison *between* the settings, not one reading: Sol to Colonia at
  50 ly with a standard drive came to **140 jumps at 5% over** (planned on
  the highway) against the **137 that 0% proves** flat in 610 s. So the
  planned route landed 2.2% over the fewest there are — inside the 5% it
  claimed, on one corridor, by measurement. Which cap matters:

  - **By effort** — expansions or time, as EDDA does — severs the bound
    outright past 0%: a leg that runs out yields whatever the fallback
    yields, unbounded and invisible, and the setting comes to mean "within
    X%, unless some leg ran out". Same defect as the silent flat fallback,
    one level down.
  - **By cost** is the slider applied one level down, and is the shape to
    write. The plan already states a promise for every hop — `hops()`
    prices it at `1 + bridges` jumps — so a leg can be searched under
    `f_max = ceil(promise * (1 + over/100))`, pruning any node past it
    admissibly *with respect to that bound*. It bounds the answer rather
    than the effort, its failure is the definite "no leg within the
    allowance" that already merges with the next hop and retries, and the
    plateau it prunes is exactly what the core makes enormous.

  The measurement to take first, and it is cheap: per searched leg, the
  plan's promise against the jumps actually flown. The promise is a *lower*
  bound — it assumes a chain of systems exists at the bridging spacing — so
  real legs come in above it, and that 17.93 s leg flew 8 jumps against a
  promise that may have been three. If the usual overshoot is large, a
  tight `f_max` fails constantly and the merge-retry path dominates, which
  is both slower and worse. `[INFERENCE]` until measured.

**A gap is stepped across now, not searched.** Reported: EDDA plots
Colonia to Sgr A\* in 50 s where we took minutes. The instrumented answer
was that the wait is not spread over a plan's hundred-odd gaps but sits in
**one** of them — 17.93 s of an 18.97 s route — and the expensive ones are
near the core, where a supercharged jump sees thousands of systems and an
A\* frontier is quadratic in that. Which is also why parallel legs (2i
lever #1) would have bought nothing here.

EDDA's answer is `bridge_leg` (`long_range.rs:299-410`): pick the landing
star nearest the far side by one sphere scan, and search only the tail.
Ours is `Crossing::Nearest` — jump to whatever lands closest to the boost
star the gap ends at, every step required to land closer than the last, and
the gap handed to the search where none does. Measured over `.index/full`:

| crossing | stepped | searched | jumps |
|---|---|---|---|
| Sol → Colonia, 50 ly, 5% | 141 in **0.30 s** | 140 in 2.60 s | +1 |
| Sol → Colonia, 25 ly, 5% | 334 in **0.84 s** | 332 in 6.19 s | +2 |
| Sol → Colonia, 80 ly, 5% | 82 in 0.12 s | 81 in 0.14 s | +1 |
| Colonia → Sgr A\*, 45 ly, 25% | 81 in **0.97 s** | 80 in 19.84 s | +1 |

One jump in a hundred and forty — 0.7%, inside even the smallest allowance
— for 8–20×, so it is the default. The 80 ly row is where it buys nothing:
a wide jump crosses a gap in one or two anyway, and there was no plateau to
skip. The guard moved with it: the 50 ly crossing is 0.33 s warm against
2.0 s, and the 25 ly crossing 0.89 s against 4.8 s — and 233 s before any
of this.

**Where that leaves the comparison.** Colonia to Sgr A\* at 45 ly is 0.97 s
here against EDDA's 50 s, and Sol to Colonia at 50 ly is 0.33 s. The jump
counts are not comparable without knowing which multipliers their "Mk I"
carries — ours is 81 with a standard drive (×4) and 55 with the SCO Mk II
(×6) on the same corridor, against their 72 — so **nothing is claimed about
route quality against theirs**. What is claimed is that the wall-clock gap
that prompted all of this is closed.

**Where it sits, and why the beam stays width one.** The walk is a beam
search of width one, scored on the estimate alone rather than
cost-plus-estimate, with no frontier to return to and a monotone-progress
rule doing the work of a visited set. Measured across the three corridors:
of the **32 gaps** that needed crossing at all, width one carried **29**,
and all three fallbacks were on the 25 ly crossing — 2.8 ms of walking
against that route's 0.74 s coarse plan. A width-`k` beam costs `k` sphere
scans a step and would survive the pockets width one gives up on, so it
would remove those three searches and nothing else. Not worth it for speed.
Where it *would* earn its keep is bounded time: a beam with a step cap never
reaches the unbounded search, trading "sometimes minutes" for "sometimes a
worse route". Distinct from the fanout cap, which truncates the neighbours
of one expansion while the search keeps its frontier — EDDA has both, and
its greedy cone (`ED_CONE_CAP` = 8) is the beam-shaped one.

**And it did not cap anything.** The unbounded leg search is still
unbounded; what changed is that the common path no longer reaches it. A
corridor where stepping keeps failing would still grind, which is the case
a cost cap (above) is for.

**A stalled plan hands over what it reached.** Reported as a cliff: Colonia
to Sgr A\* at 45 ly came back in 776 ms at 25% over and in **4 min 05 s at
5%**. Instrumented, the 5% plot spent **no time planning at all** — the
coarse search hit `Tuning::stall`, answered `None`, and the entire wait was
the flat galaxy-wide fallback. Which is the silent fallback this document
had listed as open, met in the wild: a nearly admissible coarse estimate
expands the plateau of boost stars, and near the core that plateau is
enormous.

`Highway::plan` now returns the chain to the closest cone it reached when it
stalls, and the stretch left over becomes one more gap for
`Tuning::crossing` to cross — stepping first, which is cheap over any
distance. EDDA's rule (`long_range.rs:3273-3281`). A search that closed on
*nothing* — no cone nearer than the door — still answers `None`, which is
what the flat fallback is actually for, and
`a_plan_that_closed_on_nothing_is_no_plan` pins that.

| | before | after |
|---|---|---|
| Colonia → Sgr A\*, 45 ly, 5% | 74 jumps in **226.89 s** | 73 jumps in **8.53 s** |
| Colonia → Sgr A\*, 45 ly, 25% | 81 jumps in 0.97 s | 81 jumps in 0.96 s |

**27× faster and a jump better** — the stalled plan's route beats what the
flat weighted search found. And the setting reads as a dial again: eight
jumps between its ends for nine times the wait, rather than a cliff into
minutes.

What the 5% plot now pays is the stall allowance itself: 8.5 s of coarse
search before it gives up and flies what it has. That is `Tuning::stall`
(200,000 expansions), the one knob still not exposed, and the next thing to
measure — a smaller allowance would trade plan quality for the wait exactly
as the percentage does.

**What 100% optimality actually means, since the fold going missing there
was read as a bug.** It is not a bug and it is not a hidden control: at 100%
there is *no plan*. The coarse graph's edges are lower bounds — "this gap is
crossable in n jumps", with no chain of systems named — so nothing about a
planned route is bounded by them, and the only thing that can honour the
word "optimal" is the flat admissible search. That is 610 s for Sol to
Colonia at 50 ly and hours at 25: the honest end of the dial, and close to
unusable across the galaxy. The fold now says so in its own header
(*Planning (unused at 100%)*) with the reason on hover, rather than
vanishing.

Which leaves a real question this document should not pretend to have
settled: **is a promise nobody can wait for worth offering?** The
alternatives are to let the plan run at 100% and relabel what the figure
means (honest about being a target everywhere, and then no setting is a
proof), or to keep the proof and say plainly that it is a different kind of
answer. Kept as it is for now, because the one corridor we have a proof for
is the only evidence that the planned route is any good — 140 jumps against
137 — and losing the ability to produce that number would be losing the
yardstick.

**A trip is one plot, and the map now says so in three places.** All three
were the same defect — machinery that is per-leg reported as though the leg
were the thing the user asked for.

- **The progress readout aggregates.** The elapsed was already the longest
  leg and the expansions already a sum, but the distance left was a
  *minimum* over the legs, each measured to its own end: on a trip through
  three stops that reads as almost arrived while two legs have twenty
  thousand light years between them, and it jumps about as each lands.
  Added up now (`Frontiers::left`), with the count of legs still searching
  said beside it where there is more than one.
- **The form waits on the whole plot.** `Plot::Working` was cleared by the
  *first* leg to land, so the spinner stopped, the stop button went with it
  and the form read as finished while the other legs were still searching —
  which made the new stop button look like it only tracked the first leg.
  What says a plot is still running is whether any leg of it is: the tasks
  are the authority, a landed leg having been taken off `fetched` in the
  same walk. `the_form_waits_for_the_last_leg_of_a_trip` lands one leg
  beside one that never finishes, and fails against the old rule.
- **Stopping is its own gesture.** It was the plot button's second meaning,
  which on a trip half worked: the legs land at different moments, so a
  second click took back the ones still searching **and re-asked the ones
  that had already landed** — half stopped, half started again. There is a
  `✕` beside the button now (`Search::Stop` →
  `route::fetch::stop_routes`), and asking is only ever asking: a leg
  already under way keeps the seconds it has spent.
- **A trip's panel never describes part of itself as the whole.** The
  joined route was built once, when the panel opened, so a panel opened
  mid-plot showed a partial trip's systems, distance and longest jump
  forever. It is rebuilt every frame off whatever legs the bar holds
  (`ui::trip_now`), its fetched systems dropped so they are asked again,
  and the summary says what is outstanding — "— 1 leg still being
  plotted". The panel's egui id had to move to the trip and the ship for
  that: keyed on the joined route, every leg that landed would have been a
  new window dropped back into the tiling.

**A route's line reads per jump, and its list says what kind of star.**
Colouring the whole line by what the plot was *asked for* was the wrong
fact: a supercharged route is mostly ordinary jumps, so a blue line said the
ship flew a cone at every stop. What is per jump and *verifiable* is
whether the jump could have been flown at all unaided — a jump longer than
the ship's range **is** a charged jump, there being no other way across it
(`route::spawn::charged`). So `LineList` carries a colour per vertex, one
mesh and one material still, and the material's fade multiplies the jump's
own reading rather than replacing it. The converse is not claimed and is
documented as not claimed: a short jump may still have set out from a cone
with the charge going to waste.

**A mark is only a mark if the face holds it.** The stop button beside
*Plot Route* was lettered U+2715 and reached the screen as a hollow square,
which is what a missing glyph looks like. The map letters its chrome in
egui's own faces and adds none of its own (`epaint_default_fonts`, every
text style resolved to the monospace family), so the set of marks available
is fixed and small. Measured rather than guessed: U+2715 is in none of
them, while U+2716 `✖`, U+00D7 `×`, U+2A2F and the 🗙 egui letters its own
window close with all are. The button now carries `ui::STOP` = U+2716, and
`a_lettered_mark_is_one_the_font_has` asks `Fonts::has_glyphs` of every
lettered mark — `INFO`, `CLOSE`, `STOP`, `ARROW`, `CUT` — in both styles
the chrome uses, so the next mark chosen from a character picker fails a
test rather than a screenshot. Confirmed by putting U+2715 back: `"✕" is
drawn as an empty box in Body`.

**A dash that cannot be read at every zoom is not a dash.** The stretch a
leg trails off in, where the stop it is heading for is not on the map, was
dashed at half a light year in the world. That is a clear mark with one
system in view and about a hundredth of a pixel with the galaxy in view, so
at the zoom a whole route is looked at from the thing that says *the route
goes on past what is drawn* read as a faint solid line. Reported.

Measured against the view instead, the way the rings inside a system are
(`orbit::Spacing`'s `DASHES`): twenty dashes and their gaps across the sky
the camera takes in (`route::dash_of`), so a dash is some tens of pixels at
every zoom. With one clamp — a dash is never longer than a fifth of what
trails off the map, so a leg too short to hold the view's dash still gets
two of them rather than coming back as the solid stub alone. The view
decides where there is room and the leg decides where there is not, which
is the same two-ended rule the rings use.

That makes the dash a fact about the camera, so it sits beside `shown` on
`route::Path` and `trim` watches both: the line is cut again when what is
on the map moves *or* when the dash the view wants has drifted past
`REDASHED_AT` (a third, the ratio the rings are relaid on). Zooming is
continuous and a mesh is not — a four hundred jump route rebuilt on every
click of the wheel is work for nothing, and one never rebuilt is drawn for
a zoom the camera has left.

**Told apart by hue alone did not work.** The first blue was a gentle
`(0.45, 0.70, 1.0)` at the quarter alpha the whole line is drawn at, and
reported back as lines that could hardly be told apart — correctly, because
blending toward black takes a faint line's colour before it takes its
light. Over black that blue came out `(0.11, 0.18, 0.25)` against white's
`(0.25, 0.25, 0.25)`: a slightly dim grey with a hint of blue in it.

Two levers, since a saturated blue carries about a third of white's
luminance at the same alpha and turning the hue up alone would have made
the interesting jumps the *dimmer* ones. The route's faintness
(`spawn::FAINT`) doubled to 0.5 and an ordinary jump now takes half of it —
coming out at exactly the quarter it always was — while a charged jump
takes all of it at `(0.12, 0.55, 1.0)`. Over black: `(0.06, 0.28, 0.50)`,
luminance 0.246 against white's 0.250. **Same light, different colour**,
which is the only difference worth drawing. Both halves are held by the
test: red under 0.25, and the two luminances within a third of each
other.

The list says the star's real class, looked up by address. **A list is a
finite thing where the galaxy is not** — a panel lists tens or hundreds of
systems, and the index answers one address at a time
(`Source::bodies`) — so `info::StarClasses` reads the arrival star of
exactly what is listed, off the task pool, at most a few a frame, and holds
the answers per address rather than per panel. The class shown is the
primary's (the star that goes round nothing, which is the one a ship drops
in at). Until a read lands, and for a system nothing has scanned, the row
falls back to the one thing published for *every* system — what it can
supercharge on, `Boost::named` — and says nothing at all where there is
neither.

That was the wrong shape at first: the resident boost table was treated as
the answer rather than the fallback, on the argument that a spectral class
needs a per-system fetch. True, and the fetch is affordable *here*, which
is the difference between a view layer over a finished route and the walk
that drew the sky.

**And a name is not crushed to make room for it.** With a class and a jump
distance after the name, the panel's width left `CO…` and `SW…` where a
system's name should be. A name is the one thing a row cannot do without —
`CO…` is not a system — so where the two will not fit, the name takes the
width and the reading goes on a line of its own beneath, indented. Two
lines of a scrolling list cost height the panel has; a truncated name costs
the row its point. Widening the panel was the alternative, and a panel wide
enough for these rows overlaps the route it is about.

**Settled for the list, not for the line** (`ui::Rows`). Deciding it per
line — the first go at it — meant a list where some stops read one way and
some the other, the column of readings breaking wherever a long name
happened to fall, and it was reported as exactly that. So the widest line
in the list decides for all of them, measured before a line of it is drawn,
which is why the readings are formatted once into a `Vec` and both measured
and drawn from those same strings. A line with no reading at all settles
nothing: what cannot fit is a name *and* a reading, and a name alone has
the width already. The room is the caller's to hand over rather than
`Rows`' to assume, because a trip's stops are drawn indented under their
leg's name and get an indent less than the panel they stand in.

**One thing this still leaves open.** The flat fallback is still silent
where it is genuinely taken: a plan that
cannot finish hands a galactic crossing to the flat search, which is
minutes, with nothing said. EDDA truncates its coarse chain at the closest
cone reached and hands only the *rest* to the exact planner
(`long_range.rs:3273-3281`), which is the better shape.

**What is left, and two cheapenings measured and rejected.** The plan is
4.4–4.8 s of the 6.5 s at 50 ly and 6–7 s of the 8.6 s at 80 ly — the
estimate is why: `ceil(d / 200 ly)` is the fewest jumps a *pure* chain of
cones could take, and a bridged stretch covers 80 ly a jump, so it is weak
by up to 2.5× and A\* pays for it.

- **A goal-direction prune** — drop a hop that ends further from the goal
  than a boosted hop's worth of backtracking, which is what EDDA does
  (`router.rs:525-528`) and switches off in its exact mode. Measured
  **worse**: 6.5 → 7.6 s at 50 ly and 8.7 → 10.4 s at 80 ly for the same
  139 and 80 jumps, squared arithmetic or rooted. Same shape as the fanout
  result in 2e — what it prunes, the search pays for in longer chains.
- **Cells sized to the query.** The grid is 250 ly and the query is
  `charged + 4 range`: 400 ly at a 50 ly range (span 2, 125 cells) and
  640 ly at 80 (span 3, 343 cells), which is most of why 80 ly costs more.
  400 ly cells measured worse *at 50 ly* (6.0 s against 4.8 s), so the
  ratio that works is about query/1.6 — which would mean a grid rebuilt per
  range rather than per session. 213 ms to rebuild, `[INFERENCE]` 1.3–1.5×
  at 80 ly. Not taken yet.

A goal field over the highway's own cells — one Dijkstra per plot, EDDA's
`cgraph.rs` — is the tightening that fits the estimate, and the legs want
rayon: they are independent and run serially (2.0–2.3 s of the 6.5).

**What `Direct` and `Shortest` get out of this: nothing, and that is the
answer.** A coarse edge says how few jumps *could* cross a gap, so a plan
over 1.9 % of the galaxy cannot carry a fewest-jumps claim, and
`Routing::highway()` is the one place that says so —
`only_a_quick_route_plans_on_the_highway` is what stops a setting added
later from inheriting it by accident, as `fanout` already has.

What they do get is 2f: the plan is no use to them, but the *search* had
work in it that changed no answer, and an admissible cone-aware bound is
the lever that would make a proven crossing something other than ten
minutes. The UI says which of the two the commander is getting — EDDA has
no such flag at all.

#### 2f. What the exact settings cost, and what would help

2e's answer for `Direct` and `Shortest` was "nothing, and that is the
answer". That was wrong, and measuring it says so. Profiled on
`.index/full` (200,071,629 systems, Apple M5 Pro, release, 2026-09-13),
exact `Direct` from Sol to a system three thousand light years out:

| | jumps | was | now | expansions | relaxations |
|---|---|---|---|---|---|
| unaided | 62 | 832 ms | **655 ms** | 11,512 | 248,067 |
| charged | 32 | 124.0 s | **72–80 s** | 2,635,464 | 3,893,601 |

The expansion and relaxation counts are identical before and after, which
is the point: what landed removes work that changed no answer.

**Where the time went.** Per charged expansion, before: 21 cells swept and
**17,237 systems measured** to find 774 inside the reach and relax 1.5 of
them. Forty-five *billion* distance measurements over the search, and each
in-range hit then asked a hash map whether it already held something
cheaper.

**What landed** (`graph.rs`, `Ledger`/`Page`): what a system has been
reached for is an array over its own cell, allocated when the search first
touches that cell, instead of an `FxHashMap<Node, C>` over everything
reached. Two skips fall out of it, both exact — a jump more is dearer under
either ordering, so a system already reached in no more jumps than the
chain standing here cannot be improved:

- **per record**, four bytes out of the cell's own run instead of a
  position off a 41-byte record and a distance measured to it;
- **per cell**, where every system in it is already reached that cheaply:
  the payload is not even read.

Measured apart: the cell skip alone is 124 s → 91.6 s and fires on 1.5 of
21 cells an expansion; the per-record test takes it to 72.2 s and removes
40 % of the measurements. After both, an expansion visits 15,820 records,
measures 9,526 and finds 145 in range.

**The levers, in measured order.** One is built and reverted; read 3 before
reaching for it again.

1. **Fewer expansions: a cone-aware goal field.** 2.6 M expansions for a
   32-jump charged route against 11,512 for a *longer* unaided one — two
   hundred times the work, and all of it the admissibility tax. The
   estimate divides by the widest jump the drive could make (200 ly) while
   98 % of systems can only jump 50. A uniform correction buys nothing (a
   constant added to `h` reorders no frontier); what is needed is a bound
   that knows *where the cones are not*, and the highway now answers that
   per cell. Admissible construction: a grid over the corridor, edge cost
   `ceil(gap / reach(A))` where `reach(A)` is the boosted range if cell A
   holds a cone and the plain range if it does not, and one Dijkstra from
   the goal's cell. Every real jump out of A is at least the gap and at
   most `reach(A)`, so the field never overstates — it is EDDA's
   `cgraph.rs` goal field with our own boosts in place of their reference
   ship, which is what makes it a *bound* rather than a proxy. `[INFERENCE]`
   an order of magnitude on the charged exact case; it is the only lever
   that touches the expansion count.
2. **Payload records ordered by position.** 9,526 systems measured to find
   145 — a 1.5 % yield — and the waste is structural: `Index::each_near`
   hands over every cell whose *box* meets the sphere, the thirteen
   ancestor slices included, whose 512 members apiece are scattered over
   the whole subtree. Sorted spatially inside the payload, a sphere query
   becomes a binary search and a run. It costs no bytes, it helps every
   reader — the 24 s, 6.1 GB zoom of item 1 above included — and it is a
   layout change to `cells/`, so it wants checking first against whatever
   reads payload order for the LOD slices.
3. ~~**Bidirectional exact search.**~~ **Built, measured, reverted.** Two
   frontiers — one out from the start, one back from the goal over the
   reversed edge relation — joined under the standard rule: stop as soon as
   either side's cheapest estimated total is no better than the join
   already found, which is sound because both estimates are the same
   admissible one pointed the other way. It answered the same routes (the
   whole exact suite passed unchanged, the directed-edge tests included)
   and it is slower both ways:

   | | one way | both ways |
   |---|---|---|
   | unaided 3 kly | 600–690 ms | 687 ms — no change |
   | charged 3 kly | 72 s | **> 1,400 s**, timed out |

   Both results have the same cause and it is worth writing down.
   **Unaided there is nothing to save**: meeting in the middle pays where
   the frontier balloons with depth, which is the `h ≈ 0` case — a ball of
   radius r, halved, is an eighth of the volume. Our estimate is tight
   unaided (11,512 expansions for 62 jumps, 185 a jump), so the search is
   already a tube, and half a tube twice is the same tube. **Charged the
   reversed graph is dearer**: a boost belongs to the system a jump
   *leaves*, so the predecessors of a system are everything within the
   plain range *plus every cone within the widest jump* — a 200 ly sphere,
   40–60 k records at 200 M, with a supercharge-table lookup per candidate
   past 50 ly, against the forward sweep's 50 ly. Three times the work an
   expansion at best, and no reduction in the count: the slack blob around
   each end is the bulk either way and each half carries its own.

   Not kept — 250 lines in the hot path for a measured loss. What would
   change the verdict is a cheap predecessor set (the cones out of the
   highway rather than out of the galaxy, with an address-to-node cache)
   *and* a heuristic weak enough that depth is what costs. EDDA's up-to-250×
   (`long_range.rs:1827`) is on its *coarse* graph, where both hold.

**And ALT, which is what EDDA reaches for here and we should not yet.**
A\* with Landmarks and the Triangle inequality: pick a handful of landmarks
(EDDA takes 16, farthest-point sampled), precompute the distance from every
node to every landmark, and then bound the distance from `v` to the goal
below by `max over landmarks L of |d(L, goal) − d(L, v)|` — the triangle
inequality, which holds in any metric and so is admissible by construction.
What it buys over a straight line is *detours*: where a void or a desert
forces a way round, the landmark on the far side of it knows, and the
straight line does not. What it costs is a table (cells × landmarks) and a
lookup per pushed candidate.

Two reasons it is not our next move. It bounds *distance*, and our cost is
*jumps* — so the table has to be built over a metric that already prices
jumps, which for EDDA means their cell graph (`alt.rs:137-190` runs the
landmark Dijkstras over `graph250.bin`); an ALT table over a plain
straight-line metric "measured null on the real galaxy (the straight line
never lost)" (`alt.rs:129-134`). And even with the gap-aware metric they
gate every consult behind a per-plot non-uniformity check, because uniform
inflation "reorders nothing under weighted A\* and only costs — measured at
+25 % wall with IDENTICAL expansions" (`long_range.rs:1731-1735`). So ALT
comes *after* a jump-priced cell graph, not before it, and lever 1 is that
graph.

None of these is the plateau argument in `Routing`'s header, which
stands: proving the fewest jumps means expanding every system that could
have been on an equally short chain. What the two that are left attack is
what surrounds it — the slack in the estimate, and the systems measured to
find the ones in reach.

#### 2g. Where the two routers stand, side by side

Their numbers are their `docs/benches/2026-09-09-galos-index-spike.csv`,
which ran both routers over their 199,636,869-system index; ours are the
perf guard over `.index/full` (200,071,629) on an M5 Pro, release. Sol →
Colonia with supercharge unless said otherwise.

**Where it stands now, after this wall's work:**

| | jumps | found in |
|---|---|---|
| ours `Quick`, 50 ly | 140 | **2.0 s** (cold 2.2 s) |
| EDDA product API, 50 ly | 141 | 1.14 s (1.50 s "try harder") |
| ours `Quick`, 80 ly | 81 | **0.80 s** |
| EDDA product, 80 ly (reported) | — | 1.52 s |
| ours `Direct` — proven fewest, 50 ly | **137** | 610–644 s |
| EDDA `thorough` — their admissible one | — | killed at 2 h 08 min |

Ours split, at 50 ly: the highway sorts in 213 ms once a session, the
coarse plan is 285–349 ms, and its 117 legs are **1.79–1.88 s** — so after
2e's leaning the profile flipped and the legs are now 86 % of the wall.
They are independent searches run one after another; EDDA refines its legs
over rayon (`refine_waypoints`, `par_iter`). That is the next lever and it
is the cheapest one left.

The session moved the charged crossing from **130 s** (their row 55, our
router at `a8fc2fc`, with a 57.1 s graph build and 7.98 GB resident) to
2.0 s with nothing built and nothing resident but the index — 65×, and the
build gone rather than faster.

**And the three reasons they were ahead, of which two are not available to
a setting that proves anything:**

| row | who | mode | jumps | expansions | time |
|---|---|---|---|---|---|
| 58 | EDDA | product API (coarse + refine) | 141 | **1,597** | **1.14 s** |
| 59 | EDDA | same, "Try harder" | 141 | 1,597 | 1.50 s |
| 49 | EDDA | `plan()` weighted 1.3, flat | 453 | 2,708 | 353 ms |
| 50 | EDDA | `plan()` weight 1.0, flat | 317 | 546,687 | 33.6 s |
| 60 | EDDA | `plan()` `thorough` — the admissible one | — | — | **killed at 2 h 08 min** |
| 55 | ours at `a8fc2fc` | `Quick` | 137 | 10,092,328 | 130 s |
| 56 | ours | `Direct` | 136 | 14,966,472 | 175 s |
| 57 | ours | `Shortest` | 136 | 15,410,738 | 261 s |

**1. Their fast number is not an exact route, and their exact mode is
slower than ours.** Row 60 is their own admissible search — weight 1.0 with
the estimate credited at the neutron boost, which is exactly what `Direct`
does — and it did not finish in over two hours, their note reading
"Admissible boost heuristic on the full galaxy is a long search". Row 49 is
what their flat planner answers *quickly*: **453 jumps**, because a
weighted plain-range estimate walks the 50 ly chain and never expands a
cone (their own test says so, `long_range.rs:3993-3996`). So the 1.14 s
belongs beside our highway `Quick` — 139 jumps in 6.3 s — and nothing in
their product belongs beside `Direct`. Their bench's own summary of the
comparison was "five jumps for 100-200× the time"; it is now one jump for
0.6× the time at 50 ly and one jump for 1.9× *their* speed at 80, and the
three jumps between 140 and 137 are the proof.

**2. On the shared ground — the coarse plan — they are 100× fewer
expansions, and it costs them two jumps.** (Read with 2e's knee: leaning
the coarse estimate by a twentieth has since taken our 178,910 expansions
down to a 2.0 s plan for one jump, so the gap below is against the *exact*
coarse search.) 1,597 against our 178,910, from
weighting the coarse estimate at 1.5, thinning each expansion to two
candidates per ~55 ly bucket, a greedy cone, and a goal settle. We measured
both of the first two on our own graph: weighting cost **164 jumps against
139**, thinning **160 against 139** (2e). Their 141 is the same trade taken
once more. Worth knowing that their two effort levels disagree with each
other, 140 jumps against 141 for "the same" request
(`ed-api/src/plot.rs:118-121`) — a portfolio race has no answer to "is this
the fewest".

**3. Where they are honestly cheaper per expansion, and it is ours to
take.** Their index is a **flat partition**: 50 ly cells, records sorted by
cell so a cell's systems are one contiguous run, a sorted
`(morton, start, count)` array as the whole directory, each cell rejected by
its nearest box point against the sphere before a record is read, and 29 B
records (`format.rs:25-44`, `:878-947`). No ancestor slices — which is
precisely 2f's lever 2: our `Index::each_near` hands over thirteen ancestor
cells whose 512 members apiece are scattered over the subtree, and we
measure 9,526 systems an expansion to find 145. Their own verdict reads the
other way round too: our octree cost *their* planner 1.7–2.6× what their
grid costs, on identical plans. They also prune cells by direction
(`router.rs:525-528`, "from a neutron that halves a 400 ly sphere") — an
approximation, which is why they switch it off under `thorough` — refine
legs and portfolio variants over rayon, and derive their sub-indices at
import, where our highway join is 7.9 s of every session because the feed
never stops.

So: **1** is a different product, **2** is a trade we have measured and
declined at its current price, and **3** is a layout change worth doing for
every reader we have.

#### 2d. Two axes, not one list: what a route optimises, and how hard it tries

`Routing` is one enum of three values — `Quick`, `Direct`, `Shortest` —
which conflates two independent questions, and the conflation became a
correctness problem the moment the fanout cap landed: a cap that thins an
expansion's neighbours cannot be applied under a setting that claims the
fewest jumps, so it belongs to `Quick` and to nothing else.

The two axes:

- **What a route is judged by.** Fewest jumps (`Direct` today); fewest
  jumps and the shortest of them (`Shortest`); and later **economical**,
  judged by fuel, and **fastest**, judged by pilot-seconds — which is
  EDDA's `route_score`/`time_units` (`long_range.rs:518-570`), fitted from
  journal cadence at 18 s a jump plus a stop overhead
  (`ed-route/src/cost.rs:33-49`), and is the only judge that answers "which
  of these two routes would I rather fly".
- **How hard the search tries.** Exact, or quick. Quick is where every
  approximation lives and nowhere else: the leaned-on estimate
  (`LEANING`, weighted A\*), the per-expansion fanout cap, and any later
  beam or corridor. Exact means no approximation is switched on, and a
  route that comes back is the one the judge says is best.

So the carried type becomes a pair rather than a value:

```rust
pub struct Search { pub goal: Goal, pub effort: Effort }
pub enum Goal { Direct, Shortest }        // later: Economical, Fastest
pub enum Effort { Exact, Quick }
```

`Search::default()` is `{ Direct, Exact }`, and `Shortest` becomes askable
*quickly* — which it is not today, and which is most of the reason the
current list is wrong: it offers "shortest, slowest to find" and no way to
say "shortest, and I am not going to wait".

Churn is ~60 sites, nearly all of them `Routing::default()` in tests, plus
the combo box in `ui.rs:3787-3808` which becomes two controls, plus
`Filter::Route`'s carried field and `Routing::said` (the word a plotted
route is labelled with, which becomes two words).

The one thing to get right while doing it: **the UI must say when a route
is not proven**, which the current wording half does by calling one setting
"Quick". With the axes split, `Effort::Quick` is the flag, and EDDA's
lesson is worth taking — it has no not-proven-minimal flag at all and its
own docs call the coarse cost model a proxy, so a commander cannot tell a
proven route from a good guess. We can.

#### 2c. What the EDDN feed constrains, and one hard blocker

Per publish today (`store.rs:143-158`): `index.bin` **rewritten whole** —
40 MB at 200 M, every beat, already its own item in `TODO-scale.md`;
each changed cell's payload written whole, a leaf averaging ~978 systems ×
41 B ≈ 40 KB; and one appended row per changed system in
`names/delta.bin`. A beat naming fifty systems writes ~50 log rows, ~2 MB
of payloads and 40 MB of index. The index dominates by twenty to one.

**Blocker: `write_payload` is `fs::write`** (`store.rs:164-170`) —
create-truncate-rewrite, in place. Safe today *only* because every reader
decodes a snapshot into a `Vec`. Map the payloads and a feed rewriting one
under a reader's mapping is a torn read, and truncating a mapped file is
**SIGBUS** on the truncated pages. So mapping the payloads requires payload
writes to go beside-and-rename first, the same way `write_meta` and the
names generations already do. Not optional, and cheap.

Three more things the feed decides:

- **Derive nothing and there is nothing to invalidate.** That is why
  reading the mapping directly survives a live feed, and it is the property
  EDDA gets for free by publishing an immutable index daily rather than by
  design.
- **Node identity cannot be cached across publishes.** A payload rewrite
  renumbers its cell's members, so `(cell, offset)` holds within one
  snapshot only. A route in flight already holds an `Arc<JumpGraph>`; with
  rename-not-rewrite it would hold mappings that stay valid for the route's
  life, which is the answer we want — a search half-run against a galaxy
  that moved underneath it has searched two skies.
- **Anything genuinely derived needs the names table's base/overlay
  shape.** A boost sub-index for the >1,500 ly coarse plan is derived from
  the whole galaxy and is stale every beat; it wants building at compaction
  with a small overlay of the arrivals that supercharge. That is exactly
  what `JumpGraph::extended` does for places and `Delta` for names, and it
  is the part EDDA has no answer for because a published daily artifact
  never faces it.

#### 2i. What is left to do to routing, in measured order

Everything above is done and measured; this is the standing list, newest
measurement first. The state it is measured against: a supercharged Sol →
Colonia crossing is **140 jumps in 2.0 s** at 50 ly and **81 in 0.80 s** at
80 ly, and the proven one is 137 jumps in 610 s.

1. **Refine the legs in parallel.** After 2e's leaning the profile flipped:
   at 50 ly the plan is 285–349 ms and its 117 legs are **1.79–1.88 s, 86 %
   of the wall**. They are independent searches run one after another;
   EDDA's `refine_waypoints` does them over rayon. `[INFERENCE]` core-count
   division puts a 50 ly crossing near half a second. Cheapest lever left
   by a distance.
2. **A flat, spatially ordered cell scan.** 2f's lever 2, and 2g's row 3:
   `Index::each_near` hands over every cell whose *box* meets the sphere —
   the thirteen ancestor slices included, whose 512 members apiece are
   scattered over the subtree — so an exact expansion measures **9,526
   systems to find 145 in range, a 1.5 % yield**. EDDA's flat 50 ly
   partition is why their expansions are cheaper, and their own spike
   measured our octree costing *their* planner 1.7–2.6×. Helps every
   reader, item 3 included.
3. **A jump-priced cell graph**, which is 2f's lever 1 and the prerequisite
   for ALT: the only thing that attacks the exact settings' expansion count
   (2.6 M for a 32-jump charged route against 11,512 for a longer unaided
   one), and the only route to a proven crossing in something other than
   ten minutes.
4. **Positions from the payloads, cell-sorted.** The router's second copy of
   every position is unnecessary: the payloads hold `[f64; 3]` per system and
   are already paged per cell. A cell-sorted CSR directory over mapped
   payloads is the layout `ROUTING-INDEX.md` §3 measured at **1.6× the
   neighbour query and −62 MB** at 2.6 M. Partly overtaken by 2a — the
   router reads the payloads where they lie already — so what is left of it
   is the *ordering*, which is 2 above.
- ~~**Contract the graph.**~~ **Done — 2e.** §6.1 named this the only path to
  parity and §5 measured why it did not work then: boost-to-boost at 200 ly
  reached 127 of 106,642 nodes because 2.6 M systems were
  bubble-concentrated. *That was a coverage finding, and the coverage
  arrived.* Re-flooded over `.index/full`: 135 of 3,846,802 at one hop, and
  2,419,398 with four ordinary jumps of bridging after it. Query cost now
  scales with the boost count, and a charged crossing is 6.5 s against
  610 s.
5. **Hierarchy over the cells we already have.** `index.bin` is 38 MB and
   204,466 cells, resident for free. Route cell-to-cell on the aggregates,
   then refine inside the corridor. Still the structural answer for
   *unaided* long-range routes, where no contraction exists — the highway is
   jet cones and unaided there is nothing on it to use.
6. **`Quick` by default at long range.** Measured 18× cheaper unaided
   (72 ms against 1.58 s) for a route that is not proven minimal. Proven
   minimality over a galaxy is a thing to ask for, not a default.

#### 2h. Taking a plot back — done

Reported as "there is no way out of a plot once it starts", and there was
not: the button was live but a second click did nothing, because
`fetch_route` treated a leg already under way as the same question and
skipped it. Two halves, and the second is the one that mattered:

- **The click takes it back.** A leg in flight is dropped from
  `FetchTasks` and its frontier told to give up; a trip whose ends have
  changed cancels what is running and asks for the new one. `fetch_route`
  answers `Asked::{Plotting, Cancelled}` so the form stops saying it is
  working — nothing else clears `Plot::Working` and a cancelled route never
  lands.
- **The search actually stops.** Dropping a bevy `Task` does not interrupt
  a body the pool has begun, and a route walk is one unbroken stretch of
  arithmetic with nothing to await: a cancelled crossing would have gone on
  burning a pool thread for its full ten minutes with nobody left to read
  the answer. `Frontier::abandon` sets an `AtomicBool` beside the picture's
  lock and the search reads it once an expansion — one relaxed load against
  a 27 µs expansion. **Measured: the 610 s flat charged crossing gives up
  92 µs after being told to.**

### 3. Framing is a drawing decision, and it must stop being a loading one

**Reported: ~2 M systems in the spawn queue, right after a Sol → Colonia
plot started.** Framing the trip is what does it, and nothing in the chain
is wrong on its own:

1. `route::frame_trip` stands the camera back over the whole trip and sets
   the spyglass to the framing — clamped to `Spyglass::UNASKED`, **200 ly**,
   about the trip's *midpoint*. For a 22,000 ly route that bubble is eleven
   thousand light years from either end: it holds none of the route and
   bounds none of the load.
2. The LOD walk then marks every cell whose systems separate on screen,
   which at that distance is *most of the galaxy's cells*. Measured by the
   perf guard over `.index/full`: **12,503 marks at 100 kly and 97,891 at
   25 kly**.
3. The loader reads each marked cell's **whole payload** — 10.45 M points
   (0.4 GB) and 98.2 M points (3.9 GB) for those two zooms, 23.7 s of it at
   the middle zooms.
4. `bounded::reconcile` queues each resident cell's *resolvable prefix*
   into `PendingSpawns`, which drains `SPAWN_BUDGET` (2,048) a frame.
   Summed over tens of thousands of cells that is the two million the
   panel showed, and at 2,048 a frame it is minutes of draining for a
   picture that wants a few thousand marks. As measured the queue was also
   unbounded and each entry a built `System`; both of those are fixed
   below, so what remains of this step is the *count*.

`PendingSpawns::prune` is the only thing that culls it by position, and it
culls only while the spyglass is clearing and only against that same 200 ly
bubble.

**Six things, three of them done: the queue no longer builds what it
discards, no longer holds more than a frame can use, and draws what was
asked for before what it offered itself. What remains bounds the *read*,
orders the rest of the queue, and narrows the *reach*.**

- **Read a prefix, not a payload.** A cell's payload is written *brightest
  first* by construction (`tree.rs`, `a_payload_is_ordered_brightest_first`,
  "which the client leans on to draw a prefix without re-sorting") and the
  client already draws a prefix (`resolvable_count`). So the transport
  should ask for *the first N records of cell C* rather than the cell: a
  range request, which is exactly what `Source`'s `Part`/`Stamp` seam was
  built for (4.1). It bounds the read by what is drawn instead of by what a
  cell happens to hold — 12,503 marks wanting a handful apiece is a few
  hundred kilobytes against 0.4 GB.
- **Queue the reference, not the system — done.** A queued system is
  mostly one that never gets drawn: the camera moves, the walk moves with
  it, and the queue is weighed against the reach before anything is taken
  from it. So building a `System` in order to queue it was building what
  gets thrown away — a binary search of the names table, a hash of the
  populated table, and a heap-allocated `SystemName` — two million times,
  to draw a few thousand. `PendingSpawns` now holds `Waiting`: either a
  system built elsewhere (the fetch tasks build on their own threads on
  purpose, and a route's stop has no payload behind it at all) or *which
  point of which cell*, and the join happens in the frame that draws it.
  A queued point is 48 bytes measured against 144 for a built system, and
  none of the allocation. The cell may be freed or published again while
  the offer waits, so the build checks the point still carries the address
  that was offered and drops it otherwise — the walk offers whatever is
  there now next frame.
- **Bound the queue by the frame — done.** The same re-offer is what lets
  the queue be bounded: `QUEUE_CEILING` is thirty-two frames' worth
  (65,536), past which an unpinned offer is dropped unqueued, a pinned
  stop never. Nothing is lost that the walk will not say again a frame
  later, and the queue holds what the next half-second can draw rather
  than everything a view could ever want.
- **Draw what was asked for first — done.** The reported slowness, and the
  sharpest of the three: *a plotted route's own stops were queued behind
  every mark the walk had just offered.* One arrival-ordered queue at
  2,048 a frame, tens of thousands of walk offers ahead of them, so the
  line landed and stayed blank for seconds and then appeared whole — the
  frame the queue finally reached the contiguous run of stops `spawn`
  had pushed. `PendingSpawns` now keeps two queues and drains `asked`
  before `order`: what the user named by hand — a route's stops, a
  searched system flown to, and the walk's own re-offer of both — against
  what the map offered of its own accord. A landed route is ≈140 stops
  against a budget of 2,048, so it is drawn in the drain of the frame it
  lands in (`drain_spawns` runs `.after(spawn)`).
- **Prioritise the rest.** Within `order` it is still arrival order, where
  nearest-and-brightest is what a frame actually wants. The brightest part
  is nearly free — the walk offers each cell's prefix in payload order,
  which is brightness order — and the nearest part wants the cells walked
  near-to-far.
- **Frame the route, not a sphere about its middle.** What a framed route
  needs drawn is its own hops — already pinned and read out of the names
  table (`bounded::reconcile`'s "the route's own stops, which no cell prefix
  answers for") — plus whatever the splats carry. Every ordinary system in
  the corridor is sub-pixel at that distance. So the clamp should follow the
  route's *corridor* rather than a sphere at its midpoint, or be dropped in
  favour of a budget and the mark/splat cut trusted to say what is visible.

**What is measured and what is not**: the queue's two halves were fixed on
the structure rather than on a number — building and holding what is
thrown away is waste at any depth — and the sizes are measured (48 bytes a
reference against 144 a system, ceiling 65,536 entries against the two
million offered). What is still unmeasured is the frame that matters:
`pending.queued()`, the resident cell count and the points read, for a
Sol → Colonia plot at the framed view, which is what says whether the
prefix read and the corridor clamp are worth their complexity. The
diagnostics panel already shows the queue; the perf guard's zoom half
already shows the marks and the read.

This is the same defect as item 1a seen from the other end — "24 s and
6.1 GB to page in one zoom" — and the same sentence covers both: **the
payload is the unit of storage, not the unit of a question.**

### 4. Where this meets EDDA

There is no EDDA document in this tree — I grepped; `TODO-postgres.md`,
`TODO-source-sink.md`, `TODO-scale.md`, `TODO-scale-regions.md` and
`ROUTING-INDEX.md` are all of it. So this plan is written against the
`Source` seam as it stands, and two decisions in it should be taken with the
EDDA formats in hand rather than ahead of them:

1. **The served parts.** Items 1, 2 and 3 turn names, reaches, positions
   and a cell's drawn prefix into mapped fixed-width files. Over a
   transport those are range requests against the same files — which is what `Source`'s `Part`/`Stamp` seam
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

Item 1 has landed and is measured. `.index/full` and `.index/7day` are both
folded; a directory built before the format still holds MessagePack chunks,
and there are two ways across. A sync open does it on the way past
(`galos_index::migrate`, and `names::Writer::onto` seeds from the chunks
where there is no base, so a *resumed* build carries its names whether or
not anything migrated first). Or do it on purpose with `galos-index
fold-names <dir>`. A galaxy's worth is one external sort of gigabytes and
it happens once, ever, per directory — measured inside the 200 M rebuild
at 496 s for the whole index, names included.

**Both of those take `<dir>.lock`, and that is not optional.** The names
writer's scratch is a fixed path (`names/.building`) and whoever opens it
second removes what the first is streaming into. Measured the hard way: a
`fold-names` run beside a live import unlinked the import's row file, and
the build ended three minutes later in a bare `No such file or directory`
with no path in it. `galos-sync` already took the lock; `galos-index`
`fold-names` and `pack` now do too, and the four last steps of a cold
build's publish name themselves so an `io::Error` out of one is readable.

`.index/7day` is 730,544 systems and behaves as it always did. The perf
guard (`galos_map/src/perf.rs`, `GALOS_PERF_DIR`) only ever ran against a
seven-day directory, which is why none of this showed up in it — a guard at
131 M is still the first thing item 0 should add, and it is now the thing
that would have caught item 1a.

## What optimality should default to

Measured over `.index/full`, unaided, from the bubble outward, exact against
95%:

| crossing | range | jumps | optimal | 95% |
|---|---|---|---|---|
| 60 ly | 25 ly | 4 | 0.24 ms | 0.14 ms |
| 150 ly | 25 ly | 8 | 0.26 ms | 0.28 ms |
| 400 ly | 25 ly | 18 | 3.98 ms | 2.17 ms |
| 1 kly | 50 ly | 22 | 51 ms | 51 ms |
| 1 kly | 25 ly | 43 | 273 ms | 206 ms |
| 2 kly | 50 ly | 42 | 272 ms | 226 ms |
| 2 kly | 25 ly | 86 | **8.47 s** | **1.98 s** |

Every row came back with the same jump count at both settings. What the
slack bought was speed, and only where the route was long: nothing at all
under forty jumps, 4.3× at eighty-six, 300× at the hundred and forty of
Sol to Colonia.

**The cost is in jumps, not in light years.** Two kly at 50 ly is 42 jumps
and 272 ms; the same two kly at 25 ly is 86 jumps and 8.5 s. Thirty times
the wait over the same distance, because what the exact search has to prove
is that no shorter *chain* exists and the plateau of equally short chains
grows with the chain. So a default keyed to distance is keyed to the wrong
variable. The one to read is the jumps the crossing needs, and there is a
sound free estimate of it: `ceil(distance / range)`, which no route can
beat.

**And 95% already *is* the proof for short routes.** Weighted A\* returns a
route within `1 + over/100` of the fewest, jumps are integers, so at five
percent a returned route of `J` jumps says the fewest is at least
`ceil(J/1.05)` — equal to `J` for every `J` up to twenty. Below twenty
jumps the bound admits nothing but the optimum: there is no quality
difference between 95% and 100% to trade away, only the speed. That covers
most of the bubble at 25 ly and about a kly at 50.

What that does *not* cover is the other two approximations, which the one
gate switches on with the weighting and which no bound describes:

- The **fanout cap** keeps the 512 neighbours nearest the goal. Measured
  neighbour counts: 172 at 25 ly in the bubble, **1239 at 50 ly**, 165 in
  the core at 50 ly, tens of thousands at a boosted 200 ly. So the cap
  never bites at 25 ly and bites on every expansion at 50 — inside the
  bubble, where the weighted bound was saying the answer is optimal. It
  thins the half of the sphere pointing away from the goal, which is
  exactly the half a route around a void has to step into.
- The **coarse plan** over the boost stars is gated on distance and drive
  already, and is a guess at which cones are worth taking.

Which is the shape of the fix: the gate is one question — has the route
allowed jumps over the fewest — where the three approximations have three
different preconditions, and two of them are about the *geometry* rather
than about the ask. Each should be asked its own question, and each should
say whether it actually bit: a search whose cap truncated nothing, whose
plan never engaged, and whose returned jump count is inside the bound's
integer floor **is** the optimum, and can say so however it was asked.

### The cap keeps the cones

Done, and it was the first of the three to fix because it is the one that
loses a route rather than a few percent of one. The cap kept the 512
neighbours nearest the goal, which is the right order for an ordinary
system — whose only use is where it stands — and the wrong order for one
that can supercharge, whose use is the reach of the jump *out* of it: six
hundred light years at a fifty light year range. A cone off to the side of
the heading is worth more than a system a little further along it, and the
old order could not see the difference.

So `graph::thinned` puts the cones at the front and spends what is left of
the cap on the ordinary systems nearest the goal. Measured over
`.index/full`, on the charged routes where the cap actually bites:

| route | old | keeping cones | old time | now |
|---|---|---|---|---|
| 1 kly, 100 ly range | 9 jumps | **7 jumps** | 13.5 s | 17.3 s |
| 300 ly, SCO at 50 ly | 5 jumps | **4 jumps** | 0.33 s | 0.52 s |
| 1 kly, 50 ly | 19 jumps | **18 jumps** | 30.7 s | 32.0 s |
| 300 ly, 50 ly | 7 jumps | 7 jumps | 1.04 s | 1.39 s |
| Sol to Colonia, 50 ly | 141 jumps | 141 jumps | 0.47 s | 0.50 s |

Two jumps in nine, for 15–35% more time in the cap's own currency. And the
old rule was throwing away **half the cones it saw**: instrumented, one 300
ly crossing dropped 3,605 of 7,376, and the 1 kly one 464 of 545.

The instrumentation answered something else worth keeping: **the cap does
not bite at all on a planned galactic route** — zero bites over the whole
of Sol to Colonia. What `neighbors` collects is the neighbours worth
*relaxing*, not every system in the sphere, so the settled interior is
already gone by the time the cap is asked. The cap is a fix for dense space
at a boosted range, which is the bubble with a supercharging drive, and
that is exactly where it was costing jumps.

Cones are rare enough for this to be free — four systems in a hundred — and
where a sphere holds more cones than the cap allows, the cap is spent on
cones, nearest the goal first. An unaided drive pays no lookup at all.

Where slack genuinely costs something, beyond the jump count:

- **Fuel.** The router does not model scooping. An extra jump is a minute
  of flying, which is cheap against ten minutes of proving — but a route
  thinned toward the goal can string together non-scoopable stars, and
  that is not a 5% worse route, it is one that strands.
- **Voids.** The cap's bias toward the goal is wrong precisely where
  lateral movement is the route, so a detour is a hundred light years
  rather than a few percent.
- **Cone chains.** Dropping one boost star from a sphere of thousands can
  cost a whole leg of supercharged reach, which the bound says nothing
  about.

## Fuel

**A route that cannot be refuelled is not a slower route, it is a stranded
ship.** Which makes it the one thing on this page where being approximately
right is worthless, and it starts from a fact nothing in the index was
publishing.

### What can be known, and how

A fuel scoop takes hydrogen off the main sequence and off nothing else:
**K, G, B, F, O, A, M**, and nothing that is left over after the hydrogen
went. `galos_index::meta::scoopable` is that fact, matched on the whole
class rather than its first letter — `MS` and `S` are S-type stars and
`AeBe` is a Herbig star, all of which begin with a scoopable letter and
hold nothing to scoop, while `M_RedGiant` and `K_OrangeGiant` are the same
stars grown and do.

**Temperature cannot answer it**, which is worth writing down because it
looks as though it could. The payload's six log-spaced buckets put M
dwarfs, brown dwarfs and black holes in bucket 0 and O stars in with
neutron stars in bucket 5:

| bucket | kelvin | holds |
|---|---|---|
| 0 | 2 000–3 420 | **M**, L/T/Y brown dwarfs, black holes (clamped) |
| 1 | 3 420–5 848 | **K**, **G**, carbon stars, T Tauri |
| 5 | 29 240–50 000 | **O**, neutron stars, hot white dwarfs |

Subtracting the boost table takes out the neutron stars and white dwarfs
and leaves brown dwarfs indistinguishable from M dwarfs, which is the
commonest star in the galaxy against the commonest thing that is not a
star. So the class itself is the only honest source.

The class *is* clean where it exists: `Galaxy::arrival_class` answers
`None` where nothing has been scanned and no plotted route named one, so
"unknown" and "not scoopable" are already told apart at the source. The
panel now reads the class by the index's own rule
(`derive::arrival_class` — nearest the arrival point, ties by body id)
rather than a second rule beside it; reading it as "the star that goes
round nothing" would have named a different star in a close pair than the
published boost table does, and the two would have disagreed about the same
system on the same screen.

### The audit needs no new table

A galaxy-wide scoopable table is ~95 M rows — the scanned set, going by
`reaches.bin`'s 1.1 GB — and the existing sidecar pattern holds those in a
`HashMap<i64, _>`, which at that size is another gigabyte or two resident.
That cost buys a *search-time constraint* and nothing else.

**It buys nothing for the audit, because a route is a finite list.** The
stops of a plotted route are twenty to four hundred and fifty addresses,
and `info::StarClasses` already reads their arrival stars one at a time off
the task pool for the panel's own rows. So `info::Scooping` walks the route
in flown order and reports the longest run of stops known to have nothing
to scoop, and which stop it sets out from:

> nothing to scoop at 3 stops in a row, from PRAEA EUQ TZ-A C15-6, 12 stops
> unread

Said as a count and not as a verdict, because a verdict wants the tank and
the tank is the ship's. Nothing is said at all where a route can refuel
everywhere it is known to pass, which is most routes through settled space:
a line saying a route is fine is a line read every time to learn nothing.

**An unread class is not a starving stop.** The run is what is *known* to
have nothing to scoop and the unknowns are counted beside it, so the
reading is a floor — at least this long, with this many unread. A route
across unexplored space is mostly unread, and reading unknown as
unscoopable would condemn every galactic plot.

### A fuel reading that needs no ship — the distance, and the rule

The distance of the jumps was the wrong thing to say, because fuel is not
linear in it. What *can* be said without knowing the ship at all falls out
of dividing the game's own cost by the drive's maximum:

```text
fuel(d)           multiplier x (d x mass / optimal mass) ^ p        / d \ p
--------------- = ------------------------------------------- =    | --- |
max fuel a jump   multiplier x (R x mass / optimal mass) ^ p        \ R /
```

because the **range is by definition the distance at which a jump costs the
whole of the drive's maximum fuel**. The multiplier cancels. The laden mass
cancels. The optimal mass cancels. What is left is the jump against the
range the route was plotted at — which the route already carries in its own
identity — and the exponent, which is the drive's *class* and nothing else.

The exponent *could* be bounded rather than asked for — a jump is no longer
than the range, so `d/R` is at most one and a larger power makes it
smaller, which makes `p = 2.0` the dearest any drive in the game can be.
The panel said that ceiling first, "96.4 jumps' worth of fuel at most", and
it was the wrong reading. **Nobody flies a bound over all drives.** They
fly a class 5 with a specific tank, and for them the ceiling is a third
high:

| twenty jumps at a 50 ly range | p=2.0 | p=2.45 | p=2.9 |
|---|---|---|---|
| all at full range | 20.0 | 20.0 | 20.0 |
| uniform 0.5R–R | 9.8 | 8.5 | 7.4 |
| all at half range | 5.0 | 3.7 | 2.7 |

A number labelled "at most" still reads as a fact about *your* ship, and a
figure a third out is worse than no figure. So the panel states the
**distances**, which are exact, and carries the rule on hover, which is
exact for whatever is fitted:

> 1 322.4 Ly flown, longest jump 49.8 Ly
>
> *A jump of d costs (d / range) ^ p of the drive's maximum fuel, where p
> is its class: 2 → 2.00, 3 → 2.15, 4 → 2.30, 5 → 2.45, 6 → 2.60, 7 →
> 2.75, 8 → 2.90. The tank holds its capacity divided by that maximum...*

The arithmetic is left to the one party who knows what is fitted, and the
one figure they need beyond the drive's class — capacity over max fuel per
jump — is on their own outfitting screen.

The starving stretch is said the same way, in **light years to cross**
rather than in stops: six short hops and two long jumps are the same count
and nothing like the same fuel. The jumps counted are the ones flown out of
the last star that could refuel the ship *up to and including the jump that
lands on the next one* — a tank filled at the one has to reach the other,
and the jump that arrives was flown on the old tank.

> nothing to scoop at 3 stops in a row, from PRAEA EUQ TZ-A C15-6, 200.0 Ly
> to cross, 12 stops unread

### Weighing by fuel — done

`Routing`'s `shortest` flag became `Weigh { Jumps, Shortest, Fuel }`,
because the third ask does not fit a flag: it is not a tie-break on the
jump count, it is another thing to count. A step costs what the jump burns
plus a floor, and the ordering is fuel first with the jump count settling
ties (`graph::Burn`).

**The least fuel there is, is as many hops as the sky offers**, and the
first cut of this section said otherwise. The game's cost of a jump is
`r x (d x mass / optimal mass) ^ p x 0.001` with `p` from 2.00 to 2.90 by
the drive's class (community-deduced, and the wiki says so), so splitting
*any* jump into two always costs less and nothing in the arithmetic ever
stops splitting. What stops it is the star field running out of stars.
Measured with the jump priced at nothing, over `.index/full`, Sol outward
at a fifty light year range:

| crossing | fewest jumps | least fuel, unpriced |
|---|---|---|
| 85 ly | 2 jumps, 1.445 fuel | **16 jumps**, 0.323 fuel, hops down to 0.41 Ly |
| 300 ly | 6 jumps, 5.465 fuel | **94 jumps**, 0.848 fuel, hops down to 0.43 Ly |

Ninety-four jumps of a third of a light year each — an hour and a half of
flying to cross what six jumps cross in five minutes, for a sixth of the
fuel. That *is* the optimum, and it is not what anybody would fly. It also
took 9.2 s to find against 2.1 ms: unpriced there is no lower bound on the
fuel left to burn, so the estimate is nothing, A\* has no heading, and the
walk is Dijkstra's.

**So the trade is the reader's and the setting says it.** Crossing `D`
light years in `k` equal jumps is priced at
`k·FLOOR + TANKFUL·D²/(k·range²)`, least at a jump of
`range·√(FLOOR/TANKFUL)` — so pricing a jump at the fuel of a
`hop`-of-range jump is exactly saying *split no further than that*.
`Weigh::Fuel { hop }` carries it, the form offers it as **Shortest hop**,
and it is in the route's own identity, because it changes the answer:

| hop | jumps against the fewest | fuel against the fewest |
|---|---|---|
| 75% | 1.3x | 75% |
| 50% | 2x | 50% |
| 25% | 4x | 25% |

Half the range by default: twice the jumps for half the fuel. **Nothing is
the off position and it is a real setting**: no price on a jump is the least
fuel there is, which is the true optimum and what a reader is entitled to
ask for. It costs what it costs — with no price there is no lower bound on
the fuel left to burn, so the estimate is nothing and the walk has no
heading: 9.2 s over three hundred light years against 2.1 ms weighed by
jumps, and the galaxy off the scale. Said on the slider rather than
forbidden.
Measured over `.index/full` from Sol, 50 ly range, unaided, at that
default:

| route | weighed by | jumps | fuel | found in |
|---|---|---|---|---|
| 300 ly | jumps | 6 | 5.47 | 2.2 ms |
| 300 ly | **fuel** | 11 | **2.99** | 25 ms |
| 300 ly, 95% | fuel | 10 | 3.26 | 1.1 ms |
| 1 kly | jumps | 21 | 19.47 | 64 ms |
| 1 kly | **fuel** | 39 | **10.16** | 1.34 s |
| 1 kly, 95% | fuel | 36 | 11.00 | 4.6 ms |

The trade landed where the arithmetic said: half the fuel for one jump
under twice as many. What is *proven* is the least fuel at that price, not
the least fuel there is — which is the honest reading of the mode, and why
the route's own record says "least fuel over 50% hops" rather than "least
fuel".

**Two things had to be got right for it to be cheap, and one of them I got
wrong first.**

The estimate is the cheapest a light year can possibly be, times the light
years left. The first cut was a floor per jump over `left / widest` jumps —
admissible, four times too slack, and it cost two orders of magnitude:

| route | slack estimate | tightened |
|---|---|---|
| 300 ly | 4.39 s | **25 ms** |
| 1 kly | 102.97 s | **1.34 s** |

Same answers, seventy-seven times over. The tight bound is
`2·(L/range)·√(FLOOR·TANKFUL)` from the arithmetic above, *unless the drive
supercharges* — a jet cone throws the ship four times its range for one
jump's fuel, so a light year can cost `(FLOOR + TANKFUL)/widest` and an
estimate ignoring cones would overstate what is left and send A\* home with
the wrong answer. The cheaper of the two.

The other is the ledger. Its pruning rested on "one jump more is dearer",
which is true of both orderings that count jumps first and **false of
fuel**: five short hops can burn less than three long ones, so a
fuel-weighed search pruned by jump count would throw away the chains it is
looking for. `Metric::jumps` became `Metric::spent` — the monotone scalar a
step can only add to, the jump count for one ordering and the fuel for the
other — which is also why the floor is load-bearing twice: a step that can
cost nothing leaves the pruning with nothing to prune and the estimate with
nothing to bound.

**It changes nothing on a planned supercharged route.** Sol to Colonia at
95% comes back as the same 140 jumps and the same 134 jumps' worth of fuel
either way, because a plan's gaps are crossed by arithmetic rather than by
a search (`Crossing::Nearest`) — there is no chain to choose between.
Weighing by fuel bites on a flat search, which is what an unaided route is
at any distance, and on a plan whose gaps are searched.

### The ship stats ladder, which is the open design question

Fuel per jump in the game is `multiplier x (distance x mass / optimal
mass) ^ power`, which wants the FSD's class and rating, the ship's laden
mass and its tank. That is four numbers the map has never asked for, and
most of them fall out of one it already has: the jump **range** is by
definition the distance at which a jump costs the drive's maximum fuel. So

```text
fuel(d) / max fuel per jump = (d / range) ^ power
```

and everything needed is *dimensionless*: how many full-range jumps the
tank holds (`capacity / max fuel per jump`), and the exponent (2.0 to 2.9,
by FSD class). Which gives a ladder rather than a wall, and each rung is
honest about what it can say:

| what the user gives | what can be said |
|---|---|
| nothing | how long the unscoopable stretches are — **done** |
| jumps per tank | whether a stretch strands the ship, and where |
| + FSD class | how much cheaper hopping is, which is the whole of rung three |

**Two jumps of N light years are not one jump of 2N.** The exponent is the
point: one long jump costs `2^(p-1)` times what the pair costs, which is
2.0x at an FSD class 2 and **2.7x at a class 5**, the common case. Covering
the same ground in more hops, against one full-range jump:

| hops | fuel |
|---|---|
| 1 x range | 1.000 |
| 2 x range/2 | **0.366** |
| 3 x range/3 | 0.203 |
| 4 x range/4 | 0.134 |

Halving the jump length cuts fuel per light year by some sixty percent.
Which says something uncomfortable about what is already here: **the router
optimises the metric that maximises fuel burn.** Fewest jumps means every
jump as near the ship's range as the sky allows, and a jump at full range
costs the drive's maximum fuel by definition. A fuel-hungry route is not a
side effect of the search, it is the search's objective.

So fuel and jumps are opposed, and rung three is not "avoid the starving
stretch" but a second dimension on the cost. A ship a jump and a half from
empty, facing a six jump stretch with nothing to scoop, can often cross it
by hopping: the same distance, a third of the fuel, three jumps more. That
is a route the search cannot express today, because the only thing it ever
bids for is fewer jumps.

It sharpens the rung above it too. Jumps per tank is exact only if every
jump is at full range, and they will not be — so it is a *floor* on what
the ship can cross, the same shape of honest reading as the audit's floor
on the run length. And rung three needs the exponent specifically because
the saving *is* the exponent: without it the map can say hopping is
cheaper, but not that hopping gets you across.

One thing to settle before any of it: whether a jet cone boosted jump
charges the drive's maximum fuel whatever the boosted distance comes to.
If it does — and nothing here has verified it — a supercharged jump is the
cheapest light year in the game, and a fuel-aware router should reach for
cones for a second reason entirely.

Nothing has been built for rung three, and nothing should be until the
audit has measured how often a plotted route actually starves.

## The optimality slider is the greediness

Reported, and correctly: a route under a hundred percent should not need
*proving*. It does not. Weighted A\* multiplies the estimate by
`1 + over/100`, which drives the search harder at the goal, and the theorem
says the **first** route it arrives at already costs no more than that
multiple of the fewest. The promise is kept by the arithmetic.

A cut of this went the other way and built a three-rung ladder: a greedy
chain for an upper bound, a hurried walk pruned by it, then the asked-for
walk pruned at `U x WHOLE / (WHOLE + over)`. Every rung was sound and the
whole thing was pointless — and it is what produced the *other* report, the
map sticking for a moment on the last jump. Measured on a 1 kly crossing:
the answer was in hand at **2 ms** (twenty-one jumps, inside the five
percent asked for) and the last pass then spent **52 ms establishing that
no twenty-jump route exists**. A proof nobody wanted, running after the
route was found, which is exactly what a pause at the end looks like. The
ladder is gone; `search` is one walk that returns on arrival.

What the ladder's measurements were good for was showing where the slider
actually bites. Sol outward, unaided, 50 ly range, over `.index/full`:

| optimality | 1 kly | 2 kly |
|---|---|---|
| 100% | 69 ms | 318 ms |
| 95% | 54 ms | 230 ms |
| 90% | 34 ms | 54 ms |
| **80%** | **6.4 ms** | **18 ms** |
| 70% | 4.7 ms | 13 ms |
| 50% | 2.6 ms | 5.7 ms |
| 30% | 2.5 ms | 5.2 ms |

**Every row came back with the same route** — 21 jumps and 41 jumps. So
95%, which is where the earlier thinking landed, is A\* with a nudge: it
buys 1.3x. The knee is between 90% and 80%, which buys eleven times the
speed against the proof, and below 70% there is nothing left to buy.
`Routing::default` is now 80% for that reason, with the table in its doc
comment.

None of which says the slack is free in general. These two corridors had no
real choice in them; the promise remains "within that percent of the
fewest", and a corridor that genuinely forks will spend some of it.

## Pruning, and why A* balloons anyway

Reported: the drawn closed set swallows the screen even at 95%. Two things
came out of chasing it, and the second is the one that matters.

**The pruning was made certified rather than guessed, and then removed as
unnecessary** — see the section above, which is the later and better
answer. What it did was walk three rungs, each paying for the next one's
pruning:

1. A **greedy** chain — take whatever jump lands closest to the goal, one
   sphere a jump, no heap and no ledger (`stepped`, off the gap-crossing
   walk). Measured on a 1 kly corridor it arrives in 22 stops and **is the
   optimum** (21 jumps), in a millisecond.
2. The same A\* leaning hard on the estimate (`HURRIED`, twice the
   estimate), pruned by what the greedy chain cost. Pruning at an
   incumbent's own cost loses nothing: a route dearer than one already in
   hand is not wanted at any optimality.
3. The asked-for walk, pruned at `U x WHOLE / (WHOLE + over)`.

Rung three is the part that is *inside the promise rather than a guess at
it*: if the best route there is went through a system pruned that way, then
the best route costs more than `U / (1 + over/100)`, so `U` is already
inside the percent asked for — and `U` is in hand. Either the walk finds
something at least that good or what is held was already good enough. At
nothing over the ceiling is `U` itself, which is branch and bound and
lossless.

For that to be sound the estimates had to become **admissible again**: the
weight used to be baked into each estimate closure, which left the ceiling
nothing honest to measure against. The leaning now happens in `walk`
(`Metric::share`), and the estimate handed to it is a true lower bound.

**And it buys 1.6x, not the order of magnitude the picture suggests.**
Measured, 1 kly unaided: 83 ms at 100%, 53 ms at 95%, same 21 jumps. The
reason is the theorem rather than the code: **A\* is optimally efficient
given its estimate** — it already expands only systems with
`g + h <= C*`, and a ceiling of `C*` cannot beat that. The cloud *is* that
set. On a 1 kly route at 50 ly it is every system whose jumps-so-far plus
straight-line-jumps-remaining is within twenty-one, which around a void is
most of the near side of it: the estimate cannot see that the route has to
go round, so every system on the wrong side looks promising.

So the lever is not more pruning. It is **a better lower bound**, which is
the one thing that shrinks the expanded set without giving up the promise:
a coarse pass over cells — the same trick `highway` plays over boost stars
— giving a distance-to-goal that *knows about the void*, and a landmark
(ALT) heuristic on top. That keeps optimality and makes the picture smaller,
where a corridor drawn about the straight line would make the picture
smaller by ruling out the answer.

## The star kind rides in the payload

**A router asks what kind of star every system it expands has**, and the
answer decides two things it cannot get anywhere cheap: whether a ship can
refuel there, and whether it can supercharge. So the kind is one byte in the
cell payload, beside a position the expansion loop has already faulted —
not a sidecar table, which at the ninety-five million classed systems is
gigabytes resident, and not the names table, which the router never reads at
all. `galos_index::meta::StarKind`: sixteen families, `Unknown = 0`, with
`scoops()`, `boost()` and `named()` off the byte.

`Unknown` is load-bearing rather than a default. Most of the galaxy has
never been scanned, and "nothing has looked" has to stay distinct from
"nothing to scoop" — otherwise every route through unexplored space reads
as a route that strands.

Landed, end to end: the kind is derived where every other fact is
(`Galaxy::arrival_class`, and `primary_star_class` on the database side),
carried through `System` and the build's `Record`, and written into the
payload. Two versions moved with it, deliberately:

- `INDEX_VERSION` 2 → 3, because a payload block carries no header and a
  width change cannot be caught in the block itself. A stale directory is
  refused at `index.bin` and rebuilt, which is the rule that constant
  already documented.
- `checkpoint::VERSION` 1 → 2, because `System` went from 56 bytes to 64
  (the byte did not fit the `u32` pair's tail) and a resume point at the old
  width would be read as another galaxy.

### The payload is columns now

**Elite's coordinates are multiples of 1/32 of a light year**, which is a
measured fact and not a hope: of 1,230,297 axes sampled out of
`.index/full`, 415 — 0.034% — were off that grid, the worst by 0.04 ly. So a
position does not need an `f64` triple. It is an integer count of
thirty-seconds from its cell's low corner, and for a 1024 ly cell a `u16`
holds it **exactly**; a 2048 ly cell wants 65,537 counts and misses by one,
so it and everything coarser take a `u32` — 3,741 cells of 204,466, holding
fewest systems each.

And the fields are laid in runs rather than records, because the two readers
want different ones:

```text
cells/<shard>/<id>.bin
  header   12 B   magic, version, count, axis width
  pos      N x 6  u16 x 3, cell-relative on the 1/32 ly grid
  kind     N x 1  what star a ship arrives at
  id64     N x 8
  lit      N x 9  magnitude, temperature bucket, updated_at
```

The router's expansion loop reads **7 bytes a system** where it used to
fault 41, and never touches the magnitude, the temperature or the moment at
all. The block also carries its own magic, version and count, which the
record blocks did not — a stale one is refused rather than read as a
plausible number of systems with every field out of the wrong bytes, which
is what `INDEX_VERSION` existed to catch on its behalf.

#### And 5.9x fewer bytes is not 5.9x anything else

That was the claim here, and it was arithmetic — 41 over 7 — on the grounds
that the loop measured 45.4 billion systems looked at to relax 3.9 million,
so the bytes it streams are the cost. Measured, both layouts over the *same*
730,544-system directory (`.index/7day` copied twice, one copy left as
records and the other put through `galos-index upgrade`, read by a HEAD
worktree and by this tree respectively, M5 Pro, release):

| | records, 41 B | columns, 24 B |
|---|---|---|
| `cells/` on disk | 31 MB | **21 MB** |
| zoom read, first, cold | 106.1 ms | **91.6 ms** |
| zoom read, warm | 11.8 / 11.3 / 9.7 ms | 12.1 / 10.9 / 9.5 ms |
| a 24-stop route, 95% | 787.9 ms | 796.0 ms |
| a 24-stop route, optimal | 798.2 ms | 782.7 ms |

So the disk win is real and is the byte count — 0.68x, which is 24 over 41
with the header paid — and **the expansion loop did not move**: 783–796 ms
against 788–798 ms, inside the run-to-run spread either way. One cold read
improved by 14%, which is the only place fewer bytes showed at all.

The reason is in `store::Payload::position_at`. A record's position was
three `f64`s to load; a column's is three unaligned integer loads, a
`match` on the axis width *per axis*, three converts to `f64` and three
multiply-adds off the cell's corner. The rewrite traded bytes for
arithmetic, and over a working set that fits in the page cache the
arithmetic is what the loop is paying. Fewer bytes buy time where the bytes
are actually being fetched, which is the cold column and the 165 GB
directory — not a corridor read twice.

**What is still unmeasured, and why.** The same A/B over `.index/full`,
where the working set is far past RAM and a charged route streams billions
of system reads, cannot be run: that directory was rewritten *in place*, so
no record-layout copy of it exists and making one is a reimport. What can
be compared is a recorded figure against a fresh one, over the same two
ends — the 3 kly exact charged route, 32 stops, **72 s recorded before the
rewrite against 81.1–82.8 s measured after it**, and the unaided one at
682–702 ms against 600–680 ms. Both are slower, both by about a seventh,
and both readings are of the same shape as the decode cost above. It is a
doc row against a re-run rather than an A/B, so it is a suspicion with a
number on it and not a result.

### And a rebuild that is not a reimport

`galos-index upgrade <dir>` brings a directory forward in place. **Named
for the job and not for this version's layout**: it is what
`store::Index::read`'s refusal tells an operator to run, and the next format
change lands beside the payload rewrite in `upgrade` rather than as another
subcommand. Not run at open, unlike the resharding in `source::migrate` —
that is a rename a file, this is a re-encode of every cell plus a sweep of
the scan record, and a client that wants to draw cannot spend hours without
saying so. The old
payloads hold everything the new ones do but the star kind, and the kind is
derivable from the directory itself — `bodies/` is the scan record the class
comes from. So:

1. `pack::each_arrival_class` sweeps the scan record **shard by shard**,
   mapping each index once and walking its live entries in written order.
   Asking `pack::find` per address would map the same index a thousand times
   a shard and seek at random through gigabytes; ninety-five million of
   those is not a migration, it is a reimport with extra steps.
2. The kinds are held as two sorted columns — addresses and bytes, nine
   bytes an entry, answered by binary search — rather than a `HashMap` over
   ninety-five million keys, which is gigabytes of buckets.
3. Each cell's old block is read, the kind joined on, and the new block
   written. `index.bin` is rewritten **last**, so a run stopped part way is
   told apart from a finished one by the file every reader checks first, and
   running it again takes up where it left off: a cell already columnar is
   counted and left alone.
4. The supercharge table is given the places its rows imply, where it has
   not been already — `source::place_boosts`, the same pass an open runs.
   It belongs here because an open over a *stale* directory runs nothing:
   `migrate` asks `store::stale` first and returns without touching a
   single thing, naming this command. So a directory brought forward by the
   command alone would still hold the two-field table it was built with,
   and anything reading the boosts without opening the galaxy first fails
   to decode a row rather than finding a jet cone. The map's own perf guard
   does exactly that, and did exactly that: `invalid length 2, expected
   struct SystemBoost with 3 elements`, on a directory `upgrade` had just
   reported finished. It is one read, one pass over the address column and
   one write, and a table already placed costs one decode to find out.

Nothing else in the directory is touched. Only `Point`'s width changed and
`Cell::LEN` did not, so the tree, the aggregates, the names table, the
bodies and every sidecar stay exactly as they were.

**And the automatic migration says when it cannot help.** It did not, which
was the hole: everything `source::migrate` does is content-blind — moving
files into shards, folding chunks — so it ran happily over payloads of a
layout this build cannot read, and the refusal surfaced later out of
whatever first asked for a cell, as a failed open with no remedy attached.
It now asks `store::stale` *first* and returns `Migrated::upgrade`, having
touched nothing, and `galos_db` says so at `warn` naming the command — the
only place the remedy appears before every read starts failing.

The same check found a real bug in the rewrite: the sweep reads the packed
shards and nothing else, so a directory with body files still loose would
have had those systems swept as though nothing had ever looked at them,
every one coming out `Unknown` and the kind column quietly wrong. The
rewrite packs first now, which is idempotent and is the same pack an open
runs.

### What is still owed

**`Weigh::Refuels { tank }`**, which is what the byte was for: fewest
refuels as the whole metric, with `(refuels, jumps)` left for the time in
seat mode.

## The pack was broken, and nobody knew

Running the upgrade turned up something older and worse than anything the
upgrade needed. `pack::append` reads the sixteen header bytes off a shard
index to learn its generation and base, and handed *that buffer* to the
header check — which validates "a base of n entries has n entries behind
it" against `bytes.len()`. Sixteen bytes hold zero entries, so **every
append to a shard that had ever been folded failed**:

```text
.index/full/bodies/797.idx: a base of 8218 entries in a file holding 0
```

Which means packing had never worked on that directory. Measured on it:
3,941 shard directories holding an average of 12,845 loose files each —
**about fifty million body files** that every open had tried and failed to
pack, silently, for as long as the fold threshold had been reachable. The
sweep the upgrade needs reads packed shards, so without this the kind
column would have come out `Unknown` for nearly the whole galaxy and
nothing would have said why.

The check is now split: `header_fields` reads the sixteen bytes and claims
nothing about what follows, `header_of` checks a base against the file it
came from, and `append` checks against the file's real length.
`a_folded_shard_still_takes_an_append` folds a shard by hand and appends to
it — nothing caught this before because a fold needs a tail of thousands
and no test had reached one.

**And the lock leaked.** `std::process::exit` runs no destructors, so a
command that exits from an error arm while holding the directory lock
leaves the lock file behind and the next run refuses the directory as
"already being written" by a process that is gone. Three commands did it;
they route through `leave(lock, code)` now.

### Then it was slow, and that was three separate things

Packing fifty million files at the rate it started would have taken twenty
hours. Measured, each fix in turn:

| | files a second |
|---|---|
| as it was | 670 |
| membership off a cached table, not a `find` apiece | 1,350 |
| the shard's directory dropped whole, not fifty million `unlink`s | 2,100 |
| eight shards packed at once | **4,061** |

The first was asking [`find`] per loose file — mapping the shard index,
scanning its tail, binary searching its base and reading the data file — to
answer whether the pack already held that system. One `Table::read` a
shard answers it instead, with what this run appended added to the set so a
duplicate loose file is still dropped rather than appended twice.

The second: a shard directory's files are all one shard's, so they are
appended in batches and the directory taken away in one call once the
records are durable. Only where *everything* in it was a loose body file —
a directory holding anything else keeps it, and what was packed out of it
goes a file at a time, which is the case
`a_shard_directory_keeps_what_the_pack_does_not_understand` holds (and
which I got wrong first: the mixed case removed nothing at all).

The third is threads over shard directories, which share nothing but the
disk — sequential *within* a shard, since its index is appended to and
folded and two threads doing that to one file is a corrupt shard.
