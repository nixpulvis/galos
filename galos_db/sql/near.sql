EXPLAIN ANALYZE SELECT
    name,
    near.id,
    ST_3DDistance(systems.position, near.position) AS distance
FROM systems
JOIN systems AS near ON ST_3DDWithin(systems.position, near.position, 50)
JOIN identifiers ON near.id = identifiers.system_id
WHERE systems.id = (
    SELECT id FROM systems
    JOIN identifiers ON systems.id = identifiers.system_id
    WHERE name = 'Sol');
ORDER BY distance;
