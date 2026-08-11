-- Five kinds of station and five services these types had no label for.
--
-- Odyssey brought settlements walked around on foot, Trailblazers brought the
-- depots a colonisation project is built from and the services for claiming a
-- system and handing materials over, and `SurfaceStation` and `Dodec` have
-- been sent all along without being read. A message naming any of them failed
-- to parse, and the whole of it went with the one word: the station, its
-- prices, its economies and where it stands.
--
-- Ninety seconds of the live feed carried fifty six such messages.
ALTER TYPE stationtype ADD VALUE IF NOT EXISTS 'SurfaceStation';
ALTER TYPE stationtype ADD VALUE IF NOT EXISTS 'OnFootSettlement';
ALTER TYPE stationtype ADD VALUE IF NOT EXISTS 'Dodec';
ALTER TYPE stationtype ADD VALUE IF NOT EXISTS 'SpaceConstructionDepot';
ALTER TYPE stationtype ADD VALUE IF NOT EXISTS 'PlanetaryConstructionDepot';

ALTER TYPE service ADD VALUE IF NOT EXISTS 'ColonisationContribution';
ALTER TYPE service ADD VALUE IF NOT EXISTS 'OnDockMission';
ALTER TYPE service ADD VALUE IF NOT EXISTS 'SquadronBank';
ALTER TYPE service ADD VALUE IF NOT EXISTS 'Refinery';
ALTER TYPE service ADD VALUE IF NOT EXISTS 'CarrierVendor';
