# Backing up, and restoring without stopping EDDN

A runbook. The goal it is written against is that EDDN keeps taking data
even when something has gone wrong with the database.

## What EDDN uptime actually costs

EDDN is a live stream and it cannot be replayed. A message missed is gone,
unlike EDSM or Spansh, which can be fetched again tomorrow.

Nothing between the stream and the database holds a message anywhere else.
`process_message` writes straight into Postgres, and every one of the fifteen
error paths in `eddn.rs` warns and carries on to the next message. So a
write that fails is a message discarded, and **the window in which EDDN is
down is exactly the window in which the database is not accepting writes**.

That is the thing to attack. Every technique below shortens or removes a
window where the database is unavailable, but none of them can reach zero:
restoring a whole cluster means the cluster is not there for a while. The
only way for that to stop costing data is for the stream to stop depending
on the database being up.

### The spool

Write each envelope to an append only file as it arrives, before anything
looks at Postgres, and drain the file into the database separately. Then
database downtime costs spool depth rather than data, and a full restore —
the one operation that cannot avoid taking the database away — becomes free
in the only currency that matters.

This is a change to `galos-sync eddn` and it is not written yet. It is worth
saying plainly that it is worth more than everything else in this document:
with it, a two hour restore loses nothing; without it, a two hour restore
loses two hours of a stream that will never carry that data again.

Until it exists, read the rest of this as ways to keep the unavailable
window small.

## Backing up

**`pg_dump` does not block writers.** It takes `ACCESS SHARE` and reads from
an MVCC snapshot, so EDDN keeps writing throughout. Backups are not a
downtime question. They have two other costs: a long transaction holds back
vacuum for its duration, and at a hundred million systems a single stream is
slow.

```sh
# Nightly. Directory format, four ways in parallel.
pg_dump -Fd -j4 -f backups/$(date +%F) elite_development
```

Take the tables our own tooling can damage separately as well. They are
small, fast, and are what a targeted restore actually wants:

```sh
pg_dump -Fc -t systems -f backups/systems-$(date +%F).dump elite_development
```

Do not run a backup across a Spansh import. The import is already a long
transaction and heavy on writes; sequence them, backup first. `SPANSH.md`
opens its own run with a dump for exactly this reason.

### Write ahead log archiving

A nightly dump means a bad day costs up to a day of EDDN, permanently. WAL
archiving is what turns that into seconds. Set `archive_mode = on` with an
`archive_command` copying segments somewhere off the data disk, and keep a
`pg_basebackup` weekly to archive against.

The restore it enables rebuilds a cluster, so it is the slowest path here
and takes the database away while it runs. It is still the right thing to
have: losing an hour and being down an hour are different sizes of problem,
and the spool above removes the second one.

### Keeping them

Seven nightly, three monthly, and whatever base backup the WAL archive is
anchored to. A dump of a full galaxy database is large; check the disk
before adding a retention tier rather than after.

## Restoring

Restoring into a running database is possible only for part of one. If the
cluster is gone there is nothing running to restore into. So the question
is always which of these three a given incident needs, and the answer is
usually the first.

### Merge restore — EDDN never stops

The primary tool. Nothing here takes a lock that excludes a writer; it is
the same shape as the Spansh merge, with a backup as the source instead of
a dump.

Restore the backup into a scratch database beside the live one, then move
just the rows the incident touched across a pipe:

```sh
pg_restore -C -d postgres -j4 backups/2026-08-13    # becomes galos_restore

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
while this runs, and a restore that replaces every row it can reach will
push good new data back to yesterday. What identifies the damage is
`updated_by`, or a window of `updated_at` around when it happened, or a list
of addresses recorded while doing the damage — which is the argument for
recording them.

Where the damage was a deletion rather than a bad write, the statement is an
insert instead, and `ON CONFLICT DO NOTHING` keeps it from touching anything
that came back on its own:

```sql
INSERT INTO systems SELECT * FROM restore_systems
ON CONFLICT DO NOTHING;
```

Both are idempotent, so a merge restore can be run again, narrowed, and run
again.

### Swapping a table — a lock measured in milliseconds

For a table nothing references, restore beside it and rename:

```sql
BEGIN;
ALTER TABLE commodities RENAME TO commodities_broken;
ALTER TABLE commodities_restored RENAME TO commodities;
COMMIT;
```

EDDN blocks for the length of the transaction and no longer.

**This does not work for `systems`.** Every other table hangs off it —
factions, bodies, stations, markets, stars, barycenters, conflicts and the
faction tables all name it in a `REFERENCES`. A foreign key follows the
table's identity, not its name, so renaming `systems` out of the way leaves
all of them still pointing at it under its new name, and the replacement
arrives with no children. `systems` is restored by merging, above.

### Restoring the cluster — the one with real downtime

For corruption, a lost disk, or a mistake wide enough that no fingerprint
describes it. Either PITR against the WAL archive, or:

```sh
pg_restore -Cd postgres < backups/latest.dump
```

`-C` creates the database, which means nothing may be connected to it, which
means EDDN is down from here until it finishes. This is the command in the
README and it is the full downtime path; reach for it third, not first.

Restore to a scratch database and swap names if you can, rather than
restoring over the live one — it turns most of the outage into a rename.

## Testing that any of this works

A backup nobody has restored is not a backup. Restore into a scratch
database monthly, alongside the Spansh import so the two share one slot in
the calendar, and check row counts against the live database and against the
previous test.

That scratch database is also where a migration gets tried before it is run
for real, and where the routing work can be measured at scale without
touching anything EDDN is writing to. Every migration in `galos_db` has a
`.down.sql`, which is worth keeping true; a migration that cannot be reversed
turns a merge restore into a cluster restore.

## The order to reach for things

| Incident | Path | EDDN |
|---|---|---|
| Our own import wrote something wrong | undo by fingerprint, `SPANSH.md` | up |
| A table damaged, identifiable rows | merge restore | up |
| A table damaged, nothing references it | restore beside, rename swap | milliseconds |
| Bad migration | `.down.sql`, then merge restore | up |
| Cluster corrupt or lost | PITR, or `pg_restore -C` | down, and this is what the spool is for |
