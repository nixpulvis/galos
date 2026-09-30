ALTER TABLE systems
    DROP COLUMN state,
    DROP COLUMN controlling_power,
    DROP COLUMN powerplay_state;

DROP TYPE PowerplayState;
DROP TYPE Power;

-- 'None' stays on `state`. Postgres has no `ALTER TYPE ... DROP VALUE`, and
-- an unused label costs nothing.
