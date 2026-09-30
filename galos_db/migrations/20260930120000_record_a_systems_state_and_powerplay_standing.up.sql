-- The three things about a system that go stale by the week: the state its
-- controlling faction is in, the power holding it, and where it stands in
-- Powerplay. An arrival states all three; so do Spansh's galaxy dump and, in
-- part, EDSM's and EDDB's.
--
-- Each column holds two kinds of nothing, because they merge differently.
-- A null is a system nothing has spoken to the column about, which a report
-- of any age fills in. 'None' is a system an arrival said has none -- a
-- faction in no state, no power holding it -- and is a reading like any
-- other: it wins over an older one and an older one does not fill it back
-- in, which is what takes last week's state or a lost system's power off
-- the row. `galos_index::accumulate::report` states the rule and the
-- `systems` upsert repeats it.
--
-- `state` already has every label but that one. The two new types are
-- labelled as `elite_journal`'s `Power` and `PowerplayState` spell their
-- variants, which is what their `sqlx::Type` derives read and write, with
-- 'None' after them for the second kind of nothing. Neither enum has a
-- variant for it, so nothing decodes 'None' into one: reads say
-- `NULLIF(column, 'None')`.
ALTER TYPE state ADD VALUE IF NOT EXISTS 'None';

CREATE TYPE Power AS ENUM (
    'AislingDuval',
    'ArchonDelaine',
    'ArissaLavignyDuval',
    'DentonPatreus',
    'EdmundMahon',
    'FeliciaWinters',
    'JeromeArcher',
    'LiYongRui',
    'NakatoKaine',
    'PranavAntal',
    'YuriGrom',
    'ZacharyHudson',
    'ZeminaTorval',
    'None'
);

CREATE TYPE PowerplayState AS ENUM (
    'InPrepareRadius',
    'Prepared',
    'Exploited',
    'Contested',
    'Controlled',
    'Turmoil',
    'HomeSystem',
    'Unoccupied',
    'Fortified',
    'Stronghold',
    'None'
);

-- Nullable and without a default, so adding them rewrites nothing.
ALTER TABLE systems
    ADD COLUMN state              State,
    ADD COLUMN controlling_power  Power,
    ADD COLUMN powerplay_state    PowerplayState;
