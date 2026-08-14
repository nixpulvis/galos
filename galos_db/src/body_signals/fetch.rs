use super::BodySignal;
use crate::{Database, Error};

impl BodySignal {
    /// Everything found on one body
    pub async fn fetch(
        db: &Database,
        system_address: i64,
        body_id: i16,
    ) -> Result<Vec<BodySignal>, Error> {
        let rows = sqlx::query!(
            "
            SELECT * FROM body_signals
             WHERE system_address = $1 AND body_id = $2
             ORDER BY signal_type
            ",
            system_address,
            body_id,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| BodySignal {
                system_address: row.system_address,
                body_id: row.body_id,
                signal_type: row.signal_type,
                count: row.count,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect())
    }

    /// Everything found on every body in a system
    pub async fn fetch_all(
        db: &Database,
        system_address: i64,
    ) -> Result<Vec<BodySignal>, Error> {
        let rows = sqlx::query!(
            "
            SELECT * FROM body_signals
             WHERE system_address = $1
             ORDER BY body_id, signal_type
            ",
            system_address,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| BodySignal {
                system_address: row.system_address,
                body_id: row.body_id,
                signal_type: row.signal_type,
                count: row.count,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect())
    }
}
