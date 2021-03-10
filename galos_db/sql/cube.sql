SELECT
    name,
    ST_X(position) AS x,
    ST_Y(position) AS y,
    ST_Z(position) AS z,
    (SELECT ST_3DDistance(origin.position, systems.position)
     FROM systems origin
     WHERE name = 'MELIAE') AS distance
FROM systems
WHERE position &&& (SELECT
    ST_3DMakeBox(
        ST_MakePoint(ST_X(position)+20, ST_Y(position)+20, ST_Z(position)+20),
        ST_MakePoint(ST_X(position)-20, ST_Y(position)-20, ST_Z(position)-20))
    FROM systems
    WHERE name = 'MELIAE')
ORDER BY distance;
