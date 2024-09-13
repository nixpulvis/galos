CREATE TABLE stars (
    system_address  bigint     REFERENCES systems  NOT NULL,
    id              smallint   NOT NULL,
    name            varchar    NOT NULL,
    parent_id       smallint,
    updated_at      timestamp  NOT NULL,
    updated_by      varchar    NOT NULL,

    absolute_magnitude        real      NOT NULL,
    age_my                    int       NOT NULL,
    distance_from_arrival_ls  real      NOT NULL,
    luminosity                varchar   NOT NULL,
    star_type                 varchar   NOT NULL,
    stellar_mass              real      NOT NULL,
    subclass                  smallint  NOT NULL,

    ascending_node       real  NOT NULL,
    axial_tilt           real  NOT NULL,
    eccentricity         real  NOT NULL,
    mean_anomaly         real  NOT NULL,
    orbital_inclination  real  NOT NULL,
    orbital_period       real  NOT NULL,
    periapsis            real  NOT NULL,
    radius               real  NOT NULL,
    rotation_period      real  NOT NULL,
    semi_major_axis      real  NOT NULL,
    surface_temperature  real  NOT NULL,

    was_mapped      boolean  NOT NULL,
    was_discovered  boolean  NOT NULL,

    PRIMARY KEY(system_address, id),
    UNIQUE (system_address, name)
);
