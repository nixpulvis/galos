-- Searching a system by part of its name reads every row without this.
--
-- `systems_name` is a b-tree over `upper(name)`, which answers a name given
-- in full and nothing else: a b-tree is ordered by whole values, so a pattern
-- with anything before the letters being looked for has no range to descend
-- into. A search box asks exactly that pattern, since the user is part way
-- through typing.
--
-- Trigrams index the letters themselves, three at a time, so a fragment held
-- anywhere in a name is looked up rather than scanned for. Measured over the
-- 284,000 systems on record the day this was written: `%sol%` falls from 30ms
-- to 0.1ms and `%285 sector%` from 57ms to 10ms. A reading rather than a
-- standing figure, and left at what was read: the table has grown since, so
-- the times are what the index was worth against a sky that size, which is
-- what the case for adding it rested on.
--
-- A query of one or two letters still reads every row, having no trigram in
-- it to look up. Nothing to be done about that here, and little reason to:
-- two letters match a third of the systems on record, so the reading is the
-- cheap half of answering it.
-- `articles` created this four years earlier and indexes its bodies through
-- it, so this is asked for only to stand on its own should that table ever
-- go. It is left behind on the way back down for the same reason.
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX systems_name_trgm ON systems USING gin (name gin_trgm_ops);
