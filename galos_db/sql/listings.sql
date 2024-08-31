SELECT
   listings.name,
   sell_price,
   demand,
   systems.name,
   stations.name,
   stations.landing_pads,
   stations.dist_from_star_ls,
   (SELECT ST_3DDistance(origin.position, systems.position)
      FROM systems origin
      WHERE name = 'MELIAE') AS distance
FROM listings
JOIN markets ON listings.market_id = markets.id
JOIN systems ON systems.address = markets.system_address
JOIN stations ON markets.station_name = stations.name
WHERE listings.name ILIKE 'lowtemperaturediamond'
ORDER BY sell_price DESC;
