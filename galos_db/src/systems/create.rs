use crate::factions::{Conflict, Faction, SystemFaction};
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::{prelude::*, system::System as JournalSystem};
use geozero::wkb;
use super::System;

impl System {
    pub async fn create(
        db: &Database,
        address: i64,
        name: &str,
        position: Option<Coordinate>,
        population: Option<u64>,
        security: Option<Security>,
        government: Option<Government>,
        allegiance: Option<Allegiance>,
        primary_economy: Option<Economy>,
        secondary_economy: Option<Economy>,
        updated_at: DateTime<Utc>,
        updated_by: &str,
    ) -> Result<(), Error> {
        sqlx::query!(
            r#"
            INSERT INTO systems
                (address,
                 name,
                 position,
                 population,
                 security,
                 government,
                 allegiance,
                 primary_economy,
                 secondary_economy,
                 updated_at,
                 updated_by)
            VALUES ($1, UPPER($2), $3::geometry, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (address)
            DO UPDATE SET
                population = $4,
                security = $5,
                government = $6,
                allegiance = $7,
                primary_economy = $8,
                secondary_economy = $9,
                updated_at = $10,
                updated_by = $11
            WHERE systems.updated_at < $10
            "#,
            address as i64,
            name,
            position.map(|p| wkb::Encode(p)) as _,
            population.map(|n| n as i64),
            security as _,
            government as _,
            allegiance as _,
            primary_economy as _,
            secondary_economy as _,
            updated_at.naive_utc(),
            updated_by
        )
        .execute(&db.pool)
        .await?;

        Ok(())
    }

    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        system: &JournalSystem,
    ) -> Result<(), Error> {
        let position = system.pos.map(|p| Coordinate {
            x: p.x,
            y: p.y,
            z: p.z,
        });
        // TODO: Conflicts on pos need to do something else.
        sqlx::query!(
            r#"
            INSERT INTO systems
                (address,
                 name,
                 position,
                 population,
                 security,
                 government,
                 allegiance,
                 primary_economy,
                 secondary_economy,
                 updated_at,
                 updated_by)
            VALUES ($1, UPPER($2), $3::geometry, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (address)
            DO UPDATE SET
                population = $4,
                security = $5,
                government = $6,
                allegiance = $7,
                primary_economy = $8,
                secondary_economy = $9,
                updated_at = $10,
                updated_by = $11
            "#,
            system.address as i64,
            system.name,
            position.map(|p| wkb::Encode(p)) as _,
            system.population.map(|n| n as i64),
            system.security as _,
            system.government as _,
            system.allegiance as _,
            system.economy as _,
            system.second_economy as _,
            timestamp.naive_utc(),
            user
        )
        .execute(&db.pool)
        .await?;

        for faction in &system.factions {
            let faction_id = Faction::create(db, &faction.name).await?.id;
            SystemFaction::from_journal(db, system.address, faction_id as u32, &faction, timestamp)
                .await?;
        }

        for conflict in &system.conflicts {
            Conflict::from_journal(db, system.address, &conflict, timestamp).await?;
        }

        Ok(())
    }
}