-- How many bodies a system holds, which is what says whether what is on
-- record about it is all of it or some of it.
--
-- `bodies` answers how many are known. It cannot answer how many there are,
-- so a system with four bodies on record has always been indistinguishable
-- from a system with forty where thirty-six have never been scanned. This is
-- the other half of that comparison.
--
-- Three schemas report it and they agree: `FSSDiscoveryScan` calls it
-- `BodyCount`, `FSSAllBodiesFound` calls it `Count`, and `NavBeaconScan`
-- calls it `NumBodies`. One column takes all three.
--
-- `non_body_count` is the rest of what the honk finds -- belts, rings, the
-- things that are scanned but are not bodies and so will never appear in
-- `bodies`. Only `FSSDiscoveryScan` reports it.
ALTER TABLE systems
    ADD COLUMN body_count      int,
    ADD COLUMN non_body_count  int;
