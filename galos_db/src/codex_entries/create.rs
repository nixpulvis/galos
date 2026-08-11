use super::CodexEntry;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::incremental::exploration::CodexEntry as JournalEntry;

impl CodexEntry {
    /// Record something found in a system
    ///
    /// The same kind of thing found again in the same system updates the row
    /// rather than adding one, and the later sighting wins: where it was found
    /// is worth keeping current, since a second sighting may place it on a
    /// body the first could not name.
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        entry: &JournalEntry,
    ) -> Result<(), Error> {
        sqlx::query!(
            "
            INSERT INTO codex_entries (
                system_address,
                entry_id,
                updated_at,
                updated_by,

                name,
                category,
                sub_category,
                region,
                body_id,
                body_name,
                nearest_destination,
                latitude,
                longitude)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (system_address, entry_id)
            DO UPDATE SET
                updated_at = $3,
                updated_by = $4,

                name = COALESCE($5, codex_entries.name),
                category = COALESCE($6, codex_entries.category),
                sub_category = COALESCE($7, codex_entries.sub_category),
                region = COALESCE($8, codex_entries.region),
                body_id = COALESCE($9, codex_entries.body_id),
                body_name = COALESCE($10, codex_entries.body_name),
                nearest_destination =
                    COALESCE($11, codex_entries.nearest_destination),
                latitude = COALESCE($12, codex_entries.latitude),
                longitude = COALESCE($13, codex_entries.longitude)
            WHERE codex_entries.updated_at <= $3
            ",
            entry.system_address,
            entry.entry_id,
            timestamp.naive_utc(),
            user,
            entry.name,
            entry.category,
            entry.sub_category,
            entry.region,
            entry.body_id,
            entry.body_name,
            entry.nearest_destination,
            entry.latitude,
            entry.longitude,
        )
        .execute(&db.pool)
        .await?;

        Ok(())
    }
}
