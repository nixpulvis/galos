use super::Outfitting;
use crate::markets::Market;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::market::Outfitting as JournalOutfitting;

impl Outfitting {
    /// Record everything a station's outfitting bay sells
    ///
    /// Read as the whole of what is stocked, the way a commodity message is:
    /// what the message leaves out is no longer sold, and nothing else can
    /// retire a module. Without that a station keeps advertising a module it
    /// stopped stocking months ago.
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        outfitting: &JournalOutfitting,
    ) -> Result<(), Error> {
        // The bay is emptied and refilled together. Between the two it sells
        // nothing, which is not a state any reader should be shown.
        let mut tx = db.pool.begin().await?;

        Market::touch(
            db,
            &mut tx,
            timestamp,
            outfitting.market_id,
            &outfitting.system_name,
            &outfitting.station_name,
        )
        .await?;

        sqlx::query!(
            "DELETE FROM outfitting WHERE market_id = $1",
            outfitting.market_id,
        )
        .execute(&mut *tx)
        .await?;

        for module in &outfitting.modules {
            sqlx::query!(
                "
                INSERT INTO outfitting (
                    market_id,
                    module_name,
                    buy_price,
                    merc_coins_price,
                    listed_at)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (market_id, module_name)
                DO UPDATE SET
                    buy_price = $3,
                    merc_coins_price = $4,
                    listed_at = $5
                ",
                outfitting.market_id,
                module.name(),
                module.buy_price(),
                module.merc_coins_price(),
                timestamp.naive_utc(),
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }
}
