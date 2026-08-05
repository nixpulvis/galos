-- Put it back as `create_factions` wrote it, so that reverting past this
-- migration reaches the state that one's own revert expects to drop.
CREATE UNIQUE INDEX system_factions_join ON system_factions (system_address, faction_id);
