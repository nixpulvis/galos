# Thorns left in the source and sink rework

What is known to be wrong or unfinished after `galos-sync` became one
process with `--from`, `--db` and `--index`. Ranked by what bites first.
Each names where it lives, so none of it has to be found again.

Two of the worst — a published row withdrawn for want of hearing about a
system, and an event's thin row written over a build's rich one — were found
from a map and are fixed (`5074955`, `e7304a5`). Item 4 was a third of the
same kind, found by reading the two derivations side by side; item 7 is now
what keeps a fourth from being found the same way, the fan-out and both merge
rules being one copy each with the SQL pinned to them by a test.

What grows rather than what is wrong — the four walls between 2.7 M systems
and 200 M — is `TODO-scale.md`, which shares two findings with this file:
item 1's manifest and item 5's note on why it cannot hash.

## Live data hazards

### 1. The resume gate measures the wrong thing

`Index::by` (`src/sink/index.rs`) answers `By::Database` whenever a database
is present, so the *event sink* stamps `Database` on a checkpoint whose tree
it has been maintaining from events. `galos_db::index::resume`
(`index/mod.rs:759`) refuses to resume unless that flag says `Database`, and
the reason is stated above it: an event-derived directory stands for what a
feed reported since it was opened, not for the galaxy, and adopting one as
the tree to follow from leaves a catch-up patching deltas into an index that
was never the dataset — while passing both counting gates, because such a
directory really is internally consistent about the little it holds.

**Less severe than this entry first said.** In `--from eddn --db --index`,
`derive.rs` runs `catch_up` *first*, so the directory is built from Postgres
before the event sink opens it: the tree is the galaxy and the label is
substantively true. The live file confirms it — decoded, `.galos_index.checkpoint`
is `by: Database`, cursor `2026-09-11T14:15:13`, **2,730,800** inputs,
against 2,730,844 positioned rows. Forty-four behind, which is the lag its
cursor covers. Resuming from it is correct.

What is left is narrower:

- **The pre-provenance form.** `Checkpoint::legacy`
  (`galos_index/src/checkpoint.rs:275`, dispatched from `:175`) decodes a
  two-field file as `By::Database`, and upgrades it in place. If one was
  written by an event run, `resume` adopts a
  feed-sized tree as the galaxy and both counting gates pass. That is the
  silent wrong-sized resume. The file cannot say what wrote it, so no number
  of new `By` variants reaches this case.
- **The flag answers "who wrote it" where the gate wants "is this the
  dataset".** Those coincide today only because `derive.rs` happens to catch
  up first — a call order, not an invariant.

**Better than a third state.** Gate on what the gate cares about: record the
count. `resume` already reads the served count; compare the checkpoint's
`inputs.len()` against `SELECT count(*) FROM systems WHERE position IS NOT
NULL` and refuse when it is materially short. That answers the old-form file
too. Keep `By` for `one_hand`, where provenance genuinely *is* the question:
the event sink refusing to resume onto a database build.

And it is one of four. `agrees` (`sink/index.rs:141`) trims, `one_hand`
(`:178`) refuses, `resume` rebuilds, `forget_names` (`sink/tables.rs`) drops
— four readers, each re-deriving which parts of a directory may disagree and
by how much, each using row counts as a proxy for two different questions.
The structural fix is **one manifest**, written last and renamed into place,
carrying the format version, a generation, the provenance, the cursor and the
row count of every part. Then each of the four becomes a comparison against
a recorded fact. It is O(parts), so it costs nothing as the galaxy grows.

Not hashes, at least not yet: see the note under item 5.

- **Way out today:** delete the checkpoint, not the directory, before the
  first catch-up over a directory whose history is unknown. It full-builds.

### 2. A hard kill leaves a lock nothing can clear from the command line

`Lock::force` exists (`galos_index/src/lock.rs`) and `main.rs` only ever
calls `Lock::take`. After `kill -9` the next run says the directory is held
by a pid that is gone, and there is no flag to say otherwise; the only move
is `rm <dir>.lock`.

Wants either a `--force` flag or a liveness check on the pid in the file.

### 3. A dropped message cannot be reported, and the feed's pool is five

`galos_db/src/lib.rs:40` and `:46` — `max_connections(5)`, sqlx's default
thirty-second acquire timeout, no explicit setting. An import is not held to
that five: `Database::bulk` takes the ceiling as an argument and `--bulk`
passes sixteen (`bin/sync/main.rs:843`), so five is what a feed run gets.
Collect and derive open separate pools now, so exhaustion is unlikely, but
`Sink`'s per-message methods return `()` on purpose (`src/sink/mod.rs`,
header). So a stalled acquire is a burst of warnings, permanently lost EDDN
messages, and an exit status of SUCCESS.

The acquire timeout is only reachable inside `galos_db`, which is why the
rework left it alone. Needs a decision about what a refused write should
do to the run's answer, not just a larger number.

## Divergences between the two derivations

### 4. The database placed systems the events did not — fixed

It was six events and not four. `record.rs` writes a `systems` row for
`CodexEntry`, `SAASignalsFound`, `FssSignalDiscovered`, `ApproachSettlement`
and `Docked` through `ensure_system`, and for `FssBodySignals` through
`record_body_signals`, which is the one a count of the direct calls misses.
`Galaxy::read` answered `_ => false` on all six, so `--from … --db --index`
had one feed writing two sinks that disagreed about which systems exist —
and `--index` with no `--db` was simply short of them, which is what makes
this the first thing to fix before a directory is published off the feed
alone: the honk that finds a codex entry or a signal is often the first
thing anybody sends about a place.

Taught the journal rather than stopped the positioning.
`SystemReport::of` (`galos_index/src/report.rs`) is `ensure_system`'s rule
in the index's vocabulary: the name, the place where the event carried one,
the moment, and the same guard — a report naming only an address publishes
nothing, there being no name to record. Nothing below system level
and nothing political. The thing the event is actually about has no column
in the index; a body named by a surface scan is not made a body, a record
with no orbit and no class being what the reach and the arrival star are
derived from; and a settlement's or a docking's government and allegiance
are the station's, so taking them would colour the sky by where the
commander parked.

`Docked` moves the clock and nothing else: it carries no position, so a
system nothing has placed stays out of both derivations, `position IS NOT
NULL` being what every system query in `galos_db::index` reads. Five tests
in `galaxy.rs` pin the six arms. A journal holding one of each published
four systems and no bodies where the same journal published nothing before.

### 5. Two answers that depend on the order they were computed in

**`Inside::primary` has no tie-break.** `galos_index/src/inside.rs:45-52` is
a plain `min_by` on `distance_from_arrival_ls`, so it is first-wins by vector
order. The database side builds that vector in query order and the event side
in scan order, so two stars at equal arrival distance give different
`reaches.bin` depending on which derivation last touched the system.
`derive::arrival_class` breaks the same tie by body id. This one never got the
same treatment. Small, twenty minutes, and a real divergence.

**The cell aggregates are not bit-deterministic, and cannot cheaply be
made so.** Measured: the same galaxy through both derivations publishes
byte-identical cell *payloads* and six differing bytes in `index.bin`, each
one off in the low mantissa byte. `Aggregate` carries `flux: [f64;
TEMP_BUCKETS]` and two `Moments`, and `Moments` is a running mean —
`weight`, `mean`, `m2` — combined pairwise on merge and *subtracted* on
`remove`. `Tree::build` rolls up in a batch; `Tree::upsert` maintains
incrementally with a `remove` before each re-add. Batch and incremental are
different addition orders by construction, so a full build and a catch-up of
one galaxy differ in the last bits whatever order the inputs arrive in.
`Aggregate`'s own conservation test already knows: it asserts `close(…)`,
never `==`.

Sorting the tree's inputs does *not* fix this — it was proposed and
withdrawn, since it addresses only the batch side. Bit-exactness needs
order-independent arithmetic: fixed-point flux and plain weighted sums
instead of Welford, which is a format change reaching into the client's
rendering maths and spends precision exactly where `remove` already spends
it. **Not worth it.** What follows from it:

- Any consistency scheme for the directory must be counts and generations,
  not content hashes — see item 1's manifest.
- `tests/derivations_agree.rs` compares everything but the aggregates, and
  says so in its header. If they are ever compared it must be with a
  tolerance.

### 6. Faction ids differ by design; `updated_by` no longer differs by accident

The event path publishes no faction ids (`galos_index/src/galaxy.rs`,
header): a journal names factions and numbers nothing, and the ids are
`galos_db`'s. That is argued and is not going away. `Tables::over`
(`sink/tables.rs`) is what keeps it from being a bug — an event's row merges
over the published one, so a faction list nothing can derive is not emptied
by something that cannot derive it.

`updated_by` was the other half and was worse than "by design". One index
worker takes every source, the accumulator tracked the commander from
`Commander` events, and EDDN carries none — so `--from eddn --from
journal=DIR` filed everybody else's scans under whoever was flying locally.
`Sink::entry` now takes a `Reporter` (`sink/mod.rs`) instead of a `&str`:
`Commander(name)` from a journal, `Uploader(id)` from EDDN and from a
published file, and `Nobody` where nothing named anybody. Both sinks take
`named()`, `updated_by` being provenance either way, and `unknown` is left
for what nothing named at all; the name crosses the index worker's channel
as `relay::Named`, which is where it used to be thrown away. Pinned by
`a_scan_is_filed_under_whoever_the_source_named` (`sink/index.rs`), which
reproduces the old answer when the line is removed.

So a directory both derivations have touched says `updated_by` the one way,
given the same source: `bin/sync/spansh.rs` files a dump's bodies under
`Spansh <file name>` whichever way into an index it is read.

### 7. What stops the two derivations drifting

Closed. The fan-out, both merge rules and the sidecar tables are one copy
each, in `galos_index`:

- `report::SystemReport` — a system as anything reported it, replacing
  `sink::Row` and `galos_journal`'s `Visit`, which were the same columns
  spelled two ways. `SystemReport::of` is the *only* fan-out over the
  fifteen events that name a system; `record.rs` no longer reads one off an
  event at all, and its match is down to the fourteen `galos_db` tables the
  index has no column for. A new EDDN schema is one arm now.
- `SystemReport::over` — the system merge rule, and `galos_index::merge` —
  the star, body, surface and barycentre rules, moved out of
  `galos_journal::galaxy`.
- `sidecars::Sidecars` — the five metadata tables, their read-back and their
  write-what-moved, replacing `sink::Tables`' hand-copy of
  `galos_db::index::metadata::Metadata`. What stayed apart is what genuinely
  differs: where a row comes from, and that only the database side can
  *withdraw* one. It re-reads `population > 0` from the row, so it can tell a
  system that has emptied from one nothing has mentioned; a feed cannot, so
  the setters and the takers are separate calls.
- `meta::Parent` — `galos_db::bodies::Parent` was field-identical to it and
  carried the same `chain` and `is_barycenter` written twice, one copy of
  which named the other as its source. One type now, with the two array
  columns that really are Postgres's left behind as `bodies::ancestry` and
  `bodies::columns`.
- `galos_db` keeps its `ON CONFLICT DO UPDATE` copies, because Postgres
  merges against a row Postgres holds and reading it back would be a round
  trip and a lost update per message. They are *pinned* now, by three tests
  in `galos_db/tests/write_path.rs` — for a system, a star and a body — each
  writing pairs through the upsert, merging the same pairs in Rust, and
  requiring the same answer. The row comes back through `RETURNING *` and the
  `From` impls `galos_db::index::metadata` already used to publish it.

They earned themselves immediately. Between them they caught the stamp rule
being applied to `body_count`, which the SQL deliberately does not do — a
count cannot go stale and a stamp guard throws nearly every honk away — and a
`position` clause in `set_body_counts` that weighed the stamps differently
from `create`'s. Both are fixed and both were invisible to reading. Checked
by mutation: breaking the mapped rule, the surface fallback or the count rule
fails them.

And the whole-directory test exists: `tests/derivations_agree.rs` hands one
list of events to each sink and compares everything either directory
publishes about the systems it owns — names and places, populated columns,
reaches, supercharges, and the whole of every system's bodies field for
field, `updated_by` and `discovered_at` included. What it does not compare,
each for a reason stated in its header: the other systems in the test
database (a build reads every row, so every table is filtered to the five
addresses the run owns), row order (`galos_index::names` says the names
table has none), the cell payloads (their photometry is `derive::lit` over
exactly the stars compared, and that has tests of its own), and faction ids.

It needed `sink/` and `journal/` out of the binary, which is the one thing
phase 0 of the plan was for. They are in the library now — `src/sink/`,
`src/journal/`, `src/bar.rs` and `src/shutdown.rs` — and the binary keeps
the command line, the four sources' plumbing and the supervisor. Nothing in
the moved files changed: they said `crate::` before and the lib is `crate`
for them now.

Checked by mutation, both ways round: dropping the reach line from
`sink/tables.rs` fails it on the reaches table, and making `record.rs` skip
the system write for a `CodexEntry` — which is item 4's original bug, exactly
— fails it with "the two derivations disagree about which systems exist".

Byte-identical as well, checked by hand over a real journal through both
paths: `populated.bin`, `reaches.bin`, `boosts.bin`, `factions.bin`, the cell
payloads and the body files. `names/*.bin` holds the same entries in a
different order and `index.bin` differs in its per-part stamps.

Three drifts have been found, none of them by anything that existed at the
time: a published row withdrawn because the accumulator had merely heard the
system named, and an event's row written whole over a build's, both from a
running map; and four events writing a positioned row on one side and nothing
at all on the other, from reading `record.rs` beside `Galaxy::read`. All three
would fail the test above.

## Rough edges

### 8. Ctrl-C during the initial full build waits for the metadata tables

The build itself is interruptible. `Stop` is asked per record while the
galaxy is read and per region while it is raised, and a build cut short
publishes nothing and answers `Reached::Stopped` — no index file, no names
table, no resume point (`galos_index::Build`, `galos_db::index`). What a
stop still waits for is `metadata::write_parts` behind it, which is written
whatever the run has been asked: the resume point the build just wrote is
what makes the next run resume rather than build, so a directory left with
published cells and tables short of them is one nothing would repair. A
delta pass is still bounded by the chunk read in flight.

Measured on `galos_7days`, 730,544 systems: a Ctrl-C two seconds in exits
in 0.03 s with an empty directory, where the whole run is 41 s. One sent
after the cells are built waits out the 38 s the tables take.

### 9. `--only boosts` now reads every body

Deriving the arrival star from `SystemBodies` put boosts inside
`Parts::wants_bodies` (`galos_db/src/index/mod.rs`), so the boost repair pays
what `--only bodies` pays where the old SQL was one pass over `stars`. That
was the price of deleting the rule's second copy.

### 10. A cell's Recency histogram freezes at read time

Numbered last so nothing above renumbers, but it belongs with the live data
hazards: it is a published number that goes quietly wrong, on both
derivations.

`age_bucket` is computed against `now` when a system is read — by
`derive::updated`, out of `input_from_row` on one side and `Galaxy` on the
other — and then kept in the tree's `Record`. A system nobody reports again
keeps the bucket it was built with, so over a long watch a cell's aggregate
still counts it as fresh while the `updated_at` on the payload beside it
stays exact. The far view and the near view of the same Recency filter then
disagree about a system, which is the one thing `System`'s own doc says
cannot happen: "the caller bins one from the other off one reading, so the
far view and the near view of the same filter cannot disagree about a
system". They do, a day later.

The fix is *not* to bin at publish, which is what this said first. The
roll-up is maintained incrementally now (`TODO-scale.md` wall 5), and an
age measured against the clock changes for every cell whenever the clock
moves, whether or not anything in that cell did — so re-binning at publish
would put the galaxy back into every publish to fix a stale column.

Bin by **absolute date** instead. A column counting systems per calendar
bucket does not move when the clock does, so it is maintained like
everything else, and the client knows what time it is and maps its Recency
span onto the buckets. That is a change to what the aggregate means to a
reader, so it goes with the served-format changes in `TODO-scale.md`
wall 4c, and it is a prerequisite for the served payload becoming the
durable record.

## Minor

- `galos-db` still prints sqlx's `~/.pgpass` warning; only `galos-sync`
  filters it (`HEARD`, `bin/sync/main.rs:288`).
- The layered source went with `galos_index/src/layer.rs`, and its two
  recorded caveats with it: a deep cell's glow counted a level up where one
  side refined a region and the other did not, and an unanswered `Claimed`
  double-counting the overlap.
- Root `Cargo.toml` still pins most dependencies at `"*"`. `tui`, `termion`
  and an optional `galos_map` that gated nothing were dead and are gone.
- A 6.1 MB `test/` index directory, 1,162 files, is in three of this
  branch's commits — an agent's scratch run swept in by a wide `git add`.
  Nothing is pushed, so a `filter-branch` over `85c2274..HEAD` still strips
  it cleanly.

## Working notes

Things that are not defects but cost an afternoon each to work out.

- **A catch-up is a sync, not a repair.** `--db --index` over a directory
  with a resume point re-derives only what the database changed since the
  cursor, so damage *in the directory* is invisible to it: the rows it would
  fix are rows the database has not touched. A whole-table rebuild is
  `--only PART`, which ignores cursors and writes the file entire — how
  `populated.bin` was repaired after the two bugs above.
- **`.env` names the database, and it is not `galos_development`.** It is
  `galos_postimport_backup`: 2.73 M systems, 66 k populated, migrations
  through `20260816000050`. The two migrations this branch adds
  (`20260911000000`, `…010`) are not applied there yet; nothing breaks
  without them, the arrival-star reads are just unindexed.
- **The tests make their own database.** Everything in
  `galos_db/tests/write_path.rs` reads `TEST_DATABASE_URL` and never
  `DATABASE_URL`, and what it names is a server rather than a database:
  `galos_db::testing` migrates a template on it once and hands each test a
  copy, so `TEST_DATABASE_URL=postgresql://localhost/postgres cargo test -p
  galos_db` is the whole of it. Without a server to reach they stand down
  silently and CI passes on the cached query metadata, which means the three
  tests that pin the SQL to the merge rules do not run -- worth knowing
  before trusting a green suite.

## Where to start

4, 6 and 7 are done. 1 is the one that bites next, and it bites silently:
the flag a resume trusts is written by the half that is not a build. 5 is
twenty minutes. 2 is an afternoon and saves the worst moment. 3 wants a
decision about the exit status before any code.
