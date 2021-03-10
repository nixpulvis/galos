CREATE TABLE factions (
    id    serial   PRIMARY KEY,
    name  varchar  NOT NULL
);

CREATE UNIQUE INDEX ON factions ((lower(name)));

CREATE TYPE State AS ENUM (
    'Blight',
    'Boom',
    'Bust',
    'CivilLiberty',
    'CivilUnrest',
    'CivilWar',
    'ColdWar',
    'Colonisation',
    'Drought',
    'Election',
    'Expansion',
    'Famine',
    'HistoricEvent',
    'InfrastructureFailure',
    'Investment',
    'Lockdown',
    'NaturalDisaster',
    'Outbreak',
    'PirateAttack',
    'PublicHoliday',
    'Retreat',
    'Revolution',
    'TechnologicalLeap',
    'Terrorism',
    'TradeWar',
    'War'
);

CREATE TYPE Happiness AS ENUM (
    'Elated',
    'Happy',
    'Discontented',
    'Unhappy',
    'Despondent'
);

CREATE TABLE system_factions (
    system_address  bigint      REFERENCES systems NOT NULL,
    faction_id      integer     REFERENCES factions NOT NULL,
    updated_at      timestamp   NOT NULL,
    state           State,
    influence       real        NOT NULL,
    /* TODO: What of these are actually static to a faction? */
    happiness       Happiness,
    government      Government  NOT NULL,
    allegiance      Allegiance  NOT NULL,

    PRIMARY KEY (system_address, faction_id)
);

CREATE UNIQUE INDEX ON system_factions (system_address, faction_id);
