use async_std::task;
use chrono::{DateTime, Utc};
use geozero::wkb;
use pathfinding::prelude::*;
use ordered_float::OrderedFloat;
use elite_journal::{prelude::*, system::System as JournalSystem};
use crate::{Error, Database};
use crate::factions::{Faction, SystemFaction, Conflict};

#[derive(Debug, Clone)]
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

    pub updated_at: DateTime<Utc>,
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
            let faction_id = Faction::create(db, &faction.name).await?.id;
            SystemFaction::from_journal(db,
                system.address, faction_id as u32, &faction, timestamp).await?;
        }

        for conflict in &system.conflicts {
            Conflict::from_journal(db,
                system.address, &conflict, timestamp).await?;
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
                secondary_economy as "secondary_economy: Economy",
                updated_at
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
            updated_at: DateTime::<Utc>::from_utc(row.updated_at, Utc),
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
                secondary_economy as "secondary_economy: Economy",
                updated_at
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
            updated_at: DateTime::<Utc>::from_utc(row.updated_at, Utc),
        })
    }

    pub async fn fetch_like_name(db: &Database, name: &str) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
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
                secondary_economy as "secondary_economy: Economy",
                updated_at
            FROM systems
            WHERE name ILIKE $1
            ORDER BY name
            "#, name)
            .fetch_all(&db.pool)
            .await?;

        Ok(rows.into_iter().map(|row| {
            System {
                address: row.address,
                name: row.name,
                position: row.position.geometry.expect("not null or invalid"),
                population: row.population.map(|n| n as u64).unwrap_or(0),
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                updated_at: DateTime::<Utc>::from_utc(row.updated_at, Utc),
            }
        }).collect())
    }

    pub async fn fetch_in_range_by_name(db: &Database, range: f64, name: &str) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                s1.address,
                s1.name,
                s1.position AS "position!: wkb::Decode<Coordinate>",
                s1.population,
                s1.security as "security: Security",
                s1.government as "government: Government",
                s1.allegiance as "allegiance: Allegiance",
                s1.primary_economy as "primary_economy: Economy",
                s1.secondary_economy as "secondary_economy: Economy",
                s1.updated_at
            FROM systems s1
            FULL JOIN systems s2 ON ST_3DDWithin(s1.position, s2.position, $2)
            WHERE s2.name = $1
            ORDER BY ST_3DDistance(s1.position, s2.position)
            "#, name.to_uppercase(), range)
            .fetch_all(&db.pool)
            .await?;

        Ok(rows.into_iter().map(|row| {
            System {
                address: row.address,
                name: row.name,
                position: row.position.geometry.expect("not null or invalid"),
                population: row.population.map(|n| n as u64).unwrap_or(0),
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                updated_at: DateTime::<Utc>::from_utc(row.updated_at, Utc),
            }
        }).collect())
    }

    pub async fn fetch_in_range_like_name(db: &Database, range: f64, name: &str) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                s1.address,
                s1.name,
                s1.position AS "position!: wkb::Decode<Coordinate>",
                s1.population,
                s1.security as "security: Security",
                s1.government as "government: Government",
                s1.allegiance as "allegiance: Allegiance",
                s1.primary_economy as "primary_economy: Economy",
                s1.secondary_economy as "secondary_economy: Economy",
                s1.updated_at
            FROM systems s1
            FULL JOIN systems s2 ON ST_3DDWithin(s1.position, s2.position, $2)
            WHERE s2.name ILIKE $1
            ORDER BY ST_3DDistance(s1.position, s2.position)
            "#, name, range)
            .fetch_all(&db.pool)
            .await?;

        Ok(rows.into_iter().map(|row| {
            System {
                address: row.address,
                name: row.name,
                position: row.position.geometry.expect("not null or invalid"),
                population: row.population.map(|n| n as u64).unwrap_or(0),
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                updated_at: DateTime::<Utc>::from_utc(row.updated_at, Utc),
            }
        }).collect())
    }

    pub fn neighbors(&self, db: &Database, range: f64) -> Vec<System> {
        let rows = task::block_on(async {
            sqlx::query!(
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
                    secondary_economy as "secondary_economy: Economy",
                    updated_at
                FROM systems
                WHERE ST_3DDWithin(position, $1, $2);
                "#, wkb::Encode(self.position) as _, range)
                .fetch_all(&db.pool)
                .await.unwrap()
        });

        rows.into_iter().map(|row| {
            System {
                address: row.address,
                name: row.name,
                position: row.position.geometry.expect("not null or invalid"),
                population: row.population.map(|n| n as u64).unwrap_or(0),
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                updated_at: DateTime::<Utc>::from_utc(row.updated_at, Utc),
            }
        }).collect()
    }

    pub fn distance(&self, other: &System) -> f64 {
        let p1 = self.position;
        let p2 = other.position;

        ((p2.x - p1.x).powi(2) +
            (p2.y - p1.y).powi(2) +
            (p2.z - p1.z).powi(2)).sqrt()
    }

    pub fn route_to(&self, db: &Database, end: &System, range: f64)
        -> Result<Option<(Vec<Self>, OrderedFloat<f64>)>, Error>
    {
        let successors = |s: &System| {
            s.neighbors(db, range).into_iter().map(|s| (s, OrderedFloat(1.)))
        };

        // Making the heuristic much larger than the successor's jump cost makes things run
        // faster, but is not optimal...
        let heuristic = |s: &System| {
            OrderedFloat((s.distance(end) / range).ceil())
        };

        let success = |s: &System| s == end;

        Ok(astar(self, successors, heuristic, success))
    }
}

// https://www.reddit.com/r/EliteDangerous/comments/30nx4u/the_hyperspace_fuel_equation_documented/
fn fuel_cost(distance: f64, mass: f64, optimal_mass: f64) -> f64 {
    // A: 12
    // B: 10
    // C: 8
    // D: 10
    // E: 11
    let l = 12.;
    // 2: 2.00
    // 3: 2.15
    // 4: 2.30
    // 5: 2.45
    // 6: 2.60
    // 7: 2.75
    // 8: 2.90
    let p = 2.45;

    l * 0.001 * (distance * mass / optimal_mass).powf(p)
}

impl Eq for System {}
impl PartialEq for System {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
    }
}

use std::hash::{Hash, Hasher};
impl Hash for System {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.address.hash(state);
    }
}
