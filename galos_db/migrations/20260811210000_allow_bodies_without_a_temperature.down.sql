-- A body with no temperature cannot be held by the column this restores, so
-- any on record is deleted rather than made up, and says how many.
DO $$
DECLARE dropped bigint;
BEGIN
    DELETE FROM bodies WHERE temperature IS NULL;
    GET DIAGNOSTICS dropped = ROW_COUNT;
    IF dropped > 0 THEN
        RAISE NOTICE 'dropped % body(s) with no temperature on record', dropped;
    END IF;
END $$;

ALTER TABLE bodies ALTER COLUMN temperature SET NOT NULL;
