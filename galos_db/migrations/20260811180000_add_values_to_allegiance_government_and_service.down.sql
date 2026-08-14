-- Nothing to undo.
--
-- Postgres has no `ALTER TYPE ... DROP VALUE`. Removing one means building a
-- new type without it, rewriting every column that uses it, and dropping the
-- old, which would fail anyway for any row already holding the value. An
-- unused label costs nothing, so it stays.
SELECT 1;
