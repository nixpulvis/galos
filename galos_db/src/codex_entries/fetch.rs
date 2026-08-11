use super::CodexEntry;
use crate::{Database, Error};

impl CodexEntry {
    /// Everything found in a system
    pub async fn fetch_all(
        db: &Database,
        system_address: i64,
    ) -> Result<Vec<CodexEntry>, Error> {
        let rows = sqlx::query!(
            "
            SELECT * FROM codex_entries
             WHERE system_address = $1
             ORDER BY category, sub_category, name
            ",
            system_address,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| CodexEntry {
                system_address: row.system_address,
                entry_id: row.entry_id,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
                name: row.name,
                category: row.category,
                sub_category: row.sub_category,
                region: row.region,
                body_id: row.body_id,
                body_name: row.body_name,
                nearest_destination: row.nearest_destination,
                latitude: row.latitude,
                longitude: row.longitude,
            })
            .collect())
    }
}
