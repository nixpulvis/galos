use super::BodySignal;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::body::Signal;

impl BodySignal {
    /// Record what was found on a body
    ///
    /// Both the surface scan and the honk report these, in the same terms, so
    /// both arrive here. The later of the two wins on any kind they disagree
    /// about, by the clock the game wrote rather than by which arrived first,
    /// and an older message is refused rather than kept from: a count is the
    /// whole of what a row holds, so there is nothing in it left to fill in.
    ///
    /// A kind that has stopped being reported is left alone rather than
    /// deleted. An absent signal in one message is not evidence that it is
    /// gone, only that this message did not mention it.
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        system_address: i64,
        body_id: i16,
        signals: &[Signal],
    ) -> Result<(), Error> {
        for signal in signals {
            sqlx::query!(
                "
                INSERT INTO body_signals (
                    system_address,
                    body_id,
                    signal_type,
                    count,
                    updated_at,
                    updated_by)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (system_address, body_id, signal_type)
                DO UPDATE SET
                    count = $4,
                    updated_at = $5,
                    updated_by = $6
                WHERE body_signals.updated_at <= $5
                ",
                system_address,
                body_id,
                signal.ty,
                signal.count as i32,
                timestamp.naive_utc(),
                user,
            )
            .execute(&db.pool)
            .await?;
        }

        Ok(())
    }
}
