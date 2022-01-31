CREATE TABLE bodies (
    address         bigserial  PRIMARY KEY,
    system_address  bigint     REFERENCES systems  NOT NULL,
    name            varchar    NOT NULL,
    id              smallint   NOT NULL,
    parent_id       smallint,
    updated_at      timestamp  NOT NULL,
    UNIQUE (system_address, id),
    UNIQUE (system_address, name)
);
