-- Going back means dropping whatever the looser schema let in and the
-- stricter one cannot hold. That is all the same set of rows: market data
-- knows only that a station is there and which system it claims to be in,
-- so none of it could have been recorded before this migration.
--
-- A market with no system was never representable, and neither was a
-- station with no type. Both go, along with the listings hanging off them,
-- deleted in the order the foreign keys allow: listings, then markets, then
-- stations.

DELETE FROM listings WHERE market_id IN (
    SELECT m.id
      FROM markets m
      LEFT JOIN stations s
        ON s.system_address = m.system_address AND s.name = m.station_name
     WHERE m.system_address IS NULL OR s.ty IS NULL
);

DELETE FROM markets AS m
 WHERE m.system_address IS NULL
    OR EXISTS (
        SELECT 1
          FROM stations s
         WHERE s.system_address = m.system_address
           AND s.name = m.station_name
           AND s.ty IS NULL
    );

DELETE FROM stations WHERE ty IS NULL;

DROP INDEX markets_waiting_on_system;
ALTER TABLE markets DROP COLUMN system_name;
ALTER TABLE markets ALTER COLUMN system_address SET NOT NULL;
ALTER TABLE stations ALTER COLUMN ty SET NOT NULL;
