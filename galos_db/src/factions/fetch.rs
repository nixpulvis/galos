use super::{Faction, SystemFaction};
use crate::{escaped, Database, Error};
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

    /// The factions with any of `ids`
    ///
    /// One query for a set of them, since what asks is holding a system's
    /// whole list and wants all of it named at once.
    ///
    /// Ids that match nothing are simply absent from the answer, so the
    /// caller pairs by id rather than by position.
    pub async fn fetch_many(
        db: &Database,
        ids: &[i32],
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            "
            SELECT id, name
            FROM factions
            WHERE id = ANY($1)
            ",
            ids
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Faction { id: row.id, name: row.name })
            .collect())
    }

    /// The factions whose names hold `query`, best first
    ///
    /// What a field asks where the user is part way through typing a name and
    /// wants to be shown which factions they might mean.
    ///
    /// `query` is read as letters rather than as a pattern, since a name is a
    /// thing the user is halfway through typing and `%` and `_` in it are
    /// characters they typed, which [`escaped`] takes them at their word for.
    /// That is the difference between this and [`Faction::fetch_like_name`],
    /// which takes a pattern whole from whoever wrote it, and answers with
    /// however many match.
    ///
    /// Ordered so that the `limit` keeps the rows worth keeping: the name
    /// spelled out in full, then names that start with the query, then the
    /// rest by name. Someone typing `dukes` means The Dukes of Mikunn before
    /// they mean Grand Duke Enterprise.
    ///
    /// Bounded because it has to be. A query of a letter matches most of the
    /// factions on record, `%a%` reaching 19,000 of the 23,000 held today, and
    /// a list nobody can read to the end of is no more use for being complete.
    pub async fn search_by_name(
        db: &Database,
        query: &str,
        limit: i64,
    ) -> Result<Vec<Self>, Error> {
        let query = escaped(query);
        let rows = sqlx::query!(
            r#"
            SELECT id, name
            FROM factions
            WHERE name ILIKE $1
            ORDER BY (name ILIKE $2) DESC, (name ILIKE $3) DESC, name
            LIMIT $4
            "#,
            format!("%{query}%"),
            query,
            format!("{query}%"),
            limit,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Faction { id: row.id, name: row.name })
            .collect())
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
