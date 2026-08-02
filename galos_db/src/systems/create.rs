use super::System;
use crate::factions::{Conflict, Faction, SystemFaction};
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::{prelude::*, system::System as JournalSystem};
use geozero::wkb;

impl System {
    pub async fn create(
        db: &Database,
        address: i64,
        name: &str,
        position: Option<Coordinate>,
        primary_star_class: Option<String>,
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
                 primary_star_class,
                 position,
                 population,
                 security,
                 government,
                 allegiance,
                 primary_economy,
                 secondary_economy,
                 updated_at,
                 updated_by)
            VALUES ($1, UPPER($2), $3, $4::geometry, $5, $6,
                $7, $8, $9, $10, $11, $12)
            ON CONFLICT (address)
            DO UPDATE SET
                primary_star_class = COALESCE($3, systems.primary_star_class),
                position = COALESCE($4, systems.position),
                population = COALESCE($5, systems.population),
                security = COALESCE($6, systems.security),
                government = COALESCE($7, systems.government),
                allegiance = COALESCE($8, systems.allegiance),
                primary_economy = COALESCE($9, systems.primary_economy),
                secondary_economy = COALESCE($10, systems.secondary_economy),
                updated_at = $11,
                updated_by = $12
            WHERE systems.updated_at < $11
            "#,
            address as i64,
            name,
            primary_star_class,
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

        Self::adopt_waiting_markets(db, address, name, updated_at, updated_by)
            .await?;

        Ok(())
    }

    /// Link up any markets that named this system before it existed
    ///
    /// A market message gives a system name and no address, so its market is
    /// recorded unlinked and waits. This is the moment that wait can end, so
    /// it is answered here rather than swept for later.
    ///
    /// The station has to exist before a market may point at it, because the
    /// foreign key onto it stops being satisfied by a null the instant the
    /// address is filled in.
    async fn adopt_waiting_markets(
        db: &Database,
        address: i64,
        name: &str,
        updated_at: DateTime<Utc>,
        updated_by: &str,
    ) -> Result<(), Error> {
        // This runs on every system write, including the inner loop of the
        // bulk importers, and almost always there is nothing waiting. Ask
        // the partial index before opening a transaction for no reason.
        let waiting = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM markets
                 WHERE system_address IS NULL AND system_name = UPPER($1)
            ) AS "waiting!"
            "#,
            name,
        )
        .fetch_one(&db.pool)
        .await?;

        if !waiting {
            return Ok(());
        }

        let mut tx = db.pool.begin().await?;

        sqlx::query!(
            r#"
            INSERT INTO stations (system_address, name, updated_at, updated_by)
            SELECT $1, m.station_name, $3, $4
              FROM markets m
             WHERE m.system_address IS NULL AND m.system_name = UPPER($2)
            ON CONFLICT (system_address, name) DO NOTHING
            "#,
            address,
            name,
            updated_at.naive_utc(),
            updated_by,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query!(
            r#"
            UPDATE markets SET system_address = $1
             WHERE system_address IS NULL AND system_name = UPPER($2)
            "#,
            address,
            name,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        system: &JournalSystem,
    ) -> Result<(), Error> {
        let position =
            system.pos.map(|p| Coordinate { x: p.x, y: p.y, z: p.z });
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
                position = COALESCE($3, systems.position),
                population = COALESCE($4, systems.population),
                security = COALESCE($5, systems.security),
                government = COALESCE($6, systems.government),
                allegiance = COALESCE($7, systems.allegiance),
                primary_economy = COALESCE($8, systems.primary_economy),
                secondary_economy = COALESCE($9, systems.secondary_economy),
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
            SystemFaction::from_journal(
                db,
                system.address,
                faction_id as u32,
                &faction,
                timestamp,
            )
            .await?;
        }

        for conflict in &system.conflicts {
            Conflict::from_journal(db, system.address, &conflict, timestamp)
                .await?;
        }

        Self::adopt_waiting_markets(
            db,
            system.address,
            &system.name,
            timestamp,
            user,
        )
        .await?;

        Ok(())
    }
}
