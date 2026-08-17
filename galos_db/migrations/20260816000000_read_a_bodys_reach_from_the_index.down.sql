-- no-transaction
-- Concurrently for the same reason it was built that way: dropping an index
-- takes a lock against the table that writes wait behind, and the sync writing
-- to `bodies` does not wait well.
DROP INDEX CONCURRENTLY IF EXISTS bodies_reach;
