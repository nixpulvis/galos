-- no-transaction
-- `stars_arrival` carries everything `stars_reach` did, on a key that begins
-- with the same column, so every lookup the reach path makes is answered by
-- it. Kept in a file of its own because the create before it has to be one
-- statement, and so does this.
DROP INDEX CONCURRENTLY IF EXISTS stars_reach;
