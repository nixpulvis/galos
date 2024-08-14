//! Factions of a system
use chrono::{DateTime, Utc};
use elite_journal::{faction::State as JournalState, prelude::*};

#[derive(Debug, PartialEq, Eq)]
pub struct Faction {
    pub id: i32,
    pub name: String,
}

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
