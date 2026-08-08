-- What a station sells besides commodities.
--
-- Three schemas carry it and none could ever be read: their payloads have no
-- `event` key, so nothing could tell them apart from any other message until
-- the reader started going by the `$schemaRef`. They name a market exactly as
-- a commodity message does -- an id, a station, and a system by name only --
-- so they hang off `markets` beside `commodities`.

-- Modules on sale in an outfitting bay.
--
-- The prices are nullable because two versions of the schema are live and only
-- the newer one carries them. A row from the older says a station sells the
-- module and nothing about what it costs, which is worth having: what is
-- sold where is the question outfitting data is usually asked.
CREATE TABLE outfitting (
    market_id         bigint     REFERENCES markets  NOT NULL,
    /* Symbolic name, e.g. Int_Engine_Size3_Class5_Fast */
    module_name       varchar    NOT NULL,
    buy_price         bigint,
    merc_coins_price  bigint,
    listed_at         timestamp  NOT NULL,

    PRIMARY KEY (market_id, module_name)
);

-- Ships on sale in a shipyard.
--
-- Names only. The schema carries no prices, a ship costing the same
-- everywhere, so there is nothing else to keep.
CREATE TABLE shipyard (
    market_id   bigint     REFERENCES markets  NOT NULL,
    /* Symbolic name, e.g. Federation_Corvette */
    ship_name   varchar    NOT NULL,
    listed_at   timestamp  NOT NULL,

    PRIMARY KEY (market_id, ship_name)
);

-- What a station's black market pays for a commodity.
--
-- Kept apart from `commodities` rather than folded into it, for two reasons.
-- The goods are mostly ones no legal market lists, so there is no row there to
-- fold into. And a commodity message is read as the whole of what a station
-- trades and clears the rows it does not mention, which would wipe these on
-- every legal market update -- a black market is reported one commodity at a
-- time and never says what else is traded.
CREATE TABLE black_market (
    market_id   bigint     REFERENCES markets  NOT NULL,
    name        varchar    NOT NULL,
    sell_price  int        NOT NULL,
    prohibited  boolean    NOT NULL,
    listed_at   timestamp  NOT NULL,

    PRIMARY KEY (market_id, name)
);
