-- A system's primary star goes round nothing and names no ancestor, so a scan
-- of one carries neither an orbit nor a parent. `create_stars` declared all
-- seven orbital columns NOT NULL, which reads as every star being in orbit
-- about something. Only the ones that are not primary are.
--
-- Until now this cost nothing, because no star has ever reached this table.
-- `Star` read the class from `StarClass` and the game writes `StarType`, and
-- read the distance from `DistanceFromArrivalLs` where the game writes
-- `DistanceFromArrivalLS`, so every star scan failed to deserialize and the
-- untagged `ScanTarget` dropped the entry in silence. The table has been empty
-- since it was created. Fixing the two names is what makes these columns wrong.
--
-- Nothing already stored is affected, there being nothing already stored.
ALTER TABLE stars
    ALTER COLUMN semi_major_axis DROP NOT NULL,
    ALTER COLUMN eccentricity DROP NOT NULL,
    ALTER COLUMN orbital_inclination DROP NOT NULL,
    ALTER COLUMN periapsis DROP NOT NULL,
    ALTER COLUMN orbital_period DROP NOT NULL,
    ALTER COLUMN ascending_node DROP NOT NULL,
    ALTER COLUMN mean_anomaly DROP NOT NULL,
    ADD CONSTRAINT stars_orbit_whole CHECK (
        num_nonnulls(
            semi_major_axis,
            eccentricity,
            orbital_inclination,
            periapsis,
            orbital_period,
            ascending_node,
            mean_anomaly
        ) IN (0, 7)
    );

-- The whole ancestry, as `bodies` keeps it and for the same reason: a star in
-- a multi-star system goes round a barycenter, and the walk back to the middle
-- of the system has to be able to step over one that is not on record.
ALTER TABLE stars
    ADD COLUMN parent_ids    smallint[],
    ADD COLUMN parent_types  varchar[];
