SELECT
   systems.name,
   stations.name,
   listings.*
FROM listings
JOIN markets ON listings.market_id = markets.id
JOIN stations ON (stations.name = markets.station_name AND
                  stations.system_address = markets.system_address)
JOIN systems ON systems.address = stations.system_address
WHERE listings.name = 'tritium'
ORDER BY listings.listed_at DESC;
