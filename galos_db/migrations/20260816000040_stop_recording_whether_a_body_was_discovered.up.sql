-- `was_discovered` is a fact about a scan and not about the thing it was
-- stored on, and what it was worth is a timestamp that was thrown away.
--
-- The flag arrives on a journal `Scan`, where it says whether anybody had
-- reached the body before the commander whose scan reached us. A scan that
-- reports it clear is itself the discovery, so the entry's own timestamp is
-- when the body was found. A scan that reports it set says only that somebody
-- got there earlier, and not when, which is nothing to record.
--
-- Stored as a flag it said neither of those things. Every body, star, ring and
-- belt cluster on record here got here through a scan, so every one of them is
-- discovered by construction and the column says nothing about the row it
-- hangs off. What it did do was get read out as `Discovered: No`, which
-- asserts the opposite of what the row means: Sol's bodies, whose only record
-- here is one nav beacon burst, read as a solar system nobody had ever found.
--
-- `discovered_at` holds the reading instead, and is null for everything on
-- record now, since the flag it would have been derived from is not a date and
-- the scans that carried one are not kept.
--
-- `was_mapped` stays, because it is a fact about the body. A `Scan` maps
-- nothing; that is `SAAScanComplete`, which the sync does not ingest. So
-- `WasMapped` reports whether anybody had mapped the body by the time it was
-- scanned, and a body can be discovered and unmapped.
ALTER TABLE bodies
    DROP COLUMN was_discovered,
    ADD COLUMN discovered_at timestamp;

ALTER TABLE stars
    DROP COLUMN was_discovered,
    ADD COLUMN discovered_at timestamp;

ALTER TABLE rings
    DROP COLUMN was_discovered,
    ADD COLUMN discovered_at timestamp;

ALTER TABLE clusters
    DROP COLUMN was_discovered,
    ADD COLUMN discovered_at timestamp;
