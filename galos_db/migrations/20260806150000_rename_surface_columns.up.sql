-- Both are measured at the cloud tops where there is no surface, so a gas
-- giant carries a gravity and a temperature and no surface at all. Naming
-- them for one says the opposite, and the names came from the game rather
-- than from what they hold.
--
-- `surface_pressure` keeps its name and stays where it is. That one really is
-- read at a surface, and is null for a body without one.
ALTER TABLE bodies RENAME COLUMN surface_gravity TO gravity;
ALTER TABLE bodies RENAME COLUMN surface_temperature TO temperature;

-- A star has no surface either.
ALTER TABLE stars RENAME COLUMN surface_temperature TO temperature;
