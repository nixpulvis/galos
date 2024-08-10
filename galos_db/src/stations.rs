use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::station::Station as JournalStation;
use elite_journal::station::{StationType, Service, EconomyShare};
use elite_journal::{Government, Allegiance};

#[derive(Debug, PartialEq, Eq)]
pub struct Station {
    pub system_address: i64,
    pub name: String,
    pub ty: Option<StationType>,
    pub market_id: Option<i64>,
    pub faction: Option<String>,  // TODO: Faction type?
    pub government: Option<Government>,  // TODO: Government type?
    pub allegiance: Option<Allegiance>,
    pub services: Option<Vec<Service>>,
    pub economies: Option<Vec<EconomyShare>>,
    pub updated_at: DateTime<Utc>,
}

impl Station {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        station: &JournalStation,
        system_address: i64,
    ) -> Result<Station, Error> {
        let row = sqlx::query!(
            r#"
            INSERT INTO stations (
                system_address,
                name,
                ty,
                market_id,
                faction,
                government,
                allegiance,
                services,
                economies,
                updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (system_address, name)
            DO UPDATE SET
                ty = $3,
                market_id = $4,
                faction = $5,
                government = $6,
                allegiance = $7,
                services = $8,
                economies = $9,
                updated_at = $10
            RETURNING
                system_address,
                name,
                ty as "ty: StationType",
                market_id,
                faction,
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                services as "services: Vec<Service>",
                economies as "economies: Vec<EconomyShare>",
                updated_at
            "#,
            system_address,
            station.name,
            station.ty.clone() as Option<StationType>,
            station.market_id,
            station.faction.as_ref().map(|f| f.name.clone()),
            station.government as Option<Government>,
            station.allegiance as Option<Allegiance>,
            station.services.clone() as Option<Vec<Service>>,
            station.economies.clone() as Option<Vec<EconomyShare>>,
            timestamp.naive_utc(),
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Station {
            system_address: row.system_address,
            name: row.name,
            ty: Some(row.ty),
            market_id: row.market_id,
            faction: row.faction,
            government: row.government,
            allegiance: row.allegiance,
            services: row.services,
            economies: row.economies,
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
