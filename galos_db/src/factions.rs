use chrono::{DateTime, Utc};
use elite_journal::prelude::*;
use crate::{Error, Database};

#[derive(Debug, PartialEq, Eq)]
pub struct Faction {
    pub id: i32,
    pub name: String,
}

impl Faction {
    pub async fn create(db: &Database, name: &str) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            INSERT INTO factions (name)
            VALUES ($1)
            ON CONFLICT (lower(name))
            DO UPDATE
                SET name = factions.name
            RETURNING *
            ",
            name)
            .fetch_one(&db.pool)
            .await?;

        Ok(Faction { id: row.id, name: row.name })
    }

    pub async fn fetch(db: &Database, id: i32) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM factions
            WHERE id = $1
            ", id)
            .fetch_one(&db.pool)
            .await?;

        Ok(Faction { id: row.id, name: row.name })
    }

    pub async fn fetch_by_name(db: &Database, name: &str) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM factions
            WHERE lower(name) = $1
            ", name.to_lowercase())
            .fetch_one(&db.pool)
            .await?;

        Ok(Faction { id: row.id, name: row.name })
    }
}

#[derive(Debug, PartialEq)]
pub struct Conflict {
    system_address: u64,
    ty: FactionConflictType,
    status: Status,
    faction_1_id: u32,
    faction_1_stake: Option<String>,
    faction_1_won_days: u8,
    faction_2_id: u32,
    faction_2_stake: Option<String>,
    faction_2_won_days: u8,
    updated_at: DateTime<Utc>,
}

impl Conflict {
    pub async fn from_journal(
        db: &Database,
        conflict: &FactionConflict,
        system_address: u64,
        timestamp: DateTime<Utc>)
        -> Result<Self, Error>
    {
        let faction_1 = Faction::fetch_by_name(db, &conflict.faction_1.name).await?;
        let faction_2 = Faction::fetch_by_name(db, &conflict.faction_2.name).await?;

        let row = sqlx::query!(
            r#"
            INSERT INTO conflicts (
                system_address,
                type,
                status,
                faction_1_id,
                faction_1_stake,
                faction_1_won_days,
                faction_2_id,
                faction_2_stake,
                faction_2_won_days,
                updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (system_address, faction_1_id, faction_2_id)
            DO UPDATE SET
                type = $2,
                status = $3,
                faction_1_stake = $5,
                faction_1_won_days = $6,
                faction_2_stake = $8,
                faction_2_won_days = $9,
                updated_at = $10
            RETURNING
                system_address,
                type AS "ty: FactionConflictType",
                status AS "status: Status",
                faction_1_id,
                faction_1_stake,
                faction_1_won_days,
                faction_2_id,
                faction_2_stake,
                faction_2_won_days,
                updated_at
            "#,
            system_address as i64,
            conflict.ty as _,
            conflict.status as _,
            faction_1.id,
            conflict.faction_1.stake,
            conflict.faction_1.won_days as i32,
            faction_2.id,
            conflict.faction_2.stake,
            conflict.faction_2.won_days as i32,
            timestamp.naive_utc())
            .fetch_one(&db.pool)
            .await?;

        Ok(Conflict {
            system_address: row.system_address as u64,
            ty: row.ty,
            status: row.status,
            faction_1_id: row.faction_1_id as u32,
            faction_1_stake: row.faction_1_stake,
            faction_1_won_days: row.faction_1_won_days as u8,
            faction_2_id: row.faction_2_id as u32,
            faction_2_stake: row.faction_2_stake,
            faction_2_won_days: row.faction_2_won_days as u8,
            updated_at: DateTime::<Utc>::from_utc(row.updated_at, Utc),
        })
    }
}
