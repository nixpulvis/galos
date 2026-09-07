-- `updated_at` is the time the event describes, and a watch needs the time
-- the report reached us. They are two different clocks, and comparing one
-- against the other dropped writes.
--
-- Every upsert on these three tables binds the timestamp off the journal
-- entry, and `systems.updated_at` is merged as `GREATEST(systems.updated_at,
-- $n)` so that a message delivered late cannot put the row back to when it
-- was sent. A watch, meanwhile, keeps its cursor from the database's own
-- clock: it reads the time, asks for everything changed since the last time
-- it read it, and moves the cursor forward. So the comparison was an event's
-- own time against the time somebody happened to look, and a row written with
-- an event timestamp older than the previous pass's cursor was invisible from
-- the moment it landed.
--
-- `PLIELAO KI-R B22-1` is what that looks like. Its star was scanned and
-- recorded while the watch was running, the row is there with a
-- `stars.updated_at` five seconds behind the pass that should have caught it,
-- and no body file was ever published for it, so flying there in the map shows
-- a system with nothing in it. The star scans of `WHANEE JQ-G C10-6386` show
-- the other half of it: both arrived a minute behind their own system row,
-- which a later message had already carried forward, so nothing about that
-- system was ever asked for again. A journal import is the extreme case,
-- writing rows whose event timestamps are hours or years old, none of which a
-- running watch can see.
--
-- `received_at` is stamped by the upsert that writes the row, so it says when
-- we got the report, and a watch compares one clock against itself.
--
-- Named for the arrival and not for a consumer of it. It is not when the index
-- was last updated: the index's own progress is the cursor in
-- `.galos_checkpoint`, a different thing in a different place, and calling
-- this `indexed_at` would have the row claim to know something about a reader
-- it has never heard of. `updated_at` says when the thing happened out in the
-- galaxy, `received_at` says when it reached us, and a watch can only follow
-- the second.
--
-- The partial index is what a pass reads, and it holds only the rows that have
-- arrived since this column existed.
--
-- Said `AT TIME ZONE 'utc'` because the column holds a `timestamp` and
-- `clock_timestamp()` hands back a `timestamptz`, so storing one in the other
-- converts it through whatever `TimeZone` the writing session happens to be
-- set to. This server's is `America/New_York`, and a watch reads its cursor as
-- `now() AT TIME ZONE 'utc'`, so leaving the conversion to the session would
-- put every stamp four hours behind the cursor it is compared against and drop
-- writes for exactly the reason this column exists. Naming the zone makes the
-- value the same whichever client writes the row.
--
-- Added nullable and given its default in a second statement, because a
-- default that is volatile makes `ADD COLUMN` compute it for every row on
-- record, which rewrites the whole table, and `systems` is two and a third
-- million rows and a gigabyte of them. Existing rows keep `NULL` on purpose:
-- they arrived before anything here recorded this, there is no honest value to
-- invent for them, and a watch reading `received_at > cursor` therefore never
-- re-reads them.
ALTER TABLE systems ADD COLUMN received_at timestamp;
ALTER TABLE systems ALTER COLUMN received_at
    SET DEFAULT clock_timestamp() AT TIME ZONE 'utc';
CREATE INDEX systems_received_at ON systems (received_at DESC)
    WHERE received_at IS NOT NULL;

ALTER TABLE stars ADD COLUMN received_at timestamp;
ALTER TABLE stars ALTER COLUMN received_at
    SET DEFAULT clock_timestamp() AT TIME ZONE 'utc';
CREATE INDEX stars_received_at ON stars (received_at DESC)
    WHERE received_at IS NOT NULL;

ALTER TABLE system_factions ADD COLUMN received_at timestamp;
ALTER TABLE system_factions ALTER COLUMN received_at
    SET DEFAULT clock_timestamp() AT TIME ZONE 'utc';
CREATE INDEX system_factions_received_at ON system_factions (received_at DESC)
    WHERE received_at IS NOT NULL;
