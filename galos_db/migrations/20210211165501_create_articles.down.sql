-- The index uses an operator class from pg_trgm, so the extension goes last.
DROP INDEX body_gist;
DROP TABLE articles;

DROP EXTENSION pg_trgm;
