-- A barycenter is the center of mass a close pair goes round, and the game
-- scans one as a body in its own right. Nothing here held them, so a body that
-- went round one named an id with no row behind it and was drawn at the middle
-- of its system instead. Pluto and Charon, Salacia and Actaea, Orcus and Vanth.
--
-- It sits beside `bodies` rather than in it because it is not one. A
-- `ScanBaryCentre` carries a system, an id and an orbit, and none of the mass,
-- radius, class or surface that every column in `bodies` is declared NOT NULL
-- for. It is a point that things hang off, and nothing about it is drawn.
--
-- There is no parent here, because the event does not carry one. What a
-- barycenter goes round is stated only by its children, each of which names it
-- and then names what is above it, which is what `bodies.parent_ids` keeps.
--
-- The seven numbers of an orbit arrive together or not at all. The one at the
-- root of a multi-star system goes round nothing and carries none of them,
-- which is what the check admits.
CREATE TABLE barycenters (
    system_address  bigint     REFERENCES systems  NOT NULL,
    id              smallint   NOT NULL,
    updated_at      timestamp  NOT NULL,
    updated_by      varchar    NOT NULL,

    semi_major_axis      real,
    eccentricity         real,
    orbital_inclination  real,
    periapsis            real,
    orbital_period       real,
    ascending_node       real,
    mean_anomaly         real,

    PRIMARY KEY (system_address, id),
    CONSTRAINT barycenters_orbit_whole CHECK (
        num_nonnulls(
            semi_major_axis,
            eccentricity,
            orbital_inclination,
            periapsis,
            orbital_period,
            ascending_node,
            mean_anomaly
        ) IN (0, 7)
    )
);
