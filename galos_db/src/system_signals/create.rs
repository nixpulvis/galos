use super::SystemSignal;
use crate::{Database, Error};
use elite_journal::entry::incremental::exploration::SystemSignal as JournalSignal;

impl SystemSignal {
    /// Record a system's worth of signals
    ///
    /// The game emits one event per signal and EDDN gathers a system's worth
    /// into a single message, so these arrive in batches. Each signal in the
    /// batch carries its own timestamp and is written under it, rather than
    /// under the message's -- the message is stamped with the first signal's,
    /// which would be wrong for all the others.
    pub async fn from_journal(
        db: &Database,
        user: &str,
        system_address: i64,
        signals: &[JournalSignal],
    ) -> Result<(), Error> {
        for signal in signals {
            let done = sqlx::query!(
                "
                INSERT INTO system_signals (
                    system_address,
                    name,
                    updated_at,
                    updated_by,

                    signal_type,
                    is_station,
                    uss_type,
                    spawning_state,
                    spawning_faction,
                    spawning_power,
                    opposing_power,
                    threat_level)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                ON CONFLICT (system_address, name)
                DO UPDATE SET
                    updated_at = $3,
                    updated_by = $4,

                    signal_type = $5,
                    is_station = $6,
                    uss_type = $7,
                    spawning_state = $8,
                    spawning_faction = $9,
                    spawning_power = $10,
                    opposing_power = $11,
                    threat_level = $12
                WHERE system_signals.updated_at <= $3
                ",
                system_address,
                signal.signal_name,
                signal.timestamp.naive_utc(),
                user,
                signal.signal_type,
                signal.is_station,
                signal.uss_type,
                signal.spawning_state,
                signal.spawning_faction,
                signal.spawning_power,
                signal.opposing_power,
                signal.threat_level,
            )
            .execute(&db.pool)
            .await?;

            if done.rows_affected() == 0 {
                crate::turned_away("system signal", signal.timestamp);
            }
        }

        Ok(())
    }
}
