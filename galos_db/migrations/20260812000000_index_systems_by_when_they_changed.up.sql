-- Asking which systems changed lately is a filter the map puts, so it runs
-- against the whole table rather than against a region of the galaxy. An hour
-- of the feed touches about nine thousand of the 1,067,631 rows, scattered
-- across four hundred and sixty 100 light year bands, so there is no region to
-- narrow it to first and a sequential scan is the alternative.
--
-- Descending, because every such question asks for the newest and reads
-- backwards from now. Ascending would answer it by walking the index from its
-- far end.
CREATE INDEX systems_updated_at ON systems USING btree (updated_at DESC);
