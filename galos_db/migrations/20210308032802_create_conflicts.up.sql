CREATE TYPE Conflict AS ENUM (
    'War',
    'CivilWar',
    'Election'
);

CREATE TYPE Status AS ENUM (
    'Active',
    'Pending',
    'Recovering'
);

CREATE TABLE conflicts (
    system_address      bigint     NOT NULL REFERENCES systems,
    type                Conflict   NOT NULL,
    status              Status     NOT NULL,
    faction_1_id        integer    NOT NULL REFERENCES factions,
    faction_1_stake     varchar,
    faction_1_won_days  integer    NOT NULL DEFAULT 0,
    faction_2_id        integer    NOT NULL REFERENCES factions,
    faction_2_stake     varchar,
    faction_2_won_days  integer    NOT NULL DEFAULT 0,
    updated_at          timestamp  NOT NULL,

    PRIMARY KEY (system_address, faction_1_id, faction_2_id),
    FOREIGN KEY (system_address, faction_1_id)
    REFERENCES system_factions (system_address, faction_id),
    FOREIGN KEY (system_address, faction_2_id)
    REFERENCES system_factions (system_address, faction_id)
);
