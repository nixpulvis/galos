-- no-transaction
-- The same for stars, and for the same reason, which
-- `20260816000000_read_a_bodys_reach_from_the_index` sets out.
--
-- A star carries its distance from arrival under a name of its own, ending in
-- the light seconds it is said in, where a body carries the same figure as
-- `distance_from_arrival`. So the two indexes cover different columns while
-- answering the same half of the same question.
CREATE INDEX CONCURRENTLY stars_reach ON stars (system_address)
    INCLUDE (distance_from_arrival_ls, semi_major_axis, eccentricity, radius);
