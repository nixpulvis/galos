-- Back to the server-wide setting, whatever it stands at.
ALTER TABLE bodies RESET (autovacuum_vacuum_insert_scale_factor);

ALTER TABLE stars RESET (autovacuum_vacuum_insert_scale_factor);

ALTER TABLE barycenters RESET (autovacuum_vacuum_insert_scale_factor);
