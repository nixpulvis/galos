-- Keep the visibility map current enough for the reach indexes to be worth it.
--
-- An index-only scan still has to know a row is visible, and it skips the heap
-- for that only on pages the visibility map calls wholly visible. `VACUUM` is
-- what writes that map, so the three indexes added just before this are worth
-- what they are worth only while a vacuum has been by recently.
--
-- The feed inserts into these three and almost never updates them, so the
-- ordinary trigger, a fifth of the rows dead, does not fire on them at all.
-- What does fire is the insert trigger, and it stands at a fifth of the rows
-- inserted: three hundred thousand rows on the million and a half bodies on
-- record. Until it fires, every page holding one of those rows is unmarked,
-- and the scan goes to the heap for each row on one.
--
-- A fiftieth instead, so a vacuum comes around every thirty thousand rows. It
-- does not read the whole table for that: a vacuum skips the pages the map
-- already vouches for, so what it costs tracks what has arrived since the last
-- one rather than what the table holds.
ALTER TABLE bodies SET (autovacuum_vacuum_insert_scale_factor = 0.02);

ALTER TABLE stars SET (autovacuum_vacuum_insert_scale_factor = 0.02);

ALTER TABLE barycenters SET (autovacuum_vacuum_insert_scale_factor = 0.02);
