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

#### 2a. `Places` no longer copies the galaxy — but still buckets it

Reported from the map as "clicking Plot Route starts blowing up memory a
lot… from like 7 GB to 20". It was arithmetic, not a leak. `Places` held
three copies of what the names table already maps:

| | at 200,071,629 |
|---|---|
| `points: Vec<(i64, [f64; 3])>` | 32 B a system = **6.40 GB** |
| `by_address: HashMap<i64, usize>` | 268 M slots × 17 B = **4.56 GB** |
| `buckets: HashMap<_, Vec<usize>>` | 8 B a system plus a heap block per occupied bucket = **~2.7 GB**, ~72 M allocations |
| transient: `collect()` doubling, the `filter` having killed the size hint | up to **+3.2 GB** |

≈13.7 GB, which is the +13 that was reported.

**All three are now gone.** The base is the mapped table itself
(`Held::Mapped`): addresses and positions are slices of `addr.bin` and
`pos.bin`, and an address is a binary search because they are sorted, so
`points` and `by_address` simply do not exist. The bucketing is a CSR —
every row number once in one `Vec<u32>`, grouped, and a map saying where
each group sits — which is two allocations instead of 72 M and 4 bytes a
system instead of 8 plus a vector header. The feed's arrivals stay held
(`Held::Given`), being thousands.

Measured, whole test process peak resident:

```
route graph: 200071629 systems in 32.11s   peak RSS 4.90 GB
```

which accounts as: `pos.bin` faulted in by the two counting passes 2.4 GB
(file-backed and evictable), the CSR rows 800 MB, the bucket map ~180 MB,
and the search's own `best` + `came` 1.6 GB. So **anonymous heap went from
~15.3 GB to ~2.6 GB**, and a third of what is left is not heap at all.

**Two costs remain, and neither is fixed by this.**

1. **The 32 s has not moved** (34.23 s before, 32.11 s after). It is 200 M
   × `bucket_of` plus a hash lookup a system, twice, and that is the floor
   for building *any* galaxy-wide grid. Shaving it is possible — assigning
   bucket ordinals on the first pass would delete the second pass's hash
   lookups at the cost of 800 MB transient, maybe 40 % — but that is a
   constant factor on a structure this item exists to delete. **The answer
   is not to build a global grid at all**: neighbour queries should go
   through `index.bin`'s cell tree and the payloads, which the LOD path
   already reads at 26–56 ms, so the work is the corridor's and not the
   galaxy's.
2. **`best` + `came` are 1.6 GB a leg**, allocated before the first
   expansion: `vec![C::MAX; held]` and `vec![UNSEEN; held]` over every
   system there is. Dense arrays over the galaxy cannot survive a
   corridor-bounded search either — they want to be maps over what the
   search actually reached, or arrays over the corridor's own index space.

Both of those are the same conclusion `ROUTING-INDEX.md` §5 reached: the
search space must be smaller, not the index faster.


- **Positions from the payloads, cell-sorted.** The router's second copy of
  every position is unnecessary: the payloads hold `[f64; 3]` per system and
  are already paged per cell. A cell-sorted CSR directory over mapped
  payloads is the layout `ROUTING-INDEX.md` §3 measured at **1.6× the
  neighbour query and −62 MB** at 2.6 M; at 131 M it is the difference
  between an 11 GB resident grid and a corridor-bounded read. **Deleting
  `Places` is also what makes item 1b's 2.4 GB of duplicated positions
  removable**, so take the two together.
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
