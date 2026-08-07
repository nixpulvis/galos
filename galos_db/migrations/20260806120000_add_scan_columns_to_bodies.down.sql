-- `parent_id` is left alone, holding what it held before: the nearest
-- ancestor. Everything further up is lost, which is the whole of what this
-- migration added.
DROP TABLE body_materials;

ALTER TABLE bodies DROP CONSTRAINT bodies_composition_whole;

ALTER TABLE bodies
    DROP COLUMN body_type,
    DROP COLUMN distance_from_arrival,
    DROP COLUMN composition_ice,
    DROP COLUMN composition_rock,
    DROP COLUMN composition_metal,
    DROP COLUMN parent_ids,
    DROP COLUMN parent_types;
