/* CREATE TYPE Composition AS ( */
/*     ice    real, */
/*     rock   real, */
/*     metal  real */
/* ); */

CREATE TABLE bodies (
    system_address  bigint     REFERENCES systems  NOT NULL,
    name            varchar    NOT NULL,
    id              smallint   NOT NULL,
    parent_id       smallint,
    updated_at      timestamp  NOT NULL,

    planet_class     varchar  NOT NULL,
    tidal_lock       boolean  NOT NULL,
    landable         boolean  NOT NULL,
    terraform_state  varchar,
    atmosphere       varchar,
    atmosphere_type  varchar  NOT NULL,
    volcanism        varchar,

    mass                 real  NOT NULL,
    radius               real  NOT NULL,
    surface_gravity      real  NOT NULL,
    surface_temperature  real  NOT NULL,
    surface_pressure     real  NOT NULL,
    /* composition          Composition  NOT NULL, */
    semi_major_axis      real  NOT NULL,
    eccentricity         real  NOT NULL,
    orbital_inclination  real  NOT NULL,
    periapsis            real  NOT NULL,
    orbital_period       real  NOT NULL,
    rotation_period      real  NOT NULL,
    axial_tilt           real  NOT NULL,
    ascending_node       real  NOT NULL,
    mean_anomaly         real  NOT NULL,

    was_mapped      boolean  NOT NULL,
    was_discovered  boolean  NOT NULL,

    PRIMARY KEY(system_address, id),
    UNIQUE (system_address, name)
);
