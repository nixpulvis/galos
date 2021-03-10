/* TODO: add powers */
SELECT
    power,
    power_state,
    SUM(population) as population,
    COUNT(*),
    SUM(population) / COUNT(*) as average
FROM systems
GROUP BY power, power_state
ORDER BY ratio DESC;
