-- The economy a rescue ship carries.
--
-- A megaship sent to a system whose station has been attacked trades under
-- `$economy_Rescue;`, and a docking at one failed to read for want of the
-- label: the station, its services and its pads went with the one word.
ALTER TYPE economy ADD VALUE IF NOT EXISTS 'Rescue';
