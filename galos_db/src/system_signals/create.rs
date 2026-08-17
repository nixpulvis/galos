use super::SystemSignal;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::incremental::exploration::SystemSignal as JournalSignal;

impl SystemSignal {
    /// Record a system's worth of signals
    ///
    /// A signal out of an EDDN batch carries the moment it was seen and is
    /// written under that rather than under `timestamp`, which is the first
    /// signal's and would be wrong for all the others. A signal the game
    /// wrote alone carries none and takes `timestamp`, which is its own.
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        system_address: i64,
        signals: &[JournalSignal],
    ) -> Result<(), Error> {
        for signal in signals {
            let seen = signal.timestamp.unwrap_or(timestamp);
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
                seen.naive_utc(),
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
                crate::turned_away("system signal", seen);
            }
        }

        Ok(())
    }
}
