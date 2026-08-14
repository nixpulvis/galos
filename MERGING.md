# Writing a snapshot into a live database

The monthly Spansh import and every restore that leaves EDDN running are the
same operation: take a pile of rows from somewhere else and reconcile it into
a table that is being written to at the same time.

This is the machinery they share. `SPANSH.md` and `BACKUP.md` are the two
procedures built on it, and each says only what is true of itself.

## The shape

1. **Stage.** `COPY` the snapshot into an unlogged table beside the live one.
2. **Merge.** Reconcile with set based SQL, guarded so the live table wins
   where it should.
3. **Record.** Leave a fingerprint on what was written and a row saying the
   run happened.
4. **Settle.** `VACUUM ANALYZE`, drop the staging table.

## Never a row at a time

`System::create` is an insert and an `adopt_waiting_markets` call — two round
trips for one system. That is fine for a stream arriving at thirty messages a
second and it is days for anything bulk. Everything here goes through `COPY`
into staging and then set based statements against it.

## Staging tables

Unlogged, so nothing is written to the WAL for a table that is thrown away.
No primary key and no indexes: the merge joins staging against the live table
and never seeks into staging, while a btree built during the `COPY` is the
slowest part of the run for nothing.

Where staging holds whole rows of a real table, make it with
`CREATE TABLE restore_x (LIKE x INCLUDING DEFAULTS)`, which keeps the column
order and makes `SELECT *` safe to write.

`TRUNCATE` and rebuild each run rather than trying to keep one current.

## Which side wins

The question every merge has to answer, and the answers differ. Getting one
of these from the wrong row is how good data quietly becomes bad.

| Source | Against what is on record | Rule |
|---|---|---|
| A Spansh dump | poorer, and undated | Fill nulls only. Never advance `updated_at`. New rows dated `epoch`. |
| A backup, restoring everything | fuller, but older | The row's own timestamp decides: `WHERE t.updated_at < b.updated_at`. |
| A backup, repairing damage | fuller, and correct | Scope by the incident's fingerprint, and take the backup unconditionally inside that scope. |

**Do not carry one of those rules across to another.** A dump knows less than
we do and may only fill holes; a backup knows more than a rebuilt database
and may overwrite; a repair knows better than the damage and must overwrite,
but only where the damage reaches.

### Why `updated_at` is the arbiter

EDDN passes the event's own timestamp and the journal importer passes the
journal's, not `now()`. `create.rs` writes only when
`systems.updated_at < $11`. That guard is what makes the second rule above a
single line of SQL, and it is also why the first rule exists: stamp rows with
a dump's date and every later message older than that date is dropped
silently and for good, a journal replay of an old flight most of all.

New rows a dump introduces are dated `epoch` for the same reason, and it is
the truer reading anyway — we have not observed those systems, only learnt
that they exist, so the first real sighting should win.

## Two statements, not one upsert

Rows we have never seen and rows we already hold mean different things, and
one `ON CONFLICT DO UPDATE` has to pick a single behaviour for both. Write
the insert and the update separately. It is also what lets the update carry
the predicate below.

## `ON CONFLICT DO NOTHING` carries no target

`systems.position` is `UNIQUE` as well as `address` being the key. A row
colliding on coordinates with an existing row under a different address
raises a violation that `ON CONFLICT (address)` does not catch, and it takes
the whole statement — and so the whole chunk — down with it. Bare
`DO NOTHING` arbitrates on every unique constraint the table has.

That constraint is a btree over geometry costing several GB at galaxy scale
and buying very little. Dropping it would remove this hazard entirely and is
worth doing.

## Touch only what needs touching

An `UPDATE` in Postgres is a delete and an insert. One that fires on every
row of a hundred million leaves a hundred million dead tuples, roughly
doubles the heap and hands autovacuum a job it will not finish. Every update
here carries a predicate narrowing it to the rows that actually need
changing — `IS NULL` for a backfill, the timestamp comparison for a restore,
the fingerprint for a repair.

The predicate is what makes the statement idempotent as well, so any of these
can be re-run, narrowed and re-run, or interrupted and started again.

## Why EDDN keeps running through all of it

**Nothing here takes a lock that excludes a writer.** The merge and EDDN both
hold `RowExclusiveLock`, which is compatible with itself; concurrent writers
are the ordinary case. `ACCESS EXCLUSIVE` is what dropping or rebuilding an
index takes, and it is the only reason any procedure here ever asks for EDDN
to be stopped.

**`ON CONFLICT DO NOTHING` skips a conflicting row whole.** Postgres checks
the unique indexes before writing anything, so a row that conflicts costs a
couple of btree probes and produces no heap tuple, no GIST entry and no
entry in any other index. Offering the live table a hundred million rows it
already has is a large read, not a large write.

**And it does not lock the row it conflicts with** — `DO UPDATE` would. EDDN
is never queued behind the rows we decline to write.

**Keep chunks small anyway.** Row level contention is rare but a transaction
holds its locks until it commits, so a merge runs in chunks of a few million
rows rather than one statement, and EDDN waits milliseconds rather than
minutes on the rare collision.

## Fingerprints

Every bulk write puts something identifying in `updated_by` — `Spansh dump
2026-08-14` and the like. It costs nothing at write time and it is the only
thing that makes the row findable afterwards, which is what both undoing a
run and scoping a repair depend on. A run that also records the addresses it
touched can be undone exactly rather than approximately.

## Two things a merge cannot carry across

**Surrogate keys.** `factions.id` is a `serial` whose real key is
`lower(name)`, so it is assigned differently in any database that was
populated in a different order. Everything else in the schema is keyed on
something the game issued — system addresses, body and star ids, station and
market names — so `factions` is the only one, and `BACKUP.md` has the
procedure for it.

**Foreign key order.** Anything merging more than one table has to do parents
before children; the order for this schema is in `BACKUP.md`, which is the
only procedure that needs it. The Spansh import touches `systems` alone.

## The scratch database

A second database beside the live one, restored from the most recent backup.
It is the same object in four jobs, which is why it is worth keeping one
around rather than making one each time:

- where a backup is restored to before being merged back in,
- proof that the backup restores at all,
- where a migration is tried before it is run for real,
- and where the routing work is measured at scale without touching anything
  EDDN is writing to.

Making one is `createdb galos_restore` and then `pg_restore -d galos_restore`.
Not `pg_restore -C`, which creates the database named inside the dump —
`elite_development`, the live one.
