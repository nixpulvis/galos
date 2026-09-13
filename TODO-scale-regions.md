# The region build, and what is left of it

`TODO-scale.md` named four walls between 2.7 M systems and 200 M. Three are
down and measured. This is what was built on the `scale-regions` branch, the
numbers it was measured with, and the work still open — in the order it is
worth doing.

Everything here was measured on one machine (M5 Pro, 18 cores) against
Spansh's seven-day slice: `galaxy_7days.json`, 19,947,189,475 bytes,
**730,544 systems and 3,119,799 bodies**. The full `galaxy.json` is 610 GB
and about 200 M systems; it has been started three times and **not yet run
to the end** — see items 2 and 2a for what stopped it.

## State of the tree, 2026-09-13

Branch `scale-regions`, **committed**, working tree clean:

| | |
|---|---|
| `2116e72` | Stop carrying a built index directory in the repository |
| `c3fcfed` | Build the index a region at a time, from a dump or a database |
| `4a470fb` | Stop asking a directory being raised what it already holds |
| `d401796` | Take up a stopped import where it left off |
| `4b51a2a` | Publish what a stopped read has read, and carry on from it |

`d401796` is superseded by `4b51a2a` and is history rather than code: the
spill-cutting it added — `Buckets::resume`, the mark's byte counts,
`Chunks::resuming`, the row files' lengths — is gone, replaced by one file
holding one cursor. The `elite_journal` submodule carries two commits of
its own (`2a89aa5` star classes, `77dfb45` the journal reader), and the
parent's gitlink names the second.

**Index directories on disk.** `.index/full` is the 200 M import, part
read. `.index/7day` is the seven-day slice. `.galos_index` is the feed's
own and is **format version 1, so it is refused** — `index format version
1, this build reads 2: the payload record changed width, so rebuild the
directory`. That is the intended behaviour, not a fault; see item 2c's note
on the version bump.

**Databases.** `galos_development` and `galos_postimport_backup` remain,
plus `galos_test_template`, which is the test harness's cache and is
supposed to persist. `galos_7days`, `galos_conformance`, `galos_import` and
`galos_p60k` were dropped deliberately. **Both remaining databases predate
the politics fix** below, so each holds NULL politics for every system that
came in with a body count; re-import before trusting either for a
comparison.

**Tests provision their own database.** `galos_db::testing::Scratch`
(feature `testing`) hands a test a fresh migrated database copied from
`galos_test_template` and drops it in `done()`; a reaper clears any
`galos_test_<pid>_<n>` whose pid is dead, which is the panic path.
`TEST_DATABASE_URL` now names a *server* — `postgres` will do — so nothing
has to be created by hand and nothing is left behind.

```sh
# the suite: 1,507 tests, needs no database created by hand
TEST_DATABASE_URL=postgresql://localhost/postgres cargo test --workspace

# an index from a dump, no Postgres in the path. Ctrl-C publishes what has
# been read; running it again carries on from there.
cargo build --release --bin galos-sync
./target/release/galos-sync --from spansh=~/Downloads/galaxy.json \
    --index DIR                      # GALOS_REGION_BUDGET is the memory dial

# the same, into Postgres, eight readers over one file
./target/release/galos-sync --from spansh=FILE --db --bulk --shard 0/8

# the client guard, against a built directory
GALOS_PERF_DIR=$PWD/.index/7day \
  cargo test --release -p galos_map --lib perf -- --nocapture

# what a directory holds
./target/release/galos-index info DIR
```

## Measured

**Reading the dump.** 30,000 systems a second, 7.4 MB resident: the whole
seven-day file parses in 24 s and the 200 M `systems.json` in 67.7 s. One
`serde_json` call per line into a reused buffer; there is no incremental
parser and the module header says why.

**Into Postgres.** 165 → **1,165 systems a second**, 7.1x, and the parts are
not equal:

| change | gain | what it was |
|---|---|---|
| one transaction per entry | 1.14x | 32.35 commits a system → 6.97 |
| `--bulk` | 2.47x | `synchronous_commit = off`, 16 connections |
| `--shard i/8`, 8 processes | 2.5x | one file, eight readers |

Nothing was CPU-bound at any point: the client sat at 1.1 %, each backend at
0.3 %, 18 cores idle. The whole seven-day import is 43 minutes and
3,849,515 messages.

**Into an index.** Over 240,000 systems, the same input three ways:

| route | peak RSS | per system | wall |
|---|---|---|---|
| event sink, live `Tree` | 389 MiB | 1,700 B | 31.1 s |
| region build, 7 M budget | 82.8 MiB | 362 B | 25.1 s |
| region build, 50 k budget (24 regions) | 65.5 MiB | 286 B | 26.6 s |

The budget is the dial, not the galaxy. The whole seven-day dump builds in
**64.9 s at 198.6 MiB**, the three sidecars a record can fill included (see
item 1), and the index it writes is identical in every figure to the one
built from Postgres — 907 cells, 761 leaves, levels 0..11, largest leaf
4,042, brightest M_abs −12.57, total flux 2.767e6 — where the database
route needs a 43-minute import first.

**Losing the counting pass.** The two-pass build was 90.8 s; one pass is
64.0 s. At 240 k the counting read was 5.1 s of 25.1 s, and it parsed only
`coords`. At 568 GB it is a second full read of the file.

**Stopping.** A Ctrl-C mid-migration was 97.3 s, because the one-time
body-file reshard of 559,582 files ignored the stop flag. It is 0.05 s. A
SIGINT 0.9 s into a cold build exits in 0.023 s with code 0. A `finish`
that takes the delta road is 3.4 ms where the whole-directory rewrite was
26 s over 240,000 systems. A stop during the read publishes what it has
read and records where it got to, so the map can open it and the next run
carries on — item 2b.

**One file a system, which is what the import costs.** 91 % of a dump
import's wall clock was body files: three `open`s and a `rename` a system,
two of the opens for names no directory holds. Over 50,000 systems, 26,992
of them scanned, 7.13 s to 4.65 s and 3.20 s of kernel time to 1.48 s with
those gone (`Published::raising`). What is left is the file count: at
3.16 M body files the live 200 M import had fallen from 7,012 systems a
second to **1,740**, and 188 M files at 4.4 KB allocated apiece is ~830 GB.
That is the packing, not the write path.

**The client, against `test2/`.** `walk_screen` 96–104 µs at every zoom; the
read 158 ms cold and 11–14 ms warm; the route graph 42.9 ms over 730,544
systems; a 24-jump route 223–225 ms. The read is ~5 % above the figures
taken before format 2, which is the 39 → 41 B record: payload bytes per
zoom went 26,200 → 27,544 KB.

## What the build is

One type. `galos_index::Build`: `begin(dir, checkpoint, params, budget)`,
`push(system, name)` per record, `finish(by, cursor)`.

1. Each pushed system is appended to the spill file of its **bucket**, a
   fixed level-4 cell — 4,096 possible, of which the galaxy's disc occupies a
   few hundred. Buffered at 128 records, so one file handle is open at a
   time whatever the galaxy's shape.
2. Counts are exact, tallied while writing, so the **cut** is formed from the
   buckets afterwards and no counting pass is needed. A bucket over budget is
   split by re-reading only that bucket, one level deeper, recursively; a
   piece that cannot divide — at `MAX_LEVEL`, or holding systems that share a
   position — is reported as a region over budget rather than hidden.
3. `Crown::over` settles the cells above the cut, each region is a
   `Snapshot::of_region` over its own systems minus what the crown took, its
   payloads are written and it is dropped.
4. `joined` writes the index file.

A region can be built alone because the coupling is bounded: a region's
crown cells are its own ancestors, `region.level` of them, each owning at
most `internal_slice`, so a region can lose only its brightest
`region.level × internal_slice` systems and it is enough to offer exactly
those. Nothing fainter can be claimed — a candidate that finds no room
proves the crown was full, and the crown never empties.

Two callers push into it: `galos_db::index::build_cells` from a merged pair
of SQL cursors, and `bin/sync/spansh.rs` from one read of the dump. The
oracle is `cold::tests::a_cold_build_is_the_whole_build` and
`region::tests::a_regional_build_is_the_whole_build`: a pieced build against
`Snapshot::build` over the same systems, identical cell set, identical
`rank_lo`/`rank_hi`/`child_mask`, byte-equal payloads.

A stopped build answers a value rather than an error: `Built::Stopped`
carrying `Abandoned { systems, raised, regions, intact }`, and
`galos_db::index` passes it up as `Reached<T>`. Up to the point of no return
the directory is left exactly as found — the names table is staged in
`dir/.building/names/` and published by rename, which it was not before: a
cold build used to overwrite the served table one 64 Ki chunk at a time as
it read.

## Bugs this work found

Worth keeping, because each was invisible until two derivations could be
diffed on real data, and the next one will be too.

- **The database dropped the politics of every system that carried a body
  count.** `System::report` branched on `body_count` and routed to
  `set_body_counts`, which writes no politics — true when journals and EDDN
  were the only sources, false for a dump that says both. 52,868 of the
  seven-day file's 52,908 populated systems lost allegiance, government,
  security, economies and population. Fixed: a report now applies both
  halves of what it says, verified as an exact column-for-column match
  against the dump over 60,000 lines. `edsm/src/system.rs:42` already
  deserialises `bodyCount` and the sync does not read it — wiring that up
  before the fix would have reproduced the loss.
- **Anarchy was not a security reading.** `Security::Anarchy.is_null()` is
  true and Postgres has no label for it, so every anarchy system's row was
  refused and its bodies went with it behind the foreign key. ~13 % of the
  file. Fixed in `spansh`'s deserialization, where the rest of the codebase
  already applied the rule.
- **Two star records under one name double-counted a system's light.** The
  dump lists a star twice, differing only in `bodyId` and
  `distanceToArrival`; both derivations now keep the nearer record, and the
  three affected systems were 0.76 mag too bright.
- **An uploader was filed as `unknown`.** `src/sink/relay.rs` carried only a
  commander across the channel, so every dump-sourced body file recorded
  `unknown` while Postgres recorded the file — which is why all 277,551
  body files differed between the two routes.
- **Barycentres came back in Postgres heap order.** `each_scanned`'s cursors
  lacked a secondary key, so 7,680 body files differed by ordering alone.
- **`galos-index info` measured shard directories, not payloads**, ever
  since the sharding landed.
- **The client guard was not compiled.** `galos_map/src/perf.rs` had no
  `mod perf;` declaration for part of the session. It is
  `#[cfg(test)] mod perf;` at `galos_map/src/lib.rs:31`.

`tests/derivations_agree.rs` is the oracle these came from, and its fixture
now carries a same-named twin star, a second barycentre and a non-commander
reporter — the three shapes whose absence let all of this pass.

## What is left

### 1. The sidecars the cold route holds — done

`Build::finish` writes the names chunks, the cell payloads, the index file
and the checkpoint. `bin/sync/main.rs::cold` writes `populated.bin`,
`reaches.bin` and `boosts.bin` beside it. Those three used to come out of a
`galos::sink::Tables` held across the read — one row a system, reaches one
per *scanned* system, **measured 22.4 MiB over the seven-day slice** and
some 6 GiB at 200 M. `factions.bin` stays unwritten: nothing reading
records can number a faction, and an empty table would say the galaxy has
none where an absent one says this index cannot tell.

**The holding is gone.** `galos_index::Rows` writes a row to a file as it
is derived, the way a name goes to a chunk, in `<checkpoint>.rows/`; the
tables are made from those files once the build has published, and
`Rows::onto` seeds them from the tables a directory already publishes where
a read is being carried on — see item 2b.

**And so is the sort.** `Rows::finish` read the rows back into maps to put
them in address order, which was the galaxy's worth of them in memory once,
at the end. It is an external sort now: runs of `RUN_BYTES` (128 MiB) read
back, sorted stably and written out, then merged by a scan over the runs'
heads — tens of runs over a galaxy, so a heap would cost more code than it
saves. The last row an address has still wins, because a run is a stretch
of the row file and every row in one is older than every row in the next.

The table itself is streamed too: its length is known before its elements
are, so `write_table` hands the rows to one `rmp_serde::Serializer` a row
at a time and renames the file over, which is byte for byte what
`write_meta` wrote from a whole `Vec`. `sidecars::tests::a_sorted_table_is_
what_a_map_of_every_row_would_have_written` is the oracle: rows pushed in
no order with duplicates either side of every run boundary, a one-byte run
size, and the bytes checked against the held writer's.

`Rows::onto`, which seeds a resumed read from what the directory
publishes, walks each table through a serde seed rather than decoding it
into a `Vec` — the published reaches alone are tens of millions of rows at
200 M, and a run that only means to walk one once was holding all of it.
So nothing on the cold road holds a galaxy now: what item 6 has left to
collapse is the *writer*, not the tables.

### 2. The 200 M import has not been run to the end

Every number above is the seven-day slice. The full file is 610 GB. Two
things to watch when it runs: the bucket split, which has never met the
galactic core's density, and `ColdReport::over_budget`, which is how a
region that cannot divide reports itself.

`GALOS_REGION_BUDGET` is the dial. At the 7 M default a region is about
2 GB; the measurements above are what 200 k and 50 k cost.

Worth doing in the same sitting: import the same file into a database and
diff the two indexes. That comparison is what caught every bug in the list
above, and it is cheap next to the build.

**What the first attempts met**, and it is all the body files (item 2a):

| read | systems | rate | where |
|---|---|---|---|
| 26 min | 7,585,860 | 6,465/s average, **1,740/s** by the end | 3.16 M body files, 14 GB |
| 8 s | 114,333 | ~14,000/s into an empty directory | — |

At 1,740 systems a second and still falling, the remaining 196 M systems
are **31 hours**, and 188 M body files at 4.4 KB allocated apiece is
**~830 GB** against 943 GB free. Neither number is the region build's: the
tree, the names and the spills are 30 GB of the total and the read is 30,000
systems a second when nothing is writing a file a system.

So **item 2a comes first**. It changes how every body is written, and a
read started before it is a read done twice.

### 2a. One file a system — next, and it blocks the 200 M run

A body file is a file: `bodies/{shard:03x}/{address}.bin`, 2.4 KB of
MessagePack in 4.4 KB of allocated disk, written whole and read whole. At
200 M systems that is **188 M files and ~830 GB**, and it is what makes the
import slow down as it runs rather than run at a rate.

**Measured.** `sample` over the live import: **91 %** of the wall clock in
body files — 1,300 samples of 4,053 in `open`, 548 in `rename`, 479 in
`write`, against 152 in `serde_json`. Three opens and a rename a system,
two of the opens for names no directory holds.

`4a470fb` took those two away. `Published::raising` is the store a build
raising a directory from nothing uses: a file it is not holding is one it
has not written, so nothing is read back, and nothing underneath needs
keeping, so `raise_meta` puts the file straight on its path rather than
beside it and over. Over 50,000 systems, 26,992 of them scanned: **7.13 s →
4.65 s**, kernel time 3.20 s → 1.48 s, and the two directories byte-equal
in all 27,045 files.

What is left is the file count, and no write path fixes that. The shape:

```text
bodies/{shard:03x}.idx            header, a sorted base, an unsorted tail
bodies/{shard:03x}.{gen:04x}.dat  the records, appended
```

- **A write is two appends**, neither a directory operation: the record
  (`[u32 len][MessagePack]`) onto the data file, and the entry (`[i64
  address][u64 offset][u32 len]`, 20 B) onto the index. A length of zero is
  a tombstone, which is what a withdrawal is, and it has to beat the base
  behind it rather than be an absence.
- **A read** binary-searches the base of the mapped index and scans the
  tail newest-first, then reads the record at the offset. Nothing is
  resident.
- **A fold** merges the tail into the base and writes the whole index
  beside the old one and renames it over, so a reader sees one file or the
  other and never half of each. Bound the tail at `(base / 8).clamp(8 Ki,
  52 Ki)` entries: linear in the writes, and a megabyte of scanning at
  worst. Over a 200 M import that is ~10 GB of index rewriting, 0.04 % of
  the run.
- **A compaction** is the same fold with the data file rewritten, when more
  than half of it is records nothing points at — a feed's 30 systems a
  second leave ~6 GB of dead records a day over the galaxy, which reaches
  half a shard in a couple of months. It writes the **next generation's**
  file rather than rewriting in place, because a reader holding offsets
  into the old bytes must not be handed new ones; the index naming the new
  generation is renamed over in the same step, and a reader that finds its
  data file gone reads the index again. That retry is the whole of the
  concurrency, there being one writer (the directory's `Lock`) and any
  number of readers.
- **Writes buffer per shard**, `BUFFERED` bytes each, flushed with one
  handle open at a time — the arrangement `bucket::Buckets` already uses.
  At 16 KiB over 4,096 shards that is 64 MiB held and one open per seven
  bodies, against one open, one rename and an inode apiece today.

At 200 M: 4,096 files rather than 188 M, **~450 GB rather than ~830 GB**
(the difference is the block a small file rounds up to), and the import's
writes become sequential.

**The seam is already right.** `galos_map` never builds a body path — it
asks `Source::bodies(address)` — so the change lands in `FsSource`,
`source::{read_bodies, remove_bodies}`, `bodies::Published`,
`Published::scanned`, and the two `write_meta(&bodies_path(..))` calls in
`galos_db::index::metadata`. A `pack(dir, stop)` migration walks the loose
files into the shards the way `reshard_bodies` walked the flat ones, and
`read` falls back to the loose and flat paths until it has, so a directory
part way through answers from either.

**The follow does not care which layout it is**, which is why this is an
import decision. EDDN is ~30 systems a second: 30 point reads and 30 point
writes, free either way. What it does care about is the 830 GB and the
hours any whole-tree sweep over 188 M files costs — `Published::scanned`,
a backup, an `rsync`.

One piece was written and pulled back out rather than left half done:
`galos_index/src/pack.rs`. Start it again from this.

### 2b. A stopped import publishes what it read, and is carried on

Found by running it, twice. A 610 GB read is not something anybody gets
through without stopping once, and a stop used to remove the spills, the
staged names and everything else: 26 minutes and 7,585,860 systems, gone,
with 4.2 M body files left behind in a directory the run reported as "as it
was found". Keeping the spills fixed the loss and not the point — the
directory still held no index, and a galaxy nobody can open is a galaxy
nobody can look at.

So a read cut short **publishes what it read**. A dump is read in file
order, so what has been read is a galaxy in itself: `Ending::Publish` forms
the regions over it, raises the tree, and writes the payloads, the names
table, the index file, the resume point and the mark. The map opens it. The
next run carries on.

Carrying on is the resume point read backwards. `Start::Resuming` takes
every system back out of the base the last publish wrote and pushes it into
the buckets — 56 B a system, eleven gigabytes over the galaxy, against
re-reading the 610 GB those systems came from — and the names table off its
own chunks, `Chunks::onto` filling the part-filled last one the rest of the
way rather than copying the ones behind it. The three tables come back the
same way, `Rows::onto` seeding the rows from what the directory publishes,
because they are written whole and a second publish holding only the second
read's rows would take the politics off every system the first one read.

The mark is `<checkpoint>.mark`, written by the publish and after the index
file, so what it says and what the directory holds cannot come apart.
`Build::mark(cursor)` only hands over the caller's bytes; the dump's are a
`Place` — the file, its length, the byte and line reached, and the clock
the run dated its Recency by, because a build ages every system against one
moment and a run carrying on with its own would bin half the galaxy against
another. A mark taken against a different dump, or one that has changed
length, is refused and the read starts over saying so.

Measured over 50,000 systems of the seven-day slice, stopped twice — at
11,675 and at 37,757 — and carried on to the end: all **27,045 files
byte-identical** to a build that was never stopped, every body file, every
cell payload, the names chunk and all three tables, and `index.bin`
identical in every integer column. Each stop left an index `galos-index
info` reads and the map opens.
`cold::tests::a_resumed_build_is_the_build_that_was_never_stopped` is the
same claim over a lumpy galaxy at a 6,000-system budget.

Neither of the two passes a publish makes asks the stop flag any more. A
read cut short is being published, and a run unwilling to wait for the
raise has the second Ctrl-C, which leaves the directory where it stands;
stopping in the middle of the raise would leave payloads with no index over
them, which is the one state the build is careful never to publish. The
database's derivation ends `Ending::Abandon` instead: its directory already
stands for every row Postgres has, and a read cut short must not replace it
with the prefix it reached.

### 2c. Positions as `f32`, after the 200 M run

A payload record is 41 B and **24 of them are the position**, three `f64`.
Elite's coordinates are on a 1/32 ly grid, and a 1/32 grid over the
galaxy's extent needs 2,088,632 steps at the far end (Beagle Point,
65,269.75 ly) against `f32`'s 16,777,216 exactly-representable integers —
so an `f32` position is exact for every coordinate the game produces.

Measured over 730,544 real positions: **65 do not round-trip through
`f32`**, and each is an upstream truncation rather than finer precision —
`42140.78` beside `-124.71875`, a 1/32 value printed to seven significant
digits and short of its tail. Storing those as `f32` moves them by
≤0.002 ly, against a grid step of 0.031 ly.

Worth 12 B a system: the magnitude is `f32` as of format version 2, so the
record goes from 41 B to 29 B, **5.8 GB rather than 8.2 GB at 200 M**, and a
zoom reads a quarter less. Measured at 2,885,249 systems: payloads are
118,295,209 B, exactly 41 B a system. `CellId::of_point` and the distance
arithmetic take `[f64; 3]` and would widen on read, so the arithmetic does
not change, only the storage.

The precedent for the migration is format 2 itself: `INDEX_VERSION` was
bumped and an older directory is **refused with a message naming both
versions**, because a payload block carries no magic, no version and no
count, so a 39 B file and a 41 B file cannot be told apart by inspection and
an interrupted re-encode would leave a directory nothing could read. A
rebuild is the established fallback — `bring_level` already does one when a
resume fails.

Held until the 200 M import and a follow have been run: it is a
served-format migration, and the evidence for it should come from the
galaxy rather than from a seven-day slice. A test should assert the
≤0.002 ly bound on the truncated cases rather than leaving it implicit.

### 3. Flags — done

- `--shard`'s help said "every Nth record" where a journal shards by
  *file* (`bin/sync/journal.rs:91-98`). The code was right and the help is
  now: it names both shares and says why a journal's is a file, which is
  that a file is what names the commander who flew it.
- `--watch` with only `eddn` is accepted and read by nothing: EDDN follows
  either way. It is the one flag/run pair where a value silently does
  nothing, documented as a lenience at `main.rs:168-170` rather than
  refused — and it stays that way, a refusal being worse for a run that
  names several sources and watches what can be watched.

### 4. The dumps that cannot reach the build

`--from edsm=DUMP --index DIR` and `--from eddb=DUMP --index DIR` are finite
but take the sink, so they pay a live `Tree`. EDDB is mechanical — a
row-at-a-time CSV reader already. EDSM buys less than it looks: `edsm.rs:44`
parses the whole file into a `Vec` before anything walks it, and every
system is stamped `Utc::now()`, so all of them land in one Recency bucket.

### 5. Following a feed over 200 M

`TODO-scale.md` item 3, and the last wall. **The tree is not the whole of
it**, which a count of what a follow run holds says:

| held by a follow | at 200 M | what ends it |
|---|---|---|
| `Tree` — `records`, `owner`, `leaf`, the slices | **208 GB** | the paged tree, below |
| `NameTable`, 235 B an entry measured | **47 GB** | names keyed by cell |
| `populated` + `reaches` + `boosts` | **3.3 GB** | the sidecars keyed by cell |
| the checkpoint base rewritten per fold | 11 GB of I/O | resuming from the directory |

So item 5 alone does not get a follow under budget, and the three below it
are one change: the cell is the unit of storage, of transport and of edit.
`Source::names()`/`reaches()`/`boosts()` can go on answering whole tables
by concatenation, so `galos_map` need not move at the same time.

**The paged tree.** The decision an insert makes needs **one scalar a
cell** — the cell's faintest owned magnitude, a column on `index.bin`,
which by the crate's own rule (`serialization.rs:280-296`) needs no
`INDEX_VERSION` bump, the exact-length check catching a stale index. After
it a payload is read only where a system actually displaces something.
`Cell::slice_len` and `is_leaf` already answer "is this cell full", so the
faintest owner is the only thing missing; carry its `id64` beside the
magnitude and a tie needs no payload read either. Resident becomes
`index.bin` (198 B a cell, measured; ≈73 MB at 200 M) plus an LRU of
payloads, and the publish beat amortises the paging by settling a minute's
arrivals together.

Two things fall out of the tree's own arithmetic and are worth writing
down before the work starts. A cell's aggregates depend on **physical**
membership and not on ownership, so an ownership move dirties payloads and
`rank_lo` alone and no aggregate at all. And a leaf's physical members are
its own payload plus whatever its ancestors claimed from inside it — at
most `level × internal_slice` systems, which is the same bound
`region::Offer` is built on — so a leaf can be re-summed from twelve payload
reads and no per-system map.

Prerequisite, mostly done: the payload has to be the record. The magnitude
is `f32` as of format 2, the temperature's bucket is exact for every
consumer, `age_bucket` is recomputable from `updated_at`, and the position
is already exact — so what remains is deleting the checkpoint's base
(56 B × 200 M = 11 GB) once the directory can be resumed from.

### 6. One sink

The source API is one, the builder is one type, and the publish trigger is
one idea — `held >= N || elapsed >= T`, where a bulk import sets `N` and a
feed sets `T` with `N` as a burst fallback. What is still two is the
*writer*: the event sink with its live `Tree`, and `Build`.

`publish_whole` cannot reduce to push-and-finish as they stand, and neither
blocker is incidental: `Build::finish` consumes `self` and raises no `Tree`,
while the sink is called from `Sink::finish` with a live tree its caller may
still be using, and it writes all four sidecars. The second blocker is half
down — the cold route writes the three a record can fill through the same
`Tables` the sink uses, and the fourth is a table no record can fill on
either road. The first is removed by the single-pass spill moving *behind*
`Sink::entry`: the sink accumulates, the `N` bound spills to a bucket, and
`finish` raises the regions — at which point there is one writer, and a
bulk import is a feed that ends.

Until then the split is not naming debt. See the last section.

## What did not need doing

The compatibility paths are all reachable, documented, and still needed by
a directory or a checkpoint an older build wrote:

- `Checkpoint::legacy` (`galos_index/src/checkpoint.rs:275`, dispatched from
  `:175`) reads a pre-provenance checkpoint and upgrades it in place,
  re-framing the pending log with it.
- `reshard_bodies` / `reshard_cells` move a flat directory into its shards,
  one rename at a time, idempotently, preferring a file already in the shard
  over a stale loose one. Both take a stop predicate and answer
  `Resharded { moved, finished }`; `galos_index::migrate` runs the pair.
- `read_payload` / `read_bodies` fall back to the flat path. This is not the
  same thing as the reshard: the reshard is the writer, the fallback is
  `galos_map` reading a directory mid-migration.

And two splits that look like duplication and are not: `build_cells` versus
the watch's delta pass is a build versus an edit, and `galos_index::Galaxy`
versus `galos_db::record` are one fan-out — both call `SystemReport::of` —
landing in two stores that hold different things.

An audit of every source × destination found no second import path: every
event reaches Postgres through `galos_db::record`, every directory write
goes through the one `Index` sink or `galos_index::Build`, and the three
restatements it did find — the region budget, the reshard pair, the Spansh
read loop — are now one each.

## Why there is still a cold path

Because a build from nothing and an edit of an existing directory are
different operations, and the difference is the ownership rule, not the
input.

A cell owns the brightest systems of its subtree that its ancestors have not
claimed. Inserting one system means asking "am I brighter than this cell's
faintest?", and only the cell can answer, so an incremental edit needs the
cells on that system's path resident. Measured, the editable form is 13x the
published form: 1.04 KB a system, which is 208 GB at 200 M. Paging it
instead costs about 360 KB read and 64 KB written per edit — 72 PB of I/O
for 200 M systems.

The region build escapes both by batching: a region is spatially disjoint,
so it can be raised alone, written once, and dropped. That is why the
budget bounds memory where a publish does not — `Tree::publish` writes the
dirty cells and clears the dirty set, and never drops a system.

So "cold" names the build that raises a directory from nothing with no live
tree. What is arbitrary is only the *word*: the module is `galos_index::cold`
while the type in it is `Build`, and its report is `ColdReport`. When item 6
lands, a bulk import becomes a feed that ends, one writer serves both, and
the concept goes with the split — at which point the module should be named
for the arithmetic it holds, beside `region`, rather than for the half of a
distinction that no longer exists.
