use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::station::Station as JournalStation;
use elite_journal::station::{LandingPads, EconomyShare, Service, StationType};
use elite_journal::{Allegiance, Government};

#[derive(Debug, PartialEq)]
pub struct Station {
    pub system_address: i64,
    pub name: String,
    pub ty: Option<StationType>,
    pub dist_from_star_ls: Option<f64>,
    pub market_id: Option<i64>,
    pub landing_pads: Option<LandingPads>,
    pub faction: Option<String>,        // TODO: Faction type?
    pub government: Option<Government>, // TODO: Government type?
    pub allegiance: Option<Allegiance>,
    pub services: Option<Vec<Service>>,
    pub economies: Option<Vec<EconomyShare>>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

impl Eq for Station {}

impl Station {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        station: &JournalStation,
        system_address: i64,
    ) -> Result<Station, Error> {
        let row = sqlx::query!(
            r#"
            INSERT INTO stations (
                system_address,
                name,
                ty,
                dist_from_star_ls,
                market_id,
                landing_pads,
                faction,
                government,
                allegiance,
                services,
                economies,
                updated_at,
                updated_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (system_address, name)
            DO UPDATE SET
                ty = $3,
                dist_from_star_ls = $4,
                market_id = $5,
                landing_pads = $6,
                faction = $7,
                government = $8,
                allegiance = $9,
                services = $10,
                economies = $11,
                updated_at = $12,
                updated_by = $13
            RETURNING
                system_address,
                name,
                ty as "ty: StationType",
                dist_from_star_ls,
                market_id,
                landing_pads as "landing_pads: LandingPads",
                faction,
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                services as "services: Vec<Service>",
                economies as "economies: Vec<EconomyShare>",
                updated_at,
                updated_by
            "#,
            system_address,
            station.name,
            station.ty.clone() as Option<StationType>,
            station.dist_from_star_ls,
            station.market_id,
            station.landing_pads as Option<LandingPads>,
            station.faction.as_ref().map(|f| f.name.clone()),
            station.government as Option<Government>,
            station.allegiance as Option<Allegiance>,
            station.services.clone() as Option<Vec<Service>>,
            station.economies.clone() as Option<Vec<EconomyShare>>,
            timestamp.naive_utc(),
            user,
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Station {
            system_address: row.system_address,
            name: row.name,
            ty: Some(row.ty),
            dist_from_star_ls: row.dist_from_star_ls,
            market_id: row.market_id,
            landing_pads: row.landing_pads,
            faction: row.faction,
            government: row.government,
            allegiance: row.allegiance,
            services: row.services,
            economies: row.economies,
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,
        })
    }

    pub async fn fetch(db: &Database, system_address: i64, name: &str) -> Result<Self, Error> {
        let row = sqlx::query!(
            r#"
            SELECT
                system_address,
                name,
                ty as "ty: StationType",
                dist_from_star_ls,
                market_id,
                landing_pads as "landing_pads: LandingPads",
                faction,
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                services as "services: Vec<Service>",
                economies as "economies: Vec<EconomyShare>",
                updated_at,
                updated_by
            FROM stations
            WHERE system_address = $1 AND name = $2
            "#,
            system_address,
            name
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Station {
            system_address: row.system_address,
            name: row.name,
            ty: Some(row.ty),
            dist_from_star_ls: row.dist_from_star_ls,
            market_id: row.market_id,
            landing_pads: row.landing_pads,
            faction: row.faction,
            government: row.government,
            allegiance: row.allegiance,
            services: row.services,
            economies: row.economies,
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,
        })
    }

    pub async fn fetch_all(db: &Database, system_address: i64) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                system_address,
                name,
                ty as "ty: StationType",
                dist_from_star_ls,
                market_id,
                landing_pads as "landing_pads: LandingPads",
                faction,
                government as "government: Government",
                allegiance as "allegiance: Allegiance",
                services as "services: Vec<Service>",
                economies as "economies: Vec<EconomyShare>",
                updated_at,
                updated_by
            FROM stations
            WHERE system_address = $1
            "#,
            system_address,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Station {
                system_address: row.system_address,
                name: row.name,
                ty: Some(row.ty),
                dist_from_star_ls: row.dist_from_star_ls,
                market_id: row.market_id,
                landing_pads: row.landing_pads,
                faction: row.faction,
                government: row.government,
                allegiance: row.allegiance,
                services: row.services,
                economies: row.economies,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect())
    }
}