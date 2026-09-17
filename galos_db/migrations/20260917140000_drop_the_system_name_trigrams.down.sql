-- Put the trigram index back, as `20260804140000` first built it.
--
-- `fastupdate=off` is how the table carries it: a GIN pending list is
-- cheaper to insert into and turns every read into a scan of whatever has
-- not been merged, which on a feed that writes constantly is every read.
-- Built without `CONCURRENTLY`, since a migration runs in a transaction;
-- over a galaxy this takes the table for the duration.
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX IF NOT EXISTS systems_name_trgm ON systems
    USING gin (name gin_trgm_ops) WITH (fastupdate = off);
