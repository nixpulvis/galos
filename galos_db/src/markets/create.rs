use chrono::{DateTime, Utc};
use elite_journal::entry::market::{Market as JournalMarket, Commodity};
use crate::{Database, Error};
use crate::systems::System;
use super::Market;

impl Market {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        market: &JournalMarket,
    ) -> Result<Market, Error> {
        let system = System::fetch_by_name(db, &market.system_name).await?;
        let row = sqlx::query!(
            r#"
            INSERT INTO markets (
                id,
                system_address,
                station_name,
                updated_at,
                commodities)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (id)
            DO UPDATE SET
                updated_at = $4,
                commodities = $5
            RETURNING
                id,
                system_address,
                station_name,
                updated_at,
                commodities as "commodities: Vec<Commodity>"
            "#,
            market.market_id,
            system.address,
            market.station_name,
            timestamp.naive_utc(),
            market.commodities.clone() as Vec<Commodity>,
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Market {
            id: row.id,
            system_address: row.system_address,
            station_name: row.station_name,
            updated_at: row.updated_at.and_utc(),
            commodities: row.commodities,
        })
    }
}
