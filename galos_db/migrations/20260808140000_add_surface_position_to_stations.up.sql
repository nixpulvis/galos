-- Where a station sits, for the ones that sit on something.
--
-- A settlement is a station on a planet's surface. `ApproachSettlement` is
-- what reports one, and it is the only station-bearing event that says where
-- on the body it is, which is the one thing a surface station has that an
-- orbital does not. Without these columns there was nowhere to put it, and
-- settlements were not being recorded at all.
--
-- All four are null for anything in orbit, which is most of the table.
ALTER TABLE stations
    ADD COLUMN body_id    smallint,
    ADD COLUMN body_name  varchar,
    ADD COLUMN latitude   double precision,
    ADD COLUMN longitude  double precision;
