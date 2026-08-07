ALTER TABLE stars RENAME COLUMN temperature TO surface_temperature;

ALTER TABLE bodies RENAME COLUMN temperature TO surface_temperature;
ALTER TABLE bodies RENAME COLUMN gravity TO surface_gravity;
