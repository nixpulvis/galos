CREATE TABLE bodies (
    address         bigserial  PRIMARY KEY,
    system_address  bigint     REFERENCES systems  NOT NULL,
    id              smallint   NOT NULL,
    name            varchar    NOT NULL,


    /* TODO: prospective */
    /* distance_from_arrival: f64, */


    updated_at  timestamp   NOT NULL
);
