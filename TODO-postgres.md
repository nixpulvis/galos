# What Postgres is doing, and why it reads so much

Started from a simpler question — *what is making Postgres read so much
right now, when there is one `galos-sync` on the database and it isn't doing
that much?* — and the answer turned out to be three separate things, only
one of which is Postgres's fault. Written down with the numbers so nobody
has to measure them again.

Everything below was measured on 2026-09-12/13 against
`galos_postimport_backup` on PostgreSQL 18, with `galos-sync --from eddn
--db --index` live on the feed.

## The short of it

1. Nothing extra is attached to the database. The fourth connection is
   **rust-analyzer**, running `sqlx::query!` verification.
2. Postgres is **not** the disk hog. It reads ~320 KB/s. The 569 GB
   sequential read is the *other* `galos-sync`, chewing `galaxy.json`.
3. Postgres's reads are nonetheless **100 % wasted** — reads equal
   evictions — because `shared_buffers` is the 128 MB default against an
   11 GB database.
4. Where those reads land is `systems`, and the reason they exist at all is
   one line of `galos_db`: `received_at = clock_timestamp()` makes **every**
   upsert non-HOT, so all six indexes are rewritten on every message.

Items 1 and 2 are answers, not work. Item 3 is a knob. Item 4 is the design
change and is the only one with real leverage.

## Who is actually connected

Four backends on the database, matched to owning processes with `lsof` by
client port rather than guessed:

| port(s) | owner |
|---|---|
| 50554, 50635, 50691 | PID 51837 `galos-sync --from eddn --db --index`, one sqlx pool |
| 49680 | **PID 33966 `rust-analyzer`** |
| — | PID 72338 `galos-sync --from spansh=… --index=.index/full`, **zero** connections |

So the premise was right: one sync on the database, and the spansh run is
not on it at all. The connection that made it look otherwise is
rust-analyzer holding a backend open for compile-time `sqlx::query!`
checking against `DATABASE_URL`. It had been idle 75 minutes; its last
statement was a `DELETE FROM stars WHERE system_address = $1 AND id = $2`
prepare, which is a macro being verified and not a row being deleted. It
reads nothing. It does pin a backend, which is worth knowing but is not a
problem.

The pool showing three connections with `backend_start` times hours after
the process started (22:34, 23:23, 23:37, 23:48 against a sync launched at
18:05) is just sqlx recycling on idle timeout. Not a leak.

## Postgres is not the one reading the disk

Measured rate, `pg_stat_io` over a 30 s window, client backends:

| reads | writes | extends | evictions |
|---|---|---|---|
| 912 | 0 | 8 | 942 |

912 read operations at 8254 B average is **~40 IOPS, ~320 KB/s**. That is
nothing. Autovacuum reads were flat at 22,003,339 across a separate 60 s
poll, so no vacuum was contributing either.

The disk activity is PID 72338, which holds
`/Users/nixpulvis/Downloads/galaxy.json` open read-only on fd 7 — **569 GB**
— while writing `.index/full`. That is the sequential read worth attributing
elsewhere before blaming the database.

What *does* look alarming is the cumulative counter, and it should: **7545 GB
read** across all backends since the postmaster started on Aug 24, 1806 GB
of it by client backends, at an **88.89 %** cache hit ratio. If a process
monitor is the thing showing a big number, that is the number it is showing,
and it is a total and not a rate.

## Every read evicts something live

`shared_buffers = 16384` × 8 kB = **128 MB, the stock default**, against an
**11 GB** database whose `systems` indexes alone are 1221 MB.

The tell is in the table above: **912 reads, 942 evictions.** Essentially
every physical read displaces a still-wanted buffer. There is no working set
retention at all — the cache is too small to keep even the inner pages of
one hot index, so the same pages are read, evicted, and read again forever.

Raising `shared_buffers` to 2–4 GB is the single cheapest improvement
available and needs no code change. Not done, deliberately, but it is the
first thing to reach for.

Other settings as found, for reference: `work_mem` 4 MB,
`maintenance_work_mem` 64 MB, `effective_cache_size` 4 GB,
`effective_io_concurrency` 16, `io_method` worker with 3 io workers,
`autovacuum_max_workers` 3, `autovacuum_vacuum_cost_delay` 2 ms.

## Where the reads land

`systems` is ~95 % of all index block reads. Within it, measured per index
over 25 s from `pg_statio_user_indexes`:

| index | blocks read | size | `idx_scan` |
|---|---|---|---|
| `systems_name_trgm` | **420** | 206 MB | 10 |
| `systems_position_idx` | 78 | 498 MB | **0** |
| `systems_name` | 41 | 162 MB | 771,441 |
| `systems_pkey` | 34 | 82 MB | 28,670,811 |
| `systems_updated_at` | 1 | 132 MB | 43,846 |
| `systems_received_at` | 0 | 142 MB | 25,803 |

`systems_name_trgm` is 73 % of it. It is a GIN trigram index with
**`fastupdate=off`**, so every write to `name` pushes each trigram straight
into the main entry tree — roughly 20–25 random descents per row, into an
index that cannot stay cached at 128 MB.

`fastupdate=on` would batch those into the pending list and is **not** the
answer: it moves the cost onto one unlucky insert (or onto autovacuum) when
the list flushes, and the whole point of `fastupdate=off` here is that
ingestion must not stall and back up the receiver queue. Predictable latency
was bought on purpose. Leave it.

`systems_position_idx` at 498 MB with `idx_scan = 0`, and the trigram index
at 10 scans, are cost-only *today* only because no readers are wired up yet.
They are the reason the index set exists. Leave them too.

## The real cause: every upsert is non-HOT

### What HOT is

Postgres never updates a row in place. An `UPDATE` writes a new row version
elsewhere and marks the old one dead. Indexes store physical pointers
(`ctid` — page and line number), so a version at a new location normally
means **every index on the table takes a new entry**. Six indexes, six
insertions, per update.

Heap-Only Tuple is the escape hatch. If both

1. no **indexed** column changed **value**, and
2. the new version fits on the **same heap page**,

then the new version is chained off the old one inside that page and **no
index is touched**. The existing entries keep pointing at the original line
pointer, which becomes a redirect. HOT chains are also pruned
opportunistically whenever the page is read, so dead versions are reclaimed
without waiting for autovacuum.

The HOT-blocking set is every column referenced by any non-summarizing
index, **including index predicates**. `systems_received_at` is
`(received_at DESC) WHERE received_at IS NOT NULL`, so `received_at` is in
that set as both key and predicate.

### Why ours is always blocked

`galos_db/src/systems/create.rs` writes
`received_at = clock_timestamp() AT TIME ZONE 'utc'` unconditionally from
three paths — the main upsert at **:95**, `set_body_counts`'s update at
**:406**, and its insert at **:457**. `clock_timestamp()` yields a new value
on every execution by construction, so condition 1 can never hold.

Measured, replaying the real `ON CONFLICT DO UPDATE` verbatim — same eight
columns, same `CASE`/`GREATEST`/`COALESCE` shape, same six-index set,
`fillfactor = 85`, 20,000 rows, six passes, fed a repeat report that changes
nothing:

| pass | A hot% | A idx growth | B hot% | B idx growth |
|---|---|---|---|---|
| 1 | 0.0 | 3272 kB | 18.4 | 2168 kB |
| 2 | 0.0 | 3568 kB | 33.4 | 1808 kB |
| 3 | 0.0 | 3336 kB | 30.6 | 1768 kB |
| 4 | 0.0 | 3312 kB | 31.1 | 2360 kB |
| 5 | 0.0 | 3128 kB | 31.0 | 1544 kB |
| 6 | 0.0 | 4952 kB | 31.1 | 1544 kB |

| scenario | updates | HOT | hot% | index growth |
|---|---|---|---|---|
| A: as written today | 120,000 | **0** | **0.0 %** | 21,568 kB |
| B: `received_at` left unwritten | 120,000 | 35,113 | 29.3 % | 11,192 kB |

**Zero out of 120,000.** Not low — impossible.

The detail that matters for any fix: the `CASE … ELSE systems.name END`
clauses are **not** the problem. HOT compares old and new *values*, not
whether a column appears in `SET`. Scenario B writes `name`, `position`,
`updated_at` and `updated_by` back on every row and still earns 29–31 % HOT.
`received_at` is the sole blocker, and the upsert is otherwise already
shaped correctly.

Production confirms it: `systems` has 8,406,982 updates against 1,659,404
HOT, **19.7 %**. Those HOT updates are not coming from the upsert at all —
the only HOT-capable write path is `set_primary_star_class` at **:508**,
which touches one non-indexed column behind an `IS DISTINCT FROM` guard.

### What it costs

Per repeat report, a non-HOT update inserts into all six indexes — 1221 MB
of index on a 552 MB heap:

- ~25 random trigram descents into `systems_name_trgm` (206 MB GIN)
- a GiST descent into `systems_position_idx` (498 MB)
- four btree insertions

Which is exactly the read profile in the table above. Every one of those
dead index entries then has to be collected by autovacuum later, so it is
paid for twice.

### The fix, keeping the semantics

The comment at create.rs:88-95 is explicit that `received_at` is
unconditional *on purpose*: a report that changes nothing still has to say
it arrived, because that is the feed cursor, and it is in UTC because
consumers keep their cursor in UTC and the two have to be one clock. So
making it conditional breaks a stated contract and is the wrong fix.

Move the arrival clock off the wide row instead:

```sql
CREATE TABLE system_receipts (
    address     bigint PRIMARY KEY REFERENCES systems(address) ON DELETE CASCADE,
    received_at timestamp NOT NULL
) WITH (fillfactor = 70);
CREATE INDEX system_receipts_received_at ON system_receipts (received_at DESC);
```

Then drop `received_at` and `systems_received_at` from `systems`. The
unconditional write becomes a second upsert into a ~16-byte-per-row table
whose single index is tens of MB and stays cache-resident even at 128 MB
`shared_buffers`. `systems` is then only re-indexed when a report genuinely
changes something, and repeat reports — nearly all of them — go HOT.

Semantics survive intact: arrival still recorded unconditionally, still UTC,
still one clock. `systems_received_at`'s 25,803 scans move to the side
table's index, where `ORDER BY received_at DESC LIMIT n` is cheaper than it
is now.

One prerequisite for the full win: `systems` is at **default
`fillfactor = 100`** — its `reloptions` is null, where `stars` and `bodies`
at least carry `autovacuum_vacuum_insert_scale_factor = 0.02`. HOT needs
free space on the page, condition 2, and at 100 a freshly written page has
none. `ALTER TABLE systems SET (fillfactor = 85)` goes with the change; new
pages honour it at once, existing ones as vacuum frees space. The 29–31 %
above is itself capped by that fillfactor *and* by the test rewriting all
20,000 rows per pass; a real feed touches a subset, so the practical ceiling
is higher.

## Secondary: delete-the-whole-list churn

`outfitting/create.rs:41` and `markets/create.rs:161` delete every row for a
market and re-insert the list. `bodies/create.rs:212` does the same for
`body_materials`. The resulting churn:

| table | inserts | deletes | autovacuums | total size |
|---|---|---|---|---|
| `outfitting` | 113,372,584 | 106,906,277 | 424 | 2721 MB |
| `commodities` | 71,458,741 | 67,924,833 | 391 | 1854 MB |
| `body_materials` | 11,356,813 | 4,060,077 | 46 | 2144 MB |

Each `outfitting` pass scans 2721 MB through 128 MB of buffers. One ran
23:46 → 23:49:49 while this was being looked into, which is the kind of
burst that makes read activity look spiky and unexplained. Raising
`shared_buffers` blunts this; a diff-the-list-instead-of-replacing-it change
would remove it, but that is a bigger design question than this file.

## Order of work, when it happens

1. `shared_buffers` 128 MB → 2–4 GB. No code change, biggest immediate win,
   and reads-equal-evictions is the evidence.
2. `system_receipts` side table + `fillfactor = 85` on `systems`. Unblocks
   HOT, which is what actually removes the reads rather than caching them.
3. Leave `fastupdate=off` and leave the unused indexes alone. Both are
   deliberate.
4. Diffing markets/outfitting instead of replacing them, if autovacuum on
   those two is still the loudest thing after 1 and 2.

## Reproducing any of this

Rate and attribution, no extension needed:

```sql
-- read/evict ratio by backend type, sample twice and diff
SELECT backend_type, object, context, reads, read_bytes, writes, evictions
FROM pg_stat_io WHERE reads > 0 OR writes > 0;

-- which index is being read, sample twice and diff
SELECT indexrelname, idx_blks_read
FROM pg_statio_user_indexes WHERE relname = 'systems';

-- HOT ratio per table
SELECT relname, n_tup_upd, n_tup_hot_upd,
       round(100.0 * n_tup_hot_upd / nullif(n_tup_upd, 0), 1) AS hot_pct
FROM pg_stat_user_tables ORDER BY n_tup_upd DESC;
```

Match backends to processes by client port rather than assuming —
`pg_stat_activity.client_port`, then `lsof -nP -iTCP:<port>`. That is what
turned up rust-analyzer.

For the HOT experiment: build a table with the same six indexes, seed it,
`VACUUM`, then run the upsert repeatedly with and without the `received_at`
assignment. Each pass must be **its own transaction** — inside one
transaction the xmin horizon prevents pruning and every pass looks non-HOT —
and `pg_stat_force_next_flush()` only reflects committed statements, so the
before/after marks have to be separate statements too. Both mistakes were
made on the way to the numbers above.
