-- A gas giant has no surface, and so has no reading for what is measured at
-- one.
--
-- `create_bodies` declared `atmosphere_type` and `surface_pressure` NOT NULL,
-- which reads as every body having both. A body with a surface does. A gas
-- giant is scanned without either, along with the ice, rock and metal
-- fractions a crust is described by, because none of the three is a thing it
-- has rather than a thing that went unmeasured.
--
-- Until now that cost nothing here, since the journal types required the same
-- fields and so no gas giant ever survived being read well enough to reach
-- this table. There is not one in it. Letting them in is what makes these two
-- columns wrong.
--
-- Nothing already stored is affected. Every row in the table was written from
-- a scan that carried both, so no existing value becomes NULL by this.
ALTER TABLE bodies
    ALTER COLUMN atmosphere_type DROP NOT NULL,
    ALTER COLUMN surface_pressure DROP NOT NULL;
