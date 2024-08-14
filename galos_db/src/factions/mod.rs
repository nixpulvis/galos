//! Factions of a system
use chrono::{DateTime, Utc};
use elite_journal::{faction::State as JournalState, prelude::*};

/// ### Schema
///
/// ```
///                                  Table "public.factions"
///  Column |       Type        | Collation | Nullable |               Default
/// --------+-------------------+-----------+----------+--------------------------------------
///  id     | integer           |           | not null | nextval('factions_id_seq'::regclass)
///  name   | character varying |           | not null |
/// Indexes:
///     "factions_pkey" PRIMARY KEY, btree (id)
///     "factions_lower_idx" UNIQUE, btree (lower(name::text))
/// Referenced by:
///     TABLE "conflicts" CONSTRAINT "conflicts_faction_1_id_fkey" FOREIGN KEY (faction_1_id) REFERENCES factions(id)
///     TABLE "conflicts" CONSTRAINT "conflicts_faction_2_id_fkey" FOREIGN KEY (faction_2_id) REFERENCES factions(id)
///     TABLE "system_faction_influences" CONSTRAINT "system_faction_influences_faction_id_fkey" FOREIGN KEY (faction_id) REFERENCES factions(id)
///     TABLE "system_faction_states" CONSTRAINT "system_faction_states_faction_id_fkey" FOREIGN KEY (faction_id) REFERENCES factions(id)
///     TABLE "system_factions" CONSTRAINT "system_factions_faction_id_fkey" FOREIGN KEY (faction_id) REFERENCES factions(id)
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct Faction {
    pub id: i32,
    pub name: String,
}

/// ### Schema
///
/// ```
///                         Table "public.system_factions"
///      Column     |            Type             | Collation | Nullable | Default
/// ----------------+-----------------------------+-----------+----------+---------
///  system_address | bigint                      |           | not null |
///  faction_id     | integer                     |           | not null |
///  updated_at     | timestamp without time zone |           | not null |
///  state          | state                       |           |          |
///  influence      | real                        |           | not null |
///  happiness      | happiness                   |           |          |
///  government     | government                  |           | not null |
///  allegiance     | allegiance                  |           | not null |
/// Indexes:
///     "system_factions_pkey" PRIMARY KEY, btree (system_address, faction_id)
///     "system_factions_system_address_faction_id_idx" UNIQUE, btree (system_address, faction_id)
/// Foreign-key constraints:
///     "system_factions_faction_id_fkey" FOREIGN KEY (faction_id) REFERENCES factions(id)
///     "system_factions_system_address_fkey" FOREIGN KEY (system_address) REFERENCES systems(address)
/// Referenced by:
///     TABLE "conflicts" CONSTRAINT "conflicts_system_address_faction_1_id_fkey" FOREIGN KEY (system_address, faction_1_id) REFERENCES system_factions(system_address, faction_id)
///     TABLE "conflicts" CONSTRAINT "conflicts_system_address_faction_2_id_fkey" FOREIGN KEY (system_address, faction_2_id) REFERENCES system_factions(system_address, faction_id)
///     TABLE "system_faction_influences" CONSTRAINT "system_faction_influences_system_address_faction_id_fkey" FOREIGN KEY (system_address, faction_id) REFERENCES system_factions(system_address, faction_id)
///     TABLE "system_faction_states" CONSTRAINT "system_faction_states_system_address_faction_id_fkey" FOREIGN KEY (system_address, faction_id) REFERENCES system_factions(system_address, faction_id)
/// Triggers:
///     system_faction_influence_changes AFTER UPDATE ON system_factions FOR EACH ROW EXECUTE FUNCTION insert_system_faction_influences()
/// ```
#[derive(Debug, PartialEq)]
pub struct SystemFaction {
    pub system_address: i64,
    pub faction_id: u32,
    pub state: Option<JournalState>,
    pub influence: f32,
    pub happiness: Option<Happiness>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, PartialEq)]
pub struct State {
    pub system_address: i64,
    pub faction_id: u32,
    pub state: JournalState,
    pub status: Status,
}

/// ### Schema
///
/// ```
///                              Table "public.conflicts"
///        Column       |            Type             | Collation | Nullable | Default
/// --------------------+-----------------------------+-----------+----------+---------
///  system_address     | bigint                      |           | not null |
///  type               | conflict                    |           | not null |
///  status             | status                      |           | not null |
///  faction_1_id       | integer                     |           | not null |
///  faction_1_stake    | character varying           |           |          |
///  faction_1_won_days | integer                     |           | not null | 0
///  faction_2_id       | integer                     |           | not null |
///  faction_2_stake    | character varying           |           |          |
///  faction_2_won_days | integer                     |           | not null | 0
///  updated_at         | timestamp without time zone |           | not null |
/// Indexes:
///     "conflicts_pkey" PRIMARY KEY, btree (system_address, faction_1_id, faction_2_id)
/// Foreign-key constraints:
///     "conflicts_faction_1_id_fkey" FOREIGN KEY (faction_1_id) REFERENCES factions(id)
///     "conflicts_faction_2_id_fkey" FOREIGN KEY (faction_2_id) REFERENCES factions(id)
///     "conflicts_system_address_faction_1_id_fkey" FOREIGN KEY (system_address, faction_1_id) REFERENCES system_factions(system_address, faction_id)
///     "conflicts_system_address_faction_2_id_fkey" FOREIGN KEY (system_address, faction_2_id) REFERENCES system_factions(system_address, faction_id)
///     "conflicts_system_address_fkey" FOREIGN KEY (system_address) REFERENCES systems(address)
/// ```
#[derive(Debug, PartialEq)]
pub struct Conflict {
    pub system_address: i64,
    pub ty: FactionConflictType,
    pub status: Status,
    pub faction_1_id: u32,
    pub faction_1_stake: Option<String>,
    pub faction_1_won_days: u8,
    pub faction_2_id: u32,
    pub faction_2_stake: Option<String>,
    pub faction_2_won_days: u8,
    pub updated_at: DateTime<Utc>,
}

mod create;
mod fetch;
