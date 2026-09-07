-- The indexes first, since dropping a column would take them with it and this
-- says what comes off rather than leaving it to be inferred.
DROP INDEX IF EXISTS systems_received_at;
DROP INDEX IF EXISTS stars_received_at;
DROP INDEX IF EXISTS system_factions_received_at;

ALTER TABLE systems DROP COLUMN received_at;
ALTER TABLE stars DROP COLUMN received_at;
ALTER TABLE system_factions DROP COLUMN received_at;
