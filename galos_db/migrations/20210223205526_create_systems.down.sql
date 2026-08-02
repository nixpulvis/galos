DROP INDEX systems_name;
DROP TABLE systems;

DROP TYPE Economy;
DROP TYPE Allegiance;
DROP TYPE Government;
DROP TYPE Security;

-- `systems.position` is a PostGIS type, and postgis_topology is built on
-- postgis, so the extensions outlive the table and topology goes first.
DROP EXTENSION postgis_topology;
DROP EXTENSION postgis;

-- postgis_topology installs itself into a schema of its own, which it makes
-- on the way in and leaves standing on the way out. By here it is empty.
DROP SCHEMA topology;
