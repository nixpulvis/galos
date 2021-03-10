SELECT
    s.name AS "system",
    s.population,
    s.security,
    s.primary_economy,
    s.secondary_economy,
    s.government,
    s.allegiance,
    s.updated_at AS "system updated_at",
    (SELECT count(*) FROM system_factions sf2
     WHERE sf2.system_address = sf.system_address) AS "#factions",
    state,
    influence,
    happiness,
    sf.updated_at
FROM system_factions sf
JOIN factions f ON f.id = sf.faction_id
JOIN systems s ON s.address = sf.system_address
WHERE f.name = 'New Pilots Initiative'
ORDER BY influence DESC;


/* Many Faction Single System INF trends */
SELECT
    f.name,
    state,
    influence,
    slope
FROM system_factions sf
JOIN factions f ON f.id = sf.faction_id
JOIN systems s ON s.address = sf.system_address
FULL JOIN (
    SELECT
        sfi.faction_id,
        sfi.system_address,
        regr_slope(old_influence, extract(epoch from sfi.old_timestamp))*60*60*24 AS slope
    FROM system_faction_influences sfi
    GROUP BY sfi.system_address, sfi.faction_id
) regr ON regr.faction_id = f.id AND regr.system_address = s.address
WHERE s.name = '10 CANUM VENATICORUM'
ORDER BY influence DESC;


/* Single Faction Many System INF trends */
SELECT
    s.name,
    state,
    influence,
    slope
FROM system_factions sf
JOIN factions f ON f.id = sf.faction_id
JOIN systems s ON s.address = sf.system_address
FULL JOIN (
    SELECT
        sfi.faction_id,
        sfi.system_address,
        regr_slope(old_influence, extract(epoch from sfi.old_timestamp))*60*60*24 AS slope
    FROM system_faction_influences sfi
    GROUP BY sfi.system_address, sfi.faction_id
) regr ON regr.faction_id = f.id AND regr.system_address = s.address
WHERE f.name = 'New Pilots Initiative'
ORDER BY influence DESC;


/* Single System INF history */
SELECT
    f.name,
    state,
    old_influence,
    new_influence,
    sfi.old_timestamp,
    sfi.new_timestamp
FROM system_factions sf
JOIN factions f ON f.id = sf.faction_id
JOIN systems s ON s.address = sf.system_address
JOIN system_faction_influences sfi ON s.address = sfi.system_address AND
                                      f.id = sfi.faction_id
WHERE s.name = '10 CANUM VENATICORUM'
ORDER BY sfi.new_timestamp DESC;


/* Single Faction INF history */
SELECT
    s.name,
    state,
    old_influence,
    new_influence,
    sfi.old_timestamp,
    sfi.new_timestamp
FROM system_factions sf
JOIN factions f ON f.id = sf.faction_id
JOIN systems s ON s.address = sf.system_address
JOIN system_faction_influences sfi ON s.address = sfi.system_address AND
                                      f.id = sfi.faction_id
WHERE f.name = 'New Pilots Initiative'
ORDER BY sfi.new_timestamp DESC;
