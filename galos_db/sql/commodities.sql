SELECT
   systems.name,
   stations.name,
   commodities.*
FROM commodities
JOIN markets ON commodities.market_id = markets.id
JOIN stations ON (stations.name = markets.station_name AND
                  stations.system_address = markets.system_address)
JOIN systems ON systems.address = stations.system_address
WHERE commodities.name = 'tritium'
ORDER BY commodities.listed_at DESC;
