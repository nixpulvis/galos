SELECT
    s.name,
    f1.name AS "faction_1",
    faction_1_stake,
    faction_1_won_days,
    f2.name AS "faction_2",
    faction_2_stake,
    faction_2_won_days,
    c.updated_at
FROM conflicts c
JOIN systems s ON system_address = s.address
JOIN factions f1 on faction_1_id = f1.id
JOIN factions f2 ON faction_2_id = f2.id
WHERE f1.name = 'New Pilots Initiative'
   OR f2.name = 'New Pilots Initiative';
