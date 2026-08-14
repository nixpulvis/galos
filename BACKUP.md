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

This is a change to `galos-sync eddn` and it is not written yet.

How much it is worth depends on how long the database is away, and the
procedure for losing the cluster below gets that down to about a minute by
starting EDDN against an empty database and merging history in underneath
it. What remains is the gap before anyone notices, which the spool covers
and nothing else here does.

So the order of work is: know quickly that it has happened, be able to hand
EDDN a database in a minute, and then spool so that even that minute and the
noticing before it cost nothing. Something watching that `max(updated_at)`
in `systems` is still moving is the cheapest of the three and currently the
one that is missing.

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

Three paths. The question an incident asks is which one it needs, not which
is most thorough, and all three write into a database that is running and
taking EDDN — including losing the cluster, which gets a running database
made for it first.

`MERGING.md` has the machinery they share: staging, which side wins, and why
none of this locks EDDN out.

### Merge restore — EDDN never stops

The one to reach for. It is the Spansh merge with a backup as the source
instead of a dump.

Restore the backup into the scratch database beside the live one, then move
just the rows the incident touched across a pipe:

```sh
createdb galos_restore
pg_restore -d galos_restore -j4 backups/2026-08-13

psql elite_development -c \
  'CREATE TABLE restore_systems (LIKE systems INCLUDING DEFAULTS)'

psql galos_restore -c "COPY (SELECT * FROM systems
                             WHERE updated_by LIKE 'Spansh dump%') TO STDOUT" \
  | psql elite_development -c 'COPY restore_systems FROM STDIN'
```

Not `pg_restore -C`: it creates the database named inside the dump, which is
the live one.

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
while this runs, and a restore that replaces every row it can reach will push
good new data back to yesterday. What identifies the damage is `updated_by`,
or a window of `updated_at` around when it happened, or a list of addresses
recorded while doing the damage — the third rule in `MERGING.md`, and the
reason every bulk write leaves a fingerprint.

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

### Losing the cluster — stand up an empty one, backfill underneath it

For corruption, a lost disk, or a mistake too wide for any fingerprint to
describe. The obvious order is to restore and then start EDDN again, and it
is the wrong way round: it makes the stream wait on hours of history it does
not need in order to record what is happening now.

Turn it over. What EDDN needs is a database that exists, which takes a
minute. History can arrive underneath it afterwards.

1. **Make the database and migrate it.** `cargo sqlx database setup`. Keep
   a migrated empty database around and this is instead
   `CREATE DATABASE elite_development TEMPLATE elite_empty`, which is close
   to instant and does not depend on the migrations running cleanly under
   pressure.
2. **Restore `factions` and `articles` into it before starting EDDN.** They
   are small enough to be seconds, and their keys are the reason — see
   below.
3. **Point `DATABASE_URL` at it and start `galos-sync eddn`.** Downtime ends
   here, a minute or two from the decision, and everything after this point
   happens with the stream already recording.
4. Restore the backup into a scratch database beside the live one.
5. Merge it in, table by table, in the order below, for as long as it takes.

#### What decides a row

The backup is richer than the live database and older than it. Neither fact
wins on its own; the row's own timestamp does:

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

That is `System::create`'s own rule, which is what its guard was for, and the
second of the three in `MERGING.md`. It is the opposite of the one the Spansh
import follows, for the reason set out there: a dump knows less than we do
and may only fill holes, while a backup knows more than a rebuilt database
and may overwrite.

#### Faction ids do not survive this

The one thing that will corrupt data quietly rather than fail loudly.

`factions.id` is a `serial`. Its real key is `lower(name)`, and
`create.rs` inserts by name and lets Postgres hand out the number. So a
fresh database taking EDDN invents its own ids, in the order factions happen
to be mentioned — while `system_factions`, `system_faction_influences`,
`system_faction_states` and `conflicts` in the backup all refer to the ids
the *old* database handed out.

Merge those straight in and the foreign keys are satisfied, because the ids
exist. They just point at other factions. Influence, states and war history
get filed under whoever happens to hold that number now, and nothing
anywhere reports an error.

Restoring `factions` before EDDN starts, at step 2, avoids the whole thing:
the ids are the backup's, EDDN adds new factions after them, and every child
row means what it says. `articles` has a `serial` too and is standalone, so
it rides along for the same reason.

If EDDN has already run against an empty `factions` — recovery began before
this was noticed — the ids have to be translated rather than trusted:

```sql
INSERT INTO factions (name) SELECT name FROM restore_factions
ON CONFLICT (lower(name)) DO NOTHING;

CREATE TABLE faction_id_map AS
SELECT b.id AS old_id, f.id AS new_id
FROM restore_factions b
JOIN factions f ON lower(f.name) = lower(b.name);
```

and every child row's `faction_id` goes through `faction_id_map` on its way
in. Everything else in the schema is keyed on something the game issued —
system addresses, body and star ids, station and market names — so nothing
else needs this.

#### The order to merge in

Parents before children, or the foreign keys reject the child.

| | Tables |
|---|---|
| 1 | `systems`, `factions`, `articles` |
| 2 | `bodies`, `stars`, `stations`, `barycenters`, `system_factions` |
| 3 | `body_materials`, `markets`, `system_faction_influences`, `system_faction_states`, `conflicts` |
| 4 | `commodities` |

Within that, do `systems` first and unhurriedly: the map and the router are
useful again the moment it lands, and the faction tables can trail by a day
without anyone noticing.

#### What it costs

Merging is slower than restoring — every row goes through an upsert and the
indexes are live throughout, where a restore into an empty database bulk
copies and builds indexes once at the end. Expect hours rather than the
hour a clean restore would take. That is the trade being made, and it is
usually the right one: the database is up and recording for all of it.

It also wants disk for the scratch copy alongside the live one, so budget
twice the database. And `systems.position` is `UNIQUE`, so a backup row whose
coordinates now belong to a system EDDN inserted first will abort the
statement it is in — chunk the merge, or drop that constraint, which
`MERGING.md` argues for on its own account.

The old `pg_restore -Cd postgres` from the README still exists and is still
the fastest way to get all the data back. It is now the choice you make when
the stream matters less than the history, which is rarely.

## Testing that any of this works

A backup nobody has restored is not a backup. Restore into the scratch
database monthly, alongside the Spansh import so the two share one slot in
the calendar, and check row counts against the live database and against the
previous test. `MERGING.md` lists what else that database is for, which is
the argument for keeping one rather than making one each time.

Every migration in `galos_db` has a `.down.sql`, which is worth keeping true:
a migration that cannot be reversed turns a merge restore into a cluster
restore.

## The order to reach for things

| Incident | Path | EDDN |
|---|---|---|
| Our own import wrote something wrong | undo by fingerprint, `SPANSH.md` | up |
| A table damaged, identifiable rows | merge restore | up |
| A table damaged, nothing references it | restore beside, rename swap | milliseconds |
| Bad migration | `.down.sql`, then merge restore | up |
| Cluster corrupt or lost | empty database, EDDN onto it, backfill underneath | a minute, plus however long it took to notice |
