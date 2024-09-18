use super::System;
use crate::{Database, Error};
use elite_journal::prelude::*;
use geozero::wkb;

impl System {
    pub async fn fetch(db: &Database, address: i64) -> Result<Self, Error> {
        let row = sqlx::query!(
            r#"
            SELECT
                address,
                name,
                position AS "position!: Option<wkb::Decode<Coordinate>>",
                population,
                security as "security: Security",
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                primary_economy as "primary_economy: Economy",
                secondary_economy as "secondary_economy: Economy",
                updated_at,
                updated_by
            FROM systems
            WHERE address = $1
            "#,
            address
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(System {
            address: row.address,
            name: row.name,
            position: row
                .position
                .map(|p| p.geometry.expect("not null or invalid")),
            population: row.population.map(|n| n as u64).unwrap_or(0),
            security: row.security,
            government: row.government,
            allegiance: row.allegiance,
            primary_economy: row.primary_economy,
            secondary_economy: row.secondary_economy,
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,
        })
    }

    // NOTE: Assumes systems are unique by name, which is currently untrue.
    pub async fn fetch_by_name(db: &Database, name: &str) -> Result<Self, Error> {
        let row = sqlx::query!(
            r#"
            SELECT
                address,
                name,
                position AS "position!: Option<wkb::Decode<Coordinate>>",
                population,
                security as "security: Security",
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                primary_economy as "primary_economy: Economy",
                secondary_economy as "secondary_economy: Economy",
                updated_at,
                updated_by
            FROM systems
            WHERE name = $1
            "#,
            name.to_uppercase()
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(System {
            address: row.address,
            name: row.name,
            position: row
                .position
                .map(|p| p.geometry.expect("not null or invalid")),
            population: row.population.map(|n| n as u64).unwrap_or(0),
            security: row.security,
            government: row.government,
            allegiance: row.allegiance,
            primary_economy: row.primary_economy,
            secondary_economy: row.secondary_economy,
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,
        })
    }

    pub async fn fetch_like_name(db: &Database, name: &str) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                address,
                name,
                position AS "position!: Option<wkb::Decode<Coordinate>>",
                population,
                security as "security: Security",
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                primary_economy as "primary_economy: Economy",
                secondary_economy as "secondary_economy: Economy",
                updated_at,
                updated_by
            FROM systems
            WHERE name ILIKE $1
            ORDER BY name
            "#,
            name
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                address: row.address,
                name: row.name,
                position: row
                    .position
                    .map(|p| p.geometry.expect("not null or invalid")),
                population: row.population.map(|n| n as u64).unwrap_or(0),
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect())
    }

    pub async fn fetch_in_range_by_name(
        db: &Database,
        range: f64,
        name: &str,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                s1.address,
                s1.name,
                s1.position AS "position!: Option<wkb::Decode<Coordinate>>",
                s1.population,
                s1.security as "security: Security",
                s1.government as "government: Government",
                s1.allegiance as "allegiance: Allegiance",
                s1.primary_economy as "primary_economy: Economy",
                s1.secondary_economy as "secondary_economy: Economy",
                s1.updated_at,
                s1.updated_by
            FROM systems s1
            FULL JOIN systems s2 ON ST_3DDWithin(s1.position, s2.position, $2)
            WHERE s2.name = $1
            ORDER BY ST_3DDistance(s1.position, s2.position)
            "#,
            name.to_uppercase(),
            range
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                address: row.address,
                name: row.name,
                position: row
                    .position
                    .map(|p| p.geometry.expect("not null or invalid")),
                population: row.population.map(|n| n as u64).unwrap_or(0),
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect())
    }

    pub async fn fetch_in_range_like_name(
        db: &Database,
        range: f64,
        name: &str,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                s1.address,
                s1.name,
                s1.position AS "position!: Option<wkb::Decode<Coordinate>>",
                s1.population,
                s1.security as "security: Security",
                s1.government as "government: Government",
                s1.allegiance as "allegiance: Allegiance",
                s1.primary_economy as "primary_economy: Economy",
                s1.secondary_economy as "secondary_economy: Economy",
                s1.updated_at,
                s1.updated_by
            FROM systems s1
            FULL JOIN systems s2 ON ST_3DDWithin(s1.position, s2.position, $2)
            WHERE s2.name ILIKE $1
            ORDER BY ST_3DDistance(s1.position, s2.position)
            "#,
            name,
            range
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                address: row.address,
                name: row.name,
                position: row
                    .position
                    .map(|p| p.geometry.expect("not null or invalid")),
                population: row.population.map(|n| n as u64).unwrap_or(0),
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect())
    }

    pub async fn fetch_in_range_of_point(
        db: &Database,
        range: f64,
        center: [f64; 3],
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                address,
                name,
                position AS "position!: Option<wkb::Decode<Coordinate>>",
                population,
                security as "security: Security",
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                primary_economy as "primary_economy: Economy",
                secondary_economy as "secondary_economy: Economy",
                updated_at,
                updated_by
            FROM systems
            WHERE ST_3DDWithin(ST_MakePoint($2, $3, $4), position, $1)
            "#,
            range,
            center[0],
            center[1],
            center[2],
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                address: row.address,
                name: row.name,
                position: row
                    .position
                    .map(|p| p.geometry.expect("not null or invalid")),
                population: row.population.map(|n| n as u64).unwrap_or(0),
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect())
    }

    pub async fn fetch_faction(db: &Database, faction: &str) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                systems.address,
                systems.name,
                systems.position AS "position!: Option<wkb::Decode<Coordinate>>",
                systems.population,
                systems.security as "security: Security",
                systems.government as "government: Government",
                systems.allegiance as "allegiance: Allegiance",
                systems.primary_economy as "primary_economy: Economy",
                systems.secondary_economy as "secondary_economy: Economy",
                systems.updated_at,
                systems.updated_by
            FROM systems
            JOIN system_factions ON system_factions.system_address = systems.address
            JOIN factions ON factions.id = system_factions.faction_id
            WHERE factions.name ILIKE $1
            "#,
            faction,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                address: row.address,
                name: row.name,
                position: row
                    .position
                    .map(|p| p.geometry.expect("not null or invalid")),
                population: row.population.map(|n| n as u64).unwrap_or(0),
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect())
    }
}
