use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::station::Station as JournalStation;
use elite_journal::station::{StationType, Service};
use elite_journal::{Government, Allegiance};

#[derive(Debug, PartialEq, Eq)]
pub struct Station {
    pub system_address: i64,
    pub body_id: Option<i16>,
    pub name: String,
    pub ty: StationType,
    pub market_id: Option<i64>,
    pub faction: Option<String>,  // TODO: Faction type?
    pub government: Option<Government>,  // TODO: Government type?
    pub allegiance: Option<Allegiance>,
    pub services: Option<Vec<Service>>,
    // pub economies: Option<Vec<String>>,
    pub updated_at: DateTime<Utc>,
}

impl Station {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        station: &JournalStation,
        system_address: i64,
        body_id: Option<i16>,
    ) -> Result<Station, Error> {
        let row = sqlx::query!(
            r#"
            INSERT INTO stations (
                system_address,
                body_id,
                name,
                ty,
                market_id,
                faction,
                government,
                allegiance,
                services,
                updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (system_address, name)
            DO UPDATE SET
                body_id = $2,
                ty = $4,
                market_id = $5,
                faction = $6,
                government = $7,
                allegiance = $8,
                services = $9,
                updated_at = $10
            RETURNING
                system_address,
                body_id,
                name,
                ty as "ty: StationType",
                market_id,
                faction,
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                services AS "services: Vec<Service>",
                updated_at
            "#,
            system_address,
            body_id,
            station.name,
            station.ty.clone() as StationType,
            station.market_id as i64,
            station.faction.name,
            station.government as Government,
            station.allegiance as Option<Allegiance>,
            station.services.clone() as Vec<Service>,
            timestamp.naive_utc(),
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Station {
            system_address: row.system_address,
            body_id: row.body_id,
            name: row.name,
            ty: row.ty,
            market_id: row.market_id,
            faction: row.faction,
            government: row.government,
            allegiance: row.allegiance,
            services: row.services,
            updated_at: row.updated_at.and_utc(),
        })
    }

    // pub async fn fetch(db: &Database, system_address: i64, name: &str) -> Result<Self, Error> {
    //     let row = sqlx::query!(
    //         "
    //         SELECT *
    //         FROM stations
    //         WHERE system_address = $1 AND name = $2
    //         ",
    //         system_address,
    //         name
    //     )
    //     .fetch_one(&db.pool)
    //     .await?;

    //     Ok(Station {
    //         system_address: row.system_address,
    //         body_id: row.body_id,
    //         name: row.name,
    //         ty: row.ty,
    //         market_id: row.market_id,
    //         faction: row.faction,
    //         government: row.government,
    //         allegiance: row.allegiance,
    //         services: row.services,
    //         updated_at: row.updated_at.and_utc(),
    //     })
    // }
}
