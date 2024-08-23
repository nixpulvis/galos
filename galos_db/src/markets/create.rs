use chrono::{DateTime, Utc};
use elite_journal::entry::market::Market as JournalMarket;
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
                updated_at)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (id)
            DO UPDATE SET
                updated_at = $4
            RETURNING
                id,
                system_address,
                station_name,
                updated_at
            "#,
            market.market_id,
            system.address,
            market.station_name,
            timestamp.naive_utc(),
        )
        .fetch_one(&db.pool)
        .await?;

        for commodity in &market.commodities {
            sqlx::query!(
                r#"
                INSERT INTO listings (
                    market_id,
                    name,
                    mean_price,
                    buy_price,
                    sell_price,
                    demand,
                    demand_bracket,
                    stock,
                    stock_bracket,
                    listed_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                ON CONFLICT (market_id, name)
                DO UPDATE SET
                    mean_price = $3,
                    buy_price = $4,
                    sell_price = $5,
                    demand = $6,
                    demand_bracket = $7,
                    stock = $8,
                    stock_bracket = $9,
                    listed_at = $10
                RETURNING *
                "#,
                market.market_id,
                commodity.name,
                commodity.mean_price,
                commodity.buy_price,
                commodity.sell_price,
                commodity.demand,
                commodity.demand_bracket,
                commodity.stock,
                commodity.stock_bracket,
                timestamp.naive_utc(),
            )
            .fetch_one(&db.pool)
            .await?;
        }

        Ok(Market {
            id: row.id,
            system_address: row.system_address,
            station_name: row.station_name,
            updated_at: row.updated_at.and_utc(),
        })
    }
}
