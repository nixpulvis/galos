use super::Market;
use crate::systems::System;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::market::Market as JournalMarket;

impl Market {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        market: &JournalMarket,
    ) -> Result<Market, Error> {
        // Not knowing the system is not a reason to lose the prices. The
        // name is recorded either way, and `System::create` links the market
        // up if the system turns up later.
        //
        // A market can move. Fleet carriers jump, and one of them shows up in
        // this database under three systems in an hour. So where it is now
        // replaces where it was, rather than filling in a blank: a carrier
        // that jumps somewhere unheard of goes back to waiting, instead of
        // keeping the last system it was seen in.
        let address = match System::fetch_by_name(db, &market.system_name).await
        {
            Ok(system) => Some(system.address),
            // There is no such system yet, so the market waits for one.
            Err(Error::Sqlx(sqlx::Error::RowNotFound)) => None,
            // Anything else is the database failing to answer, which is not
            // the same as an answer of no. Filing the market as waiting on
            // that would leave it waiting on a system that already exists,
            // for a write that may never come.
            Err(err) => return Err(err),
        };

        let row = sqlx::query!(
            r#"
            INSERT INTO markets (
                id,
                system_address,
                system_name,
                station_name,
                updated_at)
            VALUES ($1, $2, UPPER($3), $4, $5)
            ON CONFLICT (id)
            DO UPDATE SET
                system_address = $2,
                system_name = UPPER($3),
                station_name = $4,
                updated_at = $5
            RETURNING
                id,
                system_address,
                system_name,
                station_name,
                updated_at
            "#,
            market.market_id,
            address,
            market.system_name,
            market.station_name,
            timestamp.naive_utc(),
        )
        .fetch_one(&db.pool)
        .await?;

        for commodity in &market.commodities {
            sqlx::query!(
                r#"
                INSERT INTO commodities (
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
                VALUES ($1, LOWER($2), $3, $4, $5, $6, $7, $8, $9, $10)
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
            system_name: row.system_name,
            station_name: row.station_name,
            updated_at: row.updated_at.and_utc(),
        })
    }
}
