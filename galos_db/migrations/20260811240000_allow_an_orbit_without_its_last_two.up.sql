-- An orbit may arrive without its ascending node or its mean anomaly.
--
-- The game sends all seven. Not every uploader that passes a scan on does:
-- `Stellar Data Relay` sends five, and a message carrying five failed to read
-- and was dropped whole. What the five say is the path; what the other two add
-- is where the thing stood along it when it was looked at.
--
-- So the two stop being required, and the checks that said the seven were
-- written together now say it of the five. `bodies` and `rings` declared all
-- seven NOT NULL; `stars` and `barycenters` held them nullable already, for the
-- separate reason that a primary star goes round nothing.
ALTER TABLE bodies
    ALTER COLUMN ascending_node DROP NOT NULL,
    ALTER COLUMN mean_anomaly DROP NOT NULL;
ALTER TABLE rings
    ALTER COLUMN ascending_node DROP NOT NULL,
    ALTER COLUMN mean_anomaly DROP NOT NULL;

ALTER TABLE stars DROP CONSTRAINT stars_orbit_whole;
ALTER TABLE stars ADD CONSTRAINT stars_orbit_whole CHECK (
    num_nonnulls(
        semi_major_axis, eccentricity, orbital_inclination, periapsis,
        orbital_period
    ) IN (0, 5)
);

ALTER TABLE barycenters DROP CONSTRAINT barycenters_orbit_whole;
ALTER TABLE barycenters ADD CONSTRAINT barycenters_orbit_whole CHECK (
    num_nonnulls(
        semi_major_axis, eccentricity, orbital_inclination, periapsis,
        orbital_period
    ) IN (0, 5)
);
