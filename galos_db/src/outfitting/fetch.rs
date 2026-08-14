use super::Outfitting;
use crate::{Database, Error};

impl Outfitting {
    /// Everything one station's outfitting bay sells
    pub async fn fetch_all(
        db: &Database,
        market_id: i64,
    ) -> Result<Vec<Outfitting>, Error> {
        let rows = sqlx::query!(
            "
            SELECT * FROM outfitting
             WHERE market_id = $1
             ORDER BY module_name
            ",
            market_id,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Outfitting {
                market_id: row.market_id,
                module_name: row.module_name,
                buy_price: row.buy_price,
                merc_coins_price: row.merc_coins_price,
                listed_at: row.listed_at.and_utc(),
            })
            .collect())
    }
}
