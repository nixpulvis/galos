CREATE TYPE Commodity AS (
    name            text,
    mean_price      int,
    buy_price       int,
    sell_price      int,
    demand          int,
    demand_bracket  int,
    stock           int,
    stock_bracket   int
);

CREATE TABLE markets (
    id                 bigint      PRIMARY KEY,
    system_address     bigint      REFERENCES systems  NOT NULL,
    station_name       varchar     NOT NULL,
    updated_at         timestamp   NOT NULL,
    commodities        Commodity[],


    FOREIGN KEY (system_address) REFERENCES systems (address),
    FOREIGN KEY (system_address, station_name) REFERENCES stations (system_address, name)
);
