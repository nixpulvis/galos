-- A ring, scanned in its own right rather than as something a body carries.
--
-- A body's own scan lists the rings it has, with what each is made of and how
-- wide it is. This is the other way the game reports one, about six times in a
-- thousand scans: as a body in the numbering, going round the body it belongs
-- to, carrying an orbit and nothing else. None of the class, mass, radius or
-- temperature `bodies` is declared NOT NULL for, and none of it exists to
-- measure.
--
-- Beside `clusters` rather than in it, because the two are not the same thing
-- and the orbit is what says so. A belt cluster lies in a ring and carries no
-- orbit at all; a ring goes round a planet or a star and always carries one.
-- That is what tells them apart when a scan is read, so the seven columns are
-- NOT NULL here: a row without them is not a ring.
CREATE TABLE rings (
    system_address  bigint     REFERENCES systems  NOT NULL,
    id              smallint   NOT NULL,
    name            varchar    NOT NULL,
    updated_at      timestamp  NOT NULL,
    updated_by      varchar    NOT NULL,

    distance_from_arrival  real,
    was_discovered         boolean   NOT NULL,
    was_mapped             boolean   NOT NULL,
    parent_ids             smallint[],
    parent_types           varchar[],

    semi_major_axis      real  NOT NULL,
    eccentricity         real  NOT NULL,
    orbital_inclination  real  NOT NULL,
    periapsis            real  NOT NULL,
    orbital_period       real  NOT NULL,
    ascending_node       real  NOT NULL,
    mean_anomaly         real  NOT NULL,

    PRIMARY KEY (system_address, id)
);

-- The rings that were filed as belt clusters, when a cluster was whatever
-- carried neither a star class nor a planet class.
--
-- Deleted rather than moved. `clusters` has no orbit to carry across and a ring
-- is nothing without one, so there is no row here that could be made into a
-- valid one. The feed rescans constantly and will write them again as rings.
DELETE FROM clusters WHERE name LIKE '% Ring';
