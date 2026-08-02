-- Markets whose system is still unknown cannot be represented once the
-- address is mandatory again, so going back discards them.
DELETE FROM listings WHERE market_id IN (
    SELECT id FROM markets WHERE system_address IS NULL
);
DELETE FROM markets WHERE system_address IS NULL;

DROP INDEX markets_waiting_on_system;
ALTER TABLE markets DROP COLUMN system_name;
ALTER TABLE markets ALTER COLUMN system_address SET NOT NULL;

-- A station recorded from market data has no type, and stationtype has no
-- value meaning "unknown", so there is nothing truthful to put back. Say so
-- rather than letting a constraint violation explain it. Deleting the
-- stations is not done here: markets reference them, so it would take the
-- trade history with it.
DO $$
DECLARE untyped bigint;
BEGIN
    SELECT count(*) INTO untyped FROM stations WHERE ty IS NULL;
    IF untyped > 0 THEN
        RAISE EXCEPTION
            'cannot restore stations.ty NOT NULL: % stations came from market data and have no type',
            untyped
        USING HINT =
            'give them a type, or delete them along with their markets and listings, then revert again';
    END IF;
END $$;

ALTER TABLE stations ALTER COLUMN ty SET NOT NULL;
