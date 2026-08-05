DROP INDEX systems_name_trgm;

-- The extension stays where it is. `articles` created it and indexes its
-- bodies through it, so it is not this migration's to take away: Postgres
-- refuses the drop while `body_gist` depends on `gist_trgm_ops`, and forcing
-- it through would take that index down as well.
