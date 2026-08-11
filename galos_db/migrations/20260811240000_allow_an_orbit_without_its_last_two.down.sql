-- Rows holding an orbit short of its last two cannot be held by what this
-- restores, so they are dropped rather than made up, and say how many.
DO $$
DECLARE dropped bigint;
BEGIN
    DELETE FROM bodies WHERE ascending_node IS NULL OR mean_anomaly IS NULL;
    GET DIAGNOSTICS dropped = ROW_COUNT;
    IF dropped > 0 THEN
        RAISE NOTICE 'dropped % body(s) short of an orbit', dropped;
    END IF;
    DELETE FROM rings WHERE ascending_node IS NULL OR mean_anomaly IS NULL;
END $$;

ALTER TABLE bodies
    ALTER COLUMN ascending_node SET NOT NULL,
    ALTER COLUMN mean_anomaly SET NOT NULL;
ALTER TABLE rings
    ALTER COLUMN ascending_node SET NOT NULL,
    ALTER COLUMN mean_anomaly SET NOT NULL;

ALTER TABLE stars DROP CONSTRAINT stars_orbit_whole;
ALTER TABLE stars ADD CONSTRAINT stars_orbit_whole CHECK (
    num_nonnulls(
        semi_major_axis, eccentricity, orbital_inclination, periapsis,
        orbital_period, ascending_node, mean_anomaly
    ) IN (0, 7)
);

ALTER TABLE barycenters DROP CONSTRAINT barycenters_orbit_whole;
ALTER TABLE barycenters ADD CONSTRAINT barycenters_orbit_whole CHECK (
    num_nonnulls(
        semi_major_axis, eccentricity, orbital_inclination, periapsis,
        orbital_period, ascending_node, mean_anomaly
    ) IN (0, 7)
);
