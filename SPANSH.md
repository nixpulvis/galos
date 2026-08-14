# Importing the Spansh galaxy dump

A runbook for the monthly import. Written to be read again in thirty days,
when none of it is fresh.

Spansh publishes a snapshot of every system it knows about. We take the
skeleton from it — address, name, position, and the class of the primary star
— and nothing else. EDDN and the journals own everything that moves.

## Two kinds of run

| | Skeleton | Star class |
|---|---|---|
| File | `systems.csv.gz` | `galaxy.json.gz` |
| Size | a few GB | tens of GB |
| Cadence | monthly | when the routing work needs it |
| Fills | `position` | `primary_star_class` |
| Pause EDDN | no | no |
| Touch indexes | no | no |

The skeleton run is the routine one. Take the **whole** file every month
rather than the `_1month` window: the window is keyed on when Spansh last
touched a system, not on when we last ran, so any gap between the two leaks
systems permanently — a quiet system in deep space may never be updated
again, and would never reappear in a later window. Taking the full file each
time removes that failure entirely, and the merge below is idempotent, so
re-reading four million systems we already have costs a read and no writes.

Deltas are worth it only for `galaxy.json.gz`, where the download itself is
the cost. If a monthly skeleton run is skipped, nothing needs to be caught
up; the next run is whole.

## Before the first run

The initial load is the only one that behaves differently, because it is
inserting on the order of a hundred million rows rather than a few million.

- `pg_dump -Fc elite_development > pre-spansh.dump`
- Confirm ~60 GB free, plus ~10 GB for staging.
- Stop EDDN sync.
- Drop `systems_position_idx` and `systems_name_trgm`, load, then rebuild
  with `maintenance_work_mem` at a couple of GB. Rebuilding in bulk is far
  cheaper than maintaining a GIST index across a hundred million inserts.
- Consider dropping the `UNIQUE` on `systems.position` for good. It is a
  btree over geometry, it costs several GB at this scale, and it buys little.

None of that applies to a monthly run. Never drop the GIST index on a
database that the map or the router is using: every `ST_3DDWithin` becomes a
sequential scan over the whole table.

## Tables

Staging is thrown away and rebuilt each run. The history is kept forever —
it is the only thing that remembers what happened last month.

```sql
-- No key and no indexes. The merge joins this against `systems` and never
-- seeks into it, and a btree built across a hundred million rows during the
-- COPY is the slowest part of the run for nothing.
CREATE UNLOGGED TABLE spansh_systems (
    address bigint, name text,
    x float8, y float8, z float8, star_class varchar
);

CREATE TABLE spansh_import (
    id             serial PRIMARY KEY,
    file           text      NOT NULL,   -- systems.csv.gz, galaxy.json.gz
    dump_date      timestamp NOT NULL,   -- the file's Last-Modified
    lines_done     bigint    NOT NULL DEFAULT 0,
    rows_inserted  bigint,
    rows_backfilled bigint,
    started_at     timestamp NOT NULL,
    finished_at    timestamp,
    notes          text
);
```

`lines_done` is what makes a run resumable. Load in chunks of about five
million lines, each chunk one transaction that copies, merges, and bumps the
count. A killed run picks up by skipping that many lines of the decompressed
stream; re-reading the head of the file costs minutes of CPU and no database
work.

## The run

1. **Fetch.** `curl -C - -O https://downloads.spansh.co.uk/systems.csv.gz`.
   Resumable, so a dropped connection is not a restart. Record the
   `Last-Modified` header as `dump_date`; it is the snapshot's real age, not
   the day we happened to download it.

2. **Open the history row.** Insert into `spansh_import` with `started_at`
   and `dump_date`, `finished_at` null. An unfinished row from last month is
   how you find out a run died.

3. **Stage.** `TRUNCATE spansh_systems`, then stream the gz through
   `COPY spansh_systems (address, name, x, y, z) FROM STDIN WITH (FORMAT csv)`.
   Never a row-at-a-time insert: `System::create` costs two round trips per
   system, which is days at this size.

4. **Merge, as two statements.** New systems and known systems mean
   different things and do not belong in one upsert.

```sql
-- (a) systems we have never seen
INSERT INTO systems (address, name, position, primary_star_class,
                     updated_at, updated_by)
SELECT s.address, UPPER(s.name),
       ST_SetSRID(ST_MakePoint(s.x, s.y, s.z), 0),
       s.star_class, 'epoch', 'Spansh dump ' || :dump_date
FROM spansh_systems s
ON CONFLICT DO NOTHING;

-- (b) systems EDDN already knows: fill holes, touch nothing else
UPDATE systems t
SET position           = COALESCE(t.position,
                             ST_SetSRID(ST_MakePoint(s.x, s.y, s.z), 0)),
    primary_star_class = COALESCE(t.primary_star_class, s.star_class)
FROM spansh_systems s
WHERE t.address = s.address
  AND (t.position IS NULL OR t.primary_star_class IS NULL);
```

5. **Close the history row.** `rows_inserted`, `rows_backfilled`,
   `finished_at`.

6. **Settle.** `VACUUM ANALYZE systems`, then drop staging — unless a star
   class run is coming next, which reuses it.

## Why those statements are shaped that way

Three things about this database break under the obvious upsert, and each
one is invisible when it happens.

**`updated_at` must not move, and new rows start at `epoch`.** EDDN passes
the event's own timestamp and the journal importer passes the journal's, not
`now()`. `create.rs` only writes when `systems.updated_at < $11`. Stamp rows
with the dump's date and every later message older than that date is dropped
silently and forever — a journal replay of an old flight, most of all. So
(b) leaves `updated_at` and `updated_by` where it found them, and (a) dates
what it inserts to `epoch` rather than to the snapshot. That is also the
truer reading: we have not observed those systems, only learnt that they
exist, and the first real sighting of one should always win. `updated_by`
still carries the dump's date, which is where to look for a row's age.

**The `IS NULL` predicate in (b) is not an optimisation.** An `UPDATE` in
Postgres is a delete and an insert. Firing one on every row leaves a hundred
million dead tuples, roughly doubles the heap, and hands autovacuum a job it
will not finish. Matching only the rows actually missing something also
makes the statement idempotent, which is what lets a run be repeated,
resumed, or taken monthly against the whole file without thinking about it.

**`ON CONFLICT DO NOTHING` carries no target on purpose.** `systems.position`
is `UNIQUE`. A dump row colliding on coordinates with an existing row under a
different address raises a violation that `ON CONFLICT (address)` does not
catch, and it takes the whole chunk down with it. Bare `DO NOTHING` covers
every constraint on the table.

## Why EDDN can keep running through it

We hold a GIST index over `systems.position`, and a monthly run offers it a
hundred million rows. It is left alone anyway, because almost none of those
rows reach it.

`ON CONFLICT DO NOTHING` does not insert and then tidy up. Postgres checks
the unique indexes for a conflict first, and on finding one skips the row
whole: no heap tuple, so no GIST entry, and nothing written to any other
index either. The systems we already have — nearly all of them — cost two
btree probes each and touch the position index zero times. Only genuinely
new systems are inserted into it, a few million in a month, which is the
same order of write as EDDN itself produces in that time. The run is a large
read against the indexes, not a large write.

Nothing here takes a lock that excludes anyone. The import and EDDN both
hold `RowExclusiveLock` on `systems`, which is compatible with itself;
concurrent writers are the ordinary case. `ACCESS EXCLUSIVE` is what
dropping or rebuilding an index takes, and that is the whole reason the
first load wants EDDN stopped and a monthly one does not.

Row level contention is rarer still. `DO NOTHING` does not lock the row it
conflicts with — `DO UPDATE` would — so EDDN is never queued behind the
hundred million rows we decline to write. Statement (b) matches only what is
missing a column, which after the first run is close to nothing, and so
locks close to nothing.

Order does not matter either. (b) only fills nulls and never advances
`updated_at`, and (a) dates its rows to `epoch`, so a system written by the
import and by EDDN in either order ends up the same.

## Checking it worked

Read these against the previous run's row rather than against nothing.

```sql
SELECT count(*) FROM systems;
SELECT count(*) FROM systems WHERE position IS NULL;
SELECT count(*) FROM systems WHERE primary_star_class IS NULL;
SELECT n_dead_tup FROM pg_stat_user_tables WHERE relname = 'systems';
```

A month of exploration is a few million new systems; a skeleton run that
inserts none, or ten times that, is worth understanding before moving on.
`position IS NULL` should fall and never rise. `rows_backfilled` should
approach zero over successive runs — that is the backfill converging.

Then spot-check that nothing rich was trampled: pick a populated system and
confirm its population, factions and stations are as they were. And
`EXPLAIN (ANALYZE)` an `ST_3DDWithin` at the new row count, which is the
number the routing work actually depends on.

## Known limits

**Names and positions are written once.** Statement (b) fills nulls and
never corrects. A system renamed in the game keeps the name we first saw. It
has not mattered yet. Reconciling would mean updating where the dump
disagrees *and* the row is older than the dump, which is a separate pass and
a separate decision.

**`systems.csv` has no star class**, so `primary_star_class` stays null after
a skeleton run. Positions alone are enough for everything except the neutron
boost in the router. The class comes from `bodies[]` where `mainStar` is
true, in `galaxy.json.gz` — astronav's `get_sys_flags` is the reference for
reading it.

**The GIN trigram index on names was measured against 284,000 systems.** Its
migration says so. At a hundred million it is a different index and a
different question, and worth re-reading before trusting the note.

## Undoing a run

The inserted systems are exactly those with `updated_by LIKE 'Spansh dump%'`,
so they delete cleanly. Statement (b) only ever wrote into nulls, so undoing
it means nulling those two columns for the addresses it touched — record them
during the merge if that matters. `pre-spansh.dump` covers everything worse
than that.
