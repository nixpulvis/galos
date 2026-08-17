-- no-transaction
-- And for barycenters, which
-- `20260816000000_read_a_bodys_reach_from_the_index` sets out.
--
-- Nothing stands at a barycenter, so it has neither a distance from arrival
-- nor a radius to carry, and the ellipse the pair rides is the whole of what
-- it lends to the reach. Two columns covered rather than four.
CREATE INDEX CONCURRENTLY barycenters_reach ON barycenters (system_address)
    INCLUDE (semi_major_axis, eccentricity);
