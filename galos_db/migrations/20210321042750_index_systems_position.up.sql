CREATE INDEX systems_position_idx ON systems USING GIST (position gist_geometry_ops_nd);
