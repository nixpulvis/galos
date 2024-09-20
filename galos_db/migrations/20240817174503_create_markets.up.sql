CREATE TABLE markets (
    id                 bigint      PRIMARY KEY,
    system_address     bigint      REFERENCES systems  NOT NULL,
    station_name       varchar     NOT NULL,
    updated_at         timestamp   NOT NULL,


    FOREIGN KEY (system_address) REFERENCES systems (address),
    FOREIGN KEY (system_address, station_name) REFERENCES stations (system_address, name)
);

CREATE TABLE listings (
    market_id       bigint      REFERENCES markets NOT NULL,
    name            text        NOT NULL,
    mean_price      int         NOT NULL,
    buy_price       int         NOT NULL,
    sell_price      int         NOT NULL,
    demand          int         NOT NULL,
    demand_bracket  int         NOT NULL,
    stock           int         NOT NULL,
    stock_bracket   int         NOT NULL,
    listed_at       timestamp   NOT NULL,

    PRIMARY KEY (market_id, name)
);
