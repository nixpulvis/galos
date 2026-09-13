# The region build, and what is left of it

`TODO-scale.md` named four walls between 2.7 M systems and 200 M. Three are
down and measured. This is what was built on the `scale-regions` branch, the
numbers it was measured with, and the work still open — in the order it is
worth doing.

Everything here was measured on one machine (M5 Pro, 18 cores) against
Spansh's seven-day slice: `galaxy_7days.json`, 19,947,189,475 bytes,
**730,544 systems and 3,119,799 bodies**. The full `galaxy.json` is 610 GB
and about 200 M systems, and **has not been run**.

## State of the tree, 2026-09-13

Branch `scale-regions`, nothing committed. 58 files modified, 1,176 deleted
(`galos_journal` dissolved, and `src/bin/` moved to `bin/`), `spansh/` is a
new untracked crate, and the `elite_journal` submodule is modified — it
gained `body::{StarClass, StarSize}` and the `journal` module.

**Index directories on disk.** `test2/` is the current dump-built index:
730,544 systems, 907 cells, payload format **version 2**, with
`populated.bin`, `reaches.bin` and `boosts.bin`. `7day/` is a second build
of the same shape. **`.galos_index` is format version 1 and is refused** —
`index format version 1, this build reads 2: the payload record changed
width, so rebuild the directory`. That is the intended behaviour, not a
fault; see item 2a's note on the version bump.

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
# the suite: 1,505 tests, needs no database created by hand
TEST_DATABASE_URL=postgresql://localhost/postgres cargo test --workspace

# an index from a dump, no Postgres in the path
cargo build --release --bin galos-sync
./target/release/galos-sync --from spansh=~/Downloads/galaxy_7days.json \
    --index DIR                      # GALOS_REGION_BUDGET is the memory dial

# the same, into Postgres, eight readers over one file
./target/release/galos-sync --from spansh=FILE --db --bulk --shard 0/8

# the client guard, against a built directory
GALOS_PERF_DIR=$PWD/test2 \
  cargo test --release -p galos_map --lib perf -- --nocapture

# what a directory holds
./target/release/galos-index info DIR
```

`~/Downloads` became unreadable from a tool shell partway through the
session (macOS TCC), so anything reading `galaxy_7days.json` or
`galaxy.json` has to be run by hand.

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
26 s over 240,000 systems.

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

### 1. The sidecars the cold route holds

`Build::finish` writes the names chunks, the cell payloads, the index file
and the checkpoint. `bin/sync/main.rs::cold` writes `populated.bin`,
`reaches.bin` and `boosts.bin` beside it, out of a `galos::sink::Tables`
held across the read: patched per line from the same one-system
`galos_index::Galaxy` the tree's system comes from, before the bodies are
settled so the reach and the boost are read off what is still held rather
than off the file just written. `factions.bin` stays unwritten — nothing
reading records can number a faction, and an empty table would say the
galaxy has none where an absent one says this index cannot tell.

What is left is the holding. Each is one row a system, reaches one per
*scanned* system, and the rows accumulate for the length of the read:
**measured 22.4 MiB over the seven-day slice** — 52,908 populated, 277,550
reaches and 34,472 boosts, 208,224,256 B peak against 184,696,832 B with
the patch taken out, at 64.9 s against 67.5 s — which is some 6 GiB at
200 M systems on the same proportions. So each wants what the names table
already gets: spilled as it arrives, written in address order at the end.
Doing this is also what collapses the last two writers into one (see item
6).

### 2. The 200 M import has not been run

Every number above is the seven-day slice. The full file is 610 GB. Two
things to watch when it runs: the bucket split, which has never met the
galactic core's density, and `ColdReport::over_budget`, which is how a
region that cannot divide reports itself.

`GALOS_REGION_BUDGET` is the dial. At the 7 M default a region is about
2 GB; the measurements above are what 200 k and 50 k cost.

Worth doing in the same sitting: import the same file into a database and
diff the two indexes. That comparison is what caught every bug in the list
above, and it is cheap next to the build.

### 2a. Positions as `f32`, after the 200 M run

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

### 3. Flags

- `--shard`'s doc says "every Nth record"; a journal shards by *file*
  (`bin/sync/journal.rs:100-107`). The code is right.
- `--watch` with only `eddn` is accepted and read by nothing: EDDN follows
  either way. It is the one flag/run pair where a value silently does
  nothing, documented as a lenience at `main.rs:168-170` rather than
  refused.

### 4. The dumps that cannot reach the build

`--from edsm=DUMP --index DIR` and `--from eddb=DUMP --index DIR` are finite
but take the sink, so they pay a live `Tree`. EDDB is mechanical — a
row-at-a-time CSV reader already. EDSM buys less than it looks: `edsm.rs:44`
parses the whole file into a `Vec` before anything walks it, and every
system is stamped `Utc::now()`, so all of them land in one Recency bucket.

### 5. Following a feed over 200 M

`TODO-scale.md` item 3, and the last wall. A follower still raises the
editable tree: 1.04 KB a system, 208 GB at 200 M. The plan, with the
arithmetic, is in that file; the short form is that the decision an insert
makes needs **one scalar a cell** — the cell's faintest owned magnitude, a
2 B column on `index.bin`, ~740 KB at 370 k cells — after which a payload is
read only where a system actually displaces something. Resident becomes
`index.bin` (198 B a cell, measured; ≈73 MB at 200 M) plus an LRU of
payloads, and the publish beat amortises the paging by settling a minute's
arrivals together.

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
