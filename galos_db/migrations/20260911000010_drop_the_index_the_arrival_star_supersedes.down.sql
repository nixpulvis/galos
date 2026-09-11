-- Not `CONCURRENTLY`: sqlx runs a down migration inside a transaction
-- whatever the file says, and `CREATE INDEX CONCURRENTLY` cannot. A revert
-- is a hand operation on a database nothing is writing to, where the write
-- lock a plain build takes is what it costs to have one at all.
CREATE INDEX stars_reach ON stars (system_address)
    INCLUDE (distance_from_arrival_ls, semi_major_axis, eccentricity, radius);
