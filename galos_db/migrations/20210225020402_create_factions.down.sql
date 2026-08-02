-- `system_factions` references `factions` and is typed by both enums, so it
-- comes off before either.
DROP INDEX system_factions_join;
DROP TABLE system_factions;

DROP TYPE Happiness;
DROP TYPE State;

DROP INDEX factions_name;
DROP TABLE factions;
