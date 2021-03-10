use chrono::{DateTime, Utc};
use geozero::wkb;
use elite_journal::{prelude::*, system::System as JournalSystem};
use crate::{Error, Database};
use crate::factions::{Faction, Conflict};

#[derive(Debug, PartialEq)]
pub struct System {
    pub address: i64,
    // TODO: We need to support multiple names
    pub name: String,
    pub position: Coordinate,
    pub population: u64,
    pub security: Option<Security>,
    pub government: Option<Government>,
    pub allegiance: Option<Allegiance>,
    pub primary_economy: Option<Economy>,
    pub secondary_economy: Option<Economy>,

    // TODO: Find an elegent way to represent this.
    // & = foreign key = belongs_to
    // pub controlling_faction: &Faction,
    // pub factions: Vec<Faction>
}

impl System {
    pub async fn create(db: &Database,
        address: u64,
        name: &str,
        position: Coordinate,
        population: Option<u64>,
        security: Option<Security>,
        government: Option<Government>,
        allegiance: Option<Allegiance>,
        primary_economy: Option<Economy>,
        secondary_economy: Option<Economy>,
        updated_at: DateTime<Utc>)
        -> Result<(), Error>
    {
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
                 updated_at)
            VALUES ($1, UPPER($2), $3::geometry, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (address)
            DO UPDATE SET
                population = $4,
                security = $5,
                government = $6,
                allegiance = $7,
                primary_economy = $8,
                secondary_economy = $9
            WHERE systems.updated_at < $10
            "#,
            address as i64,
            name,
            wkb::Encode(position) as _,
            population.map(|n| n as i64),
            security as _,
            government as _,
            allegiance as _,
            primary_economy as _,
            secondary_economy as _,
            updated_at.naive_utc())
            .execute(&db.pool)
            .await?;

        Ok(())
    }

    pub async fn from_journal(db: &Database, system: &JournalSystem, timestamp: DateTime<Utc>)
        -> Result<(), Error>
    {
        let position = Coordinate {
            x: system.pos.x,
            y: system.pos.y,
            z: system.pos.z,
        };
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
                 updated_at)
            VALUES ($1, UPPER($2), $3::geometry, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (address)
            DO UPDATE SET
                population = $4,
                security = $5,
                government = $6,
                allegiance = $7,
                primary_economy = $8,
                secondary_economy = $9
            "#, system.address as i64,
                system.name,
                wkb::Encode(position) as _,
                system.population.map(|n| n as i64),
                system.security as _,
                system.government as _,
                system.allegiance as _,
                system.economy as _,
                system.second_economy as _,
                timestamp.naive_utc())
            .execute(&db.pool)
            .await?;

        for faction in &system.factions {
            let id = Faction::create(db, &faction.name).await?.id;

            sqlx::query!(
                "
                INSERT INTO system_factions
                    (system_address,
                     faction_id,
                     updated_at,
                     state,
                     influence,
                     happiness,
                     government,
                     allegiance)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (system_address, faction_id)
                DO UPDATE SET
                    updated_at = $3,
                    state = $4,
                    influence = $5,
                    happiness = $6,
                    government = $7,
                    allegiance = $8
                WHERE system_factions.updated_at < $3
                ",
                    system.address as i64,
                    id,
                    timestamp.naive_utc(),
                    faction.state as _,
                    faction.influence,
                    faction.happiness as _,
                    faction.government as _,
                    faction.allegiance as _)
                .execute(&db.pool)
                .await?;
        }

        for conflict in &system.conflicts {
            Conflict::from_journal(db, &conflict, system.address, timestamp).await?;
        }

        Ok(())
    }

    pub async fn fetch(db: &Database, address: i64) -> Result<Self, Error> {
        let row = sqlx::query!(
            r#"
            SELECT
                address,
                name,
                position AS "position!: wkb::Decode<Coordinate>",
                population,
                security as "security: Security",
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                primary_economy as "primary_economy: Economy",
                secondary_economy as "secondary_economy: Economy"
            FROM systems
            WHERE address = $1
            "#, address)
            .fetch_one(&db.pool)
            .await?;

        Ok(System {
            address: row.address,
            name: row.name,
            position: row.position.geometry.expect("not null or invalid"),
            population: row.population.map(|n| n as u64).unwrap_or(0),
            security: row.security,
            government: row.government,
            allegiance: row.allegiance,
            primary_economy: row.primary_economy,
            secondary_economy: row.secondary_economy,
        })
    }

    // NOTE: Assumes systems are unique by name, which is currently untrue.
    pub async fn fetch_by_name(db: &Database, name: &str) -> Result<Self, Error> {
        let row = sqlx::query!(
            r#"
            SELECT
                address,
                name,
                position AS "position!: wkb::Decode<Coordinate>",
                population,
                security as "security: Security",
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                primary_economy as "primary_economy: Economy",
                secondary_economy as "secondary_economy: Economy"
            FROM systems
            WHERE name = $1
            "#, name.to_uppercase())
            .fetch_one(&db.pool)
            .await?;

        Ok(System {
            address: row.address,
            name: row.name,
            position: row.position.geometry.expect("not null or invalid"),
            population: row.population.map(|n| n as u64).unwrap_or(0),
            security: row.security,
            government: row.government,
            allegiance: row.allegiance,
            primary_economy: row.primary_economy,
            secondary_economy: row.secondary_economy,
        })
    }
}
