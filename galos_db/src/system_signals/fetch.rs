use super::SystemSignal;
use crate::{Database, Error};

impl SystemSignal {
    /// Everything seen in a system, most recently seen first
    pub async fn fetch_all(
        db: &Database,
        system_address: i64,
    ) -> Result<Vec<SystemSignal>, Error> {
        let rows = sqlx::query!(
            "
            SELECT * FROM system_signals
             WHERE system_address = $1
             ORDER BY updated_at DESC, name
            ",
            system_address,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| SystemSignal {
                system_address: row.system_address,
                name: row.name,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
                signal_type: row.signal_type,
                is_station: row.is_station,
                uss_type: row.uss_type,
                spawning_state: row.spawning_state,
                spawning_faction: row.spawning_faction,
                spawning_power: row.spawning_power,
                opposing_power: row.opposing_power,
                threat_level: row.threat_level,
            })
            .collect())
    }
}
