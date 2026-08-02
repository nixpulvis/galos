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
