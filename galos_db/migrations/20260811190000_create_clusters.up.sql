-- A belt cluster is a stretch of a belt, scanned as a body and measured as
-- nothing. About a quarter of the scans EDDN carries are these.
--
-- It sits beside `bodies` rather than in it for the reason a barycenter does.
-- A cluster's scan carries a name, an id, the ring it lies in and whether it
-- had been found before, and none of the class, mass, radius, gravity or
-- temperature that every one of those columns in `bodies` is declared NOT NULL
-- for. There is no single object there to weigh.
--
-- Rings are not here. A ring is never scanned in its own right; it is reported
-- among the things its parent body carries, which is where it belongs.
--
-- `parent_ids` and `parent_types` are kept as `bodies` keeps them, nearest
-- ancestor first, so a cluster can be walked back to its star the same way.
-- The first parent is always the ring it lies in.
CREATE TABLE clusters (
    system_address  bigint     REFERENCES systems  NOT NULL,
    id              smallint   NOT NULL,
    name            varchar    NOT NULL,
    updated_at      timestamp  NOT NULL,
    updated_by      varchar    NOT NULL,

    distance_from_arrival  real,
    was_discovered         boolean   NOT NULL,
    was_mapped             boolean   NOT NULL,
    parent_ids             smallint[],
    parent_types           varchar[],

    PRIMARY KEY (system_address, id)
);
