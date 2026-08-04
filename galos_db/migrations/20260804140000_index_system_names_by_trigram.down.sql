DROP INDEX systems_name_trgm;

-- The index is the only thing that asked for it, so it goes back with it and
-- leaves the database as it was found.
DROP EXTENSION pg_trgm;
