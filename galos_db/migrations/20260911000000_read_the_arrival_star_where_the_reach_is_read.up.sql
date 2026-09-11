-- no-transaction
-- Which star a ship drops at, read the way the reach beside it is read.
--
-- The supercharge table is the arrival star's class, and it used to be read
-- off `systems.primary_star_class` alone -- a column only a plotted route
-- ever writes. A scanned neutron star was therefore not in it. It is now the
-- nearest star on record, which is a `stars` row, falling back to that column
-- for a system nobody has scanned.
--
-- Nearest means ordered, so the order goes in the key: a build reads one
-- `DISTINCT ON (system_address)` pass over the whole table and a watch pass
-- reads one star per system it touched, and both want (system_address,
-- distance_from_arrival_ls, id) with the class carried along. `stars_reach`
-- answered the first column of that and none of the rest; its payload is
-- carried here too, so the next migration drops it rather than leaving two
-- indexes over a hundred million stars to answer two halves of one question.
--
-- One statement to the file: `CREATE INDEX CONCURRENTLY` cannot run in a
-- transaction, and Postgres wraps a multi-statement batch in one.
CREATE INDEX CONCURRENTLY stars_arrival
    ON stars (system_address, distance_from_arrival_ls, id)
    INCLUDE (star_class, semi_major_axis, eccentricity, radius);
