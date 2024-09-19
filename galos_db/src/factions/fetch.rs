use super::{Faction, SystemFaction};
use crate::{Database, Error};
use elite_journal::{faction::State as JournalState, prelude::*};

impl Faction {
    pub async fn fetch(db: &Database, id: i32) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM factions
            WHERE id = $1
            ",
            id
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Faction { id: row.id, name: row.name })
    }

    pub async fn fetch_by_name(
        db: &Database,
        name: &str,
    ) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM factions
            WHERE lower(name) = $1
            ",
            name.to_lowercase()
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Faction { id: row.id, name: row.name })
    }

    pub async fn fetch_like_name(
        db: &Database,
        name: &str,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT *
            FROM factions
            WHERE name ILIKE $1
            ORDER BY name
            "#,
            name
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Faction { id: row.id, name: row.name })
            .collect())
    }
}

impl SystemFaction {
    pub async fn fetch(
        db: &Database,
        address: i64,
        id: u32,
    ) -> Result<Self, Error> {
        let row = sqlx::query!(
            r#"
            SELECT
                system_address,
                faction_id,
                name,
                state AS "state: JournalState",
                influence,
                happiness AS "happiness: Happiness",
                government AS "government: Government",
                allegiance AS "allegiance: Allegiance",
                updated_at
            FROM system_factions
            JOIN factions ON faction_id = id
            WHERE system_address = $1 AND faction_id = $2
            ORDER BY influence DESC
            "#,
            address as i64,
            id as i32
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(SystemFaction {
            system_address: row.system_address,
            faction_id: row.faction_id as u32,
            state: row.state,
            influence: row.influence,
            happiness: row.happiness,
            updated_at: row.updated_at.and_utc(),
        })
    }

    pub async fn fetch_all(
        db: &Database,
        address: Option<i64>,
    ) -> Result<Vec<(String, Self)>, Error> {
        if let Some(address) = address {
            let rows = sqlx::query!(
                r#"
            SELECT
                system_address,
                faction_id,
                name,
                state AS "state: JournalState",
                influence,
                happiness AS "happiness: Happiness",
                government AS "government: Government",
                allegiance AS "allegiance: Allegiance",
                updated_at
            FROM system_factions
            JOIN factions ON faction_id = id
            WHERE system_address = $1
            ORDER BY influence DESC
            "#,
                address as i64
            )
            .fetch_all(&db.pool)
            .await?;

            Ok(rows
                .into_iter()
                .map(|row| {
                    (
                        row.name,
                        SystemFaction {
                            system_address: row.system_address,
                            faction_id: row.faction_id as u32,
                            state: row.state,
                            influence: row.influence,
                            happiness: row.happiness,
                            updated_at: row.updated_at.and_utc(),
                        },
                    )
                })
                .collect())
        } else {
            let rows = sqlx::query!(
                r#"
            SELECT
                system_address,
                faction_id,
                name,
                state AS "state: JournalState",
                influence,
                happiness AS "happiness: Happiness",
                government AS "government: Government",
                allegiance AS "allegiance: Allegiance",
                updated_at
            FROM system_factions
            JOIN factions on faction_id = id
            ORDER BY influence DESC
            "#
            )
            .fetch_all(&db.pool)
            .await?;

            Ok(rows
                .into_iter()
                .map(|row| {
                    (
                        row.name,
                        SystemFaction {
                            system_address: row.system_address,
                            faction_id: row.faction_id as u32,
                            state: row.state,
                            influence: row.influence,
                            happiness: row.happiness,
                            updated_at: row.updated_at.and_utc(),
                        },
                    )
                })
                .collect())
        }
    }
}
