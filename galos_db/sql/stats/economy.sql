SELECT
    primary_economy,
    SUM(population) as population,
    COUNT(*) as count,
    SUM(population) / COUNT(*) as average
FROM systems
GROUP BY primary_economy
ORDER BY population DESC;
