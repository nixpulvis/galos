-- A system name is uppercase, and is now both checked and indexed as itself.
--
-- Every write already uppercases it and all 1,067,631 rows are uppercase, so
-- this states an invariant the data has always had rather than changing any of
-- it. `markets.system_name` is written the same way and matched against
-- `UPPER($1)` when a waiting market is adopted, so it says the same thing.
--
-- `systems_name` indexed `upper(name)` instead. Nothing queries that
-- expression: a lookup by name uppercases in Rust and asks for `name = $1`,
-- which cannot use an index on `upper(name)` and fell to the trigram index
-- meant for `ILIKE`, at fifty times the cost and over a second per call on a
-- path every market message takes. Indexing the column plainly is what the
-- predicate has always wanted, and is what `markets_waiting_on_system` already
-- does.
ALTER TABLE systems
    ADD CONSTRAINT systems_name_uppercase CHECK (name = upper(name));
ALTER TABLE markets
    ADD CONSTRAINT markets_system_name_uppercase
    CHECK (system_name = upper(system_name));

DROP INDEX systems_name;
CREATE INDEX systems_name ON systems USING btree (name);
