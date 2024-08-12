CREATE EXTENSION postgis;
CREATE EXTENSION postgis_topology;

/* NOTE: The anarchy security level seems to be equivlent to NULL and will be
 * mapped above the database to save storage space. */
CREATE TYPE Security AS ENUM (
    'Low',
    'Medium',
    'High'
);

CREATE TYPE Government AS ENUM (
    'Anarchy',
    'Carrier',
    'Communism',
    'Confederacy',
    'Cooperative',
    'Corporate',
    'Democracy',
    'Dictatorship',
    'Engineer',
    'Feudal',
    'Patronage',
    'Prison',
    'PrisonColony',
    'Theocracy'
);


CREATE TYPE Allegiance AS ENUM (
    'Alliance',
    'Empire',
    'Federation',
    'Guardian',
    'Independent',
    'PilotsFederation',
    'PlayerPilots',
    'Thargoid'
);

CREATE TYPE Economy AS ENUM (
    'Agriculture',
    'Carrier',
    'Colony',
    'Extraction',
    'HighTech',
    'Industrial',
    'Military',
    'Prison',
    'Refinery',
    'Service',
    'Terraforming',
    'Tourism',
    'Undefined'
);

CREATE TABLE systems (
    address           bigint           PRIMARY KEY,
    name              varchar          NOT NULL,
    position          geometry(POINTZ) UNIQUE,
    population        bigint,
    security          Security,
    government        Government,
    allegiance        Allegiance,
    primary_economy   Economy,
    secondary_economy Economy,
    updated_at        timestamp        NOT NULL,
    updated_by        varchar          NOT NULL
);

CREATE INDEX ON systems ((upper(name)));
