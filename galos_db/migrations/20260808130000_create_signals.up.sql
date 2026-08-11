-- What a system holds that is not a body, and what is written on the ones
-- that are.
--
-- Three events feed these tables and none of them was read. A scan says what
-- a body is made of; none of this says anything about that, which is why none
-- of it belongs in `bodies`.

-- Signals read off a body: geology, biology, what can be landed on and dug up.
--
-- Two events report them. `SAASignalsFound` is the surface scan, mapped from
-- close up; `FSSBodySignals` is what the honk sees of the same body from
-- orbit. They report the same kinds and counts and so share a table, the later
-- of the two winning.
--
-- No foreign key onto `bodies`. Signals routinely arrive for a body that has
-- never been scanned -- the honk finds them before anything identifies what it
-- found them on -- and a key there would throw exactly those away. The system
-- is keyed, and is written from the message before these rows are.
CREATE TABLE body_signals (
    system_address  bigint     REFERENCES systems  NOT NULL,
    body_id         smallint   NOT NULL,
    signal_type     varchar    NOT NULL,
    count           int        NOT NULL,
    updated_at      timestamp  NOT NULL,
    updated_by      varchar    NOT NULL,

    PRIMARY KEY (system_address, body_id, signal_type)
);

-- Signals hanging in a system rather than on a body: stations, megaships,
-- installations, beacons, and the unidentified sources that come and go.
--
-- Keyed by name rather than accumulated, because most of what arrives here is
-- the same handful of signals seen again and again. A row is what is there
-- now, not a log of every time somebody looked.
--
-- Nothing here expires, because EDDN will not say when it would. The journal
-- carries a `TimeRemaining` on a transient source and the schema disallows it,
-- so the age of a row is the only evidence of whether its signal is still
-- there. A station's is good indefinitely; an unidentified source an hour old
-- is almost certainly gone.
CREATE TABLE system_signals (
    system_address    bigint     REFERENCES systems  NOT NULL,
    name              varchar    NOT NULL,
    updated_at        timestamp  NOT NULL,
    updated_by        varchar    NOT NULL,

    signal_type       varchar,
    /* Permanent where true, which is the closest thing to an expiry there is */
    is_station        boolean,
    uss_type          varchar,
    spawning_state    varchar,
    spawning_faction  varchar,
    spawning_power    varchar,
    opposing_power    varchar,
    threat_level      int,

    PRIMARY KEY (system_address, name)
);

-- The codex: a first sighting of a kind of thing, somewhere.
--
-- Keyed on the entry and the system, so a system holds one row per kind of
-- thing found in it however many times it is found. Whether the commander who
-- sent it was first to it is not recorded and cannot be -- the schema
-- disallows `IsNewEntry` as personal data.
--
-- `name` is nullable because the schema does not require it, though in
-- practice it is always sent.
CREATE TABLE codex_entries (
    system_address       bigint     REFERENCES systems  NOT NULL,
    entry_id             bigint     NOT NULL,
    updated_at           timestamp  NOT NULL,
    updated_by           varchar    NOT NULL,

    name                 varchar,
    category             varchar,
    sub_category         varchar,
    region               varchar,
    body_id              smallint,
    body_name            varchar,
    nearest_destination  varchar,
    /* Where on a surface it was found, for the ones found on one */
    latitude             double precision,
    longitude            double precision,

    PRIMARY KEY (system_address, entry_id)
);
