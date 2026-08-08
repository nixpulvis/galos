-- A star with no orbit cannot be held by the columns this restores, so any
-- primary star on record is deleted rather than made up, and says how many.
ALTER TABLE stars
    DROP COLUMN parent_ids,
    DROP COLUMN parent_types;

ALTER TABLE stars DROP CONSTRAINT stars_orbit_whole;

DO $$
DECLARE dropped bigint;
BEGIN
    DELETE FROM stars WHERE semi_major_axis IS NULL;
    GET DIAGNOSTICS dropped = ROW_COUNT;
    IF dropped > 0 THEN
        RAISE NOTICE 'dropped % star(s) that go round nothing', dropped;
    END IF;
END $$;

ALTER TABLE stars
    ALTER COLUMN semi_major_axis SET NOT NULL,
    ALTER COLUMN eccentricity SET NOT NULL,
    ALTER COLUMN orbital_inclination SET NOT NULL,
    ALTER COLUMN periapsis SET NOT NULL,
    ALTER COLUMN orbital_period SET NOT NULL,
    ALTER COLUMN ascending_node SET NOT NULL,
    ALTER COLUMN mean_anomaly SET NOT NULL;
