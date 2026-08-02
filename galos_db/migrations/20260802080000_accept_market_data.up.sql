-- A market message knows less about where it is than a journal entry does.
-- It gives systemName, stationName and marketId, and no system address, no
-- station type. The schema asked for both.

-- Market data names a station without saying what kind it is.
ALTER TABLE stations ALTER COLUMN ty DROP NOT NULL;

-- And it names a system it cannot give an address for, so a market can
-- arrive before anything that would create the system it belongs to.
-- Record it anyway, holding the name until the address is known. Both
-- foreign keys on the table are MATCH SIMPLE, so a null address satisfies
-- them and the row can wait without weakening either constraint.
ALTER TABLE markets ALTER COLUMN system_address DROP NOT NULL;
ALTER TABLE markets ADD COLUMN system_name varchar;

UPDATE markets m
   SET system_name = s.name
  FROM systems s
 WHERE s.address = m.system_address;

ALTER TABLE markets ALTER COLUMN system_name SET NOT NULL;

-- Every system write asks whether it is the one a waiting market named, so
-- that question has to be cheap. Only the unlinked rows are ever searched.
CREATE INDEX markets_waiting_on_system
    ON markets (system_name)
 WHERE system_address IS NULL;
