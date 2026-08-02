-- A commodity's name is half its key, so `gold` and `Gold` are two rows for
-- one thing at one market. EDDN carries both spellings, sometimes for the
-- same market minutes apart, and every query against these rows sees the
-- trade split across them.

-- Where a market holds one commodity under several spellings, the most
-- recent reading is the one worth keeping. `ctid` settles a tie so the
-- survivor does not depend on the order rows are scanned in.
DELETE FROM commodities c
 USING (
     SELECT ctid,
            row_number() OVER (
                PARTITION BY market_id, lower(name)
                ORDER BY listed_at DESC, ctid DESC) AS rank
       FROM commodities
 ) dup
 WHERE c.ctid = dup.ctid
   AND dup.rank > 1;

UPDATE commodities SET name = lower(name) WHERE name <> lower(name);

-- Held here as well as at the insert, so a second spelling cannot arrive by
-- some other path and quietly split a commodity in two again.
ALTER TABLE commodities
    ADD CONSTRAINT commodities_name_is_lowercase CHECK (name = lower(name));
