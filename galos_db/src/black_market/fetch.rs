use super::BlackMarket;
use crate::{Database, Error};

impl BlackMarket {
    /// Everything one station's black market has been seen taking
    pub async fn fetch_all(
        db: &Database,
        market_id: i64,
    ) -> Result<Vec<BlackMarket>, Error> {
        let rows = sqlx::query!(
            "
            SELECT * FROM black_market
             WHERE market_id = $1
             ORDER BY name
            ",
            market_id,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| BlackMarket {
                market_id: row.market_id,
                name: row.name,
                sell_price: row.sell_price,
                prohibited: row.prohibited,
                listed_at: row.listed_at.and_utc(),
            })
            .collect())
    }
}
