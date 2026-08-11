use super::Station;
use crate::{escaped, Database, Error};
use elite_journal::station::{EconomyShare, LandingPads, Service, StationType};
use elite_journal::{Allegiance, Government};

impl Station {
    pub async fn fetch(
        db: &Database,
        system_address: i64,
        name: &str,
    ) -> Result<Self, Error> {
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
                updated_by,
                body_id,
                body_name,
                latitude,
                longitude
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

    pub async fn fetch_all(
        db: &Database,
        system_address: i64,
    ) -> Result<Vec<Self>, Error> {
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
                updated_by,
                body_id,
                body_name,
                latitude,
                longitude
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
            .collect())
    }

    /// The stations whose name holds `query`, best first
    ///
    /// Ordered the way [`System::search_by_name`] is, and for the same
    /// reasons: an exact name leads, then a name starting with what was
    /// typed, then the rest alphabetically. Someone typing `jameson` means
    /// Jameson Memorial before they mean Jameson Orbital Extraction.
    ///
    /// Bounded, since a station name is even less distinctive than a system's
    /// — every third outpost is somebody's Hub — and a list nobody reads to
    /// the end of is no more use for being complete.
    ///
    /// [`System::search_by_name`]: crate::systems::System::search_by_name
    pub async fn search_by_name(
        db: &Database,
        query: &str,
        limit: i64,
    ) -> Result<Vec<Self>, Error> {
        let query = escaped(query);
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
                updated_by,
                body_id,
                body_name,
                latitude,
                longitude
            FROM stations
            WHERE name ILIKE $1
            ORDER BY
                (name ILIKE $2) DESC,
                (name ILIKE $3) DESC,
                name
            LIMIT $4
            "#,
            format!("%{query}%"),
            query,
            format!("{query}%"),
            limit,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Station {
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
            .collect())
    }
}
