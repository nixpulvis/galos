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
            db,
            &mut tx,
            timestamp,
            shipyard.market_id,
            &shipyard.system_name,
            &shipyard.station_name,
        )
        .await?;

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
