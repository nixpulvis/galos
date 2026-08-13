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
        user: &str,
        outfitting: &JournalOutfitting,
    ) -> Result<(), Error> {
        // The bay is emptied and refilled together. Between the two it sells
        // nothing, which is not a state any reader should be shown.
        let mut tx = db.pool.begin().await?;

        Market::touch(
            &mut tx,
            timestamp,
            user,
            outfitting.market_id,
            &outfitting.system_name,
            &outfitting.station_name,
        )
        .await?;

        // Read as the whole of what is stocked, so an older message replaces a
        // newer list rather than adding to it. The market itself is already
        // placed, and settled that on its own stamp.
        if newer_on_record(&mut tx, outfitting.market_id, timestamp).await? {
            crate::turned_away("outfitting", timestamp);
            tx.commit().await?;
            return Ok(());
        }

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

/// Whether a market's modules were last written after `timestamp`
///
/// Asked of the rows themselves rather than of the market, whose stamp is the
/// newest of any of the four kinds of trade message and so says nothing about
/// how fresh the bay's list is.
async fn newer_on_record(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    market_id: i64,
    timestamp: DateTime<Utc>,
) -> Result<bool, Error> {
    let listed = sqlx::query_scalar!(
        "SELECT MAX(listed_at) FROM outfitting WHERE market_id = $1",
        market_id,
    )
    .fetch_one(&mut **tx)
    .await?;

    Ok(listed.is_some_and(|listed| listed > timestamp.naive_utc()))
}
