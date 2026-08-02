-- The trigger executes the function, so the trigger goes first.
DROP TRIGGER system_faction_influence_changes ON system_factions;
DROP FUNCTION insert_system_faction_influences;

DROP TABLE system_faction_influences;
