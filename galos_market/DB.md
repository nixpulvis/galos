# Database changes this crate would like

`galos_market` reads and never writes. It creates no table, runs no migration
and touches no schema. This file is what it would ask of the database if it
were allowed to, what each request is worth in measured time, and how to
apply any of it by hand in the meantime.

Numbers were taken on a development database on 2026-08-06, against 2,578,316
commodity rows across 14,990 markets, while the EDDN listener was running.
They move. The shapes do not.

## Why none of it is applied

A migration lives in `galos_db/migrations`, which is tracked, and this crate
is arranged to leave the repository alone: no workspace member, no entry in
the root lock file, nothing in `.sqlx`. A schema change would be the one
thing that broke that.

So everything below is either applied by hand to a development database, or
turned into a migration when these panes move into the map and the reads move
to `galos_db::markets`. Nothing here is required. Every search works without
all of it, and the times in the last column are what they cost as things
stand.

## What the searches ask for

| Search | How it reaches the rows | Now |
| --- | --- | --- |
| `Comparison::fetch_all` | primary key, both markets named | 12ms |
| `Quote::fetch_all` | sequential scan, filtered by name | 80ms |
| `Trade::from_market` | primary key for the source, then by name | 330ms |
| `Trade::near` | primary key once per market in reach | 300ms at 25ly, 2.4s at 100ly |
| `Trade::anywhere` | two sequential scans and a hash aggregate | 1.3s |
| `Summary::fetch_all` | one sequential scan and a hash aggregate | 200ms |

## Wanted: two partial indexes, for the searches that stand somewhere

`Trade::near` is the one worth spending on, because it is the question a
trader actually has and the only one whose cost grows with how much of the
galaxy is populated near them.

It is already using an index. That is not the problem:

```
Bitmap Heap Scan on commodities c  (actual rows=13 loops=3686)
  Recheck Cond: (market_id = n.id)
  Filter: ((stock >= 100) AND (buy_price > 0) AND (listed_at > ...))
  Rows Removed by Filter: 198
  Heap Blocks: exact=17695
  Buffers: shared hit=13789 read=22442
  ->  Bitmap Index Scan on commodities_pkey  (actual rows=218 loops=3686)
```

The primary key is `(market_id, name)`, so finding a market's rows is cheap.
What is not cheap is what follows: for each of 3,686 markets in reach it
fetches 218 rows from the heap and discards 198 of them. A market carries 189
commodities on average and only about 13 of them are stocked, so nine rows in
ten are read to be thrown away, twice over, once per side of the run.

An index on the same key, holding only the rows a run can be made from:

```sql
CREATE INDEX CONCURRENTLY commodities_buyable
    ON commodities (market_id) INCLUDE (name, buy_price, stock, listed_at)
 WHERE stock > 0 AND buy_price > 0;

CREATE INDEX CONCURRENTLY commodities_sellable
    ON commodities (market_id) INCLUDE (name, sell_price, demand, listed_at)
 WHERE demand > 0 AND sell_price > 0;
```

Two things are going on there.

**The predicate** is what does the work. 288,094 rows of 2,578,468 are
buyable and 1,080,739 are sellable, so the buy side index holds one row in
nine. The scan visits 13 entries per market instead of 218.

The predicate is a constant and not the filter the user set. Postgres can
prove that `stock >= 100` implies `stock > 0`, so the index serves whatever
the knob is turned to. `listed_at` cannot be treated the same way: a fixed
date in a predicate ages out, so freshness stays a filter, applied to a ninth
as many rows.

**The `INCLUDE` columns** are everything the two queries select, so the scan
need not visit the heap at all. That one is less certain than it looks, and
the next section is why.

Expect roughly 15MB for the buy index and 60MB for the sell one, against a
288MB table and a 244MB primary key.

To undo, one statement each, since `CONCURRENTLY` takes a single index at a
time:

```sql
DROP INDEX CONCURRENTLY commodities_buyable;
DROP INDEX CONCURRENTLY commodities_sellable;
```

To tell whether it worked, run `EXPLAIN (ANALYZE, BUFFERS)` on a `near`
search before and after and read two numbers: `Rows Removed by Filter`, which
should fall from ~198 to near zero, and `Buffers: read=`, which is where the
seconds are.

## Wanted: autovacuum that keeps up with the churn

`commodities` carries 453,460 dead tuples against 2,578,316 live, which is
better than one row in six.

That is not neglect, it is the write pattern. `Market::from_journal` clears a
market and writes it again for every message that names it:

```sql
DELETE FROM commodities WHERE market_id = $1
```

then one `INSERT` per commodity. So a market seen twice with nothing changed
still leaves 189 dead rows behind, and EDDN carries around 31 messages a
second.

It costs twice. The table and its primary key are 288MB and 244MB, most of
which is space no longer holding anything. And an index-only scan needs the
visibility map to say a page is all visible, which on a page that is
rewritten every few minutes it will not, so the `INCLUDE` columns above will
often be read from the heap anyway.

The default `autovacuum_vacuum_scale_factor` of 0.2 waits for a fifth of the
table, half a million rows, before it starts:

```sql
ALTER TABLE commodities SET (
    autovacuum_vacuum_scale_factor = 0.02,
    autovacuum_analyze_scale_factor = 0.01
);
```

Worth doing whether or not the indexes are added, and worth doing first:
a `REINDEX INDEX CONCURRENTLY commodities_pkey` afterwards will say how much
of that 244MB was bloat rather than data.

The root of it is not here. One `INSERT` naming every commodity at once,
which `galos_db/src/markets/create.rs` already carries a TODO for, would cut
the statements per message from several hundred to one, and an upsert that
leaves unchanged rows alone would cut the dead tuples to the rows that
actually moved. Both are `galos_db`'s to make, not this crate's.

## Not yet: indexes keyed by commodity name

`Quote::fetch_all` asks `WHERE name = LOWER($1)` and `Trade::from_market`
joins on `dst.name = src.name`. Neither has an index to use, since `name` is
the second column of the primary key and not the first, so both fall back to
scanning.

```sql
CREATE INDEX CONCURRENTLY commodities_by_name ON commodities (name);
CREATE INDEX CONCURRENTLY commodities_wanted_by_name
    ON commodities (name, sell_price DESC) WHERE demand > 0;
```

Held off because neither search is slow: 80ms and 330ms, both against a warm
cache, and the second one only ever looks at the hundred commodities one
market sells. An index that is not needed is still written on every insert,
and this table is written constantly.

Add them when a name lookup shows up in a plan as the reason something is
slow, and not before.

## What no index answers

`Summary::fetch_all` and `Trade::anywhere` read every commodity row by
design. One is the mean of every price in the galaxy and the other is the
best price anywhere for each of four hundred commodities. There is no
predicate to be selective about, so no index changes the shape of either, and
both are already fast because a hash aggregate does not sort.

Where those two got cheap was in how they were written, not in what they were
given to read. Both are recorded in `PLAN-market-ui.md`: `DISTINCT ON` over a
million rows spilled 20MB to disk and took 14 seconds where `max() GROUP BY`
takes one, and joining what a bubble sells against what it buys was 16
seconds where two passes and a join of four hundred rows against four hundred
is 300ms.

That is the order to work in. The plans are worth reading before the schema
is changed, because twice now they have said the query was wrong rather than
the database.

## What the planner gets wrong

The set of markets in reach is estimated at 11 rows and is 3,686:

```
CTE Scan on near n  (cost=0.00..0.22 rows=11) (actual rows=3686 loops=1)
```

A spatial predicate against a subquery is not something it can estimate, and
being out by 335 times is how a nested loop gets chosen where a hash join
belongs. It has not picked a bad plan here yet. If `near` ever goes strange
at a radius that ought to be easy, this is the first thing to look at, and
`ANALYZE` will not fix it.

## Not schema at all

Two things worth knowing before anyone proposes a column for them.

**Fleet carriers** are told apart by market id, in the range
`3_700_000_000..3_716_000_000`. Not by station type: a market message names a
station without saying what kind it is, so most stations on record have no
type and most carriers are among them. The id range separates them exactly on
this database, all 1,269 known carriers inside it and everything known to be
anything else outside. A generated column would only be caching a constant.

**766 markets of 14,990 name a system that is not on record**, so they have
no address and no position. They are ranked with everything else and simply
have no distance to give, which is what the `?` in the distance column means.
Nothing to fix in the schema; the system arrives later or it does not.

## When this graduates

The reads move to `galos_db::markets` and the queries become `sqlx::query!`
again, checked at compile time with their offline data in the tracked
`.sqlx`. At that point anything above that is still wanted becomes a
migration under `galos_db/migrations`, named the way the others are:

```
20260806HHMMSS_index_commodities_by_market.up.sql
20260806HHMMSS_index_commodities_by_market.down.sql
```

`CONCURRENTLY` cannot run inside a transaction, and sqlx wraps each migration
in one. Either drop the keyword and take the lock, which on a development
database is a few seconds and on a live one is as long as the index takes to
build, or make `-- no-transaction` the first line of the file. sqlx tests
that with `starts_with`, so it has to be the first line and not merely near
the top.
