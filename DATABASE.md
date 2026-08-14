# Keeping the database

Backing it up, restoring it without stopping EDDN, and taking the monthly
Spansh dump into it. Written to be read again months later, when none of it
is fresh.

| If | Then | EDDN |
|---|---|---|
| An import wrote something wrong | undo by fingerprint, [below](#undoing-a-run) | up |
| A table is damaged, rows identifiable | [merge restore](#merge-restore--eddn-never-stops) | up |
| A table is damaged, nothing references it | [restore beside, rename](#swapping-a-table--a-lock-measured-in-milliseconds) | milliseconds |
| A migration went wrong | `.down.sql`, then merge restore | up |
| The cluster is corrupt or lost | [empty database, EDDN onto it, backfill under](#losing-the-cluster--stand-up-an-empty-one-backfill-underneath-it) | a minute, plus however long it took to notice |

## What EDDN uptime costs

EDDN is a live stream and it cannot be replayed. A message missed is gone,
unlike EDSM or Spansh, which can be fetched again tomorrow.

Nothing between the stream and the database holds a message anywhere else.
`process_message` writes straight into Postgres, and every one of the fifteen
error paths in `eddn.rs` warns and carries on to the next message. So a write
that fails is a message discarded, and **the window in which EDDN is down is
exactly the window in which the database is not accepting writes**.

Everything below is shaped by that. Procedures that would normally take the
database away are rearranged so they do not, and the one that cannot avoid it
is arranged to take about a minute.

### The spool

**Nothing below exists yet.** It is a change to `galos-sync eddn`, described
here because the rest of this document keeps referring to it.

Receiving a message and storing it are one action at the moment, which is
what makes a database problem a data problem. A spool splits them in two.
The receiving loop's only job becomes appending the raw envelope to a file
and moving on, which has one failure mode — a full disk — where writing to
Postgres has many. A separate consumer reads that file into the database and
remembers how far it got. While Postgres is away the file grows; when it
comes back the consumer drains it. Downtime becomes a queue rather than a
hole.

It wants one piece of state, a position in the file, updated in the same
transaction as the work it describes. That is `lines_done` in
`spansh_import` again, and worth building the same way for the same reason.

It also makes *parsing* recoverable, which nothing else here does. If
`elite_journal` mishandles an event — a new field, a shape changed by a game
update — that data is gone today, because EDDN cannot be asked twice. Keep a
few days of spool files and the fix is to correct the parser and drain them
again.

What it is worth against downtime depends on how long the database is away,
and [losing the
cluster](#losing-the-cluster--stand-up-an-empty-one-backfill-underneath-it)
already gets that to about a minute. What remains is the gap before anyone
notices. So: know quickly that it has happened, be able to hand EDDN a
database in a minute, then spool so that even that costs nothing. Something
watching that `max(updated_at)` in `systems` is still moving is the cheapest
of the three and currently the one that is missing.

## Writing into a live database

The monthly import and every restore that leaves EDDN running are the same
operation: reconcile a pile of rows from somewhere else into a table that is
being written to at the same time. This is the machinery; the two procedures
after it say only what is true of themselves.

**The shape.** `COPY` the snapshot into an unlogged table beside the live
one; reconcile with set based SQL, guarded so the live table wins where it
should; leave a fingerprint on what was written; `VACUUM ANALYZE` and drop
staging.

**Never a row at a time.** `System::create` is an insert and an
`adopt_waiting_markets` call — two round trips for one system. Fine for a
stream arriving at thirty messages a second, days for anything bulk.

**Staging tables** are unlogged, so nothing is written to the WAL for a table
that is thrown away, and carry no primary key and no indexes: the merge joins
staging against the live table and never seeks into staging, while a btree
built during the `COPY` is the slowest part of a run for nothing. Where
staging holds whole rows of a real table, make it with `CREATE TABLE
restore_x (LIKE x INCLUDING DEFAULTS)`, which keeps column order and makes
`SELECT *` safe to write. `TRUNCATE` and rebuild each run.

### Which side wins

The question every merge answers, and the answers differ. Taking one of
these from the wrong row is how good data quietly becomes bad.

| Source | Against what is on record | Rule |
|---|---|---|
| A Spansh dump | poorer, and undated | Fill nulls only. Never advance `updated_at`. New rows dated `epoch`. |
| A backup, restoring everything | fuller, but older | The row's own timestamp decides: `WHERE t.updated_at < b.updated_at`. |
| A backup, repairing damage | fuller, and correct | Scope by the incident's fingerprint, and overwrite unconditionally inside it. |

**Do not carry one of those rules across to another.** A dump knows less than
we do and may only fill holes; a backup knows more than a rebuilt database
and may overwrite; a repair knows better than the damage and must overwrite,
but only where the damage reaches.

`updated_at` is the arbiter because EDDN passes the event's own timestamp and
the journal importer passes the journal's, not `now()`, and `create.rs`
writes only when `systems.updated_at < $11`. That guard makes the second rule
one line of SQL, and it is why the first rule exists: stamp rows with a
dump's date and every later message older than that date is dropped silently
and for good — a journal replay of an old flight, most of all. New rows a
dump introduces are dated `epoch` for the same reason, and it is the truer
reading anyway: we have not observed those systems, only learnt that they
exist, so the first real sighting should win.

### The statements

**Two, not one upsert.** Rows we have never seen and rows we already hold
mean different things, and one `ON CONFLICT DO UPDATE` has to pick a single
behaviour for both. It is also what lets the update carry its predicate.

**`ON CONFLICT DO NOTHING` carries no target.** `systems.position` is
`UNIQUE` as well as `address` being the key. A row colliding on coordinates
with an existing row under a different address raises a violation that `ON
CONFLICT (address)` does not catch, and it takes the whole statement — and so
the whole chunk — down with it. Bare `DO NOTHING` arbitrates on every unique
constraint the table has. That constraint is a btree over geometry costing
several GB at galaxy scale and buying very little; dropping it would remove
the hazard entirely and is worth doing.

**Touch only what needs touching.** An `UPDATE` in Postgres is a delete and
an insert. One firing on every row of a hundred million leaves a hundred
million dead tuples, roughly doubles the heap, and hands autovacuum a job it
will not finish. Every update here carries a predicate narrowing it to rows
that actually need changing — `IS NULL` for a backfill, the timestamp
comparison for a restore, the fingerprint for a repair. That predicate also
makes the statement idempotent, so any of this can be re-run, narrowed and
re-run, or interrupted and started again.

**Fingerprints.** Every bulk write puts something identifying in
`updated_by` — `Spansh dump 2026-08-14` and the like. It costs nothing at
write time and is the only thing making the rows findable afterwards, which
is what both undoing a run and scoping a repair depend on. A run that also
records the addresses it touched can be undone exactly rather than
approximately.

### Why EDDN keeps running through all of it

**Nothing here takes a lock that excludes a writer.** The merge and EDDN both
hold `RowExclusiveLock`, which is compatible with itself; concurrent writers
are the ordinary case. `ACCESS EXCLUSIVE` is what dropping or rebuilding an
index takes, and it is the only reason any procedure here asks for EDDN to be
stopped.

**`ON CONFLICT DO NOTHING` skips a conflicting row whole.** Postgres checks
the unique indexes before writing anything, so a conflicting row costs a
couple of btree probes and produces no heap tuple, no GIST entry and no entry
in any other index. Offering the live table a hundred million rows it already
has is a large read, not a large write — which is why the GIST index over
`systems.position` is left alone through a monthly run.

**And it does not lock the row it conflicts with** — `DO UPDATE` would. EDDN
is never queued behind the rows we decline to write.

**Keep chunks small anyway.** A transaction holds its locks until it commits,
so a merge runs in chunks of a few million rows rather than one statement, and
EDDN waits milliseconds rather than minutes on the rare collision.

### Two things a merge cannot carry across

**Surrogate keys.** `factions.id` is a `serial` whose real key is
`lower(name)` — `create.rs` inserts by name and lets Postgres hand out the
number — so it is assigned differently in any database populated in a
different order. Everything else in the schema is keyed on something the game
issued: system addresses, body and star ids, station and market names. See
[losing the
cluster](#losing-the-cluster--stand-up-an-empty-one-backfill-underneath-it),
which is where it bites.

**Foreign key order.** Anything merging more than one table does parents
before children, or the keys reject the child:

| | Tables |
|---|---|
| 1 | `systems`, `factions`, `articles` |
| 2 | `bodies`, `stars`, `stations`, `barycenters`, `system_factions` |
| 3 | `body_materials`, `markets`, `system_faction_influences`, `system_faction_states`, `conflicts` |
| 4 | `commodities` |

The Spansh import touches `systems` alone and does not need this.

## Backing up

**`pg_dump` does not block writers.** It takes `ACCESS SHARE` and reads from
an MVCC snapshot, so EDDN keeps writing throughout. Backups are not a
downtime question. They have two other costs: a long transaction holds back
vacuum for its duration, and at a hundred million systems a single stream is
slow.

```sh
# Nightly. Directory format, four ways in parallel.
pg_dump -Fd -j4 -f backups/$(date +%F) elite_development

# And the tables our own tooling can damage, separately. Small, fast, and
# what a targeted restore actually wants.
pg_dump -Fc -t systems -f backups/systems-$(date +%F).dump elite_development
```

Do not run a backup across a Spansh import. The import is already long and
heavy on writes; sequence them, backup first.

**Write ahead log archiving.** A nightly dump means a bad day costs up to a
day of EDDN, permanently. Set `archive_mode = on` with an `archive_command`
copying segments off the data disk, and keep a weekly `pg_basebackup` to
archive against. The restore it enables rebuilds a cluster, so it is the
slowest path here — but losing an hour and being down an hour are different
sizes of problem, and only the first is what this prevents.

**Keeping them.** Seven nightly, three monthly, and whatever base backup the
WAL archive is anchored to. A dump of a full galaxy database is large; check
the disk before adding a retention tier rather than after.

### The scratch database

A second database beside the live one, restored from the most recent backup.
One object in four jobs, which is the argument for keeping one rather than
making one each time:

- where a backup is restored before being merged back in,
- proof that the backup restores at all,
- where a migration is tried before it is run for real,
- where the routing work is measured at scale without touching anything EDDN
  is writing to.

```sh
createdb galos_restore
pg_restore -d galos_restore -j4 backups/2026-08-13
```

Not `pg_restore -C`, which creates the database named inside the dump. That
is the live one.

A backup nobody has restored is not a backup. Rebuild this monthly alongside
the Spansh import, so the two share one slot in the calendar, and check row
counts against the live database and against the previous test. Every
migration in `galos_db` has a `.down.sql`, which is worth keeping true: a
migration that cannot be reversed turns a merge restore into a cluster
restore.

## Restoring

Three paths. The question an incident asks is which one it needs, not which
is most thorough, and all three write into a database that is running and
taking EDDN — including losing the cluster, which gets a running database
made for it first.

### Merge restore — EDDN never stops

The one to reach for. It is the Spansh merge with a backup as the source
instead of a dump.

Restore into the scratch database, then move just the rows the incident
touched across a pipe:

```sh
psql elite_development -c \
  'CREATE TABLE restore_systems (LIKE systems INCLUDING DEFAULTS)'

psql galos_restore -c "COPY (SELECT * FROM systems
                             WHERE updated_by LIKE 'Spansh dump%') TO STDOUT" \
  | psql elite_development -c 'COPY restore_systems FROM STDIN'
```

Then reconcile, scoped to the incident:

```sql
UPDATE systems t
SET name = b.name, position = b.position, population = b.population,
    security = b.security, government = b.government,
    allegiance = b.allegiance, primary_economy = b.primary_economy,
    secondary_economy = b.secondary_economy,
    updated_at = b.updated_at, updated_by = b.updated_by
FROM restore_systems b
WHERE t.address = b.address
  AND t.updated_by = 'Spansh dump 2026-08-14';   -- the incident's fingerprint
```

**Scope by the incident, never by comparing wholesale.** EDDN is writing
while this runs, and a restore replacing every row it can reach will push
good new data back to yesterday. What identifies the damage is `updated_by`,
a window of `updated_at` around when it happened, or a list of addresses
recorded while doing the damage.

Where the damage was a deletion rather than a bad write, the statement is an
insert instead, and `ON CONFLICT DO NOTHING` keeps it from touching anything
that came back on its own:

```sql
INSERT INTO systems SELECT * FROM restore_systems
ON CONFLICT DO NOTHING;
```

Both are idempotent, so a merge restore can be run, narrowed, and run again.

### Swapping a table — a lock measured in milliseconds

For a table nothing references, restore beside it and rename:

```sql
BEGIN;
ALTER TABLE commodities RENAME TO commodities_broken;
ALTER TABLE commodities_restored RENAME TO commodities;
COMMIT;
```

EDDN blocks for the length of the transaction and no longer.

**This does not work for `systems`.** Every other table hangs off it. A
foreign key follows the table's identity rather than its name, so renaming
`systems` out of the way leaves all of them still pointing at it under its
new name, and the replacement arrives with no children. `systems` is restored
by merging, above.

### Losing the cluster — stand up an empty one, backfill underneath it

For corruption, a lost disk, or a mistake too wide for any fingerprint. The
obvious order is to restore and then start EDDN again, and it is the wrong
way round: it makes the stream wait on hours of history it does not need in
order to record what is happening now.

Turn it over. What EDDN needs is a database that exists, which takes a
minute. History arrives underneath it afterwards.

1. **Make the database and migrate it.** `cargo sqlx database setup`. Keep a
   migrated empty database around and this is instead `CREATE DATABASE
   elite_development TEMPLATE elite_empty`, which is close to instant and does
   not depend on the migrations running cleanly under pressure.
2. **Restore `factions` and `articles` before starting EDDN.** Seconds, and
   their keys are the reason — below.
3. **Point `DATABASE_URL` at it and start `galos-sync eddn`.** Downtime ends
   here, a minute or two from the decision. Everything after this happens with
   the stream already recording.
4. Restore the backup into the scratch database.
5. Merge it in, table by table, in [foreign key
   order](#two-things-a-merge-cannot-carry-across), for as long as it takes.
   Do `systems` first and unhurriedly: the map and the router are useful again
   the moment it lands, and the faction tables can trail by a day without
   anyone noticing.

The backup is fuller than the live database and older than it. Neither fact
wins on its own; the row's own timestamp does, which is `System::create`'s own
rule and the second in [which side wins](#which-side-wins):

```sql
INSERT INTO systems SELECT * FROM restore_systems
ON CONFLICT (address) DO UPDATE SET
    name = EXCLUDED.name, position = EXCLUDED.position,
    population = EXCLUDED.population, security = EXCLUDED.security,
    government = EXCLUDED.government, allegiance = EXCLUDED.allegiance,
    primary_economy = EXCLUDED.primary_economy,
    secondary_economy = EXCLUDED.secondary_economy,
    updated_at = EXCLUDED.updated_at, updated_by = EXCLUDED.updated_by
WHERE systems.updated_at < EXCLUDED.updated_at;
```

#### Faction ids do not survive this

The one thing here that corrupts data quietly rather than failing loudly.

A fresh database taking EDDN invents its own `factions.id` values, in the
order factions happen to be mentioned — while `system_factions`,
`system_faction_influences`, `system_faction_states` and `conflicts` in the
backup all refer to the ids the *old* database handed out.

Merge those straight in and the foreign keys are satisfied, because the ids
exist. They just point at other factions. Influence, states and war history
get filed under whoever holds that number now, and nothing reports an error.

Restoring `factions` at step 2 avoids it entirely: the ids are the backup's,
EDDN adds new factions after them, and every child row means what it says.
`articles` has a `serial` too and rides along for the same reason.

If EDDN already ran against an empty `factions` — recovery began before this
was noticed — the ids have to be translated rather than trusted:

```sql
INSERT INTO factions (name) SELECT name FROM restore_factions
ON CONFLICT (lower(name)) DO NOTHING;

CREATE TABLE faction_id_map AS
SELECT b.id AS old_id, f.id AS new_id
FROM restore_factions b
JOIN factions f ON lower(f.name) = lower(b.name);
```

and every child row's `faction_id` goes through `faction_id_map` on the way
in.

#### What it costs

Merging is slower than restoring — every row goes through an upsert with the
indexes live throughout, where a restore into an empty database bulk copies
and builds indexes once at the end. Expect hours rather than the hour a clean
restore would take. That is the trade, and it is usually right: the database
is up and recording for all of it.

It wants disk for the scratch copy alongside the live one, so budget twice
the database. And `systems.position` is `UNIQUE`, so a backup row whose
coordinates now belong to a system EDDN inserted first will abort the
statement it is in — chunk the merge, or drop that constraint.

`pg_restore -Cd postgres` from the README is still the fastest way to get all
the data back. It is now the choice made when the stream matters less than
the history, which is rarely.

## The monthly Spansh import

Spansh publishes a snapshot of every system it knows about. We take the
skeleton — address, name, position, and the class of the primary star — and
nothing else. EDDN and the journals own everything that moves.

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
time removes that failure entirely, and the merge is idempotent, so
re-reading systems we already have costs a read and no writes. If a monthly
run is skipped, nothing needs catching up; the next one is whole.

Deltas are worth it only for `galaxy.json.gz`, where the download is the cost.

### Before the first run

The initial load is the only one that behaves differently, because it inserts
on the order of a hundred million rows rather than a few million.

- `pg_dump -Fd -j4 -f backups/pre-spansh elite_development`
- Confirm ~60 GB free, plus ~10 GB for staging.
- Stop EDDN.
- Drop `systems_position_idx` and `systems_name_trgm`, load, then rebuild with
  `maintenance_work_mem` at a couple of GB. Rebuilding in bulk is far cheaper
  than maintaining a GIST index across a hundred million inserts.

That index rebuild is an `ACCESS EXCLUSIVE` lock and the only reason EDDN is
stopped for it. None of it applies to a monthly run — never drop the GIST
index on a database the map or the router is using, or every `ST_3DDWithin`
becomes a sequential scan.

### Tables

Staging is thrown away and rebuilt each run. The history is kept forever; it
is the only thing that remembers what happened last month.

```sql
CREATE UNLOGGED TABLE spansh_systems (
    address bigint, name text,
    x float8, y float8, z float8, star_class varchar
);

CREATE TABLE spansh_import (
    id              serial PRIMARY KEY,
    file            text      NOT NULL,   -- systems.csv.gz, galaxy.json.gz
    dump_date       timestamp NOT NULL,   -- the file's Last-Modified
    lines_done      bigint    NOT NULL DEFAULT 0,
    rows_inserted   bigint,
    rows_backfilled bigint,
    started_at      timestamp NOT NULL,
    finished_at     timestamp,
    notes           text
);
```

`lines_done` is what makes a run resumable. Load in chunks of about five
million lines, each chunk one transaction that copies, merges and bumps the
count. A killed run picks up by skipping that many lines of the decompressed
stream; re-reading the head of the file costs minutes of CPU and no database
work.

### The run

1. **Fetch.** `curl -C - -O https://downloads.spansh.co.uk/systems.csv.gz`.
   Resumable, so a dropped connection is not a restart. Record the
   `Last-Modified` header as `dump_date`; it is the snapshot's real age, not
   the day we happened to download it.

2. **Open the history row.** `started_at` and `dump_date`, `finished_at`
   null. An unfinished row from last month is how you find out a run died.

3. **Stage.** `TRUNCATE spansh_systems`, then stream the gz through `COPY
   spansh_systems (address, name, x, y, z) FROM STDIN WITH (FORMAT csv)`. A
   star class run names `star_class` in that column list too.

4. **Merge.**

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

   `UPPER(name)` matches how `create.rs` stores it, or `fetch_by_name` will
   not find what we import. A dump takes the first rule in [which side
   wins](#which-side-wins) — fill nulls, never advance `updated_at`, date new
   rows `epoch` — because it has no BGS state, no factions, no stations, and
   no idea when what it does carry was true.

5. **Close the history row.** `rows_inserted`, `rows_backfilled`,
   `finished_at`.

6. **Settle.** `VACUUM ANALYZE systems`, then drop staging — unless a star
   class run is coming next, which reuses it.

Order between the two writers does not matter: (b) only fills nulls and never
advances `updated_at`, and (a) dates its rows to `epoch`, so a system written
by the import and by EDDN in either order ends up the same.

### Checking it worked

Read these against the previous run's row rather than against nothing.

```sql
SELECT count(*) FROM systems;
SELECT count(*) FROM systems WHERE position IS NULL;
SELECT count(*) FROM systems WHERE primary_star_class IS NULL;
SELECT n_dead_tup FROM pg_stat_user_tables WHERE relname = 'systems';
```

A month of exploration is a few million new systems; a run inserting none, or
ten times that, is worth understanding before moving on. `position IS NULL`
should fall and never rise. `rows_backfilled` approaching zero over
successive runs is the backfill converging.

Then spot-check that nothing rich was trampled: pick a populated system and
confirm its population, factions and stations are as they were. And `EXPLAIN
(ANALYZE)` an `ST_3DDWithin` at the new row count, which is the number the
routing work actually depends on.

### Undoing a run

The inserted systems are exactly those with `updated_by LIKE 'Spansh dump%'`,
so they delete cleanly. Statement (b) only ever wrote into nulls, so undoing
it means nulling those two columns for the addresses it touched — record them
during the merge if that matters. `backups/pre-spansh` covers everything
worse than that.

### Known limits

**Names and positions are written once.** Statement (b) fills nulls and never
corrects, so a system renamed in the game keeps the name we first saw. It has
not mattered yet. Reconciling would mean updating where the dump disagrees
*and* the row is older than the dump, which is a separate pass and a separate
decision.

**`systems.csv` has no star class**, so `primary_star_class` stays null after
a skeleton run. Positions alone are enough for everything except the neutron
boost in the router. The class comes from `bodies[]` where `mainStar` is
true, in `galaxy.json.gz` — astronav's `get_sys_flags` is the reference.

**The GIN trigram index on names was measured against 284,000 systems.** Its
migration says so. At a hundred million it is a different index and a
different question, and worth re-reading before trusting the note. It also
uses `fastupdate`, so an import's inserts land in a pending list and are
folded in later — the cost shows up as a latency spike in name search at some
arbitrary moment after a run, not during it.
