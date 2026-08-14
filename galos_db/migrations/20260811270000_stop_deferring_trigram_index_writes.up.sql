-- Write a system name's trigrams as the name arrives, not in arrears.
--
-- A GIN index defers by default: new entries go to an unordered pending list,
-- and the writer that finds that list over `gin_pending_list_limit`, four
-- megabytes, merges the whole of it into the index. That writer pays for
-- everyone else's. A single row upsert into `systems` was measured at 2.4
-- seconds doing it, against a table where the same statement is ordinarily
-- imperceptible.
--
-- The sync writes continuously and searches rarely, so the deferral buys
-- nothing and costs a stall. It costs more than a stall: `eddn`'s subscriber
-- fills its socket's pipe while it waits, and a full pipe is what
-- `ISSUE-eddn-zmq-assert.md` names as the state libzmq aborts from.
--
-- Every write is now slightly slower and none of them is seconds slower. The
-- index stays: six `ILIKE` searches over system names have nothing else to
-- use, a leading wildcard being no use to a btree.
ALTER INDEX systems_name_trgm SET (fastupdate = off);

-- What has already been deferred, merged now rather than by whoever writes
-- next.
SELECT gin_clean_pending_list('systems_name_trgm');
