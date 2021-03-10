SELECT
    s.name as "system name",
    f.name as "faction name",
    sf.influence,
    sfi.old_influence,
    sfi.old_timestamp
FROM system_faction_influences sfi
JOIN systems s ON s.address = sfi.system_address
JOIN factions f ON f.id = sfi.faction_id
JOIN system_factions sf ON sf.faction_id = sfi.faction_id AND
     sf.system_address = sfi.system_address
ORDER BY sfi.old_timestamp DESC;


SELECT systems.name as "system name", factions.name as "faction name", count(*)
FROM system_faction_influences sfi
JOIN systems ON systems.address = sfi.system_address
JOIN factions ON factions.id = sfi.faction_id
GROUP BY systems.name, factions.id ORDER BY count DESC;


SELECT
    s.name,
    sfi.new_timestamp,
    count(*)
FROM system_faction_influences sfi
JOIN systems s ON s.address = sfi.system_address
JOIN factions f ON f.id = sfi.faction_id
JOIN system_factions sf ON sf.faction_id = sfi.faction_id AND
     sf.system_address = sfi.system_address
GROUP BY s.name, sfi.new_timestamp
ORDER BY sfi.new_timestamp DESC;
