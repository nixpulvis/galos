-- Put both columns back as `create_bodies` declared them.
--
-- A body with nothing recorded for either cannot be carried back across this,
-- there being no value that means the same as saying nothing. Reverting drops
-- those rows rather than inventing a surface for a body that has none, and
-- says how many it dropped.
DO $$
DECLARE
    surfaceless bigint;
BEGIN
    DELETE FROM bodies
    WHERE atmosphere_type IS NULL OR surface_pressure IS NULL;

    GET DIAGNOSTICS surfaceless = ROW_COUNT;
    IF surfaceless > 0 THEN
        RAISE NOTICE 'dropped % bodies with no surface to describe', surfaceless;
    END IF;
END
$$;

ALTER TABLE bodies
    ALTER COLUMN atmosphere_type SET NOT NULL,
    ALTER COLUMN surface_pressure SET NOT NULL;
