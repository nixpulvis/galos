CREATE TABLE system_faction_states (
    system_address  bigint      REFERENCES systems  NOT NULL,
    faction_id      integer     REFERENCES factions NOT NULL,
    state           State       NOT NULL,
    status          Status      NOT NULL,

    PRIMARY KEY (system_address, faction_id, state, status),
    FOREIGN KEY (system_address, faction_id)
    REFERENCES system_factions (system_address, faction_id)
);
