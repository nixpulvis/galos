-- These rows are one commodity traded at one market, which is the word both
-- the game's journal and EDDN use for them. `listings` is EDDB's word, from
-- the `listings.csv` of its data dumps, and EDDB is not among the sources
-- that write here.

ALTER TABLE listings RENAME TO commodities;

-- A table rename leaves the index and the constraint under their old names.
ALTER INDEX listings_pkey RENAME TO commodities_pkey;
ALTER TABLE commodities
    RENAME CONSTRAINT listings_market_id_fkey TO commodities_market_id_fkey;
