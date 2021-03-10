CREATE EXTENSION pg_trgm;

CREATE TABLE articles (
    id     serial  PRIMARY KEY,
    title  text,
    date   date    NOT NULL,
    body   text    NOT NULL
);

CREATE INDEX body_gist ON articles USING gist (body gist_trgm_ops);
