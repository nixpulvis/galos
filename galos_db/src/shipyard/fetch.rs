use super::Shipyard;
use crate::{Database, Error};

impl Shipyard {
    /// Everything one station's shipyard sells
    pub async fn fetch_all(
        db: &Database,
        market_id: i64,
    ) -> Result<Vec<Shipyard>, Error> {
        let rows = sqlx::query!(
            "
            SELECT * FROM shipyard
             WHERE market_id = $1
             ORDER BY ship_name
            ",
            market_id,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Shipyard {
                market_id: row.market_id,
                ship_name: row.ship_name,
                listed_at: row.listed_at.and_utc(),
            })
            .collect())
    }
}
