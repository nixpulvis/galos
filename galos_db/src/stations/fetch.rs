use crate::{Database, Error};
use elite_journal::station::{LandingPads, EconomyShare, Service, StationType};
use elite_journal::{Allegiance, Government};
use super::Station;

impl Station {
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
