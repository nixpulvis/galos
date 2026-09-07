-- What the flag held cannot be recovered. It described the scan that wrote
-- each row rather than the row, and the scans themselves are not kept, so
-- there is nothing left to read it back from. A `discovered_at` that was
-- written since cannot be turned back into one either: a date says the body
-- was found then, and the flag says only that it had or had not been found by
-- some scan, which is a different claim.
--
-- `false` is a placeholder standing where a reading would go, and not a
-- reading. Every existing row takes it because the column is NOT NULL and
-- something has to be there, then the default goes away again so the column is
-- declared the way the original `CREATE TABLE`s declared it: `boolean NOT
-- NULL`, supplied by whatever writes the row. The column comes back at the end
-- of `rings` and `clusters` rather than ahead of `was_mapped` where it was,
-- since a column cannot be put back in the middle of a table.
ALTER TABLE bodies
    DROP COLUMN discovered_at,
    ADD COLUMN was_discovered boolean NOT NULL DEFAULT false;
ALTER TABLE bodies ALTER COLUMN was_discovered DROP DEFAULT;

ALTER TABLE stars
    DROP COLUMN discovered_at,
    ADD COLUMN was_discovered boolean NOT NULL DEFAULT false;
ALTER TABLE stars ALTER COLUMN was_discovered DROP DEFAULT;

ALTER TABLE rings
    DROP COLUMN discovered_at,
    ADD COLUMN was_discovered boolean NOT NULL DEFAULT false;
ALTER TABLE rings ALTER COLUMN was_discovered DROP DEFAULT;

ALTER TABLE clusters
    DROP COLUMN discovered_at,
    ADD COLUMN was_discovered boolean NOT NULL DEFAULT false;
ALTER TABLE clusters ALTER COLUMN was_discovered DROP DEFAULT;
