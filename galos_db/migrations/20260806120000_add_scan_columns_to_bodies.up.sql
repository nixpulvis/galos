-- `bodies` kept about two thirds of what a scan says about a body. This is
-- the rest of it, and the ancestry is the part that matters.
--
-- A scan names every ancestor a body has, nearest first, each with the kind of
-- thing it is: Pluto arrives carrying `[{"Null": 31}, {"Star": 0}]`. Only the
-- first of those was kept, as `parent_id`, so the walk back to the star ended
-- at whatever was nearest even where the journal had supplied the whole chain.
-- `parent_ids` and `parent_types` keep it whole and in order, nearest first.
--
-- The kinds are worth as much as the ids. An ancestor that is not on record is
-- placed by measuring from the middle of the system, which is right for the
-- primary star and wrong for anything else. `parent_types` is what tells the
-- two apart: `Null` names a barycenter, `Star` the primary, and anything else
-- a body that ought to be here and is not.
--
-- `parent_id` stays where it is, holding the head of the chain. It is written
-- from the same ancestry in the same statement, so the two cannot disagree.
--
-- Existing rows keep the one ancestor they have. Nothing recorded what kind of
-- thing it was, so `parent_types` is left null for them rather than guessed at.
ALTER TABLE bodies
    ADD COLUMN body_type              varchar,
    ADD COLUMN distance_from_arrival  real,
    ADD COLUMN composition_ice        real,
    ADD COLUMN composition_rock       real,
    ADD COLUMN composition_metal      real,
    ADD COLUMN parent_ids             smallint[],
    ADD COLUMN parent_types           varchar[];

UPDATE bodies SET parent_ids = ARRAY[parent_id] WHERE parent_id IS NOT NULL;

-- The three fractions a crust is described by arrive together with the rest of
-- what is measured at a surface, and a gas giant is scanned without any of
-- them. Rows written before this migration have none, since the fractions were
-- read and dropped rather than stored.
ALTER TABLE bodies
    ADD CONSTRAINT bodies_composition_whole CHECK (
        num_nonnulls(composition_ice, composition_rock, composition_metal)
        IN (0, 3)
    );

-- What a body is made of, which is a list per body rather than a column.
CREATE TABLE body_materials (
    system_address  bigint            NOT NULL,
    body_id         smallint          NOT NULL,
    name            varchar           NOT NULL,
    percent         double precision  NOT NULL,

    PRIMARY KEY (system_address, body_id, name),
    FOREIGN KEY (system_address, body_id) REFERENCES bodies (system_address, id)
        ON DELETE CASCADE
);
