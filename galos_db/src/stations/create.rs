use super::Station;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::incremental::travel::ApproachSettlement;
use elite_journal::station::Station as JournalStation;
use elite_journal::station::{EconomyShare, LandingPads, Service, StationType};
use elite_journal::{Allegiance, Government};

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
                ty = COALESCE($3, stations.ty),
                dist_from_star_ls =
                    COALESCE($4, stations.dist_from_star_ls),
                market_id = COALESCE($5, stations.market_id),
                landing_pads = COALESCE($6, stations.landing_pads),
                faction = COALESCE($7, stations.faction),
                government = COALESCE($8, stations.government),
                allegiance = COALESCE($9, stations.allegiance),
                services = COALESCE($10, stations.services),
                economies = COALESCE($11, stations.economies),
                updated_at = $12,
                updated_by = $13
            WHERE stations.updated_at <= $12
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
                updated_by,
                body_id,
                body_name,
                latitude,
                longitude
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
        .fetch_optional(&db.pool)
        .await?;

        // Nothing comes back where the guard turned the update away, which is a
        // message older than what is already stored. What is on record is the
        // newer station, so that is what is answered with.
        let Some(row) = row else {
            crate::turned_away("station", timestamp);
            return Self::fetch(db, system_address, &station.name).await;
        };

        Ok(Station {
            system_address: row.system_address,
            name: row.name,
            ty: row.ty,
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
            body_id: row.body_id,
            body_name: row.body_name,
            latitude: row.latitude,
            longitude: row.longitude,
        })
    }

    /// Record a settlement, which is a station on a planet's surface
    ///
    /// Written as a station because that is what it is: it has a market, a
    /// controlling faction, services and an allegiance, and it is docked at.
    /// What it has that an orbital station does not is somewhere to be, which
    /// is the four columns nothing else fills in.
    ///
    /// `ty` is left alone rather than written. `ApproachSettlement` does not
    /// say what kind of station this is, and coming up on a settlement that
    /// has already been docked at must not throw away what docking learned.
    pub async fn from_settlement(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        settlement: &ApproachSettlement,
    ) -> Result<(), Error> {
        let done = sqlx::query!(
            r#"
            INSERT INTO stations (
                system_address,
                name,
                market_id,
                faction,
                government,
                allegiance,
                services,
                economies,
                body_id,
                body_name,
                latitude,
                longitude,
                updated_at,
                updated_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            ON CONFLICT (system_address, name)
            DO UPDATE SET
                market_id = COALESCE($3, stations.market_id),
                faction = COALESCE($4, stations.faction),
                government = COALESCE($5, stations.government),
                allegiance = COALESCE($6, stations.allegiance),
                services = COALESCE($7, stations.services),
                economies = COALESCE($8, stations.economies),
                body_id = $9,
                body_name = $10,
                latitude = COALESCE($11, stations.latitude),
                longitude = COALESCE($12, stations.longitude),
                updated_at = $13,
                updated_by = $14
            WHERE stations.updated_at <= $13
            "#,
            settlement.system_address,
            settlement.name,
            settlement.market_id,
            settlement.faction.as_ref().map(|f| f.name.clone()),
            settlement.government as Option<Government>,
            settlement.allegiance as Option<Allegiance>,
            settlement.services.clone() as Option<Vec<Service>>,
            settlement.economies.clone() as Option<Vec<EconomyShare>>,
            settlement.body_id,
            settlement.body_name,
            settlement.latitude,
            settlement.longitude,
            timestamp.naive_utc(),
            user,
        )
        .execute(&db.pool)
        .await?;

        if done.rows_affected() == 0 {
            crate::turned_away("settlement", timestamp);
        }

        Ok(())
    }
}
