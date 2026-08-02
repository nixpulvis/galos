ALTER TABLE commodities RENAME TO listings;

ALTER INDEX commodities_pkey RENAME TO listings_pkey;
ALTER TABLE listings
    RENAME CONSTRAINT commodities_market_id_fkey TO listings_market_id_fkey;
