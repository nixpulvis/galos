DROP EXTENSION postgis;
DROP EXTENSION postgis_topology;

DELETE TYPE Security;
DELETE TYPE Government;
DELETE TYPE Allegiance;
DELETE TYPE Economy;

DROP TABLE systems;
DROP INDEX systems_name ON systems;
