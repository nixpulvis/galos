SELECT
    s.name,
    s.address,
    sf.faction_id,
    sf.influence,
    s.government,
    sf.government,
    s.allegiance,
    sf.allegiance
FROM (
    SELECT system_address, max(influence) as influence
    FROM system_factions GROUP BY system_address) as isf
JOIN system_factions sf ON sf.system_address = isf.system_address AND
                           sf.influence = isf.influence
JOIN systems s ON s.address = sf.system_address
WHERE s.government != sf.government OR
      s.allegiance != s.allegiance
ORDER BY s.address;

