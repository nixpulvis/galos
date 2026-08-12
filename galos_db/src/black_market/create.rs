use super::BlackMarket;
use crate::markets::Market;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::market::BlackMarket as JournalBlackMarket;

impl BlackMarket {
    /// Record what a black market paid for one commodity
    ///
    /// Nothing is cleared here, unlike the other trade tables. The game
    /// reports a black market one commodity at a time and a message never says
    /// what else is traded, so an absent commodity is not evidence of anything.
    ///
    /// `market_id` is required despite the schema making it optional: a sale
    /// that cannot name its market cannot be placed at a station, and the
    /// system and station names alone are not what anything here is keyed by.
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        market_id: i64,
        sale: &JournalBlackMarket,
    ) -> Result<(), Error> {
        let mut tx = db.pool.begin().await?;

        Market::touch(
            db,
            &mut tx,
            timestamp,
            market_id,
            &sale.system_name,
            &sale.station_name,
        )
        .await?;

        let done = sqlx::query!(
            "
            INSERT INTO black_market (
                market_id,
                name,
                sell_price,
                prohibited,
                listed_at)
            VALUES ($1, LOWER($2), $3, $4, $5)
            ON CONFLICT (market_id, name)
            DO UPDATE SET
                sell_price = $3,
                prohibited = $4,
                listed_at = $5
            WHERE black_market.listed_at <= $5
            ",
            market_id,
            sale.name,
            sale.sell_price,
            sale.prohibited,
            timestamp.naive_utc(),
        )
        .execute(&mut *tx)
        .await?;

        if done.rows_affected() == 0 {
            crate::turned_away("black market price", timestamp);
        }

        tx.commit().await?;

        Ok(())
    }
}
