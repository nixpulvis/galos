use super::Shipyard;
use crate::markets::Market;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::market::Shipyard as JournalShipyard;

impl Shipyard {
    /// Record everything a station's shipyard sells
    ///
    /// Read as the whole of what is stocked, so what the message leaves out is
    /// no longer sold.
    ///
    /// With one exception, which is not corrected for here: whether the sender
    /// could buy a Cobra MkIV is a fact about the commander rather than about
    /// the shipyard, and most cannot. A shipyard that stocks one will lose the
    /// row every time somebody without the unlock passes through, and gain it
    /// back from the next sender who has it.
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        shipyard: &JournalShipyard,
    ) -> Result<(), Error> {
        // Emptied and refilled together, so that the yard is never seen
        // holding nothing.
        let mut tx = db.pool.begin().await?;

        Market::touch(
            &mut tx,
            timestamp,
            shipyard.market_id,
            &shipyard.system_name,
            &shipyard.station_name,
        )
        .await?;

        // Read as the whole of what is stocked, so an older message replaces a
        // newer list rather than adding to it. The market itself is already
        // placed, and settled that on its own stamp.
        if newer_on_record(&mut tx, shipyard.market_id, timestamp).await? {
            crate::turned_away("shipyard", timestamp);
            tx.commit().await?;
            return Ok(());
        }

        sqlx::query!(
            "DELETE FROM shipyard WHERE market_id = $1",
            shipyard.market_id,
        )
        .execute(&mut *tx)
        .await?;

        for ship in &shipyard.ships {
            sqlx::query!(
                "
                INSERT INTO shipyard (market_id, ship_name, listed_at)
                VALUES ($1, $2, $3)
                ON CONFLICT (market_id, ship_name)
                DO UPDATE SET listed_at = $3
                ",
                shipyard.market_id,
                ship,
                timestamp.naive_utc(),
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }
}

/// Whether a market's ships were last written after `timestamp`
///
/// Asked of the rows themselves rather than of the market, whose stamp is the
/// newest of any of the four kinds of trade message and so says nothing about
/// how fresh the yard's list is.
async fn newer_on_record(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    market_id: i64,
    timestamp: DateTime<Utc>,
) -> Result<bool, Error> {
    let listed = sqlx::query_scalar!(
        "SELECT MAX(listed_at) FROM shipyard WHERE market_id = $1",
        market_id,
    )
    .fetch_one(&mut **tx)
    .await?;

    Ok(listed.is_some_and(|listed| listed > timestamp.naive_utc()))
}
