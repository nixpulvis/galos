SELECT * FROM (SELECT
    name,
    ST_3DDistance(
        systems.position,
        (SELECT position FROM systems WHERE name = 'SOL')) AS distance
FROM systems) AS systems
WHERE distance <= 50
ORDER BY distance;
